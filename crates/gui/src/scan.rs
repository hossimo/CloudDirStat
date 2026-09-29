use std::time::{Duration, Instant};

use clouddirstat_core::{Entry, Tree};
use clouddirstat_providers::s3::{S3Location, S3Scanner, ScanOptions, ScanStats};
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
    pub tree: Tree,
    pub state: ScanState,
    started: Instant,
    entries: mpsc::Receiver<Vec<Entry>>,
    outcome: oneshot::Receiver<clouddirstat_providers::Result<ScanStats>>,
}

impl Scan {
    pub fn start(runtime: &Runtime, request: ScanRequest, ctx: &egui::Context) -> Self {
        let (sender, entries) = mpsc::channel(256);
        let (outcome_sender, outcome) = oneshot::channel();
        let location = request.location.clone();
        let ctx = ctx.clone();

        runtime.spawn(async move {
            let result = run(&request, sender).await;
            let _ = outcome_sender.send(result);
            ctx.request_repaint();
        });

        Self {
            location,
            tree: Tree::new(),
            state: ScanState::Running,
            started: Instant::now(),
            entries,
            outcome,
        }
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
        let mut changed = false;
        loop {
            match self.entries.try_recv() {
                Ok(batch) => {
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
    sender: mpsc::Sender<Vec<Entry>>,
) -> clouddirstat_providers::Result<ScanStats> {
    let scanner =
        S3Scanner::connect(&request.location.bucket, request.profile.as_deref(), None).await?;
    let options = ScanOptions {
        include_versions: request.include_versions,
        concurrency: CONCURRENCY,
    };
    scanner.scan(&request.location, &options, sender).await
}
