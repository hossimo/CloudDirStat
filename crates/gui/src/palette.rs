use std::collections::HashMap;

use clouddirstat_core::{NodeId, NodeKind, Tree, VersionState};
use eframe::egui::Color32;

pub const HOVER: Color32 = Color32::WHITE;
pub const CONTAINING_FOLDER: Color32 = Color32::from_rgb(0xff, 0xd8, 0x4d);
pub const SELECTED: Color32 = Color32::from_rgb(0x5a, 0xc8, 0xfa);
pub const FOLDER_ICON: Color32 = Color32::from_rgb(0xe8, 0xc0, 0x4a);
const NEUTRAL: Color32 = Color32::from_rgb(0x80, 0x80, 0x80);

/// Colors for top-level prefixes, largest first. Anything past these is [`NEUTRAL`].
const PREFIX_COLORS: [Color32; 12] = [
    Color32::from_rgb(0x4e, 0x79, 0xa7),
    Color32::from_rgb(0xe1, 0x57, 0x59),
    Color32::from_rgb(0x59, 0xa1, 0x4f),
    Color32::from_rgb(0xf2, 0x8e, 0x2b),
    Color32::from_rgb(0xb0, 0x7a, 0xa1),
    Color32::from_rgb(0x76, 0xb7, 0xb2),
    Color32::from_rgb(0xed, 0xc9, 0x48),
    Color32::from_rgb(0xff, 0x9d, 0xa7),
    Color32::from_rgb(0x9c, 0x75, 0x5f),
    Color32::from_rgb(0x6b, 0x6e, 0xcf),
    Color32::from_rgb(0x8c, 0xd1, 0x7d),
    Color32::from_rgb(0xd3, 0x72, 0xe0),
];

pub fn storage_class(class: &str) -> Color32 {
    match class {
        "STANDARD" => Color32::from_rgb(0x4e, 0x79, 0xa7),
        "INTELLIGENT_TIERING" => Color32::from_rgb(0xb0, 0x7a, 0xa1),
        "STANDARD_IA" => Color32::from_rgb(0x59, 0xa1, 0x4f),
        "ONEZONE_IA" => Color32::from_rgb(0x8c, 0xd1, 0x7d),
        "GLACIER_IR" => Color32::from_rgb(0x76, 0xb7, 0xb2),
        "GLACIER" => Color32::from_rgb(0xf2, 0x8e, 0x2b),
        "DEEP_ARCHIVE" => Color32::from_rgb(0xe1, 0x57, 0x59),
        "EXPRESS_ONEZONE" => Color32::from_rgb(0xed, 0xc9, 0x48),
        "REDUCED_REDUNDANCY" => Color32::from_rgb(0xba, 0xb0, 0xac),
        _ => Color32::from_rgb(0x9c, 0x75, 0x5f),
    }
}

pub fn version_state(state: VersionState) -> Color32 {
    match state {
        VersionState::Current => Color32::from_rgb(0x4e, 0x79, 0xa7),
        VersionState::WithOldVersions => Color32::from_rgb(0xf2, 0x8e, 0x2b),
        VersionState::Deleted => Color32::from_rgb(0xe1, 0x57, 0x59),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorMode {
    StorageClass,
    Versions,
    Prefixes,
}

impl ColorMode {
    pub const ALL: [ColorMode; 3] = [Self::StorageClass, Self::Versions, Self::Prefixes];

    pub fn label(self) -> &'static str {
        match self {
            Self::StorageClass => "Storage classes",
            Self::Versions => "Versions",
            Self::Prefixes => "Prefixes",
        }
    }
}

/// Decides the color of every node for one [`ColorMode`].
pub struct Colors {
    mode: ColorMode,
    root: NodeId,
    prefixes: HashMap<NodeId, Color32>,
}

impl Colors {
    pub fn new(mode: ColorMode, tree: &Tree, root: NodeId) -> Self {
        let prefixes = match mode {
            ColorMode::Prefixes => top_prefixes(tree, root)
                .into_iter()
                .zip(PREFIX_COLORS)
                .collect(),
            _ => HashMap::new(),
        };
        Self {
            mode,
            root,
            prefixes,
        }
    }

    pub fn node(&self, tree: &Tree, id: NodeId) -> Color32 {
        match self.mode {
            ColorMode::StorageClass => tree.storage_class(id).map_or(NEUTRAL, storage_class),
            ColorMode::Versions => tree.version_state(id).map_or(NEUTRAL, version_state),
            ColorMode::Prefixes => self.prefix_of(tree, id),
        }
    }

    /// The color of `prefix` if it is one of the colored top-level prefixes.
    pub fn prefix(&self, prefix: NodeId) -> Option<Color32> {
        self.prefixes.get(&prefix).copied()
    }

    pub fn other(&self) -> Color32 {
        NEUTRAL
    }

    fn prefix_of(&self, tree: &Tree, id: NodeId) -> Color32 {
        let mut current = id;
        while let Some(parent) = tree.node(current).parent() {
            if parent == self.root {
                return self.prefix(current).unwrap_or(NEUTRAL);
            }
            current = parent;
        }
        NEUTRAL
    }
}

/// The largest direct children of `root` that get their own color.
pub fn top_prefixes(tree: &Tree, root: NodeId) -> Vec<NodeId> {
    tree.children_by_size(root)
        .into_iter()
        .filter(|&child| tree.node(child).kind() == NodeKind::Directory)
        .take(PREFIX_COLORS.len())
        .collect()
}
