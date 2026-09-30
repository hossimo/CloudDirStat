use std::collections::HashSet;

use clouddirstat_core::{Filtered, NodeId, NodeKind, Tree, format_bytes, format_count, format_usd};
use eframe::egui::{self, Align, Color32, Label, Sense};
use egui_extras::{Column, TableBuilder};

use crate::palette::{self, Colors};

const ROW_HEIGHT: f32 = 20.0;
const INDENT: f32 = 16.0;
const MIN_NAME_WIDTH: f32 = 150.0;
/// Starting widths of size proportion, percent, size, and cost.
const NUMBER_COLUMNS: [f32; 4] = [110.0, 70.0, 90.0, 100.0];
/// Objects is last and fills the leftover width, but starts about this wide.
const OBJECTS_WIDTH: f32 = 90.0;

/// The folder list: one row per visible node, children sorted largest first.
#[derive(Default)]
pub struct TreeView {
    root: Option<NodeId>,
    expanded: HashSet<NodeId>,
    rows: Vec<Row>,
    stale: bool,
    scroll_to: Option<NodeId>,
}

#[derive(Clone, Copy)]
struct Row {
    id: NodeId,
    depth: u16,
}

impl TreeView {
    pub fn invalidate(&mut self) {
        self.stale = true;
    }

    /// Expands every folder above `id` and scrolls to it.
    pub fn reveal(&mut self, tree: &Tree, id: NodeId) {
        let mut current = tree.node(id).parent();
        while let Some(parent) = current {
            self.expanded.insert(parent);
            current = tree.node(parent).parent();
        }
        self.scroll_to = Some(id);
        self.stale = true;
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        view: Filtered,
        root: NodeId,
        root_label: &str,
        colors: &Colors,
        selected: &mut Option<NodeId>,
    ) {
        if self.root != Some(root) {
            self.root = Some(root);
            self.expanded.insert(root);
            self.stale = true;
        }
        let tree = view.tree;
        if self.stale {
            self.rebuild_rows(view, root);
        }

        let scroll_row = self
            .scroll_to
            .take()
            .and_then(|target| self.rows.iter().position(|row| row.id == target));

        // Name starts with whatever the number columns leave over; after that every
        // column keeps the width the user gives it, and the last column absorbs the
        // difference, so dragging a divider only moves columns to its right.
        let spacing = ui.spacing().item_spacing.x * (NUMBER_COLUMNS.len() + 1) as f32;
        let others: f32 = NUMBER_COLUMNS.iter().sum::<f32>() + OBJECTS_WIDTH + spacing;
        let name_width = (ui.available_width() - others).max(MIN_NAME_WIDTH);

        let mut table = TableBuilder::new(ui)
            .striped(true)
            .sense(Sense::click())
            .cell_layout(egui::Layout::left_to_right(Align::Center))
            .resizable(true)
            .column(
                Column::initial(name_width)
                    .at_least(MIN_NAME_WIDTH)
                    .clip(true),
            );
        for width in NUMBER_COLUMNS {
            table = table.column(Column::initial(width).at_least(40.0).clip(true));
        }
        table = table.column(Column::remainder().at_least(60.0).clip(true));
        if let Some(row) = scroll_row {
            table = table.scroll_to_row(row, Some(Align::Center));
        }

        let mut toggled = None;
        table
            .header(ROW_HEIGHT, |mut header| {
                for title in [
                    "Name",
                    "Size proportion",
                    "Percent",
                    "Size",
                    "Cost/mo",
                    "Objects",
                ] {
                    header.col(|ui| {
                        ui.strong(title);
                    });
                }
            })
            .body(|body| {
                body.rows(ROW_HEIGHT, self.rows.len(), |mut table_row| {
                    let row = self.rows[table_row.index()];
                    let node = tree.node(row.id);
                    let usage = view.usage(row.id);
                    table_row.set_selected(*selected == Some(row.id));

                    table_row.col(|ui| {
                        ui.add_space(f32::from(row.depth) * INDENT);
                        if self.expander(ui, tree, row.id) {
                            toggled = Some(row.id);
                        }
                        node_icon(ui, tree, row.id, colors);
                        let name = if row.id == root {
                            root_label
                        } else {
                            tree.name(row.id)
                        };
                        ui.add(Label::new(name).selectable(false).truncate());
                    });

                    let parent_bytes = node
                        .parent()
                        .filter(|_| row.id != root)
                        .map_or(usage.bytes, |parent| view.usage(parent).bytes);
                    let fraction = fraction(usage.bytes, parent_bytes);
                    table_row.col(|ui| proportion_bar(ui, fraction));
                    table_row.col(|ui| {
                        right_aligned(ui, format!("{:.1}%", fraction * 100.0));
                    });
                    table_row.col(|ui| right_aligned(ui, format_bytes(usage.bytes)));
                    table_row.col(|ui| right_aligned(ui, format_usd(usage.monthly_cost)));
                    table_row.col(|ui| right_aligned(ui, format_count(usage.objects)));

                    let response = table_row.response();
                    if response.clicked() {
                        *selected = Some(row.id);
                    }
                    if response.double_clicked() && node.kind() == NodeKind::Directory {
                        toggled = Some(row.id);
                    }
                });
            });

        if let Some(id) = toggled {
            if !self.expanded.remove(&id) {
                self.expanded.insert(id);
            }
            self.stale = true;
        }
    }

