use std::time::{Duration, Instant};

use clouddirstat_core::{
    EntryKind, Filtered, NodeId, Subset, Tree, format_bytes, format_count, format_counted,
    format_usd,
};
use clouddirstat_providers::{Location, Provider, ScanStats};
use eframe::egui;
use tokio::runtime::Runtime;

use crate::cloud_picker::{self, Scheme};
use crate::credentials_form::CredentialsForm;
use crate::error_view;
use crate::estimate_view::{self, EstimateTask};
use crate::filter::Filter;
use crate::help::HelpWindow;
use crate::largest_files::{self, LargestFiles};
use crate::legend;
use crate::palette::{ColorMode, Colors};
use crate::scan::{Scan, ScanRequest, ScanState};
use crate::tree_view::TreeView;
use crate::treemap_view::{self, TreemapView};

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
    /// Set in Help; off by default.
    version_in_title: bool,
    include_versions: bool,
    input_error: Option<String>,
    scan: Option<Scan>,
    /// The Estimate window, while it is open.
    estimate: Option<EstimateTask>,
    view: View,
}

/// Which list fills the middle of the window.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ListTab {
    Folders,
    LargestFiles,
}

/// Everything shown for the current scan. Replaced wholesale when a new scan starts.
struct View {
    color_mode: ColorMode,
    colors: Option<Colors>,
    /// Limits the folder list and treemap to one legend row's objects.
    filter: Option<Filter>,
    /// The objects that pass `filter`, recomputed with the colors.
    subset: Option<Subset>,
    selected: Option<NodeId>,
    /// The folder the treemap is zoomed into; `None` shows the whole scan.
    zoom: Option<NodeId>,
    list: ListTab,
    tree_view: TreeView,
    largest: LargestFiles,
    treemap: TreemapView,
    changed: bool,
    refreshed_at: Option<Instant>,
}

