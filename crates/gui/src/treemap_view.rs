use std::time::{Duration, Instant};

use clouddirstat_core::{
    Filtered, NodeId, NodeKind, VersionState, format_bytes, format_count, format_usd, squarify,
};
use eframe::egui::{self, Color32, Pos2, Sense, Stroke, StrokeKind, TextureHandle, TextureOptions};

use crate::cushion;
use crate::palette::{self, Colors};

/// Smallest rectangle worth laying out, in square pixels.
const MIN_AREA: f32 = 1.5;
/// Redraw at most this often while the window is being resized.
const REDRAW_INTERVAL: Duration = Duration::from_millis(150);
#[derive(Default)]
pub struct TreemapView {
    /// Node rectangles in screen points, parents before children.
    layout: Vec<(NodeId, egui::Rect)>,
    texture: Option<TextureHandle>,
    drawn_for: Option<(egui::Rect, NodeId)>,
    drawn_at: Option<Instant>,
    stale: bool,
}

impl TreemapView {
    /// Marks the picture out of date because the tree or the colors changed.
    pub fn invalidate(&mut self) {
        self.stale = true;
    }

    /// Draws the treemap and returns the node that was clicked, if any.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        view: Filtered,
        root: NodeId,
        colors: &Colors,
        selected: Option<NodeId>,
    ) -> Option<NodeId> {
        let tree = view.tree;
        let (response, painter) = ui.allocate_painter(ui.available_size(), Sense::click());
        let bounds = response.rect;

        if self.stale || self.drawn_for != Some((bounds, root)) {
            let due = self
                .drawn_at
                .is_none_or(|at| at.elapsed() >= REDRAW_INTERVAL);
            if due || self.texture.is_none() {
                self.redraw(ui.ctx(), view, root, colors, bounds);
            } else {
                ui.ctx().request_repaint_after(REDRAW_INTERVAL);
            }
        }

        if let Some(texture) = &self.texture {
            let uv = egui::Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
            painter.image(texture.id(), bounds, uv, Color32::WHITE);
        }

        if let Some(rect) = selected.and_then(|id| self.rect_of(id)) {
            outline(&painter, rect, palette::SELECTED);
        }

        let hovered = response
            .hover_pos()
            .and_then(|pointer| self.node_at(pointer));
        let clicked = hovered.filter(|_| response.clicked());
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
        clicked
    }

    fn redraw(
        &mut self,
        ctx: &egui::Context,
        view: Filtered,
        root: NodeId,
        colors: &Colors,
        bounds: egui::Rect,
    ) {
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
            .into_iter()
            .map(|(id, rect)| {
                let min = bounds.min + egui::vec2(rect.x, rect.y) / pixels_per_point;
                let size = egui::vec2(rect.w, rect.h) / pixels_per_point;
                (id, egui::Rect::from_min_size(min, size))
            })
            .collect();
        self.drawn_for = Some((bounds, root));
        self.drawn_at = Some(Instant::now());
        self.stale = false;
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
        ui.label(format!("{} objects", format_count(usage.objects)));
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
