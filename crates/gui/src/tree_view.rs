use std::collections::{HashMap, HashSet};

use clouddirstat_core::{
    Date, Filtered, NodeId, NodeKind, Tree, Usage, format_bytes, format_count, format_usd,
};
use eframe::egui::{self, Align, Color32, Key, Label, Sense};
use egui_extras::{Column, TableBuilder};

use crate::palette::{self, Colors};

const ROW_HEIGHT: f32 = 20.0;
const INDENT: f32 = 16.0;
const MIN_NAME_WIDTH: f32 = 150.0;
/// Starting widths of size proportion, percent, size, cost, and objects.
const NUMBER_COLUMNS: [f32; 5] = [110.0, 70.0, 90.0, 100.0, 90.0];
/// Last modified is last and fills the leftover width, but starts about this wide.
const LAST_MODIFIED_WIDTH: f32 = 100.0;
/// Items listed per folder before a "more" row; "Show more" on it shows this many more.
/// Keeps a folder with millions of objects from becoming millions of rows.
const ROWS_PER_FOLDER: usize = 1000;

/// What the user asked for from a folder's row.
pub enum Action {
    /// Zoom the treemap to this folder.
    Zoom(NodeId),
    /// Put this folder in the Location field.
    SetLocation(NodeId),
    /// Copy this item's URI (`s3://bucket/key` and so on) to the clipboard.
    CopyUri(NodeId),
}

/// The folder list: one row per visible node, children sorted largest first.
#[derive(Default)]
pub struct TreeView {
    root: Option<NodeId>,
    expanded: HashSet<NodeId>,
    /// Items shown in folders where the user asked for more than [`ROWS_PER_FOLDER`];
    /// `usize::MAX` after "Show all".
    shown: HashMap<NodeId, usize>,
    rows: Vec<Row>,
    stale: bool,
    /// A row to bring into view, and where: centered, or just enough to be visible.
    scroll_to: Option<(NodeId, Option<Align>)>,
}

/// What a click on a "more" row asks for.
#[derive(Clone, Copy)]
enum ShowMore {
    Next,
    All,
}

