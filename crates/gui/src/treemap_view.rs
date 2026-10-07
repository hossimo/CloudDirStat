use std::time::{Duration, Instant};

use clouddirstat_core::{
    Filtered, NodeId, NodeKind, Tree, VersionState, format_bytes, format_counted, format_usd,
    squarify,
};
use eframe::egui::{
    self, Color32, PointerButton, Pos2, Sense, Stroke, StrokeKind, TextureHandle, TextureOptions,
};

use crate::cushion;
use crate::filter::Filter;
use crate::palette::{self, Colors};

/// Smallest rectangle worth laying out, in square pixels.
const MIN_AREA: f32 = 1.5;
/// Redraw at most this often while the window is being resized.
const REDRAW_INTERVAL: Duration = Duration::from_millis(150);
/// While a scan is adding objects, redraw at most this often...
const SCANNING_REDRAW_INTERVAL: Duration = Duration::from_secs(1);
/// ...and wait at least this many times as long as the last redraw took, so the
/// treemap never takes more than a small share of the time the tree is built in.
const SCANNING_REDRAW_FACTOR: u32 = 10;
/// What the user did in the treemap.
pub enum Action {
    Select(NodeId),
    /// Double-click: show this folder, one level below the current root, on its own.
    ZoomIn(NodeId),
    /// Right-click or the mouse back button: show the current root's parent.
    ZoomOut,
}

#[derive(Default)]
pub struct TreemapView {
    /// Node rectangles in screen points, parents before children.
    layout: Vec<(NodeId, egui::Rect)>,
    /// The same rectangles in texture pixels.
    pixel_layout: Vec<(NodeId, clouddirstat_core::Rect)>,
    texture: Option<TextureHandle>,
    /// The overlay for a hovered legend row, until the treemap is redrawn.
    highlight: Option<(Filter, TextureHandle)>,
    drawn_for: Option<(egui::Rect, NodeId)>,
    drawn_at: Option<Instant>,
    /// How long the last redraw took.
    redraw_time: Duration,
    stale: bool,
}

impl TreemapView {
    /// Marks the picture out of date because the tree or the colors changed.
    pub fn invalidate(&mut self) {
        self.stale = true;
    }

