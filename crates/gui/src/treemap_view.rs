use std::time::{Duration, Instant};

use clouddirstat_core::{NodeId, NodeKind, Tree, format_bytes, format_count, squarify};
use eframe::egui::{self, Color32, Pos2, Sense, Stroke, StrokeKind};

use crate::palette;

/// Smallest rectangle worth laying out, in square pixels.
const MIN_AREA: f32 = 2.0;
/// Smallest side that still gets an outline; below this the outline would hide the fill.
const MIN_OUTLINED_SIDE: f32 = 4.0;
/// While a scan is adding entries, relayout at most this often.
const RELAYOUT_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Default)]
pub struct TreemapView {
    layout: Vec<(NodeId, egui::Rect)>,
    laid_out_for: Option<egui::Rect>,
    laid_out_at: Option<Instant>,
    stale: bool,
}

impl TreemapView {
    /// Marks the layout out of date because the tree changed.
    pub fn invalidate(&mut self) {
        self.stale = true;
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn show(&mut self, ui: &mut egui::Ui, tree: &Tree) {
        let (response, painter) = ui.allocate_painter(ui.available_size(), Sense::hover());
        let bounds = response.rect;
        self.relayout_if_needed(tree, bounds);

        for &(id, rect) in &self.layout {
            match tree.storage_class(id) {
                Some(class) => draw_object(&painter, rect, palette::storage_class(class)),
                None => {
                    painter.rect_filled(rect, 0.0, palette::DIRECTORY);
                }
            }
        }

        let Some(pointer) = response.hover_pos() else {
            return;
        };
        let Some(&(hovered, rect)) = self.node_at(pointer) else {
            return;
        };
        painter.rect_stroke(
            rect,
            0.0,
            Stroke::new(2.0, palette::HIGHLIGHT),
            StrokeKind::Inside,
        );
        response.on_hover_ui_at_pointer(|ui| node_tooltip(ui, tree, hovered));
    }

    fn relayout_if_needed(&mut self, tree: &Tree, bounds: egui::Rect) {
        let resized = self.laid_out_for != Some(bounds);
        let due = self
            .laid_out_at
            .is_none_or(|at| at.elapsed() >= RELAYOUT_INTERVAL);
        if !resized && !(self.stale && due) {
            return;
        }

        let root = clouddirstat_core::Rect::new(
            bounds.min.x,
            bounds.min.y,
            bounds.width(),
            bounds.height(),
        );
        self.layout = squarify(tree, Tree::ROOT, root, MIN_AREA)
            .into_iter()
            .map(|(id, rect)| {
                let min = Pos2::new(rect.x, rect.y);
                (
                    id,
                    egui::Rect::from_min_size(min, egui::vec2(rect.w, rect.h)),
                )
            })
            .collect();
        self.laid_out_for = Some(bounds);
        self.laid_out_at = Some(Instant::now());
        self.stale = false;
    }

    /// The deepest node under `pointer`. Children come after their parents in the
    /// layout, so the last match is the deepest.
    fn node_at(&self, pointer: Pos2) -> Option<&(NodeId, egui::Rect)> {
        self.layout
            .iter()
            .rev()
            .find(|(_, rect)| rect.contains(pointer))
    }
}

fn draw_object(painter: &egui::Painter, rect: egui::Rect, color: Color32) {
    if rect.width() >= MIN_OUTLINED_SIDE && rect.height() >= MIN_OUTLINED_SIDE {
        let outline = Stroke::new(1.0, Color32::from_black_alpha(110));
        painter.rect(rect, 0.0, color, outline, StrokeKind::Inside);
    } else {
        painter.rect_filled(rect, 0.0, color);
    }
}

fn node_tooltip(ui: &mut egui::Ui, tree: &Tree, id: NodeId) {
    let node = tree.node(id);
    let path = tree.path(id);
    ui.strong(if path.is_empty() { "/" } else { &path });

    let usage = node.usage();
    let total = tree.total().bytes.max(1);
    ui.label(format!(
        "{}  ({:.1}% of total)",
        format_bytes(usage.bytes),
        usage.bytes as f64 * 100.0 / total as f64
    ));
    if node.kind() == NodeKind::Directory {
        ui.label(format!("{} objects", format_count(usage.objects)));
    }
    if let Some(class) = tree.storage_class(id) {
        ui.label(class);
    }
}
