use std::sync::Arc;
use std::time::{Duration, Instant};

use clouddirstat_core::{Entry, NodeId, NodeKind, Tree};
use clouddirstat_providers::s3::{S3Location, S3Pricing, S3Scanner, ScanOptions, ScanStats};
use eframe::egui;
use tokio::runtime::Runtime;
use tokio::sync::mpsc::error::TryRecvError;
use tokio::sync::{mpsc, oneshot};

const CONCURRENCY: usize = 32;

#[derive(Clone)]
pub struct ScanRequest {
    pub location: S3Location,
    pub profile: Option<String>,
    pub include_versions: bool,
}

pub enum ScanState {
    Running,
    Finished { stats: ScanStats, elapsed: Duration },
    Stopped { elapsed: Duration },
    Failed(String),
}

/// A scan running on the tokio runtime, and the tree built from what it has sent so far.
pub struct Scan {
    pub location: S3Location,
    pub include_versions: bool,
    pub tree: Tree,
    pub pricing: Option<Arc<S3Pricing>>,
    pub state: ScanState,
    started: Instant,
    region: oneshot::Receiver<String>,
    entries: mpsc::Receiver<Vec<Entry>>,
    outcome: oneshot::Receiver<clouddirstat_providers::Result<ScanStats>>,
}

impl Scan {
    pub fn start(runtime: &Runtime, request: ScanRequest, ctx: &egui::Context) -> Self {
        let (sender, entries) = mpsc::channel(256);
        let (outcome_sender, outcome) = oneshot::channel();
        let (region_sender, region) = oneshot::channel();
        let location = request.location.clone();
        let include_versions = request.include_versions;
        let ctx = ctx.clone();

        runtime.spawn(async move {
            let result = run(&request, region_sender, sender).await;
            let _ = outcome_sender.send(result);
            ctx.request_repaint();
        });

        Self {
            location,
            include_versions,
            tree: Tree::new(),
            pricing: None,
            state: ScanState::Running,
            started: Instant::now(),
            region,
            entries,
            outcome,
        }
    }

    /// The folder matching the scanned prefix (the bucket root when there is none),
    /// so views start where the user pointed rather than at a chain of parent folders.
    pub fn root(&self) -> NodeId {
        let mut node = Tree::ROOT;
        let Some((folders, _)) = self.location.prefix.rsplit_once('/') else {
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
        let mut changed = self.receive_region();
        loop {
            match self.entries.try_recv() {
                Ok(batch) => {
                    self.receive_region();
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

    /// Switches to a priced tree once the bucket region is known. The scan task sends
    /// the region before listing anything, so the tree is still empty at that point.
    fn receive_region(&mut self) -> bool {
        if self.pricing.is_some() {
            return false;
        }
        let Ok(region) = self.region.try_recv() else {
            return false;
        };
        debug_assert_eq!(self.tree.total().objects, 0);
        let pricing = Arc::new(S3Pricing::for_region(&region));
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
            Ok(Err(error)) => self.state = ScanState::Failed(error.to_string()),
            Err(oneshot::error::TryRecvError::Empty) => {}
            Err(oneshot::error::TryRecvError::Closed) => {
                self.state = ScanState::Failed("the scan task ended unexpectedly".to_owned());
            }
        }
    }
}

async fn run(
    request: &ScanRequest,
    region: oneshot::Sender<String>,
    sender: mpsc::Sender<Vec<Entry>>,
) -> clouddirstat_providers::Result<ScanStats> {
    let scanner =
        S3Scanner::connect(&request.location.bucket, request.profile.as_deref(), None).await?;
    let _ = region.send(scanner.region().to_owned());
    let options = ScanOptions {
        include_versions: request.include_versions,
        concurrency: CONCURRENCY,
    };
    scanner.scan(&request.location, &options, sender).await
}