    /// Draws the expand/collapse arrow; returns whether it was clicked.
    fn expander(&self, ui: &mut egui::Ui, tree: &Tree, id: NodeId) -> bool {
        let size = egui::vec2(INDENT, INDENT);
        if !tree.node(id).has_children() {
            ui.add_space(size.x + ui.spacing().item_spacing.x);
            return false;
        }
        let (_, response) = ui.allocate_exact_size(size, Sense::click());
        let openness = if self.expanded.contains(&id) {
            1.0
        } else {
            0.0
        };
        egui::collapsing_header::paint_default_icon(ui, openness, &response);
        response.clicked()
    }

    fn rebuild_rows(&mut self, view: Filtered, root: NodeId) {
        self.rows.clear();
        let mut pending = vec![Row { id: root, depth: 0 }];
        while let Some(row) = pending.pop() {
            self.rows.push(row);
            if self.expanded.contains(&row.id) {
                let children = view.children_by_size(row.id);
                pending.extend(children.into_iter().rev().map(|id| Row {
                    id,
                    depth: row.depth.saturating_add(1),
                }));
            }
        }
        self.stale = false;
    }
}

fn node_icon(ui: &mut egui::Ui, tree: &Tree, id: NodeId, colors: &Colors) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), Sense::hover());
    let painter = ui.painter();
    match tree.node(id).kind() {
        NodeKind::Directory => {
            let body = egui::Rect::from_min_max(
                rect.left_top() + egui::vec2(0.0, 4.0),
                rect.right_bottom() - egui::vec2(0.0, 1.0),
            );
            let tab = egui::Rect::from_min_size(
                rect.left_top() + egui::vec2(0.0, 2.0),
                egui::vec2(6.0, 3.0),
            );
            painter.rect_filled(tab, 1.0, palette::FOLDER_ICON);
            painter.rect_filled(body, 1.0, palette::FOLDER_ICON);
        }
        NodeKind::Object => {
            painter.rect_filled(rect.shrink(2.0), 2.0, colors.node(tree, id));
        }
    }
}

fn proportion_bar(ui: &mut egui::Ui, fraction: f32) {
    let size = egui::vec2(ui.available_width() - 8.0, ROW_HEIGHT - 8.0);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 2.0, ui.visuals().extreme_bg_color);
    let filled =
        egui::Rect::from_min_size(rect.min, egui::vec2(rect.width() * fraction, rect.height()));
    painter.rect_filled(filled, 2.0, Color32::from_rgb(0x6f, 0x8f, 0xc0));
}

pub fn right_aligned(ui: &mut egui::Ui, text: String) {
    ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
        ui.add(Label::new(text).selectable(false));
    });
}

fn fraction(part: u64, whole: u64) -> f32 {
    if whole == 0 {
        0.0
    } else {
        (part as f64 / whole as f64) as f32
    }
}
