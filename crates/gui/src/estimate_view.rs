//! The Estimate window: bucket totals and scan cost from CloudWatch, without listing.

use clouddirstat_core::{format_bytes, format_count, format_counted, format_usd};
use clouddirstat_providers::Error;
use clouddirstat_providers::s3::{Estimate, S3Pricing, list_cost_usd};
use eframe::egui::{self, Align, Label, RichText, Sense};
use egui_extras::{Column, TableBuilder};
use tokio::runtime::Runtime;
use tokio::sync::oneshot;

use crate::palette;
use crate::scan::ScanRequest;
use crate::tree_view::right_aligned;

const ROW_HEIGHT: f32 = 20.0;

/// What the user did in the window.
pub enum Action {
    /// Scan the estimated location.
    Scan(Box<ScanRequest>),
    /// Put this bucket in the Location field.
    Choose(String),
}

enum State {
    Running,
    Done(Estimate),
    /// The error, and whether it came from CloudWatch (rather than, say, S3).
    Failed(String, bool),
}

/// An estimate being fetched on the tokio runtime, or its result.
pub struct EstimateTask {
    request: ScanRequest,
    state: State,
    receiver: oneshot::Receiver<clouddirstat_providers::Result<Estimate>>,
}

impl EstimateTask {
    pub fn start(runtime: &Runtime, request: ScanRequest, ctx: &egui::Context) -> Self {
        let (sender, receiver) = oneshot::channel();
        let ctx = ctx.clone();
        let task_request = request.clone();
        runtime.spawn(async move {
            let _ = sender.send(fetch(&task_request).await);
            ctx.request_repaint();
        });
        Self {
            request,
            state: State::Running,
            receiver,
        }
    }

    fn poll(&mut self) {
        if !matches!(self.state, State::Running) {
            return;
        }
        match self.receiver.try_recv() {
            Ok(Ok(estimate)) => self.state = State::Done(estimate),
            Ok(Err(error)) => {
                let cloudwatch = matches!(
                    error,
                    Error::Request {
                        permission: "cloudwatch:GetMetricData",
                        ..
                    }
                );
                self.state = State::Failed(error.to_string(), cloudwatch);
            }
            Err(oneshot::error::TryRecvError::Empty) => {}
            Err(oneshot::error::TryRecvError::Closed) => {
                let message = "the estimate task ended unexpectedly".to_owned();
                self.state = State::Failed(message, false);
            }
        }
    }

    /// Shows the window. Returns what the user did; `open` turns false when closed.
    pub fn show(&mut self, ctx: &egui::Context, open: &mut bool) -> Option<Action> {
        self.poll();
        let mut action = None;
        egui::Window::new(format!("Estimate: {}", self.request.location))
            .id(egui::Id::new("estimate"))
            .open(open)
            .default_width(720.0)
            .resizable(true)
            .show(ctx, |ui| match &self.state {
                State::Running => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Asking CloudWatch for bucket totals…");
                    });
                }
                State::Failed(error, cloudwatch) => {
                    ui.colored_label(ui.visuals().error_fg_color, error);
                    if *cloudwatch {
                        ui.weak(
                            "Estimates read S3's daily storage metrics from CloudWatch, which \
                             needs cloudwatch:GetMetricData (see Help).",
                        );
                    }
                }
                State::Done(estimate) => {
                    action = contents(ui, &self.request, estimate);
                }
            });
        action
    }
}

#[cfg(not(feature = "demo"))]
async fn fetch(request: &ScanRequest) -> clouddirstat_providers::Result<Estimate> {
    use clouddirstat_providers::Scanner;
    Scanner::connect(&request.location, &request.credentials, None)
        .await?
        .estimate()
        .await
}

#[cfg(feature = "demo")]
use crate::demo_scan::estimate as fetch;