impl View {
    fn new(color_mode: ColorMode) -> Self {
        Self {
            color_mode,
            colors: None,
            filter: None,
            subset: None,
            selected: None,
            zoom: None,
            list: ListTab::Folders,
            tree_view: TreeView::default(),
            largest: LargestFiles::default(),
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
        self.subset = self
            .filter
            .as_ref()
            .map(|filter| filter.subset(&scan.tree, scan.root()));
        self.tree_view.invalidate();
        self.largest.invalidate();
        self.treemap.invalidate();
        self.changed = false;
        self.refreshed_at = Some(Instant::now());
    }

    /// Sets the filter, or clears it when `filter` is the one already set.
    fn toggle_filter(&mut self, filter: Filter) {
        let filter = (self.filter.as_ref() != Some(&filter)).then_some(filter);
        self.set_filter(filter);
    }

    fn set_filter(&mut self, filter: Option<Filter>) {
        self.filter = filter;
        self.changed = true;
        self.refreshed_at = None;
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
            version_in_title: false,
            include_versions,
            input_error: None,
            scan: None,
            estimate: None,
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

    /// The location, credentials, and options in the toolbar, or `None` (with the
    /// problem shown in the status bar) when they are not valid.
    fn request_from_inputs(&mut self) -> Option<ScanRequest> {
        let location = match self.location_input.trim().parse::<Location>() {
            Ok(location) => location,
            Err(error) => {
                self.input_error = Some(error.to_string());
                return None;
            }
        };
        let credentials = match self.credentials.credentials(location.provider()) {
            Ok(credentials) => credentials,
            Err(error) => {
                self.input_error = Some(error);
                return None;
            }
        };
        self.input_error = None;
        Some(ScanRequest {
            location,
            credentials,
            include_versions: self.include_versions,
        })
    }

    fn start_scan_from_inputs(&mut self, ctx: &egui::Context) {
        if let Some(request) = self.request_from_inputs() {
            self.start_scan(request, ctx);
        }
    }

    fn start_estimate_from_inputs(&mut self, ctx: &egui::Context) {
        if let Some(request) = self.request_from_inputs() {
            self.estimate = Some(EstimateTask::start(&self.runtime, request, ctx));
        }
    }

    fn estimate_window(&mut self, ctx: &egui::Context) {
        let scanning = self.is_scanning();
        let Some(task) = &mut self.estimate else {
            return;
        };
        let mut open = true;
        let action = task.show(ctx, &mut open, scanning);
        if !open {
            self.estimate = None;
        }
        match action {
            Some(estimate_view::Action::Scan(request)) => {
                self.estimate = None;
                self.location_input = request.location.to_string();
                self.start_scan(*request, ctx);
            }
            Some(estimate_view::Action::Choose(bucket)) => {
                self.location_input = format!("s3://{bucket}/");
            }
            None => {}
        }
    }

    fn is_scanning(&self) -> bool {
        self.scan.as_ref().is_some_and(Scan::is_running)
    }

    /// The provider of the location being typed, going by its scheme, so the sign-in
    /// fields match it even before the location is complete.
    fn provider(&self) -> Provider {
        cloud_picker::provider_of(&self.location_input).unwrap_or(Provider::S3)
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        let provider = self.provider();
        let mut submitted = false;
        let mut scan_clicked = false;
        let mut estimate_clicked = false;
        // Wraps onto a second line in a narrow window instead of running off the edge.
        ui.horizontal_wrapped(|ui| {
            ui.label("Location");
            let picked = cloud_picker::show(ui, &mut self.location_input);
            let location = ui.add(
                egui::TextEdit::singleline(&mut self.location_input)
                    .id(egui::Id::new(LOCATION_FIELD))
                    .hint_text("bucket/folder/  or  account/container/folder/")
                    .desired_width(320.0),
            );
            if picked {
                focus_at_end(ui.ctx(), &self.location_input);
            }
            if let Some(error) = location_error(&self.location_input, location.has_focus()) {
                let stroke = egui::Stroke::new(1.5, ui.visuals().error_fg_color);
                ui.painter().rect_stroke(
                    location.rect.expand(1.0),
                    2.0,
                    stroke,
                    egui::StrokeKind::Outside,
                );
                location.clone().on_hover_text(error);
            }
            submitted |=
                location.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
            ui.separator();
            submitted |= self.credentials.show_main_row(ui, provider);
            ui.separator();
            ui.checkbox(&mut self.include_versions, "Versions")
                .on_hover_text(provider.versions_help());

            if self.is_scanning() {
                if ui.button("Stop").clicked()
                    && let Some(scan) = &mut self.scan
                {
                    scan.stop();
                }
            } else {
                scan_clicked = ui.button("Scan").clicked();
            }
            estimate_clicked = ui
                .add_enabled(provider == Provider::S3, egui::Button::new("Estimate"))
                .on_hover_text(
                    "Bucket size, object count, monthly cost, and what a full scan would \
                     cost, from CloudWatch without listing (needs cloudwatch:GetMetricData)",
                )
                .on_disabled_hover_text("Estimates are only available for Amazon S3 so far.")
                .clicked();
            ui.separator();
            if ui
                .button("Help")
                .on_hover_text("Required permissions and how to sign in")
                .clicked()
            {
                self.help.toggle();
            }
        });
        submitted |= self.credentials.show_secret_row(ui, provider);

        if (scan_clicked || submitted) && !self.is_scanning() {
            self.start_scan_from_inputs(ui.ctx());
        }
        if estimate_clicked {
            self.start_estimate_from_inputs(ui.ctx());
        }
    }

    fn status_bar(&self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                zoom_indicator(ui);
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    self.status_summary(ui);
                });
            });
        });
    }

    fn status_summary(&self, ui: &mut egui::Ui) {
        if let Some(error) = &self.input_error {
            error_view::chip(ui, error);
            return;
        }
        let Some(scan) = &self.scan else {
            ui.label("Enter a location and press Scan.");
            return;
        };

        let total = scan.tree.total();
        let summary = format!(
            "{}  {} in {}",
            scan.location,
            format_bytes(total.bytes),
            format_counted(total.objects, "object")
        );
        let elapsed = scan.elapsed().as_secs_f64();
        let uploads = scan.tree.usage_by_kind(EntryKind::IncompleteUpload);
        if uploads.objects > 0 {
            ui.colored_label(
                ui.visuals().warn_fg_color,
                format!(
                    "{} in {} (~{}/mo)",
                    format_bytes(uploads.bytes),
                    format_counted(uploads.objects, "incomplete upload"),
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
                .on_hover_text(pricing.notes());
            ui.separator();
        }
        match &scan.state {
            // The summary comes last and is cut off to fit, so the progress bar
            // and warnings stay visible in a narrow window.
            ScanState::Running => {
                ui.spinner();
                if let Some(expected) = scan.expected_objects {
                    let fraction = total.objects as f32 / expected as f32;
                    ui.add(
                        egui::ProgressBar::new(fraction.min(0.99))
                            .desired_width(120.0)
                            .show_percentage(),
                    )
                    .on_hover_text(format!(
                        "About {} objects in total, from CloudWatch. Its count is a \
                             day old and includes every version, so this is approximate.",
                        format_count(expected)
                    ));
                }
                fitted(ui, format!("Scanning {summary}  ({elapsed:.0}s)"));
            }
            ScanState::Finished { stats, .. } => {
                warnings(ui, stats);
                fitted(
                    ui,
                    format!(
                        "{summary}  ·  scanned in {elapsed:.1}s using {} LIST requests \
                             (~${:.4})",
                        format_count(stats.list_requests),
                        stats.estimated_cost_usd()
                    ),
                );
            }
            ScanState::Stopped { .. } => {
                fitted(
                    ui,
                    format!("{summary}  ·  stopped after {elapsed:.1}s (partial)"),
                );
            }
            ScanState::Failed(error) => {
                error_view::chip(ui, error);
            }
        }
    }
}

pub fn window_title(with_version: bool) -> String {
    if with_version {
        format!("CloudDirStat {}", clouddirstat_core::VERSION)
    } else {
        "CloudDirStat".to_owned()
    }
}

/// The UI zoom (Ctrl + / Ctrl −) when it isn't 100%; clicking it goes back to 100%.
fn zoom_indicator(ui: &mut egui::Ui) {
    let zoom = ui.ctx().zoom_factor();
    if (zoom - 1.0).abs() < 0.005 {
        return;
    }
    let percent = (zoom * 100.0).round();
    if ui
        .small_button(format!("🔍 {percent}%"))
        .on_hover_text("Zoom: click to reset to 100% (Ctrl+0)")
        .clicked()
    {
        ui.ctx().set_zoom_factor(1.0);
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
        // The policy in Help is for S3, filled in with the bucket being typed.
        let location = self.location_input.trim().parse::<Location>().ok();
        let bucket = location
            .as_ref()
            .filter(|location| location.provider() == Provider::S3)
            .map(|location| location.bucket().unwrap_or("*"));
        let version_in_title = self.version_in_title;
        self.help.show(ui.ctx(), bucket, &mut self.version_in_title);
        if self.version_in_title != version_in_title {
            let title = window_title(self.version_in_title);
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::Title(title));
        }
        self.estimate_window(ui.ctx());

        let Some(scan) = &self.scan else {
            egui::CentralPanel::default().show(ui, |ui| {
                ui.centered_and_justified(|ui| ui.weak("No scan yet"));
            });
            return;
        };
        // A scan that failed before finding anything has nothing to show but the error,
        // which gets the room the lists would have used.
        if let ScanState::Failed(error) = &scan.state
            && scan.tree.total().objects == 0
        {
            let action = egui::CentralPanel::default()
                .show(ui, |ui| error_view::card(ui, "Scan failed", error))
                .inner;
            if let Some(error_view::Action::Help) = action {
                self.help.open();
            }
            return;
        }
        let view = &mut self.view;
        let Some(colors) = &view.colors else {
            return;
        };
        let tree = &scan.tree;
        let root = scan.root();
        let filtered = Filtered::new(tree, view.subset.as_ref());
        // The subset catches up with a new filter on the next refresh.
        let filter_pending = view.filter.is_some() != view.subset.is_some();
        let mut new_filter = None;
        let mut clear_filter = view.filter.is_some()
            && !ui.ctx().egui_wants_keyboard_input()
            && ui.input(|input| input.key_pressed(egui::Key::Escape));

        // Nothing to draw until the scan finds something.
        if tree.total().objects > 0 {
            let available = ui.available_height();
            egui::Panel::bottom("treemap")
                .resizable(true)
                .default_size(available * 0.55)
                // Always leave a few rows of the folder list visible.
                .max_size(available * 0.8)
                .show(ui, |ui| {
                    if let Some(zoom) =
                        zoom_bar(ui, tree, root, &scan.location.to_string(), view.zoom)
                    {
                        view.zoom = zoom;
                    }
                    let map_root = view.zoom.unwrap_or(root);
                    if view.filter.is_some() && filtered.usage(map_root).objects == 0 {
                        ui.centered_and_justified(|ui| {
                            ui.weak(match (filter_pending, view.zoom) {
                                (true, _) => "Filtering…",
                                (false, None) => "Nothing matches the filter",
                                (false, Some(_)) => "Nothing in this folder matches the filter",
                            })
                        });
                        return;
                    }
                    match view.treemap.show(
                        ui,
                        filtered,
                        map_root,
                        colors,
                        view.selected,
                        scan.is_running(),
                    ) {
                        Some(treemap_view::Action::Select(clicked)) => {
                            view.selected = Some(clicked);
                            view.tree_view.reveal(tree, clicked);
                        }
                        Some(treemap_view::Action::ZoomIn(folder)) => view.zoom = Some(folder),
                        Some(treemap_view::Action::ZoomOut) => {
                            view.zoom = view.zoom.and_then(|zoom| {
                                tree.node(zoom).parent().filter(|&parent| parent != root)
                            });
                        }
                        None => {}
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
                    scan,
                    view.filter.as_ref(),
                );
                if let Some(Filter::Prefix(prefix)) = clicked {
                    view.selected = Some(prefix);
                    view.tree_view.reveal(tree, prefix);
                }
                new_filter = clicked;
            });

        egui::CentralPanel::default().show(ui, |ui| {
            if let Some(filter) = &view.filter {
                clear_filter |= filter_bar(ui, &filter.label(tree), filtered, root);
                ui.add_space(4.0);
            }
            ui.horizontal(|ui| {
                ui.selectable_value(&mut view.list, ListTab::Folders, "Folders");
                ui.selectable_value(&mut view.list, ListTab::LargestFiles, "Largest files");
            });
            match view.list {
                ListTab::Folders => {
                    let zoom = view.tree_view.show(
                        ui,
                        filtered,
                        root,
                        &scan.location.to_string(),
                        colors,
                        &mut view.selected,
                    );
                    if let Some(folder) = zoom {
                        view.zoom = (folder != root).then_some(folder);
                    }
                }
                ListTab::LargestFiles => {
                    let per_bucket = scan.location.is_all();
                    let action =
                        view.largest
                            .show(ui, filtered, root, per_bucket, colors, view.selected);
                    match action {
                        Some(largest_files::Action::Select(file)) => {
                            view.selected = Some(file);
                            view.tree_view.reveal(tree, file);
                        }
                        Some(largest_files::Action::Reveal(file)) => {
                            view.selected = Some(file);
                            view.tree_view.reveal(tree, file);
                            view.list = ListTab::Folders;
                        }
                        None => {}
                    }
                }
            }
        });

        if clear_filter {
            view.set_filter(None);
        } else if let Some(filter) = new_filter {
            view.toggle_filter(filter);
        }
        if color_mode != view.color_mode {
            view.color_mode = color_mode;
            view.changed = true;
            view.refreshed_at = None;
        }
    }
}

/// The folders from the scan root down to where the treemap is zoomed, each a link
/// back to that level. Returns the zoom to switch to, if one was clicked.
fn zoom_bar(
    ui: &mut egui::Ui,
    tree: &Tree,
    root: NodeId,
    root_label: &str,
    zoom: Option<NodeId>,
) -> Option<Option<NodeId>> {
    let mut path = Vec::new();
    let mut current = zoom;
    while let Some(folder) = current.filter(|&folder| folder != root) {
        path.push(folder);
        current = tree.node(folder).parent();
    }
    path.reverse();

    let mut clicked = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        if path.is_empty() {
            ui.strong(root_label);
        } else if ui.link(root_label).clicked() {
            clicked = Some(None);
        }
        for (index, &folder) in path.iter().enumerate() {
            let name = format!("{}/", tree.name(folder));
            if index + 1 == path.len() {
                ui.strong(name);
            } else if ui.link(name).clicked() {
                clicked = Some(Some(folder));
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            if zoom.is_some() && ui.button("Zoom out").clicked() {
                let parent = path.len().checked_sub(2).map(|index| path[index]);
                clicked = Some(parent);
            }
            ui.weak(if zoom.is_some() {
                "Double-click to zoom in, right-click to zoom out"
            } else {
                "Double-click to zoom in"
            });
        });
    });
    clicked
}

