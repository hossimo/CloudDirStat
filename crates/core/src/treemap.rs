use crate::{NodeId, NodeKind, Tree};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    pub fn area(&self) -> f32 {
        self.w * self.h
    }

    fn shorter_side(&self) -> f32 {
        self.w.min(self.h)
    }
}

/// Lays out `root` and its descendants as a squarified treemap inside `bounds`.
///
/// Every node's area is proportional to its size in bytes. Parents come before their
/// children in the result, so drawing in order paints files on top of their folders.
/// Nodes smaller than `min_area` (and everything under them) are left out, which keeps
/// the output bounded by the number of visible pixels rather than the number of objects.
pub fn squarify(tree: &Tree, root: NodeId, bounds: Rect, min_area: f32) -> Vec<(NodeId, Rect)> {
    let mut out = vec![(root, bounds)];
    let mut pending = vec![(root, bounds)];

    while let Some((parent, rect)) = pending.pop() {
        let first_child = out.len();
        layout_children(tree, parent, rect, min_area, &mut out);
        pending.extend(
            out[first_child..]
                .iter()
                .filter(|&&(id, _)| tree.node(id).kind() == NodeKind::Directory),
        );
    }
    out
}

fn layout_children(
    tree: &Tree,
    parent: NodeId,
    rect: Rect,
    min_area: f32,
    out: &mut Vec<(NodeId, Rect)>,
) {
    let parent_bytes = tree.node(parent).usage().bytes;
    if parent_bytes == 0 || rect.area() <= 0.0 {
        return;
    }

    let scale = f64::from(rect.area()) / parent_bytes as f64;
    let areas: Vec<(NodeId, f32)> = tree
        .children_by_size(parent)
        .into_iter()
        .map(|child| {
            (
                child,
                (tree.node(child).usage().bytes as f64 * scale) as f32,
            )
        })
        .take_while(|&(_, area)| area > 0.0 && area >= min_area)
        .collect();

    let mut remaining = rect;
    let mut start = 0;
    while start < areas.len() {
        let end = row_end(&areas, start, remaining.shorter_side());
        place_row(&areas[start..end], &mut remaining, out);
        start = end;
    }
}

/// Grows a row from `start` for as long as adding the next item makes the row's worst
/// aspect ratio better. `areas` is sorted largest first.
fn row_end(areas: &[(NodeId, f32)], start: usize, side: f32) -> usize {
    let largest = areas[start].1;
    let mut sum = largest;
    let mut worst = worst_aspect_ratio(sum, largest, largest, side);

    let mut end = start + 1;
    while let Some(&(_, next)) = areas.get(end) {
        let candidate = worst_aspect_ratio(sum + next, largest, next, side);
        if candidate > worst {
            break;
        }
        sum += next;
        worst = candidate;
        end += 1;
    }
    end
}

fn worst_aspect_ratio(row_sum: f32, largest: f32, smallest: f32, side: f32) -> f32 {
    let side_sq = side * side;
    let sum_sq = row_sum * row_sum;
    (side_sq * largest / sum_sq).max(sum_sq / (side_sq * smallest))
}

