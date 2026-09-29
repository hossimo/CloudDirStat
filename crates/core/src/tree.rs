use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};
use std::sync::Arc;

use crate::{Entry, EntryKind, Usage};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(u32);

impl NodeId {
    fn index(self) -> usize {
        self.0 as usize
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NodeKind {
    Directory,
    Object,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct NameId(u32);

#[derive(Debug)]
pub struct Node {
    name: NameId,
    kind: NodeKind,
    parent: Option<NodeId>,
    children: Vec<NodeId>,
    usage: Usage,
}

impl Node {
    pub fn kind(&self) -> NodeKind {
        self.kind
    }

    pub fn parent(&self) -> Option<NodeId> {
        self.parent
    }

    pub fn children(&self) -> &[NodeId] {
        &self.children
    }

    pub fn usage(&self) -> Usage {
        self.usage
    }
}

#[derive(Debug)]
pub struct Tree {
    nodes: Vec<Node>,
    names: Vec<Arc<str>>,
    name_ids: HashMap<Arc<str>, NameId>,
    children_by_name: HashMap<(NodeId, NameId, NodeKind), NodeId>,
    storage_classes: Vec<(String, Usage)>,
    kinds: [Usage; EntryKind::ALL.len()],
}

impl Default for Tree {
    fn default() -> Self {
        Self::new()
    }
}

impl Tree {
    pub const ROOT: NodeId = NodeId(0);

    pub fn new() -> Self {
        let mut tree = Self {
            nodes: Vec::new(),
            names: Vec::new(),
            name_ids: HashMap::new(),
            children_by_name: HashMap::new(),
            storage_classes: Vec::new(),
            kinds: Default::default(),
        };
        let root_name = tree.intern("");
        tree.nodes.push(Node {
            name: root_name,
            kind: NodeKind::Directory,
            parent: None,
            children: Vec::new(),
            usage: Usage::default(),
        });
        tree
    }

    pub fn insert(&mut self, entry: &Entry) {
        let usage = Usage {
            bytes: entry.size,
            objects: 1,
        };

        let mut current = Self::ROOT;
        self.nodes[current.index()].usage += usage;

        let mut segments = entry.key.split('/').peekable();
        while let Some(segment) = segments.next() {
            let is_last = segments.peek().is_none();
            if is_last && segment.is_empty() {
                break;
            }
            let kind = if is_last {
                NodeKind::Object
            } else {
                NodeKind::Directory
            };
            current = self.child_or_insert(current, segment, kind);
            self.nodes[current.index()].usage += usage;
        }

        self.add_storage_class_usage(&entry.storage_class, usage);
        self.kinds[entry.kind as usize] += usage;
    }

    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id.index()]
    }

    pub fn name(&self, id: NodeId) -> &str {
        &self.names[self.node(id).name.0 as usize]
    }

    pub fn path(&self, id: NodeId) -> String {
        let mut segments = Vec::new();
        let mut current = id;
        while let Some(parent) = self.node(current).parent {
            segments.push(self.name(current));
            current = parent;
        }
        segments.reverse();

        let mut path = segments.join("/");
        if self.node(id).kind == NodeKind::Directory && id != Self::ROOT {
            path.push('/');
        }
        path
    }

    pub fn total(&self) -> Usage {
        self.node(Self::ROOT).usage
    }

    pub fn usage_by_kind(&self, kind: EntryKind) -> Usage {
        self.kinds[kind as usize]
    }

    pub fn storage_classes(&self) -> Vec<(&str, Usage)> {
        let mut classes: Vec<_> = self
            .storage_classes
            .iter()
            .map(|(name, usage)| (name.as_str(), *usage))
            .collect();
        classes.sort_by_key(|(_, usage)| Reverse(usage.bytes));
        classes
    }

    pub fn children_by_size(&self, id: NodeId) -> Vec<NodeId> {
        let mut children = self.node(id).children.clone();
        children.sort_by_key(|&child| Reverse(self.node(child).usage.bytes));
        children
    }

    pub fn largest_objects(&self, count: usize) -> Vec<NodeId> {
        let mut smallest_first = BinaryHeap::with_capacity(count + 1);
        for (index, node) in self.nodes.iter().enumerate() {
            if node.kind == NodeKind::Object {
                smallest_first.push(Reverse((node.usage.bytes, NodeId(index as u32))));
                if smallest_first.len() > count {
                    smallest_first.pop();
                }
            }
        }

        let mut largest: Vec<_> = smallest_first
            .into_iter()
            .map(|Reverse((_, id))| id)
            .collect();
        largest.sort_by_key(|&id| Reverse(self.node(id).usage.bytes));
        largest
    }

    fn child_or_insert(&mut self, parent: NodeId, name: &str, kind: NodeKind) -> NodeId {
        let name = self.intern(name);
        if let Some(&existing) = self.children_by_name.get(&(parent, name, kind)) {
            return existing;
        }

        let id = NodeId(u32::try_from(self.nodes.len()).expect("tree exceeds u32::MAX nodes"));
        self.nodes.push(Node {
            name,
            kind,
            parent: Some(parent),
            children: Vec::new(),
            usage: Usage::default(),
        });
        self.nodes[parent.index()].children.push(id);
        self.children_by_name.insert((parent, name, kind), id);
        id
    }

    fn intern(&mut self, name: &str) -> NameId {
        if let Some(&id) = self.name_ids.get(name) {
            return id;
        }

        let id = NameId(u32::try_from(self.names.len()).expect("tree exceeds u32::MAX names"));
        let name: Arc<str> = Arc::from(name);
        self.names.push(Arc::clone(&name));
        self.name_ids.insert(name, id);
        id
    }

    fn add_storage_class_usage(&mut self, class: &str, usage: Usage) {
        match self
            .storage_classes
            .iter_mut()
            .find(|(name, _)| name == class)
        {
            Some((_, total)) => *total += usage,
            None => self.storage_classes.push((class.to_owned(), usage)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(key: &str, size: u64) -> Entry {
        Entry {
            key: key.to_owned(),
            size,
            storage_class: "STANDARD".to_owned(),
            kind: EntryKind::Current,
        }
    }

    fn find(tree: &Tree, path: &str) -> NodeId {
        (0..tree.nodes.len() as u32)
            .map(NodeId)
            .find(|&id| tree.path(id) == path)
            .unwrap_or_else(|| panic!("no node at {path}"))
    }

    #[test]
    fn aggregates_sizes_up_the_tree() {
        let mut tree = Tree::new();
        tree.insert(&entry("logs/2024/a.log", 100));
        tree.insert(&entry("logs/2024/b.log", 50));
        tree.insert(&entry("logs/2025/c.log", 25));
        tree.insert(&entry("readme.txt", 5));

        assert_eq!(
            tree.total(),
            Usage {
                bytes: 180,
                objects: 4
            }
        );
        assert_eq!(tree.node(find(&tree, "logs/")).usage().bytes, 175);
        assert_eq!(tree.node(find(&tree, "logs/2024/")).usage().objects, 2);
        assert_eq!(tree.node(Tree::ROOT).children().len(), 2);
    }

    #[test]
    fn folder_markers_count_toward_their_directory() {
        let mut tree = Tree::new();
        tree.insert(&entry("photos/", 0));
        tree.insert(&entry("photos/cat.jpg", 10));

        let photos = find(&tree, "photos/");
        assert_eq!(
            tree.node(photos).usage(),
            Usage {
                bytes: 10,
                objects: 2
            }
        );
        assert_eq!(tree.node(photos).children().len(), 1);
    }

    #[test]
    fn object_and_directory_with_same_name_are_separate() {
        let mut tree = Tree::new();
        tree.insert(&entry("data", 1));
        tree.insert(&entry("data/part-0", 2));

        assert_eq!(tree.node(Tree::ROOT).children().len(), 2);
        assert_eq!(tree.node(find(&tree, "data")).kind(), NodeKind::Object);
        assert_eq!(tree.node(find(&tree, "data/")).kind(), NodeKind::Directory);
    }

    #[test]
    fn versions_of_one_key_share_a_node() {
        let mut tree = Tree::new();
        tree.insert(&entry("report.csv", 10));
        tree.insert(&Entry {
            kind: EntryKind::Noncurrent,
            ..entry("report.csv", 7)
        });

        assert_eq!(tree.node(find(&tree, "report.csv")).usage().bytes, 17);
        assert_eq!(tree.usage_by_kind(EntryKind::Noncurrent).bytes, 7);
        assert_eq!(tree.usage_by_kind(EntryKind::Current).bytes, 10);
    }

    #[test]
    fn largest_objects_are_sorted_descending() {
        let mut tree = Tree::new();
        for (key, size) in [("a", 5), ("b/c", 50), ("d", 20), ("e", 1)] {
            tree.insert(&entry(key, size));
        }

        let paths: Vec<_> = tree
            .largest_objects(3)
            .into_iter()
            .map(|id| tree.path(id))
            .collect();
        assert_eq!(paths, ["b/c", "d", "a"]);
    }

    #[test]
    fn storage_classes_are_sorted_by_size() {
        let mut tree = Tree::new();
        tree.insert(&entry("a", 5));
        tree.insert(&Entry {
            storage_class: "GLACIER".to_owned(),
            ..entry("b", 50)
        });

        let classes = tree.storage_classes();
        assert_eq!(
            classes[0],
            (
                "GLACIER",
                Usage {
                    bytes: 50,
                    objects: 1
                }
            )
        );
        assert_eq!(
            classes[1],
            (
                "STANDARD",
                Usage {
                    bytes: 5,
                    objects: 1
                }
            )
        );
    }
}
