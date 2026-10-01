//! The Estimate window: bucket totals and scan cost from CloudWatch, without listing.

use clouddirstat_core::{format_bytes, format_count, format_counted, format_usd};
use clouddirstat_providers::Error;
use clouddirstat_providers::s3::{Estimate, S3Pricing, list_cost_usd};
use eframe::egui::{self, Align, Label, RichText, Sense};
use egui_extras::{Column, TableBuilder};
use tokio::runtime::Runtime;
use tokio::sync::oneshot;

use crate::error_view;
use crate::palette;
use crate::scan::ScanRequest;
use crate::tree_view::right_aligned;

const ROW_HEIGHT: f32 = 20.0;
const INITIAL_WIDTH: f32 = 560.0;
const EMBEDDED_TABLE_HEIGHT: f32 = 320.0;
/// Bucket names can be 63 characters; past this they are truncated (the column resizes).
const MAX_BUCKET_WIDTH: f32 = 360.0;
/// Fixed widths of Size, Objects, Cost/mo, Scan cost, and As of.
const NUMBER_COLUMNS: [f32; 5] = [80.0, 90.0, 80.0, 80.0, 90.0];
/// Window margins, the gaps between table columns, and the scroll bar.
const WINDOW_PADDING: f32 = 40.0;
const MAX_FITTED_HEIGHT: f32 = 760.0;

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
    /// Whether the window has been sized to the finished estimate.
    fitted: bool,
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
            fitted: false,
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
    /// `scanning` disables the Scan button while another scan runs. Opens as its own OS
    /// window, like Help, so it can sit beside the main window.
    pub fn show(&mut self, ctx: &egui::Context, open: &mut bool, scanning: bool) -> Option<Action> {
        self.poll();
        let builder = crate::with_app_icon(egui::ViewportBuilder::default())
            .with_title(format!("Estimate: {}", self.request.location))
            .with_inner_size([INITIAL_WIDTH, 240.0])
            .with_min_inner_size([360.0, 200.0]);
        let (action, closed) = ctx.show_viewport_immediate(
            egui::ViewportId::from_hash_of("estimate"),
            builder,
            |ui, class| {
                if class == egui::ViewportClass::EmbeddedWindow {
                    // No OS windows on this platform: egui shows it inside the main window.
                    let action = egui::ScrollArea::vertical()
                        .show(ui, |ui| {
                            let action = self.top(ui, EMBEDDED_TABLE_HEIGHT);
                            self.bottom(ui, scanning).or(action)
                        })
                        .inner;
                    return (action, false);
                }
                self.fit_window(ui);
                let bottom = egui::Panel::bottom("estimate_footer")
                    .show(ui, |ui| self.bottom(ui, scanning))
                    .inner;
                let top = egui::CentralPanel::default()
                    .show(ui, |ui| self.top(ui, f32::INFINITY))
                    .inner;
                let closed = ui.input(|input| input.viewport().close_requested());
                (bottom.or(top), closed)
            },
        );
        if closed {
            *open = false;
        }
        action
    }

    /// Sizes the window to the estimate once it arrives; after that the user's size stays.
    fn fit_window(&mut self, ui: &egui::Ui) {
        let State::Done(estimate) = &self.state else {
            return;
        };
        if self.fitted {
            return;
        }
        self.fitted = true;
        let size = fitting_size(ui, &self.request, estimate);
        ui.ctx()
            .send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
    }

    /// Everything above the footer; the bucket table gets at most `table_height`.
    fn top(&self, ui: &mut egui::Ui, table_height: f32) -> Option<Action> {
        match &self.state {
            State::Running => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Asking CloudWatch for bucket totals…");
                });
                None
            }
            State::Failed(error, cloudwatch) => {
                error_view::block(ui, error);
                if *cloudwatch {
                    ui.weak(
                        "Estimates read S3's daily storage metrics from CloudWatch, which \
                         needs cloudwatch:GetMetricData (see Help).",
                    );
                }
                None
            }
            State::Done(estimate) => summary(ui, &self.request, estimate, table_height),
        }
    }

    /// The note and Scan button, once the estimate is in.
    fn bottom(&self, ui: &mut egui::Ui, scanning: bool) -> Option<Action> {
        let State::Done(estimate) = &self.state else {
            return None;
        };
        footer(ui, &self.request, estimate, scanning)
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

fn summary(
    ui: &mut egui::Ui,
    request: &ScanRequest,
    estimate: &Estimate,
    table_height: f32,
) -> Option<Action> {
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
    class_table(ui, estimate);

    if !request.location.is_all() {
        return None;
    }
    ui.add_space(8.0);
    bucket_table(ui, estimate, table_height).map(Action::Choose)
}

/// One row per storage class: color, name, size, and share of the total.
fn class_table(ui: &mut egui::Ui, estimate: &Estimate) {
    let total = estimate.bytes().max(1);
    egui::Grid::new("estimate_classes")
        .num_columns(4)
        .spacing([12.0, 4.0])
        .show(ui, |ui| {
            for (class, bytes) in estimate.classes() {
                ui.horizontal(|ui| {
                    let (rect, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), Sense::hover());
                    ui.painter()
                        .rect_filled(rect, 2.0, palette::storage_class(&class));
                    ui.label(class);
                });
                right_aligned(ui, format_bytes(bytes));
                right_aligned(ui, format!("{:.1}%", bytes as f64 * 100.0 / total as f64));
                ui.end_row();
            }
        });
}

