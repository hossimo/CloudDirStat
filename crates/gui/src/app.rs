use std::time::Duration;

use clouddirstat_core::{Tree, format_bytes, format_count};
use clouddirstat_providers::s3::S3Location;
use eframe::egui::{self, Color32};
use tokio::runtime::Runtime;

use crate::palette;
use crate::scan::{Scan, ScanRequest, ScanState};
use crate::treemap_view::TreemapView;

/// Time per frame spent moving scanned entries into the tree.
const INSERT_BUDGET: Duration = Duration::from_millis(8);
const SCANNING_REPAINT_INTERVAL: Duration = Duration::from_millis(50);

pub struct App {
    runtime: Runtime,
    location_input: String,
    profile_input: String,
    include_versions: bool,
    input_error: Option<String>,
    scan: Option<Scan>,
    treemap: TreemapView,
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
            treemap: TreemapView::default(),
        };
        if let Some(request) = initial_scan {
            app.location_input = request.location.to_string();
            app.start_scan(request, ctx);
        }
        app
    }

    fn start_scan(&mut self, request: ScanRequest, ctx: &egui::Context) {
        self.input_error = None;
        self.treemap.reset();
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
                self.treemap.invalidate();
            }
            if scan.is_running() {
                ui.ctx().request_repaint_after(SCANNING_REPAINT_INTERVAL);
            }
        }

        egui::Panel::top("toolbar").show(ui, |ui| self.toolbar(ui));
        egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui));
        if let Some(scan) = &self.scan {
            egui::Panel::right("legend")
                .default_size(240.0)
                .show(ui, |ui| legend(ui, &scan.tree));
        }
        egui::CentralPanel::default().show(ui, |ui| match &self.scan {
            Some(scan) => self.treemap.show(ui, &scan.tree),
            None => {
                ui.centered_and_justified(|ui| ui.weak("No scan yet"));
            }
        });
    }
}

fn legend(ui: &mut egui::Ui, tree: &Tree) {
    ui.heading("Storage classes");
    let total = tree.total().bytes.max(1);
    egui::Grid::new("storage_classes")
        .num_columns(3)
        .striped(true)
        .show(ui, |ui| {
            for (class, usage) in tree.storage_classes() {
                ui.horizontal(|ui| {
                    swatch(ui, palette::storage_class(class));
                    ui.label(class);
                });
                ui.label(format_bytes(usage.bytes));
                ui.label(format!("{:.1}%", usage.bytes as f64 * 100.0 / total as f64));
                ui.end_row();
            }
        });
}

fn swatch(ui: &mut egui::Ui, color: Color32) {
    let size = egui::vec2(12.0, 12.0);
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    ui.painter().rect_filled(rect, 2.0, color);
}
