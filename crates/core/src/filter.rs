use std::cmp::Reverse;
use std::collections::HashMap;

use crate::{Date, Node, NodeId, NodeKind, Tree, Usage};

/// The objects that pass a filter, and what each folder adds up to when only those
/// count. Memory grows with the number of folders, not objects: kept objects are one
/// bit each.
#[derive(Debug, Default)]
pub struct Subset {
    directories: HashMap<NodeId, Totals>,
    objects: Vec<u64>,
}

impl Subset {
    /// Keeps the objects for which `keep` is true. Folder markers (`photos/`) are not
    /// objects in the tree, so they never pass a filter.
    pub fn new(tree: &Tree, keep: impl Fn(NodeId) -> bool) -> Self {
        let len = tree.node_count();
        let mut subset = Self {
            directories: HashMap::new(),
            objects: vec![0; len.div_ceil(64)],
        };
        // Children are always added after their parents, so going backwards finishes
        // every folder before its total is passed up.
        for index in (0..len).rev() {
            let id = NodeId::from_index(index);
            let node = tree.node(id);
            let totals = match node.kind() {
                NodeKind::Directory => match subset.directories.get(&id) {
                    Some(&totals) => totals,
                    None => continue,
                },
                NodeKind::Object if keep(id) => {
                    subset.objects[index / 64] |= 1 << (index % 64);
                    Totals::of(node)
                }
                NodeKind::Object => continue,
            };
            if let Some(parent) = node.parent() {
                let parent = subset.directories.entry(parent).or_default();
                parent.usage += totals.usage;
                parent.last_modified = parent.last_modified.max(totals.last_modified);
            }
        }
        subset
    }

    pub fn usage(&self, tree: &Tree, id: NodeId) -> Usage {
        self.totals(tree, id).usage
    }

    pub fn last_modified(&self, tree: &Tree, id: NodeId) -> Option<Date> {
        self.totals(tree, id).last_modified
    }

    fn totals(&self, tree: &Tree, id: NodeId) -> Totals {
        let node = tree.node(id);
        match node.kind() {
            NodeKind::Directory => self.directories.get(&id).copied().unwrap_or_default(),
            NodeKind::Object if self.contains_object(id) => Totals::of(node),
            NodeKind::Object => Totals::default(),
        }
    }

    fn contains_object(&self, id: NodeId) -> bool {
        let index = id.index();
        self.objects
            .get(index / 64)
            .is_some_and(|bits| bits & (1 << (index % 64)) != 0)
    }
}

/// What the kept objects in a folder add up to.
#[derive(Clone, Copy, Debug, Default)]
struct Totals {
    usage: Usage,
    last_modified: Option<Date>,
}

impl Totals {
    fn of(node: &Node) -> Self {
        Self {
            usage: node.usage(),
            last_modified: node.last_modified(),
        }
    }
}

/// A tree seen through an optional [`Subset`]: filtered-out objects, and folders with
/// nothing left in them, are treated as absent.
#[derive(Clone, Copy, Debug)]
pub struct Filtered<'a> {
    pub tree: &'a Tree,
    subset: Option<&'a Subset>,
}

impl<'a> Filtered<'a> {
    pub fn new(tree: &'a Tree, subset: Option<&'a Subset>) -> Self {
        Self { tree, subset }
    }

    pub fn usage(&self, id: NodeId) -> Usage {
        match self.subset {
            Some(subset) => subset.usage(self.tree, id),
            None => self.tree.node(id).usage(),
        }
    }

    pub fn last_modified(&self, id: NodeId) -> Option<Date> {
        match self.subset {
            Some(subset) => subset.last_modified(self.tree, id),
            None => self.tree.node(id).last_modified(),
        }
    }

    pub fn children_by_size(&self, id: NodeId) -> Vec<NodeId> {
        let Some(subset) = self.subset else {
            return self.tree.children_by_size(id);
        };
        let mut children: Vec<_> = self
            .tree
            .children(id)
            .map(|child| (child, subset.usage(self.tree, child)))
            .filter(|(_, usage)| usage.objects > 0)
            .collect();
        children.sort_by_key(|&(_, usage)| Reverse(usage.bytes));
        children.into_iter().map(|(child, _)| child).collect()
    }

    /// The `limit` largest children of `id`, largest first, plus how many others there
    /// are and what they add up to. Unlike [`Filtered::children_by_size`] it never sorts
    /// every child, which matters for a folder with millions of objects.
    pub fn largest_children(&self, id: NodeId, limit: usize) -> (Vec<NodeId>, usize, Usage) {
        let mut children: Vec<(NodeId, Usage)> = self
            .tree
            .children(id)
            .map(|child| (child, self.usage(child)))
            .filter(|(_, usage)| self.subset.is_none() || usage.objects > 0)
            .collect();
        let mut rest = Usage::default();
        let mut hidden = 0;
        if children.len() > limit {
            children.select_nth_unstable_by_key(limit, |&(_, usage)| Reverse(usage.bytes));
            for &(_, usage) in &children[limit..] {
                rest += usage;
            }
            hidden = children.len() - limit;
            children.truncate(limit);
        }
        children.sort_by_key(|&(_, usage)| Reverse(usage.bytes));
        let largest = children.into_iter().map(|(child, _)| child).collect();
        (largest, hidden, rest)
    }