/// Shows which filter is on and what passes it. Returns whether Clear was clicked.
fn filter_bar(ui: &mut egui::Ui, label: &str, filtered: Filtered, root: NodeId) -> bool {
    let visuals = ui.visuals().selection;
    let usage = filtered.usage(root);
    let total = filtered.tree.node(root).usage();
    let share = if total.bytes == 0 {
        0.0
    } else {
        usage.bytes as f64 * 100.0 / total.bytes as f64
    };
    egui::Frame::new()
        .fill(visuals.bg_fill)
        .corner_radius(4.0)
        .inner_margin(egui::Margin::symmetric(8, 4))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let text = |text: String| egui::RichText::new(text).color(visuals.stroke.color);
                ui.label(text(format!("Filtered: {label}")).strong());
                let mut summary = format!(
                    "{} in {} ({share:.1}% of {})",
                    format_bytes(usage.bytes),
                    format_counted(usage.objects, "object"),
                    format_bytes(total.bytes),
                );
                if filtered.tree.has_pricing() {
                    summary += &format!(" · ~{}/mo", format_usd(usage.monthly_cost));
                }
                ui.label(text(summary));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.button("Clear filter")
                        .on_hover_text("Show everything again (Esc)")
                        .clicked()
                })
                .inner
            })
            .inner
        })
        .inner
}

