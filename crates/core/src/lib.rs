mod format;
mod tree;
mod treemap;

use std::ops::AddAssign;

pub use format::{format_bytes, format_count};
pub use tree::{Node, NodeId, NodeKind, Tree};
pub use treemap::{Rect, squarify};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    pub bytes: u64,
    pub objects: u64,
}

impl AddAssign for Usage {
    fn add_assign(&mut self, other: Self) {
        self.bytes += other.bytes;
        self.objects += other.objects;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    Current,
    Noncurrent,
    DeleteMarker,
}

impl EntryKind {
    pub const ALL: [EntryKind; 3] = [Self::Current, Self::Noncurrent, Self::DeleteMarker];

    pub fn label(self) -> &'static str {
        match self {
            Self::Current => "Current versions",
            Self::Noncurrent => "Noncurrent versions",
            Self::DeleteMarker => "Delete markers",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub key: String,
    pub size: u64,
    pub storage_class: String,
    pub kind: EntryKind,
}
