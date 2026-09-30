use std::collections::HashSet;

use clouddirstat_core::{NodeId, Subset, Tree, VersionState};

use crate::palette;

/// Most file types listed by name in the legend; the rest share one row.
pub const MAX_FILE_TYPE_ROWS: usize = 50;

/// Limits the folder list and treemap to the objects of one legend row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Filter {
    StorageClass(String),
    VersionState(VersionState),
    /// Everything under one top-level prefix.
    Prefix(NodeId),
    /// Everything outside the colored top-level prefixes.
    OtherPrefixes,
    FileType(String),
    /// File types past the ones the legend lists by name.
    OtherFileTypes,
}

impl Filter {
    pub fn label(&self, tree: &Tree) -> String {
        match self {
            Self::StorageClass(class) => format!("Storage class {class}"),
            Self::VersionState(state) => state.label().to_owned(),
            Self::Prefix(prefix) => format!("Prefix {}/", tree.name(*prefix)),
            Self::OtherPrefixes => "Outside the largest prefixes".to_owned(),
            Self::FileType(file_type) => format!("File type {}", file_type_label(file_type)),
            Self::OtherFileTypes => "Less common file types".to_owned(),
        }
    }

    /// The objects under `root` that pass this filter.
    pub fn subset(&self, tree: &Tree, root: NodeId) -> Subset {
        match self {
            Self::StorageClass(class) => {
                Subset::new(tree, |id| tree.storage_class(id) == Some(class.as_str()))
            }
            Self::VersionState(state) => {
                Subset::new(tree, |id| tree.version_state(id) == Some(*state))
            }
            Self::Prefix(prefix) => {
                Subset::new(tree, |id| top_level(tree, root, id) == Some(*prefix))
            }
            Self::OtherPrefixes => {
                let top: HashSet<NodeId> = palette::top_prefixes(tree, root).into_iter().collect();
                Subset::new(tree, |id| {
                    top_level(tree, root, id).is_some_and(|outer| !top.contains(&outer))
                })
            }
            Self::FileType(file_type) => {
                Subset::new(tree, |id| tree.file_type(id) == Some(file_type.as_str()))
            }
            Self::OtherFileTypes => {
                let listed: HashSet<&str> = tree
                    .file_types()
                    .into_iter()
                    .take(MAX_FILE_TYPE_ROWS)
                    .map(|(file_type, _)| file_type)
                    .collect();
                Subset::new(tree, |id| {
                    tree.file_type(id)
                        .is_some_and(|file_type| !listed.contains(file_type))
                })
            }
        }
    }
}

pub fn file_type_label(file_type: &str) -> String {
    if file_type.is_empty() {
        "(no extension)".to_owned()
    } else {
        format!(".{file_type}")
    }
}

/// The child of `root` that `id` is in (or is), or `None` when `id` is not under `root`.
fn top_level(tree: &Tree, root: NodeId, id: NodeId) -> Option<NodeId> {
    let mut current = id;
    while let Some(parent) = tree.node(current).parent() {
        if parent == root {
            return Some(current);
        }
        current = parent;
    }
    None
}
