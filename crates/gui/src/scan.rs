use std::sync::Arc;
use std::time::{Duration, Instant};

use clouddirstat_core::Pricing;
use clouddirstat_core::{Entry, NodeId, NodeKind, Tree};
use clouddirstat_providers::{Credentials, Location, ScanStats};
use eframe::egui;
use tokio::runtime::Runtime;
use tokio::sync::mpsc::error::TryRecvError;
use tokio::sync::{mpsc, oneshot};

use crate::error_view::Problem;

#[derive(Clone)]
pub struct ScanRequest {
    pub location: Location,
    // Demo builds scan made-up data and need no credentials.
    #[cfg_attr(feature = "demo", allow(dead_code))]
    pub credentials: Credentials,
    pub include_versions: bool,
}

pub enum ScanState {
    Running,
    Finished { stats: ScanStats, elapsed: Duration },
    Stopped { elapsed: Duration },
    Failed(Problem),
}

/// A scan running on the tokio runtime, and the tree built from what it has sent so far.
pub struct Scan {
    pub location: Location,
    pub include_versions: bool,
    pub tree: Tree,
    pub pricing: Option<Arc<dyn Pricing>>,
    /// CloudWatch's count of the objects to scan, for progress. Whole-bucket scans only.
    pub expected_objects: Option<u64>,
    pub state: ScanState,
    started: Instant,
    priced: oneshot::Receiver<Arc<dyn Pricing>>,
    counted: oneshot::Receiver<u64>,
    entries: mpsc::Receiver<Vec<Entry>>,
    outcome: oneshot::Receiver<clouddirstat_providers::Result<ScanStats>>,
}

impl Scan {
    pub fn start(runtime: &Runtime, request: ScanRequest, ctx: &egui::Context) -> Self {
        let (sender, entries) = mpsc::channel(256);
        let (outcome_sender, outcome) = oneshot::channel();
        let (pricing_sender, priced) = oneshot::channel();
        let (count_sender, counted) = oneshot::channel();
        let location = request.location.clone();
        let include_versions = request.include_versions;
        let ctx = ctx.clone();

        runtime.spawn(async move {
            let result = run(&request, pricing_sender, count_sender, sender).await;
            let _ = outcome_sender.send(result);
            ctx.request_repaint();
        });

        Self {
            location,
            include_versions,
            tree: Tree::new(),
            pricing: None,
            expected_objects: None,
            state: ScanState::Running,
            started: Instant::now(),
            priced,
            counted,
            entries,
            outcome,
        }
    }

    /// The folder matching the scanned prefix (the bucket root when there is none),
    /// so views start where the user pointed rather than at a chain of parent folders.
    pub fn root(&self) -> NodeId {
        let mut node = Tree::ROOT;
        let Some((folders, _)) = self.location.prefix().rsplit_once('/') else {
            return node;
        };
        for name in folders.split('/') {
            match self.tree.find_child(node, name, NodeKind::Directory) {
                Some(child) => node = child,
                None => break,
            }
        }
        node
    }

    pub fn is_running(&self) -> bool {
        matches!(self.state, ScanState::Running)
    }

    pub fn elapsed(&self) -> Duration {
        match self.state {
            ScanState::Finished { elapsed, .. } | ScanState::Stopped { elapsed } => elapsed,
            _ => self.started.elapsed(),
        }
    }

    /// Moves received entries into the tree, spending at most `budget` so the UI stays
    /// responsive. Returns whether the tree changed.
    pub fn poll(&mut self, budget: Duration) -> bool {
        if !self.is_running() {
            return false;
        }

        let deadline = Instant::now() + budget;
        let mut changed = self.receive_pricing();
        if self.expected_objects.is_none()
            && let Ok(objects) = self.counted.try_recv()
        {
            self.expected_objects = Some(objects).filter(|&objects| objects > 0);
        }
        loop {
            match self.entries.try_recv() {
                Ok(batch) => {
                    self.receive_pricing();
                    for entry in &batch {
                        self.tree.insert(entry);
                    }
                    changed = true;
                    if Instant::now() >= deadline {
                        break;
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.check_outcome();
                    break;
                }
            }
        }
        changed
    }

    /// Switches to a priced tree once the bucket regions are known. The scan task sends
    /// the pricing before listing anything, so the tree is still empty at that point.
    fn receive_pricing(&mut self) -> bool {
        if self.pricing.is_some() {
            return false;
        }
        let Ok(pricing) = self.priced.try_recv() else {
            return false;
        };
        debug_assert_eq!(self.tree.total().objects, 0);
        self.tree = Tree::with_pricing(pricing.clone());
        self.pricing = Some(pricing);
        true
    }

    /// Stops listing but keeps the partial tree. The scanner sees the closed channel
    /// and ends on its next page.
    pub fn stop(&mut self) {
        if self.is_running() {
            self.entries.close();
            self.state = ScanState::Stopped {
                elapsed: self.started.elapsed(),
            };
        }
    }

    fn check_outcome(&mut self) {
        match self.outcome.try_recv() {
            Ok(Ok(stats)) => {
                self.state = ScanState::Finished {
                    stats,
                    elapsed: self.started.elapsed(),
                };
            }
            Ok(Err(error)) => self.state = ScanState::Failed(Problem::from_error(&error)),
            Err(oneshot::error::TryRecvError::Empty) => {}
            Err(oneshot::error::TryRecvError::Closed) => {
                self.state = ScanState::Failed(Problem::new("The scan ended unexpectedly"));
            }
        }
    }
}

#[cfg(feature = "demo")]
use crate::demo_scan::run;

#[cfg(not(feature = "demo"))]
async fn run(
    request: &ScanRequest,
    pricing: oneshot::Sender<Arc<dyn Pricing>>,
    expected_objects: oneshot::Sender<u64>,
    sender: mpsc::Sender<Vec<Entry>>,
) -> clouddirstat_providers::Result<ScanStats> {
    use clouddirstat_providers::{ScanOptions, Scanner};
    const CONCURRENCY: usize = 32;

    let scanner = Scanner::connect(&request.location, &request.credentials, None).await?;
    let _ = pricing.send(scanner.pricing());
    let options = ScanOptions {
        include_versions: request.include_versions,
        concurrency: CONCURRENCY,
    };
    // CloudWatch counts whole buckets, so a prefix scan gets no progress. Without
    // cloudwatch:GetMetricData the scan simply shows none.
    let count = async {
        if request.location.prefix().is_empty()
            && let Ok(estimate) = scanner.object_counts().await
        {
            let _ = expected_objects.send(estimate.objects());
        }
    };
    let scan = scanner.scan(&options, sender);
    tokio::pin!(count, scan);
    // The count is only for show: the scan never waits for it, even if CloudWatch is
    // slow or unreachable.
    tokio::select! {
        result = &mut scan => result,
        () = &mut count => scan.await,
    }
}
