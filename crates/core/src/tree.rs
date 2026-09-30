use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};
use std::sync::Arc;

use crate::arena::{BlockVec, TextArena};
use crate::{Cost, Entry, EntryKind, Pricing, Usage};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(u32);

impl NodeId {
    pub(crate) fn from_index(index: usize) -> Self {
        Self(index as u32)
    }

    pub(crate) fn index(self) -> usize {
        self.0 as usize
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NodeKind {
    Directory,
    Object,
}

/// Marks a missing parent, child, sibling, class, or file type in the compact node fields.
const NONE: u32 = u32::MAX;
const NO_CLASS: u16 = u16::MAX;
const NO_FILE_TYPE: u16 = u16::MAX;

/// Where a node's name sits in [`Tree::names`]: offset in the high 48 bits, length in
/// the low 16. Object keys are at most 1024 bytes, so 16 bits of length is plenty.
#[derive(Clone, Copy, Debug)]
struct NameRef(u64);

impl NameRef {
    fn new(range: std::ops::Range<usize>) -> Self {
        // Key segments are at most 1024 bytes (the S3 key limit).
        let len = u16::try_from(range.len()).expect("name segment longer than 65535 bytes");
        Self(((range.start as u64) << 16) | u64::from(len))
    }

    fn range(self) -> std::ops::Range<usize> {
        let offset = (self.0 >> 16) as usize;
        offset..offset + (self.0 & 0xffff) as usize
    }
}

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
    /// Parts of an upload that was never finished: billed, but not an object.
    IncompleteUpload,
}

impl VersionState {
    pub const ALL: [VersionState; 4] = [
        Self::Current,
        Self::WithOldVersions,
        Self::Deleted,
        Self::IncompleteUpload,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Current => "Current only",
            Self::WithOldVersions => "Has old versions",
            Self::Deleted => "Deleted, old versions remain",
            Self::IncompleteUpload => "Incomplete upload",
        }
    }