#[derive(Clone, Copy)]
struct Row {
    /// The node, or for a "more" row the folder whose items it stands for.
    id: NodeId,
    depth: u16,
    /// Set on the row after a folder's first items: how many more there are, and what
    /// they add up to.
    more: Option<(usize, Usage)>,
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
        self.scroll_to = Some((id, Some(Align::Center)));
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
    ) -> Option<Action> {
        if self.root != Some(root) {
            self.root = Some(root);
            self.expanded.insert(root);
            self.stale = true;
        }
        let tree = view.tree;
        if self.stale {
            self.rebuild_rows(view, root);
        }
        self.keys(ui, tree, root, selected);
        if self.stale {
            self.rebuild_rows(view, root);
        }

        let scroll_to = self
            .scroll_to
            .take()
            .and_then(|(target, align)| Some((self.row_of(target)?, align)));

        // Name starts with whatever the number columns leave over; after that every
        // column keeps the width the user gives it, and the last column absorbs the
        // difference, so dragging a divider only moves columns to its right.
        let spacing = ui.spacing().item_spacing.x * (NUMBER_COLUMNS.len() + 1) as f32;
        let others: f32 = NUMBER_COLUMNS.iter().sum::<f32>() + LAST_MODIFIED_WIDTH + spacing;
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
        if let Some((row, align)) = scroll_to {
            table = table.scroll_to_row(row, align);
        }

        let mut toggled = None;
        let mut show_more = None;
        let mut action = None;
        table
            .header(ROW_HEIGHT, |mut header| {
                for title in [
                    "Name",
                    "Size proportion",
                    "Percent",
                    "Size",
                    "Cost/mo",
                    "Objects",
                    "Last modified",
                ] {
                    header.col(|ui| {
                        ui.strong(title);
                    });
                }
            })
            .body(|body| {
                body.rows(ROW_HEIGHT, self.rows.len(), |mut table_row| {
                    let row = self.rows[table_row.index()];
                    if let Some((hidden, usage)) = row.more {
                        let parent_bytes = view.usage(row.id).bytes;
                        if let Some(more) =
                            more_row(&mut table_row, row.depth, hidden, usage, parent_bytes)
                        {
                            show_more = Some((row.id, more));
                        }
                        return;
                    }
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
                    table_row.col(|ui| date_label(ui, view.last_modified(row.id)));

                    let response = table_row.response();
                    if response.clicked() {
                        *selected = Some(row.id);
                    }
                    let is_folder = node.kind() == NodeKind::Directory;
                    if is_folder && response.double_clicked() {
                        action = Some(Action::Zoom(row.id));
                    }
                    response.context_menu(|ui| {
                        if is_folder {
                            if ui.button("Zoom treemap here (double-click)").clicked() {
                                action = Some(Action::Zoom(row.id));
                                ui.close();
                            }
                            if ui.button("Set as Location").clicked() {
                                action = Some(Action::SetLocation(row.id));
                                ui.close();
                            }
                        }
                        if ui.button("Copy URI").clicked() {
                            action = Some(Action::CopyUri(row.id));
                            ui.close();
                        }
                    });
                });
            });

        if let Some(id) = toggled {
            if !self.expanded.remove(&id) {
                self.expanded.insert(id);
            }
            self.stale = true;
        }
        if let Some((folder, more)) = show_more {
            let shown = self.shown.entry(folder).or_insert(ROWS_PER_FOLDER);
            *shown = match more {
                ShowMore::Next => shown.saturating_add(ROWS_PER_FOLDER),
                ShowMore::All => usize::MAX,
            };
            self.stale = true;
        }
        action
    }

    /// With no text field taking the keyboard, Home and End select the first and last
    /// item, and Page Up and Page Down move the selection by one screenful. With an item
    /// selected, Up and Down select the previous and next item, Right opens a folder (or
    /// goes to its first item when open), and Left closes it (or goes to the folder above).
    fn keys(&mut self, ui: &egui::Ui, tree: &Tree, root: NodeId, selected: &mut Option<NodeId>) {
        if ui.ctx().egui_wants_keyboard_input() {
            return;
        }
        let key = ui.input_mut(|input| {
            [
                Key::ArrowUp,
                Key::ArrowDown,
                Key::ArrowLeft,
                Key::ArrowRight,
                Key::Home,
                Key::End,
                Key::PageUp,
                Key::PageDown,
            ]
            .into_iter()
            .find(|key| input.consume_key(egui::Modifiers::NONE, *key))
        });
        // One row less for the header and one so the old edge row stays in view.
        let page = ((ui.available_height() / ROW_HEIGHT) as usize)
            .saturating_sub(2)
            .max(1);
        let mut items = self.rows.iter().filter(|row| row.more.is_none());
        let target = match key {
            Some(Key::Home) => items.next().map(|row| row.id),
            Some(Key::End) => items.next_back().map(|row| row.id),
            Some(Key::PageUp) => self.paged(*selected, |index| index.saturating_sub(page)),
            Some(Key::PageDown) => self.paged(*selected, |index| index.saturating_add(page)),
            _ => self.arrow_target(key, tree, root, *selected),
        };
        if let Some(target) = target {
            *selected = Some(target);
            self.scroll_to = Some((target, None));
        }
    }

    /// The item `step` rows from the selected one (or from the top), kept within the list.
    /// A "more" row there gives the item just above it.
    fn paged(&self, selected: Option<NodeId>, step: impl Fn(usize) -> usize) -> Option<NodeId> {
        let last = self.rows.len().checked_sub(1)?;
        let from = selected.and_then(|id| self.row_of(id)).unwrap_or(0);
        let index = step(from).min(last);
        self.rows[..=index]
            .iter()
            .rev()
            .find(|row| row.more.is_none())
            .map(|row| row.id)
    }

    fn arrow_target(
        &mut self,
        key: Option<Key>,
        tree: &Tree,
        root: NodeId,
        selected: Option<NodeId>,
    ) -> Option<NodeId> {
        let current = selected?;
        let index = self.row_of(current)?;
        let is_folder = tree.node(current).has_children();
        let is_open = self.expanded.contains(&current);
        match key {
            Some(Key::ArrowUp) => self.rows[..index]
                .iter()
                .rev()
                .find(|row| row.more.is_none())
                .map(|row| row.id),
            Some(Key::ArrowDown) => self.rows[index + 1..]
                .iter()
                .find(|row| row.more.is_none())
                .map(|row| row.id),
            Some(Key::ArrowRight) if is_folder && !is_open => {
                self.expanded.insert(current);
                self.stale = true;
                None
            }
            Some(Key::ArrowRight) if is_folder => self
                .rows
                .get(index + 1)
                .filter(|row| row.depth > self.rows[index].depth && row.more.is_none())
                .map(|row| row.id),
            Some(Key::ArrowLeft) if is_folder && is_open => {
                self.expanded.remove(&current);
                self.stale = true;
                None
            }
            Some(Key::ArrowLeft) if current != root => tree.node(current).parent(),
            _ => None,
        }
    }

    /// The row showing `id`, unless it is hidden in a closed folder or past a "more" row.
    fn row_of(&self, id: NodeId) -> Option<usize> {
        self.rows
            .iter()
            .position(|row| row.id == id && row.more.is_none())
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
        let mut pending = vec![Row {
            id: root,
            depth: 0,
            more: None,
        }];
        while let Some(row) = pending.pop() {
            self.rows.push(row);
            if row.more.is_some() || !self.expanded.contains(&row.id) {
                continue;
            }
            let depth = row.depth.saturating_add(1);
            let limit = self.shown.get(&row.id).copied().unwrap_or(ROWS_PER_FOLDER);
            let (children, hidden, rest) = view.largest_children(row.id, limit);
            if hidden > 0 {
                pending.push(Row {
                    id: row.id,
                    depth,
                    more: Some((hidden, rest)),
                });
            }
            pending.extend(children.into_iter().rev().map(|id| Row {
                id,
                depth,
                more: None,
            }));
        }
        self.stale = false;
    }
}

