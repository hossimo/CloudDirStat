use std::collections::HashSet;

use clouddirstat_core::{Filtered, NodeId, NodeKind, Tree, format_bytes, format_usd};
use eframe::egui::{self, Align, Label, RichText, Sense};
use egui_extras::{Column, TableBuilder};

use crate::palette::Colors;
use crate::tree_view::{date_label, right_aligned};

const ROW_HEIGHT: f32 = 20.0;
const DEFAULT_COUNT: usize = 50;

/// The largest files: overall for one bucket, or the largest in each bucket when
/// scanning all of them.
pub struct LargestFiles {
    count: usize,
    /// Buckets whose files are hidden. Kept across rebuilds while a scan runs.
    collapsed: HashSet<NodeId>,
    rows: Vec<Row>,
    stale: bool,
}

#[derive(Clone, Copy)]
enum Row {
    Bucket(NodeId),
    /// A file, and the folder its Folder column is shown relative to.
    File {
        file: NodeId,
        base: NodeId,
    },
}

/// What the user did in the list.
pub enum Action {
    Select(NodeId),
    /// Double-click: select it and show it in the folder list.
    Reveal(NodeId),
    /// Copy the file's URI (`s3://bucket/key` and so on) to the clipboard.
    CopyUri(NodeId),
}

impl Default for LargestFiles {
    fn default() -> Self {
        Self {
            count: DEFAULT_COUNT,
            collapsed: HashSet::new(),
            rows: Vec::new(),
            stale: true,
        }
    }
}

impl LargestFiles {
    pub fn invalidate(&mut self) {
        self.stale = true;
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        view: Filtered,
        root: NodeId,
        per_bucket: bool,
        colors: &Colors,
        selected: Option<NodeId>,
    ) -> Option<Action> {
        let tree = view.tree;
        ui.horizontal(|ui| {
            ui.label("Largest");
            let changed = ui
                .add(egui::DragValue::new(&mut self.count).range(1..=1000))
                .changed();
            ui.label(if per_bucket {
                "files per bucket"
            } else {
                "files"
            });
            self.stale |= changed;

            if per_bucket {
                ui.separator();
                if ui.button("Collapse all").clicked() {
                    self.collapsed = tree.children(root).collect();
                    self.stale = true;
                }
                if ui.button("Expand all").clicked() {
                    self.collapsed.clear();
                    self.stale = true;
                }
            }
        });
        if self.stale {
            self.rebuild(view, root, per_bucket);
        }

        let mut action = None;
        let mut toggled = None;
        TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .sense(Sense::click())
            .cell_layout(egui::Layout::left_to_right(Align::Center))
            .column(Column::initial(260.0).at_least(120.0).clip(true))
            .column(Column::initial(90.0).at_least(50.0).clip(true))
            .column(Column::initial(90.0).at_least(50.0).clip(true))
            .column(Column::initial(150.0).at_least(60.0).clip(true))
            .column(Column::initial(100.0).at_least(60.0).clip(true))
            .column(Column::remainder().at_least(100.0).clip(true))
            .header(ROW_HEIGHT, |mut header| {
                let titles = [
                    "Name",
                    "Size",
                    "Cost/mo",
                    "Storage class",
                    "Last modified",
                    "Folder",
                ];
                for title in titles {
                    header.col(|ui| {
                        ui.strong(title);
                    });
                }
            })
            .body(|body| {
                body.rows(ROW_HEIGHT, self.rows.len(), |mut table_row| {
                    let row = self.rows[table_row.index()];
                    match row {
                        Row::Bucket(bucket) => {
                            let open = !self.collapsed.contains(&bucket);
                            let arrow_clicked = bucket_row(&mut table_row, view, bucket, open);
                            if arrow_clicked || table_row.response().clicked() {
                                toggled = Some(bucket);
                            }
                        }
                        Row::File { file, base } => {
                            table_row.set_selected(selected == Some(file));
                            file_row(&mut table_row, tree, base, file, colors);
                            let response = table_row.response();
                            if response.double_clicked() {
                                action = Some(Action::Reveal(file));
                            } else if response.clicked() {
                                action = Some(Action::Select(file));
                            }
                            response.context_menu(|ui| {
                                if ui.button("Show in folder list (double-click)").clicked() {
                                    action = Some(Action::Reveal(file));
                                    ui.close();
                                }
                                if ui.button("Copy URI").clicked() {
                                    action = Some(Action::CopyUri(file));
                                    ui.close();
                                }
                            });
                        }
                    }
                });
            });

        if let Some(bucket) = toggled {
            if !self.collapsed.remove(&bucket) {
                self.collapsed.insert(bucket);
            }
            self.stale = true;
        }
        action
    }

