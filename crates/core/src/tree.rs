use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};
use std::sync::Arc;

use crate::{Entry, EntryKind, Pricing, Usage};

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ClassId(u16);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FileTypeId(u32);

/// Longest extension counted as a file type; longer suffixes are usually part of a name
/// (`backup.2024-01-01T12-00`), not a type.
const MAX_EXTENSION_LEN: usize = 10;

/// The lowercase extension of an object name, or `""` when it has none.
pub fn file_type(name: &str) -> String {
    match name.rsplit_once('.') {
        Some((stem, extension))
            if !stem.is_empty()
                && (1..=MAX_EXTENSION_LEN).contains(&extension.len())
                && extension.bytes().all(|byte| byte.is_ascii_alphanumeric()) =>
        {
            extension.to_ascii_lowercase()
        }
        _ => String::new(),
    }
}

/// What an object's versions add up to, as far as the scan saw them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VersionState {
    /// Only a current version.
    Current,
    /// A current version plus older, noncurrent versions that are still billed.
    WithOldVersions,
    /// No current version: deleted (or overwritten away), but older versions remain.
    Deleted,
}

impl VersionState {
    pub const ALL: [VersionState; 3] = [Self::Current, Self::WithOldVersions, Self::Deleted];

    pub fn label(self) -> &'static str {
        match self {
            Self::Current => "Current only",
            Self::WithOldVersions => "Has old versions",
            Self::Deleted => "Deleted, old versions remain",
        }
    }

    fn from_flags(flags: u8) -> Option<Self> {
        let has = |kind: EntryKind| flags & version_flag(kind) != 0;
        match (has(EntryKind::Current), has(EntryKind::Noncurrent)) {
            _ if flags == 0 => None,
            (true, false) => Some(Self::Current),
            (true, true) => Some(Self::WithOldVersions),
            (false, _) => Some(Self::Deleted),
        }
    }
}

fn version_flag(kind: EntryKind) -> u8 {
    1 << kind as u8
}