    fn from_flags(flags: u8) -> Option<Self> {
        let has = |kind: EntryKind| flags & version_flag(kind) != 0;
        if has(EntryKind::IncompleteUpload) {
            return Some(Self::IncompleteUpload);
        }
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

/// One folder or object. Kept to 48 bytes because a big bucket has hundreds of millions.
#[derive(Debug)]
pub struct Node {
    name: NameRef,
    bytes: u64,
    monthly_cost: Cost,
    /// Saturates at u32::MAX; bucket totals are kept exactly in [`Tree::kinds`].
    objects: u32,
    parent: u32,
    /// Children form a linked list, newest first: no per-node vector.
    first_child: u32,
    next_sibling: u32,
    storage_class: u16,
    file_type: u16,
    kind: NodeKind,
    version_flags: u8,
}

impl Node {
    fn new(name: NameRef, kind: NodeKind, parent: u32) -> Self {
        Self {
            name,
            bytes: 0,
            monthly_cost: Cost::ZERO,
            objects: 0,
            parent,
            first_child: NONE,
            next_sibling: NONE,
            storage_class: NO_CLASS,
            file_type: NO_FILE_TYPE,
            kind,
            version_flags: 0,
        }
    }

    pub fn kind(&self) -> NodeKind {
        self.kind
    }

    pub fn parent(&self) -> Option<NodeId> {
        (self.parent != NONE).then_some(NodeId(self.parent))
    }

    pub fn has_children(&self) -> bool {
        self.first_child != NONE
    }

    pub fn usage(&self) -> Usage {
        Usage {
            bytes: self.bytes,
            objects: u64::from(self.objects),
            monthly_cost: self.monthly_cost,
        }
    }

    fn add(&mut self, usage: Usage) {
        self.bytes += usage.bytes;
        self.monthly_cost += usage.monthly_cost;
        let objects = u32::try_from(usage.objects).unwrap_or(u32::MAX);
        self.objects = self.objects.saturating_add(objects);
    }
}

#[derive(Debug)]
pub struct Tree {
    nodes: BlockVec<Node>,
    /// Every node's name, back to back. See [`NameRef`].
    names: TextArena,
    /// Folders by (parent, name hash), to find the folder for each key segment. Objects
    /// are not indexed: see [`Tree::insert`].
    directories: HashMap<(u32, u64), u32>,
    storage_classes: Vec<(String, Usage)>,
    file_types: Vec<(String, Usage)>,
    file_type_ids: HashMap<String, u16>,
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
        let mut nodes = BlockVec::new();
        nodes.push(Node::new(NameRef::new(0..0), NodeKind::Directory, NONE));
        Self {
            nodes,
            names: TextArena::default(),
            directories: HashMap::new(),
            storage_classes: Vec::new(),
            file_types: Vec::new(),
            file_type_ids: HashMap::new(),
            kinds: Default::default(),
            version_states: Default::default(),
            pricing: None,
        }
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

    /// Adds one object version (or delete marker) to the tree.
    ///
    /// Versions of the same key are merged into one object node when they arrive one
    /// after another, as S3 listings return them. Objects are not indexed by name (that
    /// would cost more memory than the node itself), so a version that arrives after
    /// other objects of the same folder gets its own node.
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
        self.nodes[current.index()].add(usage);

        let mut segments = entry.key.split('/').peekable();
        while let Some(segment) = segments.next() {
            let is_last = segments.peek().is_none();
            if is_last && segment.is_empty() {
                break;
            }
            current = if is_last {
                self.object_or_insert(current, segment, entry.kind)
            } else {
                self.directory_or_insert(current, segment)
            };
            self.nodes[current.index()].add(usage);
        }

        let class = self.add_storage_class_usage(&entry.storage_class, usage);
        if self.nodes[current.index()].kind == NodeKind::Object {
            self.update_object(current, entry, class, usage);
        }
        self.kinds[entry.kind as usize] += usage;
    }

    fn update_object(&mut self, id: NodeId, entry: &Entry, class: u16, added: Usage) {
        let file_type = self.add_file_type_usage(id, added);
        let node = &mut self.nodes[id.index()];
        node.file_type = file_type;
        if entry.kind == EntryKind::Current || node.storage_class == NO_CLASS {
            node.storage_class = class;
        }

        let old_state = VersionState::from_flags(node.version_flags);
        node.version_flags |= version_flag(entry.kind);
        let new_state = VersionState::from_flags(node.version_flags);
        let usage = node.usage();

        if let Some(old) = old_state {
            let mut previous = usage;
            previous -= added;
            self.version_states[old as usize] -= previous;
        }
        if let Some(new) = new_state {
            self.version_states[new as usize] += usage;
        }
    }

    /// Number of nodes, including the root. Node ids run from 0 to `node_count() - 1`, and
    /// every node comes after its parent.
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id.index()]
    }

    pub fn name(&self, id: NodeId) -> &str {
        self.names.get(self.node(id).name.range())
    }

    /// Children of a folder, newest first.
    pub fn children(&self, id: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        let mut next = self.node(id).first_child;
        std::iter::from_fn(move || {
            (next != NONE).then(|| {
                let child = NodeId(next);
                next = self.node(child).next_sibling;
                child
            })
        })
    }

    pub fn path(&self, id: NodeId) -> String {
        let mut segments = Vec::new();
        let mut current = id;
        while let Some(parent) = self.node(current).parent() {
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
        let class = self.node(id).storage_class;
        (class != NO_CLASS).then(|| self.storage_classes[usize::from(class)].0.as_str())
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
        match kind {
            NodeKind::Directory => self.find_directory(parent, name),
            NodeKind::Object => self
                .children(parent)
                .find(|&child| self.node(child).kind == kind && self.name(child) == name),
        }
    }

    /// Everything inserted: all versions and delete markers.
    pub fn total(&self) -> Usage {
        let mut total = Usage::default();
        for usage in self.kinds {
            total += usage;
        }
        total
    }

    pub fn usage_by_kind(&self, kind: EntryKind) -> Usage {
        self.kinds[kind as usize]
    }

    /// The lowercase extension of an object (`""` for none). `None` for directories.
    pub fn file_type(&self, id: NodeId) -> Option<&str> {
        let file_type = self.node(id).file_type;
        (file_type != NO_FILE_TYPE).then(|| self.file_types[usize::from(file_type)].0.as_str())
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
        let mut children: Vec<_> = self.children(id).collect();
        children.sort_by_key(|&child| Reverse(self.node(child).bytes));
        children
    }

    pub fn largest_objects(&self, count: usize) -> Vec<NodeId> {
        let mut smallest_first = BinaryHeap::with_capacity(count + 1);
        for (index, node) in self.nodes.iter().enumerate() {
            if node.kind == NodeKind::Object {
                smallest_first.push(Reverse((node.bytes, NodeId(index as u32))));
                if smallest_first.len() > count {
                    smallest_first.pop();
                }
            }
        }

        let mut largest: Vec<_> = smallest_first
            .into_iter()
            .map(|Reverse((_, id))| id)
            .collect();
        largest.sort_by_key(|&id| Reverse(self.node(id).bytes));
        largest
    }

    fn find_directory(&self, parent: NodeId, name: &str) -> Option<NodeId> {
        match self.directories.get(&(parent.0, name_hash(name))) {
            Some(&id) if self.name(NodeId(id)) == name => Some(NodeId(id)),
            // Two names with the same hash: the second one is only found by searching.
            Some(_) => self.children(parent).find(|&child| {
                self.node(child).kind == NodeKind::Directory && self.name(child) == name
            }),
            None => None,
        }
    }

    fn directory_or_insert(&mut self, parent: NodeId, name: &str) -> NodeId {
        if let Some(existing) = self.find_directory(parent, name) {
            return existing;
        }
        let id = self.add_node(parent, name, NodeKind::Directory);
        self.directories
            .entry((parent.0, name_hash(name)))
            .or_insert(id.0);
        id
    }

    /// Reuses the folder's newest child when it is the same object (another version of
    /// the key just inserted); otherwise adds a new object node.
    /// An incomplete upload is never merged with a real object of the same key (or the
    /// other way round), so it keeps its own node and stays visible.
    fn object_or_insert(&mut self, parent: NodeId, name: &str, kind: EntryKind) -> NodeId {
        let newest = self.node(parent).first_child;
        if newest != NONE {
            let newest = NodeId(newest);
            let node = self.node(newest);
            let is_upload = |flags: u8| flags & version_flag(EntryKind::IncompleteUpload) != 0;
            if node.kind == NodeKind::Object
                && self.name(newest) == name
                && is_upload(node.version_flags) == (kind == EntryKind::IncompleteUpload)
            {
                return newest;
            }
        }
        self.add_node(parent, name, NodeKind::Object)
    }

    fn add_node(&mut self, parent: NodeId, name: &str, kind: NodeKind) -> NodeId {
        // Indexes are u32 to keep nodes small; NONE (u32::MAX) is reserved.
        let id = u32::try_from(self.nodes.len())
            .ok()
            .filter(|&id| id != NONE)
            .expect("tree exceeds u32::MAX - 1 nodes");
        let name_ref = NameRef::new(self.names.push(name));

        let mut node = Node::new(name_ref, kind, parent.0);
        let parent_node = &mut self.nodes[parent.index()];
        node.next_sibling = parent_node.first_child;
        parent_node.first_child = id;
        self.nodes.push(node);
        NodeId(id)
    }

    fn add_file_type_usage(&mut self, id: NodeId, usage: Usage) -> u16 {
        let mut file_type = file_type(self.name(id));
        // Past 65534 distinct types, new ones count as "no extension".
        if !self.file_type_ids.contains_key(&file_type)
            && self.file_types.len() >= usize::from(NO_FILE_TYPE)
        {
            file_type = String::new();
        }
        let index = match self.file_type_ids.get(&file_type) {
            Some(&index) => index,
            None => {
                let index = self.file_types.len() as u16;
                self.file_types.push((file_type.clone(), Usage::default()));
                self.file_type_ids.insert(file_type, index);
                index
            }
        };
        self.file_types[usize::from(index)].1 += usage;
        index
    }

    fn add_storage_class_usage(&mut self, class: &str, usage: Usage) -> u16 {
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
        // There are about ten storage classes; u16 leaves room for any provider.
        u16::try_from(index).expect("more than 65534 distinct storage classes")
    }
}