    fn rebuild(&mut self, view: Filtered, root: NodeId, per_bucket: bool) {
        self.rows.clear();
        if per_bucket {
            for bucket in view.children_by_size(root) {
                if view.tree.node(bucket).kind() != NodeKind::Directory {
                    continue;
                }
                self.rows.push(Row::Bucket(bucket));
                if self.collapsed.contains(&bucket) {
                    continue;
                }
                let files = view.largest_objects_in(bucket, self.count);
                self.rows.extend(
                    files
                        .into_iter()
                        .map(|file| Row::File { file, base: bucket }),
                );
            }
        } else {
            let files = view.largest_objects_in(root, self.count);
            self.rows
                .extend(files.into_iter().map(|file| Row::File { file, base: root }));
        }
        self.stale = false;
    }
}

/// A bucket heading with a disclosure arrow. Returns whether the arrow was clicked.
fn bucket_row(
    table_row: &mut egui_extras::TableRow<'_, '_>,
    view: Filtered,
    bucket: NodeId,
    open: bool,
) -> bool {
    let tree = view.tree;
    let usage = view.usage(bucket);
    let mut arrow_clicked = false;
    table_row.col(|ui| {
        let (_, arrow) = ui.allocate_exact_size(egui::vec2(16.0, 16.0), Sense::click());
        let openness = if open { 1.0 } else { 0.0 };
        egui::collapsing_header::paint_default_icon(ui, openness, &arrow);
        arrow_clicked = arrow.clicked();
        ui.add(
            Label::new(RichText::new(tree.name(bucket)).strong())
                .selectable(false)
                .truncate(),
        );
    });
    table_row.col(|ui| right_aligned(ui, format_bytes(usage.bytes)));
    table_row.col(|ui| right_aligned(ui, format_usd(usage.monthly_cost)));
    table_row.col(|_| {});
    table_row.col(|ui| date_label(ui, view.last_modified(bucket)));
    table_row.col(|_| {});
    arrow_clicked
}

fn file_row(
    table_row: &mut egui_extras::TableRow<'_, '_>,
    tree: &Tree,
    base: NodeId,
    file: NodeId,
    colors: &Colors,
) {
    let usage = tree.node(file).usage();
    table_row.col(|ui| {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), Sense::hover());
        ui.painter().rect_filled(rect, 2.0, colors.node(tree, file));
        ui.add(Label::new(tree.name(file)).selectable(false).truncate());
    });
    table_row.col(|ui| right_aligned(ui, format_bytes(usage.bytes)));
    table_row.col(|ui| right_aligned(ui, format_usd(usage.monthly_cost)));
    table_row.col(|ui| {
        ui.add(Label::new(tree.storage_class(file).unwrap_or("")).selectable(false));
    });
    table_row.col(|ui| date_label(ui, tree.node(file).last_modified()));
    table_row.col(|ui| {
        let folder = tree.node(file).parent().map(|parent| tree.path(parent));
        let folder = folder.as_deref().unwrap_or("");
        let shown = relative_to(tree, base, folder);
        ui.add(
            Label::new(if shown.is_empty() { "/" } else { shown })
                .selectable(false)
                .truncate(),
        );
    });
}

/// `folder` without `base`'s own path in front of it.
fn relative_to<'a>(tree: &Tree, base: NodeId, folder: &'a str) -> &'a str {
    let base_path = tree.path(base);
    folder.strip_prefix(base_path.as_str()).unwrap_or(folder)
}