#[derive(Debug)]
pub struct Node {
    name: NameId,
    kind: NodeKind,
    parent: Option<NodeId>,
    children: Vec<NodeId>,
    usage: Usage,
    storage_class: Option<ClassId>,
    file_type: Option<FileTypeId>,
    version_flags: u8,
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
    file_types: Vec<(String, Usage)>,
    file_type_ids: HashMap<String, FileTypeId>,
    kinds: [Usage; EntryKind::ALL.len()],
    version_states: [Usage; VersionState::ALL.len()],
    pricing: Option<Arc<dyn Pricing>>,
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
            file_types: Vec::new(),
            file_type_ids: HashMap::new(),
            kinds: Default::default(),
            version_states: Default::default(),
            pricing: None,
        };
        let root_name = tree.intern("");
        tree.nodes.push(Node {
            name: root_name,
            kind: NodeKind::Directory,
            parent: None,
            children: Vec::new(),
            usage: Usage::default(),
            storage_class: None,
            file_type: None,
            version_flags: 0,
        });
        tree
    }

    /// A tree that also adds up the monthly cost of everything inserted.
    pub fn with_pricing(pricing: Arc<dyn Pricing>) -> Self {
        Self {
            pricing: Some(pricing),
            ..Self::new()
        }
    }

    pub fn has_pricing(&self) -> bool {
        self.pricing.is_some()
    }

    pub fn insert(&mut self, entry: &Entry) {
        let usage = Usage {
            bytes: entry.size,
            objects: 1,
            monthly_cost: self
                .pricing
                .as_ref()
                .map(|pricing| pricing.monthly_cost(entry))
                .unwrap_or_default(),
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

        let class = self.add_storage_class_usage(&entry.storage_class, usage);
        if self.nodes[current.index()].kind == NodeKind::Object {
            self.update_object(current, entry, class, usage);
        }
        self.kinds[entry.kind as usize] += usage;
    }

    fn update_object(&mut self, id: NodeId, entry: &Entry, class: ClassId, added: Usage) {
        let file_type = self.add_file_type_usage(self.node(id).name, added);
        self.nodes[id.index()].file_type = Some(file_type);

        let node = &mut self.nodes[id.index()];
        if entry.kind == EntryKind::Current || node.storage_class.is_none() {
            node.storage_class = Some(class);
        }

        let old_state = VersionState::from_flags(node.version_flags);
        node.version_flags |= version_flag(entry.kind);
        let new_state = VersionState::from_flags(node.version_flags);
        let usage = node.usage;

        if let Some(old) = old_state {
            let mut previous = usage;
            previous -= added;
            self.version_states[old as usize] -= previous;
        }
        if let Some(new) = new_state {
            self.version_states[new as usize] += usage;
        }
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

    /// The storage class of an object's current version (or of any version when
    /// there is no current one). `None` for directories.
    pub fn storage_class(&self, id: NodeId) -> Option<&str> {
        let class = self.node(id).storage_class?;
        Some(&self.storage_classes[usize::from(class.0)].0)
    }

    /// `None` for directories.
    pub fn version_state(&self, id: NodeId) -> Option<VersionState> {
        VersionState::from_flags(self.node(id).version_flags)
    }

    /// Total usage of objects in each [`VersionState`].
    pub fn usage_by_version_state(&self, state: VersionState) -> Usage {
        self.version_states[state as usize]
    }

    pub fn find_child(&self, parent: NodeId, name: &str, kind: NodeKind) -> Option<NodeId> {
        let name = *self.name_ids.get(name)?;
        self.children_by_name.get(&(parent, name, kind)).copied()
    }

    pub fn total(&self) -> Usage {
        self.node(Self::ROOT).usage
    }

    pub fn usage_by_kind(&self, kind: EntryKind) -> Usage {
        self.kinds[kind as usize]
    }

    /// The lowercase extension of an object (`""` for none). `None` for directories.
    pub fn file_type(&self, id: NodeId) -> Option<&str> {
        let file_type = self.node(id).file_type?;
        Some(&self.file_types[file_type.0 as usize].0)
    }

    /// Usage per file type (`""` for no extension), largest first.
    pub fn file_types(&self) -> Vec<(&str, Usage)> {
        let mut types: Vec<_> = self
            .file_types
            .iter()
            .map(|(name, usage)| (name.as_str(), *usage))
            .collect();
        types.sort_by_key(|(_, usage)| Reverse(usage.bytes));
        types
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
            storage_class: None,
            file_type: None,
            version_flags: 0,
        });
        self.nodes[parent.index()].children.push(id);
        self.children_by_name.insert((parent, name, kind), id);
        id
    }

    fn add_file_type_usage(&mut self, name: NameId, usage: Usage) -> FileTypeId {
        let file_type = file_type(&self.names[name.0 as usize]);
        let id = match self.file_type_ids.get(&file_type) {
            Some(&id) => id,
            None => {
                let id = FileTypeId(
                    u32::try_from(self.file_types.len()).expect("tree exceeds u32::MAX file types"),
                );
                self.file_types.push((file_type.clone(), Usage::default()));
                self.file_type_ids.insert(file_type, id);
                id
            }
        };
        self.file_types[id.0 as usize].1 += usage;
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

    fn add_storage_class_usage(&mut self, class: &str, usage: Usage) -> ClassId {
        let index = match self
            .storage_classes
            .iter()
            .position(|(name, _)| name == class)
        {
            Some(index) => index,
            None => {
                self.storage_classes
                    .push((class.to_owned(), Usage::default()));
                self.storage_classes.len() - 1
            }
        };
        self.storage_classes[index].1 += usage;
        ClassId(u16::try_from(index).expect("more than u16::MAX distinct storage classes"))
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

        assert_eq!(tree.total(), Usage::new(180, 4));
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
        assert_eq!(tree.node(photos).usage(), Usage::new(10, 2));
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
    fn objects_keep_the_storage_class_of_their_current_version() {
        let mut tree = Tree::new();
        tree.insert(&Entry {
            storage_class: "GLACIER".to_owned(),
            kind: EntryKind::Noncurrent,
            ..entry("a/old.bin", 5)
        });
        tree.insert(&entry("a/old.bin", 5));
        tree.insert(&Entry {
            storage_class: "GLACIER".to_owned(),
            kind: EntryKind::Noncurrent,
            ..entry("b.bin", 1)
        });

        assert_eq!(
            tree.storage_class(find(&tree, "a/old.bin")),
            Some("STANDARD")
        );
        assert_eq!(tree.storage_class(find(&tree, "b.bin")), Some("GLACIER"));
        assert_eq!(tree.storage_class(find(&tree, "a/")), None);
    }

    #[test]
    fn version_states_follow_each_object() {
        let mut tree = Tree::new();
        tree.insert(&entry("plain", 10));
        tree.insert(&entry("edited", 20));
        tree.insert(&Entry {
            kind: EntryKind::Noncurrent,
            ..entry("edited", 5)
        });
        tree.insert(&Entry {
            kind: EntryKind::Noncurrent,
            ..entry("gone", 7)
        });
        tree.insert(&Entry {
            kind: EntryKind::DeleteMarker,
            ..entry("gone", 0)
        });

        assert_eq!(
            tree.version_state(find(&tree, "plain")),
            Some(VersionState::Current)
        );
        assert_eq!(
            tree.version_state(find(&tree, "edited")),
            Some(VersionState::WithOldVersions)
        );
        assert_eq!(
            tree.version_state(find(&tree, "gone")),
            Some(VersionState::Deleted)
        );
        assert_eq!(tree.version_state(Tree::ROOT), None);

        let usage = |state| tree.usage_by_version_state(state);
        assert_eq!(usage(VersionState::Current), Usage::new(10, 1));
        assert_eq!(usage(VersionState::WithOldVersions), Usage::new(25, 2));
        assert_eq!(usage(VersionState::Deleted), Usage::new(7, 2));
    }

    #[derive(Debug)]
    struct OneNanodollarPerByte;

    impl Pricing for OneNanodollarPerByte {
        fn monthly_cost(&self, entry: &Entry) -> crate::Cost {
            crate::Cost::from_usd(entry.size as f64 / 1e9)
        }
    }

    #[test]
    fn adds_up_monthly_cost_when_priced() {
        let mut tree = Tree::with_pricing(Arc::new(OneNanodollarPerByte));
        tree.insert(&entry("logs/a", 100));
        tree.insert(&entry("logs/b", 50));
        tree.insert(&entry("c", 7));

        let cost = |id| tree.node(id).usage().monthly_cost;
        assert_eq!(cost(find(&tree, "logs/")), crate::Cost::from_usd(150e-9));
        assert_eq!(tree.total().monthly_cost, crate::Cost::from_usd(157e-9));
        assert_eq!(
            tree.storage_classes()[0].1.monthly_cost,
            tree.total().monthly_cost
        );
        assert!(!Tree::new().has_pricing());
    }

    #[test]
    fn groups_objects_by_file_type() {
        let mut tree = Tree::new();
        tree.insert(&entry("video/a.MOV", 100));
        tree.insert(&entry("video/b.mov", 50));
        tree.insert(&entry("notes.txt", 5));
        tree.insert(&entry("Makefile", 1));
        tree.insert(&entry("folder/", 0));

        assert_eq!(
            tree.file_types(),
            [
                ("mov", Usage::new(150, 2)),
                ("txt", Usage::new(5, 1)),
                ("", Usage::new(1, 1)),
            ]
        );
        assert_eq!(tree.file_type(find(&tree, "video/a.MOV")), Some("mov"));
        assert_eq!(tree.file_type(find(&tree, "video/")), None);
    }

    #[test]
    fn only_short_alphanumeric_suffixes_are_file_types() {
        assert_eq!(file_type("photo.JPG"), "jpg");
        assert_eq!(file_type("syslog.1"), "1");
        assert_eq!(file_type("archive.tar.gz"), "gz");
        assert_eq!(file_type(".gitignore"), "");
        assert_eq!(file_type("README"), "");
        assert_eq!(file_type("backup.2024-01-01T12-00"), "");
        assert_eq!(file_type("name.averyveryverylongsuffix"), "");
    }

    #[test]
    fn finds_children_by_name() {
        let mut tree = Tree::new();
        tree.insert(&entry("logs/2024/a.log", 1));

        let logs = tree.find_child(Tree::ROOT, "logs", NodeKind::Directory);
        assert_eq!(logs, Some(find(&tree, "logs/")));
        assert_eq!(tree.find_child(Tree::ROOT, "logs", NodeKind::Object), None);
        assert_eq!(
            tree.find_child(Tree::ROOT, "missing", NodeKind::Directory),
            None
        );
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
        assert_eq!(classes[0], ("GLACIER", Usage::new(50, 1)));
        assert_eq!(classes[1], ("STANDARD", Usage::new(5, 1)));
    }
}
