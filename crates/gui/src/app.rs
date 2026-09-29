use std::time::{Duration, Instant};

use clouddirstat_core::{EntryKind, NodeId, format_bytes, format_count, format_usd};
use clouddirstat_providers::s3::{S3Location, S3Pricing, ScanStats};
use eframe::egui;
use tokio::runtime::Runtime;

use crate::credentials_form::CredentialsForm;
use crate::help::HelpWindow;
use crate::legend;
use crate::palette::{ColorMode, Colors};
use crate::scan::{Scan, ScanRequest, ScanState};
use crate::tree_view::TreeView;
use crate::treemap_view::TreemapView;

/// Time per frame spent moving scanned entries into the tree.
const INSERT_BUDGET: Duration = Duration::from_millis(8);
const SCANNING_REPAINT_INTERVAL: Duration = Duration::from_millis(50);
/// While a scan is adding entries, re-sort the views at most this often.
const REFRESH_INTERVAL: Duration = Duration::from_millis(250);
const LEGEND_WIDTH: f32 = 520.0;

pub struct App {
    runtime: Runtime,
    location_input: String,
    credentials: CredentialsForm,
    help: HelpWindow,
    include_versions: bool,
    input_error: Option<String>,
    scan: Option<Scan>,
    view: View,
}

/// Everything shown for the current scan. Replaced wholesale when a new scan starts.
struct View {
    color_mode: ColorMode,
    colors: Option<Colors>,
    selected: Option<NodeId>,
    tree_view: TreeView,
    treemap: TreemapView,
    changed: bool,
    refreshed_at: Option<Instant>,
}

impl View {
    fn new(color_mode: ColorMode) -> Self {
        Self {
            color_mode,
            colors: None,
            selected: None,
            tree_view: TreeView::default(),
            treemap: TreemapView::default(),
            changed: true,
            refreshed_at: None,
        }
    }

    fn refresh(&mut self, scan: &Scan) {
        let due = self
            .refreshed_at
            .is_none_or(|at| at.elapsed() >= REFRESH_INTERVAL);
        if !self.changed || (scan.is_running() && !due) {
            return;
        }
        self.colors = Some(Colors::new(self.color_mode, &scan.tree, scan.root()));
        self.tree_view.invalidate();
        self.treemap.invalidate();
        self.changed = false;
        self.refreshed_at = Some(Instant::now());
    }
}

impl App {
    pub fn new(
        runtime: Runtime,
        ctx: &egui::Context,
        profile: Option<String>,
        include_versions: bool,
        initial_scan: Option<ScanRequest>,
    ) -> Self {
        let mut app = Self {
            runtime,
            location_input: String::new(),
            credentials: CredentialsForm::new(profile),
            help: HelpWindow::default(),
            include_versions,
            input_error: None,
            scan: None,
            view: View::new(ColorMode::StorageClass),
        };
        if let Some(request) = initial_scan {
            app.location_input = request.location.to_string();
            app.start_scan(request, ctx);
        }
        app
    }

    fn start_scan(&mut self, request: ScanRequest, ctx: &egui::Context) {
        self.input_error = None;
        self.view = View::new(self.view.color_mode);
        self.scan = Some(Scan::start(&self.runtime, request, ctx));
    }

    fn start_scan_from_inputs(&mut self, ctx: &egui::Context) {
        let location = match self.location_input.trim().parse::<S3Location>() {
            Ok(location) => location,
            Err(error) => {
                self.input_error = Some(error.to_string());
                return;
            }
        };
        let credentials = match self.credentials.source() {
            Ok(credentials) => credentials,
            Err(error) => {
                self.input_error = Some(error);
                return;
            }
        };
        let request = ScanRequest {
            location,
            credentials,
            include_versions: self.include_versions,
        };
        self.start_scan(request, ctx);
    }

    fn is_scanning(&self) -> bool {
        self.scan.as_ref().is_some_and(Scan::is_running)
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        let mut submitted = false;
        let mut scan_clicked = false;
        ui.horizontal(|ui| {
            ui.label("Location");
            let location = ui.add(
                egui::TextEdit::singleline(&mut self.location_input)
                    .hint_text("s3://bucket/prefix/  or  s3:// for all buckets")
                    .desired_width(320.0),
            );
            submitted |=
                location.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
            ui.separator();
            submitted |= self.credentials.show_main_row(ui);
            ui.separator();
            ui.checkbox(&mut self.include_versions, "Versions")
                .on_hover_text("Include noncurrent versions and delete markers");

            if self.is_scanning() {
                if ui.button("Stop").clicked()
                    && let Some(scan) = &mut self.scan
                {
                    scan.stop();
                }
            } else {
                scan_clicked = ui.button("Scan").clicked();
            }
            ui.separator();
            if ui
                .button("Help")
                .on_hover_text("Required permissions and how to sign in")
                .clicked()
            {
                self.help.toggle();
            }
        });
        submitted |= self.credentials.show_access_key_row(ui);

        if (scan_clicked || submitted) && !self.is_scanning() {
            self.start_scan_from_inputs(ui.ctx());
        }
    }