/// Places a row along the shorter side of `remaining`, then shrinks `remaining` past it.
fn place_row(row: &[(NodeId, f32)], remaining: &mut Rect, out: &mut Vec<(NodeId, Rect)>) {
    let row_area: f32 = row.iter().map(|&(_, area)| area).sum();
    let vertical = remaining.w >= remaining.h;
    let side = if vertical { remaining.h } else { remaining.w };
    let available = if vertical { remaining.w } else { remaining.h };
    let thickness = (row_area / side).min(available);

    let mut offset = 0.0;
    for (index, &(id, area)) in row.iter().enumerate() {
        // Snap the last item to the edge so float rounding never leaves a gap.
        let length = if index + 1 == row.len() {
            side - offset
        } else {
            area / thickness
        };
        let rect = if vertical {
            Rect::new(remaining.x, remaining.y + offset, thickness, length)
        } else {
            Rect::new(remaining.x + offset, remaining.y, length, thickness)
        };
        out.push((id, rect));
        offset += length;
    }

    if vertical {
        remaining.x += thickness;
        remaining.w -= thickness;
    } else {
        remaining.y += thickness;
        remaining.h -= thickness;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Entry, EntryKind};

    const EPSILON: f32 = 1e-3;

    fn tree_of(files: &[(&str, u64)]) -> Tree {
        let mut tree = Tree::new();
        for &(key, size) in files {
            tree.insert(&Entry {
                key: key.to_owned(),
                size,
                storage_class: "STANDARD".to_owned(),
                kind: EntryKind::Current,
            });
        }
        tree
    }

    fn children_of(tree: &Tree, layout: &[(NodeId, Rect)], parent: NodeId) -> Vec<Rect> {
        layout
            .iter()
            .filter(|&&(id, _)| tree.node(id).parent() == Some(parent))
            .map(|&(_, rect)| rect)
            .collect()
    }

    fn contains(outer: Rect, inner: Rect) -> bool {
        inner.x >= outer.x - EPSILON
            && inner.y >= outer.y - EPSILON
            && inner.x + inner.w <= outer.x + outer.w + EPSILON
            && inner.y + inner.h <= outer.y + outer.h + EPSILON
    }

    fn overlap(a: Rect, b: Rect) -> bool {
        a.x + a.w > b.x + EPSILON
            && b.x + b.w > a.x + EPSILON
            && a.y + a.h > b.y + EPSILON
            && b.y + b.h > a.y + EPSILON
    }

    fn aspect_ratio(rect: Rect) -> f32 {
        rect.w.max(rect.h) / rect.w.min(rect.h)
    }

    #[test]
    fn root_fills_bounds_and_single_child_fills_root() {
        let tree = tree_of(&[("only.bin", 42)]);
        let bounds = Rect::new(10.0, 20.0, 300.0, 200.0);

        let layout = squarify(&tree, Tree::ROOT, bounds, 0.0);

        assert_eq!(layout.len(), 2);
        assert_eq!(layout[0], (Tree::ROOT, bounds));
        assert!((layout[1].1.area() - bounds.area()).abs() < 1.0);
    }

    #[test]
    fn areas_are_proportional_and_fill_the_parent() {
        let files = [
            ("a", 6),
            ("b", 6),
            ("c", 4),
            ("d", 3),
            ("e", 2),
            ("f", 2),
            ("g", 1),
        ];
        let tree = tree_of(&files);
        let bounds = Rect::new(0.0, 0.0, 6.0, 4.0);

        let layout = squarify(&tree, Tree::ROOT, bounds, 0.0);

        let total: f32 = children_of(&tree, &layout, Tree::ROOT)
            .iter()
            .map(Rect::area)
            .sum();
        assert!((total - bounds.area()).abs() < EPSILON);
        for &(id, rect) in &layout[1..] {
            let expected = tree.node(id).usage().bytes as f32;
            assert!(
                (rect.area() - expected).abs() < EPSILON,
                "{}: area {} != {expected}",
                tree.path(id),
                rect.area()
            );
        }
    }

    #[test]
    fn siblings_stay_inside_parent_without_overlapping() {
        let tree = tree_of(&[
            ("logs/a", 500),
            ("logs/b", 300),
            ("logs/2024/c", 120),
            ("logs/2024/d", 80),
            ("img/x", 400),
            ("img/y", 10),
            ("readme", 1),
        ]);
        let bounds = Rect::new(0.0, 0.0, 800.0, 600.0);

        let layout = squarify(&tree, Tree::ROOT, bounds, 0.0);

        assert_eq!(layout.len(), 11);
        for &(parent, parent_rect) in &layout {
            let children = children_of(&tree, &layout, parent);
            for (i, &a) in children.iter().enumerate() {
                assert!(contains(parent_rect, a), "{a:?} escapes {parent_rect:?}");
                for &b in &children[i + 1..] {
                    assert!(!overlap(a, b), "{a:?} overlaps {b:?}");
                }
            }
        }
    }

    #[test]
    fn equal_sizes_in_a_square_become_squares() {
        let names: Vec<String> = (0..16).map(|i| format!("file{i}")).collect();
        let files: Vec<(&str, u64)> = names.iter().map(|name| (name.as_str(), 1)).collect();
        let tree = tree_of(&files);

        let layout = squarify(&tree, Tree::ROOT, Rect::new(0.0, 0.0, 400.0, 400.0), 0.0);

        for &(_, rect) in &layout[1..] {
            assert!(aspect_ratio(rect) < 1.01, "{rect:?} is not square");
        }
    }

    #[test]
    fn parents_come_before_their_children() {
        let tree = tree_of(&[("a/b/c/d", 10), ("a/e", 5)]);

        let layout = squarify(&tree, Tree::ROOT, Rect::new(0.0, 0.0, 100.0, 100.0), 0.0);

        let position = |id| layout.iter().position(|&(node, _)| node == id);
        for &(id, _) in &layout {
            if let Some(parent) = tree.node(id).parent() {
                assert!(position(parent) < position(id));
            }
        }
    }

    #[test]
    fn skips_empty_and_too_small_nodes() {
        let tree = tree_of(&[("big", 10_000), ("tiny", 1), ("empty", 0), ("dir/", 0)]);

        let layout = squarify(&tree, Tree::ROOT, Rect::new(0.0, 0.0, 100.0, 100.0), 4.0);

        let paths: Vec<_> = layout.iter().map(|&(id, _)| tree.path(id)).collect();
        assert_eq!(paths, ["", "big"]);
    }

    #[test]
    fn empty_tree_is_just_the_root() {
        let tree = Tree::new();
        let bounds = Rect::new(0.0, 0.0, 100.0, 100.0);

        assert_eq!(
            squarify(&tree, Tree::ROOT, bounds, 0.0),
            [(Tree::ROOT, bounds)]
        );
    }
}