    /// The `count` largest objects under `root` that pass the filter, largest first.
    pub fn largest_objects_in(&self, root: NodeId, count: usize) -> Vec<NodeId> {
        match self.subset {
            Some(subset) => self
                .tree
                .largest_objects_where(root, count, |id| subset.contains_object(id)),
            None => self.tree.largest_objects_in(root, count),
        }
    }
}

impl<'a> From<&'a Tree> for Filtered<'a> {
    fn from(tree: &'a Tree) -> Self {
        Self::new(tree, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Entry, EntryKind};

    fn tree_of(files: &[(&str, u64, &str)]) -> Tree {
        let mut tree = Tree::new();
        for &(key, size, class) in files {
            tree.insert(&Entry {
                key: key.to_owned(),
                size,
                storage_class: class.to_owned(),
                kind: EntryKind::Current,
                // Bigger files are newer, so the dates show which objects were kept.
                last_modified: Some(size * 86_400),
            });
        }
        tree
    }

    fn names(view: Filtered, id: NodeId) -> Vec<String> {
        view.children_by_size(id)
            .into_iter()
            .map(|child| view.tree.name(child).to_owned())
            .collect()
    }

    #[test]
    fn folders_add_up_only_kept_objects() {
        let tree = tree_of(&[
            ("logs/a.log", 100, "GLACIER"),
            ("logs/b.log", 50, "STANDARD"),
            ("logs/old/c.log", 30, "GLACIER"),
            ("img/x.png", 400, "STANDARD"),
        ]);
        let subset = Subset::new(&tree, |id| tree.storage_class(id) == Some("GLACIER"));
        let view = Filtered::new(&tree, Some(&subset));

        assert_eq!(view.usage(Tree::ROOT), Usage::new(130, 2));
        assert_eq!(names(view, Tree::ROOT), ["logs"]);
        let logs = tree
            .find_child(Tree::ROOT, "logs", NodeKind::Directory)
            .unwrap();
        assert_eq!(view.usage(logs), Usage::new(130, 2));
        assert_eq!(view.last_modified(logs).map(Date::days), Some(100));
        assert_eq!(
            tree.node(Tree::ROOT).last_modified().map(Date::days),
            Some(400)
        );
        assert_eq!(names(view, logs), ["a.log", "old"]);
        let largest: Vec<_> = view
            .largest_objects_in(Tree::ROOT, 5)
            .into_iter()
            .map(|id| tree.path(id))
            .collect();
        assert_eq!(largest, ["logs/a.log", "logs/old/c.log"]);
    }

    #[test]
    fn without_a_subset_everything_counts() {
        let tree = tree_of(&[("a", 5, "STANDARD"), ("b/c", 7, "STANDARD")]);
        let view = Filtered::from(&tree);

        assert_eq!(view.usage(Tree::ROOT), tree.total());
        assert_eq!(names(view, Tree::ROOT), ["b", "a"]);
    }

    #[test]
    fn largest_children_sums_up_the_rest() {
        let tree = tree_of(&[
            ("a", 5, "STANDARD"),
            ("b", 50, "STANDARD"),
            ("c", 20, "GLACIER"),
            ("d", 1, "STANDARD"),
        ]);
        let view = Filtered::from(&tree);
        let (largest, hidden, rest) = view.largest_children(Tree::ROOT, 2);
        let names: Vec<_> = largest.iter().map(|&id| tree.name(id)).collect();
        assert_eq!(names, ["b", "c"]);
        assert_eq!((hidden, rest), (2, Usage::new(6, 2)));

        let (all, hidden, _) = view.largest_children(Tree::ROOT, 10);
        assert_eq!((all, hidden), (view.children_by_size(Tree::ROOT), 0));

        let subset = Subset::new(&tree, |id| tree.storage_class(id) == Some("STANDARD"));
        let filtered = Filtered::new(&tree, Some(&subset));
        let (_, hidden, rest) = filtered.largest_children(Tree::ROOT, 1);
        assert_eq!((hidden, rest), (2, Usage::new(6, 2)));
    }

    #[test]
    fn keeping_nothing_leaves_an_empty_root() {
        let tree = tree_of(&[("a", 5, "STANDARD")]);
        let subset = Subset::new(&tree, |_| false);
        let view = Filtered::new(&tree, Some(&subset));

        assert_eq!(view.usage(Tree::ROOT), Usage::default());
        assert!(view.children_by_size(Tree::ROOT).is_empty());
    }
}
