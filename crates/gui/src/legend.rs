use clouddirstat_core::{
    EntryKind, NodeId, Tree, Usage, VersionState, format_bytes, format_count, format_usd,
};
use eframe::egui::{self, Align, Color32, Label, Sense};
use egui_extras::{Column, TableBuilder};

use crate::filter::{Filter, MAX_FILE_TYPE_ROWS, file_type_label};
use crate::palette::{self, ColorMode, Colors};
use crate::scan::Scan;
use crate::tree_view::right_aligned;

struct LegendRow {
    color: Option<Color32>,
    label: String,
    usage: Usage,
    /// What clicking this row filters the views to, if anything.
    filter: Option<Filter>,
}

/// The right-hand panel: tabs pick what the treemap is colored by, and the list
/// below explains the colors. Returns the filter of the row the user clicked, if any.
pub fn show(
    ui: &mut egui::Ui,
    tree: &Tree,
    root: NodeId,
    mode: &mut ColorMode,
    colors: &Colors,
    scan: &Scan,
    active: Option<&Filter>,
) -> Option<Filter> {
    ui.horizontal(|ui| {
        for option in ColorMode::ALL {
            ui.selectable_value(mode, option, option.label());
        }
    });
    ui.weak("Click a row to show only its objects; click it again to show everything.");
    ui.separator();

    let total = tree.node(root).usage();
    egui::ScrollArea::vertical()
        .show(ui, |ui| match *mode {
            ColorMode::StorageClass => table(
                ui,
                "Storage class",
                &storage_class_rows(tree),
                total,
                active,
            ),
            ColorMode::Prefixes => {
                let rows = prefix_rows(tree, root, colors);
                table(ui, "Prefix", &rows, total, active)
            }
            ColorMode::FileTypes => table(
                ui,
                "File type",
                &file_type_rows(tree, colors),
                total,
                active,
            ),
            ColorMode::Versions => {
                let clicked = table(ui, "Object state", &version_state_rows(tree), total, active);
                ui.add_space(12.0);
                let provider = scan.location.provider();
                let rows = entry_kind_rows(tree, provider.entry_kinds());
                table(ui, "Version type", &rows, total, active);
                if !scan.include_versions {
                    ui.add_space(12.0);
                    ui.weak(format!(
                        "Scan with Versions checked to {}.",
                        provider.versions_help().to_lowercase()
                    ));
                }
                clicked
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
            filter: Some(Filter::StorageClass(class.to_owned())),
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
            filter: Some(Filter::VersionState(state)),
        })
        .collect()
}

fn entry_kind_rows(tree: &Tree, kinds: &[EntryKind]) -> Vec<LegendRow> {
    kinds
        .iter()
        .map(|&kind| LegendRow {
            color: None,
            label: kind.label().to_owned(),
            usage: tree.usage_by_kind(kind),
            // Objects can mix versions of several kinds, so these rows don't filter.
            filter: None,
        })
        .collect()
}

/// The largest file types, then everything else in one row.
fn file_type_rows(tree: &Tree, colors: &Colors) -> Vec<LegendRow> {
    let types = tree.file_types();
    let mut rows: Vec<LegendRow> = types
        .iter()
        .take(MAX_FILE_TYPE_ROWS)
        .map(|&(file_type, usage)| LegendRow {
            color: Some(colors.file_type(file_type).unwrap_or(colors.other())),
            label: file_type_label(file_type),
            usage,
            filter: Some(Filter::FileType(file_type.to_owned())),
        })
        .collect();

    let rest = &types[types.len().min(MAX_FILE_TYPE_ROWS)..];
    if !rest.is_empty() {
        let mut usage = Usage::default();
        for &(_, type_usage) in rest {
            usage += type_usage;
        }
        rows.push(LegendRow {
            color: Some(colors.other()),
            label: format!("{} more types", rest.len()),
            usage,
            filter: Some(Filter::OtherFileTypes),
        });
    }
    rows
}

fn prefix_rows(tree: &Tree, root: NodeId, colors: &Colors) -> Vec<LegendRow> {
    let mut other = tree.node(root).usage();
    let top = match colors.top_prefixes() {
        Some(top) => top.to_vec(),
        None => palette::top_prefixes(tree, root),
    };
    let mut rows: Vec<LegendRow> = top
        .into_iter()
        .map(|prefix| {
            let usage = tree.node(prefix).usage();
            other -= usage;
            LegendRow {
                color: colors.prefix(prefix),
                label: format!("{}/", tree.name(prefix)),
                usage,
                filter: Some(Filter::Prefix(prefix)),
            }
        })
        .collect();
    if other.objects > 0 {
        rows.push(LegendRow {
            color: Some(colors.other()),
            label: "Everything else".to_owned(),
            usage: other,
            filter: Some(Filter::OtherPrefixes),
        });
    }
    rows
}

const ROW_HEIGHT: f32 = 20.0;

/// A table with resizable columns. Tables are keyed by `title`, so each one keeps its
/// own column widths. Returns the filter of the row that was clicked, if any.
fn table(
    ui: &mut egui::Ui,
    title: &str,
    rows: &[LegendRow],
    total: Usage,
    active: Option<&Filter>,
) -> Option<Filter> {
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
            .column(Column::initial(90.0).at_least(40.0).clip(true))
            .column(Column::remainder().at_least(50.0).clip(true))
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
                        table_row
                            .set_selected(row.filter.is_some() && row.filter.as_ref() == active);
                        table_row.col(|ui| {
                            swatch(ui, row.color);
                            ui.add(Label::new(&row.label).selectable(false).truncate());
                        });
                        table_row.col(|ui| right_aligned(ui, format_bytes(row.usage.bytes)));
                        table_row.col(|ui| {
                            let share = percent(row.usage.bytes, total.bytes);
                            right_aligned(ui, format!("{share:.1}%"));
                        });
                        table_row.col(|ui| right_aligned(ui, format_usd(row.usage.monthly_cost)));
                        table_row.col(|ui| right_aligned(ui, format_count(row.usage.objects)));
                        if table_row.response().clicked() && row.filter.is_some() {
                            clicked = row.filter.clone();
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
