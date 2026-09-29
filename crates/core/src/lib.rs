mod format;
mod tree;
mod treemap;

use std::ops::{AddAssign, SubAssign};

pub use format::{format_bytes, format_count, format_usd};
pub use tree::{Node, NodeId, NodeKind, Tree, VersionState};
pub use treemap::{Rect, squarify};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    pub bytes: u64,
    pub objects: u64,
    pub monthly_cost: Cost,
}

impl Usage {
    pub const fn new(bytes: u64, objects: u64) -> Self {
        Self {
            bytes,
            objects,
            monthly_cost: Cost::ZERO,
        }
    }
}

impl AddAssign for Usage {
    fn add_assign(&mut self, other: Self) {
        self.bytes += other.bytes;
        self.objects += other.objects;
        self.monthly_cost += other.monthly_cost;
    }
}

impl SubAssign for Usage {
    fn sub_assign(&mut self, other: Self) {
        self.bytes -= other.bytes;
        self.objects -= other.objects;
        self.monthly_cost -= other.monthly_cost;
    }
}

/// An amount of money in billionths of a US dollar. Whole numbers keep sums over
/// millions of tiny objects exact.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Cost(u64);

impl Cost {
    pub const ZERO: Cost = Cost(0);

    pub fn from_usd(usd: f64) -> Self {
        Self((usd * 1e9).round().max(0.0) as u64)
    }

    pub fn usd(self) -> f64 {
        self.0 as f64 / 1e9
    }
}

impl AddAssign for Cost {
    fn add_assign(&mut self, other: Self) {
        self.0 += other.0;
    }
}

impl SubAssign for Cost {
    fn sub_assign(&mut self, other: Self) {
        self.0 -= other.0;
    }
}

/// Estimates what keeping an entry stored costs per month. Each provider supplies one.
pub trait Pricing: std::fmt::Debug + Send + Sync {
    fn monthly_cost(&self, entry: &Entry) -> Cost;
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
