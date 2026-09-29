use clouddirstat_core::{
    EntryKind, NodeId, Tree, Usage, VersionState, format_bytes, format_count, format_usd,
};
use eframe::egui::{self, Align, Color32, Label, Sense};
use egui_extras::{Column, TableBuilder};

use crate::palette::{self, ColorMode, Colors};
use crate::tree_view::right_aligned;

struct LegendRow {
    color: Option<Color32>,
    label: String,
    usage: Usage,
    /// The folder this row stands for, if clicking it should select one.
    node: Option<NodeId>,
}

/// The right-hand panel: tabs pick what the treemap is colored by, and the list
/// below explains the colors. Returns the prefix the user clicked, if any.
pub fn show(
    ui: &mut egui::Ui,
    tree: &Tree,
    root: NodeId,
    mode: &mut ColorMode,
    colors: &Colors,
    versions_scanned: bool,
    selected: Option<NodeId>,
) -> Option<NodeId> {
    ui.horizontal(|ui| {
        for option in ColorMode::ALL {
            ui.selectable_value(mode, option, option.label());
        }
    });
    ui.separator();

    let total = tree.node(root).usage();
    egui::ScrollArea::vertical()
        .show(ui, |ui| match *mode {
            ColorMode::StorageClass => {
                table(ui, "Storage class", &storage_class_rows(tree), total, None)
            }
            ColorMode::Prefixes => {
                let rows = prefix_rows(tree, root, colors);
                table(ui, "Prefix", &rows, total, selected)
            }
            ColorMode::Versions => {
                table(ui, "Object state", &version_state_rows(tree), total, None);
                ui.add_space(12.0);
                table(ui, "Version type", &entry_kind_rows(tree), total, None);
                if !versions_scanned {
                    ui.add_space(12.0);
                    ui.weak(
                        "Scan with Versions checked to include noncurrent versions and delete markers.",
                    );
                }
                None
            }
        })
        .inner
}

fn storage_class_rows(tree: &Tree) -> Vec<LegendRow> {
    tree.storage_classes()
        .into_iter()
        .map(|(class, usage)| LegendRow {
            color: Some(palette::storage_class(class)),
            label: class.to_owned(),
            usage,
            node: None,
        })
        .collect()
}

fn version_state_rows(tree: &Tree) -> Vec<LegendRow> {
    VersionState::ALL
        .into_iter()
        .map(|state| LegendRow {
            color: Some(palette::version_state(state)),
            label: state.label().to_owned(),
            usage: tree.usage_by_version_state(state),
            node: None,
        })
        .collect()
}

fn entry_kind_rows(tree: &Tree) -> Vec<LegendRow> {
    EntryKind::ALL
        .into_iter()
        .map(|kind| LegendRow {
            color: None,
            label: kind.label().to_owned(),
            usage: tree.usage_by_kind(kind),
            node: None,
        })
        .collect()
}

fn prefix_rows(tree: &Tree, root: NodeId, colors: &Colors) -> Vec<LegendRow> {
    let mut other = tree.node(root).usage();
    let mut rows: Vec<LegendRow> = palette::top_prefixes(tree, root)
        .into_iter()
        .map(|prefix| {
            let usage = tree.node(prefix).usage();
            other -= usage;
            LegendRow {
                color: colors.prefix(prefix),
                label: format!("{}/", tree.name(prefix)),
                usage,
                node: Some(prefix),
            }
        })
        .collect();
    if other.objects > 0 {
        rows.push(LegendRow {
            color: Some(colors.other()),
            label: "Everything else".to_owned(),
            usage: other,
            node: None,
        });
    }
    rows
}

const ROW_HEIGHT: f32 = 20.0;

/// A table with resizable columns. Tables are keyed by `title`, so each one keeps its
/// own column widths. Returns the node of the row that was clicked, if any.
fn table(
    ui: &mut egui::Ui,
    title: &str,
    rows: &[LegendRow],
    total: Usage,
    selected: Option<NodeId>,
) -> Option<NodeId> {
    let mut clicked = None;
    ui.push_id(title, |ui| {
        TableBuilder::new(ui)
            .striped(true)
            .sense(Sense::click())
            .resizable(true)
            .vscroll(false)
            .cell_layout(egui::Layout::left_to_right(Align::Center))
            .column(Column::initial(170.0).at_least(60.0).clip(true))
            .column(Column::initial(80.0).at_least(40.0).clip(true))
            .column(Column::initial(60.0).at_least(40.0).clip(true))
            .column(Column::initial(80.0).at_least(40.0).clip(true))
            .column(Column::remainder().at_least(40.0).clip(true))
            .header(ROW_HEIGHT, |mut header| {
                for heading in [title, "Size", "Percent", "Cost/mo", "Objects"] {
                    header.col(|ui| {
                        ui.strong(heading);
                    });
                }
            })
            .body(|mut body| {
                for row in rows {
                    body.row(ROW_HEIGHT, |mut table_row| {
                        table_row.set_selected(row.node.is_some() && row.node == selected);
                        table_row.col(|ui| {
                            swatch(ui, row.color);
                            ui.add(Label::new(&row.label).truncate());
                        });
                        table_row.col(|ui| right_aligned(ui, format_bytes(row.usage.bytes)));
                        table_row.col(|ui| {
                            let share = percent(row.usage.bytes, total.bytes);
                            right_aligned(ui, format!("{share:.1}%"));
                        });
                        table_row.col(|ui| right_aligned(ui, format_usd(row.usage.monthly_cost)));
                        table_row.col(|ui| right_aligned(ui, format_count(row.usage.objects)));
                        if table_row.response().clicked() && row.node.is_some() {
                            clicked = row.node;
                        }
                    });
                }
            });
    });
    clicked
}

fn swatch(ui: &mut egui::Ui, color: Option<Color32>) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), Sense::hover());
    if let Some(color) = color {
        ui.painter().rect_filled(rect, 2.0, color);
    }
}

fn percent(part: u64, whole: u64) -> f64 {
    if whole == 0 {
        0.0
    } else {
        part as f64 * 100.0 / whole as f64
    }
}