fn contents(ui: &mut egui::Ui, request: &ScanRequest, estimate: &Estimate) -> Option<Action> {
    let mut action = None;
    let as_of = estimate
        .as_of()
        .map_or_else(|| "no data yet".to_owned(), |date| date.to_string());
    ui.label(
        RichText::new(format!(
            "{} in {}  ·  ~{}/mo",
            format_bytes(estimate.bytes()),
            format_counted(estimate.objects(), "object"),
            format_usd(estimate.monthly_cost())
        ))
        .heading(),
    );
    ui.label(format!(
        "A full scan needs about {} LIST requests (~${:.4}).",
        format_count(estimate.list_requests()),
        estimate.scan_cost_usd()
    ));
    ui.weak(format!(
        "From CloudWatch as of {as_of}; this estimate cost ~${:.4}. Storage cost at {} \
         list prices.",
        estimate.request_cost_usd(),
        S3Pricing::published()
    ));
    if !request.location.prefix().is_empty() {
        ui.colored_label(
            ui.visuals().warn_fg_color,
            "Totals are for the whole bucket; CloudWatch does not report prefixes.",
        );
    }

    ui.add_space(8.0);
    ui.horizontal_wrapped(|ui| {
        let total = estimate.bytes().max(1);
        for (class, bytes) in estimate.classes() {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), Sense::hover());
            ui.painter()
                .rect_filled(rect, 2.0, palette::storage_class(&class));
            ui.label(format!(
                "{class} {} ({:.1}%)",
                format_bytes(bytes),
                bytes as f64 * 100.0 / total as f64
            ));
            ui.add_space(8.0);
        }
    });

    if request.location.is_all() {
        ui.add_space(8.0);
        if let Some(bucket) = bucket_table(ui, estimate) {
            action = Some(Action::Choose(bucket));
        }
    }
    for skipped in &estimate.skipped {
        ui.colored_label(
            ui.visuals().warn_fg_color,
            format!("Skipped {}: {}", skipped.bucket, skipped.reason),
        );
    }

    ui.add_space(8.0);
    ui.weak(
        "CloudWatch updates these once a day. Object counts include every version, delete \
         marker, and upload part, so a scan without Versions may list fewer.",
    );
    ui.add_space(4.0);
    if ui.button(format!("Scan {}", request.location)).clicked() {
        action = Some(Action::Scan(Box::new(request.clone())));
    }
    action
}

/// One row per bucket. Returns the bucket that was clicked, if any.
fn bucket_table(ui: &mut egui::Ui, estimate: &Estimate) -> Option<String> {
    let mut clicked = None;
    ui.weak("Click a bucket to put it in Location.");
    TableBuilder::new(ui)
        .striped(true)
        .resizable(true)
        .sense(Sense::click())
        .max_scroll_height(320.0)
        .cell_layout(egui::Layout::left_to_right(Align::Center))
        .column(Column::initial(200.0).at_least(80.0).clip(true))
        .column(Column::initial(100.0).at_least(60.0).clip(true))
        .column(Column::initial(80.0).at_least(50.0).clip(true))
        .column(Column::initial(90.0).at_least(50.0).clip(true))
        .column(Column::initial(80.0).at_least(50.0).clip(true))
        .column(Column::initial(80.0).at_least(50.0).clip(true))
        .column(Column::remainder().at_least(70.0).clip(true))
        .header(ROW_HEIGHT, |mut header| {
            for title in [
                "Bucket",
                "Region",
                "Size",
                "Objects",
                "Cost/mo",
                "Scan cost",
                "As of",
            ] {
                header.col(|ui| {
                    ui.strong(title);
                });
            }
        })
        .body(|body| {
            body.rows(ROW_HEIGHT, estimate.buckets.len(), |mut row| {
                let bucket = &estimate.buckets[row.index()];
                row.col(|ui| {
                    ui.add(Label::new(&bucket.bucket).selectable(false).truncate());
                });
                row.col(|ui| {
                    ui.add(Label::new(&bucket.region).selectable(false));
                });
                row.col(|ui| right_aligned(ui, format_bytes(bucket.bytes())));
                row.col(|ui| {
                    let objects = bucket.objects.map_or_else(|| "-".to_owned(), format_count);
                    right_aligned(ui, objects);
                });
                row.col(|ui| right_aligned(ui, format_usd(bucket.monthly_cost)));
                row.col(|ui| {
                    right_aligned(ui, format!("${:.4}", list_cost_usd(bucket.list_requests())));
                });
                row.col(|ui| {
                    let as_of = bucket
                        .as_of
                        .map_or_else(|| "no data".to_owned(), |date| date.to_string());
                    ui.add(Label::new(as_of).selectable(false));
                });
                if row.response().clicked() {
                    clicked = Some(bucket.bucket.clone());
                }
            });
        });
    clicked
}