    fn status_bar(&self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if let Some(error) = &self.input_error {
                ui.colored_label(ui.visuals().error_fg_color, error);
                return;
            }
            let Some(scan) = &self.scan else {
                ui.label("Enter an S3 location and press Scan.");
                return;
            };

            let total = scan.tree.total();
            let summary = format!(
                "{}  {} in {} objects",
                scan.location,
                format_bytes(total.bytes),
                format_count(total.objects)
            );
            let elapsed = scan.elapsed().as_secs_f64();
            let uploads = scan.tree.usage_by_kind(EntryKind::IncompleteUpload);
            if uploads.objects > 0 {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    format!(
                        "{} in {} incomplete uploads (~{}/mo)",
                        format_bytes(uploads.bytes),
                        format_count(uploads.objects),
                        format_usd(uploads.monthly_cost)
                    ),
                )
                .on_hover_text(
                    "Parts of multipart uploads that were never completed or aborted. \
                     They are billed but hidden from normal listings. See them in the \
                     Versions tab, and clean them up with a lifecycle rule \
                     (AbortIncompleteMultipartUpload).",
                );
                ui.separator();
            }
            if let Some(pricing) = &scan.pricing {
                ui.label(format!("~{}/mo", format_usd(total.monthly_cost)))
                    .on_hover_text(pricing_note(pricing));
                ui.separator();
            }
            match &scan.state {
                ScanState::Running => {
                    ui.spinner();
                    ui.label(format!("Scanning {summary}  ({elapsed:.0}s)"));
                }
                ScanState::Finished { stats, .. } => {
                    ui.label(format!(
                        "{summary}  ·  scanned in {elapsed:.1}s using {} LIST requests (~${:.4})",
                        format_count(stats.list_requests),
                        stats.estimated_cost_usd()
                    ));
                    warnings(ui, stats);
                }
                ScanState::Stopped { .. } => {
                    ui.label(format!(
                        "{summary}  ·  stopped after {elapsed:.1}s (partial)"
                    ));
                }
                ScanState::Failed(error) => {
                    ui.colored_label(ui.visuals().error_fg_color, error);
                }
            }
        });
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if let Some(scan) = &mut self.scan {
            if scan.poll(INSERT_BUDGET) {
                self.view.changed = true;
            }
            if scan.is_running() {
                ui.ctx().request_repaint_after(SCANNING_REPAINT_INTERVAL);
            }
            self.view.refresh(scan);
        }

        egui::Panel::top("toolbar").show(ui, |ui| self.toolbar(ui));
        egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui));
        let bucket = self.location_input.trim().parse::<S3Location>().ok();
        let bucket = bucket.as_ref().map(|location| {
            if location.is_all_buckets() {
                "*"
            } else {
                location.bucket.as_str()
            }
        });
        self.help.show(ui.ctx(), bucket);

        let Some(scan) = &self.scan else {
            egui::CentralPanel::default().show(ui, |ui| {
                ui.centered_and_justified(|ui| ui.weak("No scan yet"));
            });
            return;
        };
        let view = &mut self.view;
        let Some(colors) = &view.colors else {
            return;
        };
        let tree = &scan.tree;
        let root = scan.root();

        if !scan.is_running() {
            let available = ui.available_height();
            egui::Panel::bottom("treemap")
                .resizable(true)
                .default_size(available * 0.55)
                // Always leave a few rows of the folder list visible.
                .max_size(available * 0.8)
                .show(ui, |ui| {
                    if let Some(clicked) = view.treemap.show(ui, tree, root, colors, view.selected)
                    {
                        view.selected = Some(clicked);
                        view.tree_view.reveal(tree, clicked);
                    }
                });
        }

        let mut color_mode = view.color_mode;
        egui::Panel::right("legend")
            .resizable(true)
            // Wide enough for the legend tables, so the panel doesn't grow on its
            // first frames and push the folder list's last columns out of view.
            .default_size(LEGEND_WIDTH)
            .show(ui, |ui| {
                let clicked = legend::show(
                    ui,
                    tree,
                    root,
                    &mut color_mode,
                    colors,
                    scan.include_versions,
                    view.selected,
                );
                if let Some(prefix) = clicked {
                    view.selected = Some(prefix);
                    view.tree_view.reveal(tree, prefix);
                }
            });

        egui::CentralPanel::default().show(ui, |ui| {
            view.tree_view.show(
                ui,
                tree,
                root,
                &scan.location.to_string(),
                colors,
                &mut view.selected,
            );
        });

        if color_mode != view.color_mode {
            view.color_mode = color_mode;
            view.changed = true;
            view.refreshed_at = None;
        }
    }
}

fn pricing_note(pricing: &S3Pricing) -> String {
    format!(
        "Estimated storage cost from {} list prices (AWS Price List, {}).
         First volume tier; Intelligent-Tiering at Frequent Access rates.
         Includes minimum billable sizes and archive overhead; excludes requests,
         retrieval, data transfer, and minimum storage duration charges.",
        pricing.region_label(),
        S3Pricing::published()
    )
}

/// Skipped buckets and checks that could not run, with the details on hover.
fn warnings(ui: &mut egui::Ui, stats: &ScanStats) {
    let skipped = stats
        .skipped_buckets
        .iter()
        .map(|skipped| format!("Skipped {}: {}", skipped.bucket, skipped.reason));
    let details: Vec<String> = skipped.chain(stats.warnings.iter().cloned()).collect();
    if details.is_empty() {
        return;
    }
    let summary = match (stats.skipped_buckets.len(), stats.warnings.len()) {
        (0, warnings) => format!("{warnings} warnings"),
        (buckets, 0) => format!("{buckets} buckets skipped"),
        (buckets, warnings) => format!("{buckets} buckets skipped, {warnings} warnings"),
    };
    ui.colored_label(ui.visuals().warn_fg_color, summary)
        .on_hover_text(details.join("\n"));
}