/// FNV-1a: fast, and good enough to key the folder index (matches are verified).
fn name_hash(name: &str) -> u64 {
    name.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    })
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
        assert_eq!(tree.children(Tree::ROOT).count(), 2);
    }

    #[test]
    fn folder_markers_count_toward_their_directory() {
        let mut tree = Tree::new();
        tree.insert(&entry("photos/", 0));
        tree.insert(&entry("photos/cat.jpg", 10));

        let photos = find(&tree, "photos/");
        assert_eq!(tree.node(photos).usage(), Usage::new(10, 2));
        assert_eq!(tree.children(photos).count(), 1);
    }

    #[test]
    fn object_and_directory_with_same_name_are_separate() {
        let mut tree = Tree::new();
        tree.insert(&entry("data", 1));
        tree.insert(&entry("data/part-0", 2));

        assert_eq!(tree.children(Tree::ROOT).count(), 2);
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
    fn nodes_stay_small() {
        assert!(std::mem::size_of::<Node>() <= 48);
    }

    #[test]
    fn merges_versions_that_arrive_together() {
        let mut tree = Tree::new();
        tree.insert(&entry("docs/a.txt", 5));
        tree.insert(&Entry {
            kind: EntryKind::Noncurrent,
            ..entry("docs/a.txt", 3)
        });
        tree.insert(&entry("docs/b.txt", 1));

        let docs = find(&tree, "docs/");
        assert_eq!(tree.children(docs).count(), 2);
        assert_eq!(
            tree.node(find(&tree, "docs/a.txt")).usage(),
            Usage::new(8, 2)
        );
    }

    #[test]
    fn incomplete_uploads_get_their_own_node() {
        let mut tree = Tree::new();
        tree.insert(&entry("video/cut.mov", 100));
        tree.insert(&Entry {
            kind: EntryKind::IncompleteUpload,
            ..entry("video/cut.mov", 40)
        });
        tree.insert(&Entry {
            kind: EntryKind::IncompleteUpload,
            ..entry("video/cut.mov", 10)
        });

        let video = find(&tree, "video/");
        let states: Vec<_> = tree
            .children(video)
            .map(|child| (tree.version_state(child), tree.node(child).usage().bytes))
            .collect();
        assert_eq!(
            states,
            [
                (Some(VersionState::IncompleteUpload), 50),
                (Some(VersionState::Current), 100),
            ]
        );
        assert_eq!(
            tree.usage_by_kind(EntryKind::IncompleteUpload),
            Usage::new(50, 2)
        );
        assert_eq!(
            tree.usage_by_version_state(VersionState::IncompleteUpload),
            Usage::new(50, 2)
        );
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