fn footer(
    ui: &mut egui::Ui,
    request: &ScanRequest,
    estimate: &Estimate,
    scanning: bool,
) -> Option<Action> {
    ui.add_space(4.0);
    for skipped in &estimate.skipped {
        ui.colored_label(
            ui.visuals().warn_fg_color,
            format!("Skipped {}: {}", skipped.bucket, skipped.reason),
        );
    }
    ui.weak(
        "CloudWatch updates these once a day. Object counts include every version, delete \
         marker, and upload part, so a scan without Versions may list fewer.",
    );
    ui.add_space(4.0);
    let scan = ui
        .add_enabled(
            !scanning,
            egui::Button::new(format!("Scan {}", request.location)),
        )
        .on_disabled_hover_text("A scan is already running. Stop it first.");
    ui.add_space(4.0);
    scan.clicked()
        .then(|| Action::Scan(Box::new(request.clone())))
}

/// One row per bucket. Returns the bucket that was clicked, if any.
fn bucket_table(ui: &mut egui::Ui, estimate: &Estimate, max_height: f32) -> Option<String> {
    let mut clicked = None;
    let [bucket_width, region_width] = name_widths(ui, estimate);
    ui.weak("Click a bucket to put it in Location.");
    let mut table = TableBuilder::new(ui)
        .striped(true)
        .resizable(true)
        .sense(Sense::click())
        .max_scroll_height(max_height)
        .cell_layout(egui::Layout::left_to_right(Align::Center))
        .column(Column::initial(bucket_width).at_least(80.0).clip(true))
        .column(Column::initial(region_width).at_least(60.0).clip(true));
    for width in &NUMBER_COLUMNS[..NUMBER_COLUMNS.len() - 1] {
        table = table.column(Column::initial(*width).at_least(50.0).clip(true));
    }
    table
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

/// Widths of the Bucket and Region columns that fit their longest names.
fn name_widths(ui: &egui::Ui, estimate: &Estimate) -> [f32; 2] {
    let buckets = estimate.buckets.iter().map(|bucket| bucket.bucket.as_str());
    let regions = estimate.buckets.iter().map(|bucket| bucket.region.as_str());
    let bucket_width = widest(ui, buckets.chain(["Bucket"]));
    let region_width = widest(ui, regions.chain(["Region"]));
    [bucket_width.min(MAX_BUCKET_WIDTH), region_width]
}

/// The width of the widest text, plus the cell padding.
fn widest<'a>(ui: &egui::Ui, texts: impl Iterator<Item = &'a str>) -> f32 {
    let font = egui::TextStyle::Body.resolve(ui.style());
    let text_width = |text: &str| {
        ui.painter()
            .layout_no_wrap(text.to_owned(), font.clone(), egui::Color32::WHITE)
            .size()
            .x
    };
    let padding = 2.0 * ui.spacing().item_spacing.x;
    texts.map(text_width).fold(0.0, f32::max) + padding
}

/// A window size that shows the whole estimate without scrolling, up to a limit.
fn fitting_size(ui: &egui::Ui, request: &ScanRequest, estimate: &Estimate) -> egui::Vec2 {
    let line = ui.text_style_height(&egui::TextStyle::Body) + ui.spacing().item_spacing.y;
    // Heading, two text lines, the footer note (two lines), the Scan button, and margins.
    let mut height = 8.0 * line + 40.0;
    height += estimate.classes().len() as f32 * line;
    if !request.location.prefix().is_empty() {
        height += line;
    }
    height += estimate.skipped.len() as f32 * line;
    let mut width = INITIAL_WIDTH;
    if request.location.is_all() {
        let [bucket, region] = name_widths(ui, estimate);
        let columns: f32 = NUMBER_COLUMNS.iter().sum();
        width = width.max(bucket + region + columns + WINDOW_PADDING);
        height += line + (estimate.buckets.len() + 1) as f32 * ROW_HEIGHT + 8.0;
    }
    egui::vec2(width, height.min(MAX_FITTED_HEIGHT))
}