/// The row after a folder's first items. Clicking the row or "Show N more" shows the
/// next [`ROWS_PER_FOLDER`] items; "Show all" shows the rest.
fn more_row(
    table_row: &mut egui_extras::TableRow<'_, '_>,
    depth: u16,
    hidden: usize,
    usage: Usage,
    parent_bytes: u64,
) -> Option<ShowMore> {
    let fraction = fraction(usage.bytes, parent_bytes);
    let mut clicked = None;
    table_row.col(|ui| {
        ui.add_space(f32::from(depth) * INDENT + INDENT + ui.spacing().item_spacing.x);
        let more = egui::RichText::new(format!("… {} more", format_count(hidden as u64)));
        ui.add(Label::new(more.italics()).selectable(false));
        if hidden > ROWS_PER_FOLDER {
            let next = format!("Show {} more", format_count(ROWS_PER_FOLDER as u64));
            if ui.link(next).clicked() {
                clicked = Some(ShowMore::Next);
            }
        }
        if ui.link("Show all").clicked() {
            clicked = Some(ShowMore::All);
        }
    });
    table_row.col(|ui| proportion_bar(ui, fraction));
    table_row.col(|ui| right_aligned(ui, format!("{:.1}%", fraction * 100.0)));
    table_row.col(|ui| right_aligned(ui, format_bytes(usage.bytes)));
    table_row.col(|ui| right_aligned(ui, format_usd(usage.monthly_cost)));
    table_row.col(|ui| right_aligned(ui, format_count(usage.objects)));
    table_row.col(|_| {});
    if clicked.is_none() && table_row.response().clicked() {
        clicked = Some(ShowMore::Next);
    }
    clicked
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

/// A last-modified date, or nothing when it is unknown.
pub fn date_label(ui: &mut egui::Ui, date: Option<Date>) {
    if let Some(date) = date {
        ui.add(Label::new(date.to_string()).selectable(false));
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::palette::ColorMode;
    use clouddirstat_core::{Entry, EntryKind};

    fn tree_of(files: &[(&str, u64)]) -> Tree {
        let mut tree = Tree::new();
        for &(key, size) in files {
            tree.insert(&Entry {
                key: key.to_owned(),
                size,
                storage_class: "STANDARD".into(),
                kind: EntryKind::Current,
                last_modified: None,
            });
        }
        tree
    }

    /// Shows the folder list for one frame, with `key` pressed if given.
    fn frame(
        ctx: &egui::Context,
        list: &mut TreeView,
        tree: &Tree,
        selected: &mut Option<NodeId>,
        key: Option<Key>,
    ) {
        let colors = Colors::new(ColorMode::StorageClass, tree, Tree::ROOT);
        let mut input = egui::RawInput::default();
        if let Some(key) = key {
            input.events.push(egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            });
        }
        let output = ctx.run_ui(input, |ui| {
            list.show(ui, tree.into(), Tree::ROOT, "root", &colors, selected);
        });
        output.drop_without_applying_deltas();
    }

    #[test]
    fn arrow_keys_move_the_selection_and_open_and_close_folders() {
        let tree = tree_of(&[("a/x.bin", 10), ("a/y.bin", 5), ("b.bin", 3)]);
        let find = |parent, name, kind| tree.find_child(parent, name, kind).unwrap();
        let a = find(Tree::ROOT, "a", NodeKind::Directory);
        let x = find(a, "x.bin", NodeKind::Object);
        let y = find(a, "y.bin", NodeKind::Object);
        let b = find(Tree::ROOT, "b.bin", NodeKind::Object);

        let ctx = egui::Context::default();
        let mut list = TreeView::default();
        let mut selected = Some(a);
        frame(&ctx, &mut list, &tree, &mut selected, None);

        let mut press = |key| {
            frame(&ctx, &mut list, &tree, &mut selected, Some(key));
            selected
        };
        assert_eq!(press(Key::ArrowRight), Some(a), "opens the folder");
        assert_eq!(press(Key::ArrowRight), Some(x), "goes to its first item");
        assert_eq!(press(Key::ArrowDown), Some(y));
        assert_eq!(press(Key::ArrowDown), Some(b), "leaves the folder");
        assert_eq!(press(Key::ArrowUp), Some(y));
        assert_eq!(press(Key::ArrowLeft), Some(a), "goes to the folder above");
        assert_eq!(press(Key::ArrowLeft), Some(a), "closes the folder");
        assert_eq!(
            press(Key::ArrowDown),
            Some(b),
            "skips the closed folder's items"
        );
        assert_eq!(press(Key::ArrowLeft), Some(Tree::ROOT));
        assert_eq!(
            press(Key::ArrowUp),
            Some(Tree::ROOT),
            "stays on the first row"
        );
        assert_eq!(press(Key::End), Some(b), "goes to the last row");
        assert_eq!(press(Key::Home), Some(Tree::ROOT), "goes to the first row");
    }

    #[test]
    fn home_and_end_work_with_nothing_selected() {
        let tree = tree_of(&[("a.bin", 10), ("b.bin", 3)]);
        let b = tree
            .find_child(Tree::ROOT, "b.bin", NodeKind::Object)
            .unwrap();
        let ctx = egui::Context::default();
        let mut list = TreeView::default();
        let mut selected = None;
        frame(&ctx, &mut list, &tree, &mut selected, Some(Key::End));
        assert_eq!(selected, Some(b));
    }

    #[test]
    fn page_keys_stop_at_the_ends_of_the_list() {
        let tree = tree_of(&[("a.bin", 10), ("b.bin", 3)]);
        let b = tree
            .find_child(Tree::ROOT, "b.bin", NodeKind::Object)
            .unwrap();
        let ctx = egui::Context::default();
        let mut list = TreeView::default();
        let mut selected = Some(Tree::ROOT);
        frame(&ctx, &mut list, &tree, &mut selected, Some(Key::PageDown));
        assert_eq!(selected, Some(b));
        frame(&ctx, &mut list, &tree, &mut selected, Some(Key::PageUp));
        assert_eq!(selected, Some(Tree::ROOT));
    }
}
