use std::time::{Duration, Instant};

use clouddirstat_core::{NodeId, format_bytes, format_count};
use clouddirstat_providers::s3::S3Location;
use eframe::egui;
use tokio::runtime::Runtime;

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

pub struct App {
    runtime: Runtime,
    location_input: String,
    profile_input: String,
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
            profile_input: profile.unwrap_or_default(),
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
        let profile = self.profile_input.trim();
        let request = ScanRequest {
            location,
            profile: (!profile.is_empty()).then(|| profile.to_owned()),
            include_versions: self.include_versions,
        };
        self.start_scan(request, ctx);
    }

    fn is_scanning(&self) -> bool {
        self.scan.as_ref().is_some_and(Scan::is_running)
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Location");
            let location = ui.add(
                egui::TextEdit::singleline(&mut self.location_input)
                    .hint_text("s3://bucket/prefix/")
                    .desired_width(320.0),
            );
            ui.label("Profile");
            let profile = ui.add(
                egui::TextEdit::singleline(&mut self.profile_input)
                    .hint_text("default")
                    .desired_width(120.0),
            );
            ui.checkbox(&mut self.include_versions, "Versions")
                .on_hover_text("Include noncurrent versions and delete markers");

            let submitted = (location.lost_focus() || profile.lost_focus())
                && ui.input(|input| input.key_pressed(egui::Key::Enter));
            if self.is_scanning() {
                if ui.button("Stop").clicked()
                    && let Some(scan) = &mut self.scan
                {
                    scan.stop();
                }
            } else if ui.button("Scan").clicked() || submitted {
                self.start_scan_from_inputs(ui.ctx());
            }
        });
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
            let height = ui.available_height() * 0.55;
            egui::Panel::bottom("treemap")
                .resizable(true)
                .default_size(height)
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
            .default_size(340.0)
            .show(ui, |ui| {
                legend::show(
                    ui,
                    tree,
                    root,
                    &mut color_mode,
                    colors,
                    scan.include_versions,
                );
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