/// The Location field's id, so choosing a cloud can put the cursor back in it.
const LOCATION_FIELD: &str = "location";

/// What is wrong with the location being typed, for the field's error outline. An
/// unknown scheme shows at once; other mistakes (e.g. a bucket name still being typed)
/// only once the field loses focus.
fn location_error(input: &str, typing: bool) -> Option<String> {
    let input = input.trim();
    if input.is_empty() {
        return None;
    }
    if cloud_picker::scheme_of(input) == Scheme::Unknown {
        return Some(
            "Unknown scheme: locations start with s3://, gs://, or az:// (or choose S3, \
             Google, or Azure on the left)"
                .to_owned(),
        );
    }
    if typing {
        return None;
    }
    input
        .parse::<Location>()
        .err()
        .map(|error| error.to_string())
}

/// Focuses the Location field with the cursor after `text`, ready to type the rest.
fn focus_at_end(ctx: &egui::Context, text: &str) {
    use egui::text::{CCursor, CCursorRange};
    let id = egui::Id::new(LOCATION_FIELD);
    let mut state = egui::text_edit::TextEditState::load(ctx, id).unwrap_or_default();
    let end = CCursor::new(text.chars().count());
    state.cursor.set_char_range(Some(CCursorRange::one(end)));
    state.store(ctx, id);
    ctx.memory_mut(|memory| memory.request_focus(id));
}

/// A label cut off with "…" to fit the space left; hovering shows all of it.
fn fitted(ui: &mut egui::Ui, text: String) {
    ui.add(egui::Label::new(text).truncate());
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
        (0, warnings) => format_counted(warnings as u64, "warning"),
        (buckets, 0) => format!("{} skipped", format_counted(buckets as u64, "bucket")),
        (buckets, warnings) => format!(
            "{} skipped, {}",
            format_counted(buckets as u64, "bucket"),
            format_counted(warnings as u64, "warning")
        ),
    };
    ui.colored_label(ui.visuals().warn_fg_color, summary)
        .on_hover_text(details.join("\n"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_schemes_are_flagged_while_typing() {
        assert!(location_error("ftp://host/x", true).is_some());
        assert!(location_error("s4://bucket", false).is_some());
    }

    #[test]
    fn other_mistakes_wait_until_the_field_is_left() {
        // "s3://my" is a bucket name still being typed.
        assert_eq!(location_error("s3://my", true), None);
        assert!(location_error("s3://my", false).is_some());
        assert_eq!(location_error("", false), None);
        assert_eq!(location_error("gs://my-bucket/logs/", false), None);
        assert_eq!(location_error("my-bucket/logs/", false), None);
    }
}