    /// Draws the treemap with `root` filling it, and returns what the user did.
    /// While `scanning`, a tree that keeps changing is redrawn only now and then.
    /// `highlight` dims everything outside a legend row (for a scan rooted at
    /// `scan_root`).
    #[allow(clippy::too_many_arguments)]
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        view: Filtered,
        root: NodeId,
        colors: &Colors,
        selected: Option<NodeId>,
        scanning: bool,
        highlight: Option<&Filter>,
        scan_root: NodeId,
    ) -> Option<Action> {
        let tree = view.tree;
        let (response, painter) = ui.allocate_painter(ui.available_size(), Sense::click());
        let bounds = response.rect;

        // Zooming redraws at once; the user is waiting for it.
        let zoomed = self
            .drawn_for
            .is_none_or(|(_, drawn_root)| drawn_root != root);
        if self.stale || self.drawn_for != Some((bounds, root)) {
            let interval = self.redraw_interval(scanning);
            let due = self.drawn_at.is_none_or(|at| at.elapsed() >= interval);
            if due || zoomed || self.texture.is_none() {
                self.redraw(ui.ctx(), view, root, colors, bounds);
            } else {
                ui.ctx().request_repaint_after(interval);
            }
        }

        let uv = egui::Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
        if let Some(texture) = self.texture.as_ref().map(TextureHandle::id) {
            painter.image(texture, bounds, uv, Color32::WHITE);
            if let Some(filter) = highlight {
                let mask = self.highlight_mask(ui.ctx(), tree, filter, scan_root);
                painter.image(mask, bounds, uv, Color32::WHITE);
            }
        }

        if let Some(rect) = selected.and_then(|id| self.rect_of(id)) {
            outline(&painter, rect, palette::SELECTED);
        }

        let hovered = response
            .hover_pos()
            .and_then(|pointer| self.node_at(pointer));
        let action = if response.secondary_clicked() || response.clicked_by(PointerButton::Extra1) {
            Some(Action::ZoomOut)
        } else if response.double_clicked() {
            // Zoom one level at a time toward the object under the pointer.
            hovered
                .and_then(|id| tree.child_toward(root, id))
                .filter(|&child| tree.node(child).kind() == NodeKind::Directory)
                .map(Action::ZoomIn)
        } else if response.clicked() {
            hovered.map(Action::Select)
        } else {
            None
        };
        if let Some(id) = hovered {
            let folder = tree.node(id).parent().filter(|&parent| parent != root);
            if let Some(rect) = folder.and_then(|folder| self.rect_of(folder)) {
                outline(&painter, rect, palette::CONTAINING_FOLDER);
            }
            if let Some(rect) = self.rect_of(id) {
                outline(&painter, rect, palette::HOVER);
            }
            response.on_hover_ui_at_pointer(|ui| node_tooltip(ui, view, root, id));
        }
        action
    }

    fn redraw_interval(&self, scanning: bool) -> Duration {
        if scanning {
            SCANNING_REDRAW_INTERVAL.max(self.redraw_time * SCANNING_REDRAW_FACTOR)
        } else {
            REDRAW_INTERVAL
        }
    }

    fn redraw(
        &mut self,
        ctx: &egui::Context,
        view: Filtered,
        root: NodeId,
        colors: &Colors,
        bounds: egui::Rect,
    ) {
        let started = Instant::now();
        let tree = view.tree;
        let pixels_per_point = ctx.pixels_per_point();
        let width = (bounds.width() * pixels_per_point).round().max(1.0);
        let height = (bounds.height() * pixels_per_point).round().max(1.0);

        let pixel_bounds = clouddirstat_core::Rect::new(0.0, 0.0, width, height);
        let layout = squarify(view, root, pixel_bounds, MIN_AREA);
        let image = cushion::render(tree, &layout, [width as usize, height as usize], |id| {
            colors.node(tree, id)
        });

        match &mut self.texture {
            Some(texture) => texture.set(image, TextureOptions::NEAREST),
            None => {
                self.texture = Some(ctx.load_texture("treemap", image, TextureOptions::NEAREST));
            }
        }

        self.layout = layout
            .iter()
            .map(|&(id, rect)| {
                let min = bounds.min + egui::vec2(rect.x, rect.y) / pixels_per_point;
                let size = egui::vec2(rect.w, rect.h) / pixels_per_point;
                (id, egui::Rect::from_min_size(min, size))
            })
            .collect();
        self.pixel_layout = layout;
        self.highlight = None;
        self.drawn_for = Some((bounds, root));
        self.drawn_at = Some(Instant::now());
        self.redraw_time = started.elapsed();
        self.stale = false;
    }

    /// The overlay for `filter`, made once per hovered row and redraw.
    fn highlight_mask(
        &mut self,
        ctx: &egui::Context,
        tree: &Tree,
        filter: &Filter,
        scan_root: NodeId,
    ) -> egui::TextureId {
        if let Some((drawn, mask)) = &self.highlight
            && drawn == filter
        {
            return mask.id();
        }
        let size = self
            .texture
            .as_ref()
            .map_or([1, 1], |texture| texture.size());
        let image = cushion::highlight_mask(
            tree,
            &self.pixel_layout,
            size,
            filter.matcher(tree, scan_root),
        );
        let mask = ctx.load_texture("treemap highlight", image, TextureOptions::NEAREST);
        let id = mask.id();
        self.highlight = Some((filter.clone(), mask));
        id
    }

    /// The deepest node under `pointer`. Children come after their parents in the
    /// layout, so the last match is the deepest.
    fn node_at(&self, pointer: Pos2) -> Option<NodeId> {
        self.layout
            .iter()
            .rev()
            .find(|(_, rect)| rect.contains(pointer))
            .map(|&(id, _)| id)
    }

    fn rect_of(&self, id: NodeId) -> Option<egui::Rect> {
        self.layout
            .iter()
            .find(|&&(node, _)| node == id)
            .map(|&(_, rect)| rect)
    }
}

fn outline(painter: &egui::Painter, rect: egui::Rect, color: Color32) {
    painter.rect_stroke(rect, 0.0, Stroke::new(2.0, color), StrokeKind::Inside);
}

fn node_tooltip(ui: &mut egui::Ui, view: Filtered, root: NodeId, id: NodeId) {
    let tree = view.tree;
    let node = tree.node(id);
    ui.strong(tree.name(id));

    let usage = view.usage(id);
    let total = view.usage(root).bytes.max(1);
    ui.label(format!(
        "{}  ({:.1}% of total)",
        format_bytes(usage.bytes),
        usage.bytes as f64 * 100.0 / total as f64
    ));
    if tree.has_pricing() {
        ui.label(format!("~{} per month", format_usd(usage.monthly_cost)));
    }
    if node.kind() == NodeKind::Directory {
        ui.label(format_counted(usage.objects, "object"));
    }
    if let Some(class) = tree.storage_class(id) {
        ui.label(class);
    }
    if let Some(date) = view.last_modified(id) {
        ui.label(format!("Last modified {date}"));
    }
    if let Some(state) = tree
        .version_state(id)
        .filter(|&state| state != VersionState::Current)
    {
        ui.label(state.label());
    }
    if let Some(folder) = node.parent() {
        let path = tree.path(folder);
        ui.weak(format!("in {}", if path.is_empty() { "/" } else { &path }));
    }
}
