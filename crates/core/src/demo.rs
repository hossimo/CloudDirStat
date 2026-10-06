//! Made-up buckets and objects for screenshots, demos, and benchmarks, so no real data
//! is ever shown. Deterministic: the same seed always produces the same objects.
//! Only compiled with the `demo` feature.

use crate::{Entry, EntryKind};

const KB: u64 = 1024;
const MB: u64 = 1024 * KB;
const GB: u64 = 1024 * MB;
const DAY: u64 = 86_400;
/// 2026-09-01: demo files were last changed up to a few years before this.
const DEMO_TODAY: u64 = 20_697 * DAY;

/// A made-up bucket with its own mix of folders, file types, and storage classes.
pub struct DemoBucket {
    pub name: &'static str,
    pub region: &'static str,
    /// Share of all demo objects that land in this bucket.
    weight: u32,
    top_prefixes: &'static [&'static str],
    sub_prefixes: &'static [&'static str],
    stems: &'static [&'static str],
    /// Extension, smallest and largest size, weight.
    file_types: &'static [(&'static str, u64, u64, u32)],
    storage_classes: &'static [(&'static str, u32)],
    /// Percent of objects with older versions, and percent deleted with versions left.
    versioned_percent: u32,
    deleted_percent: u32,
    /// Per mille of objects that are abandoned multipart uploads instead.
    abandoned_uploads_per_mille: u32,
}

pub const BUCKETS: &[DemoBucket] = &[
    DemoBucket {
        name: "acme-media-archive",
        region: "us-east-1",
        weight: 8,
        top_prefixes: &["projects", "stock-footage", "music", "archive-2019"],
        sub_prefixes: &[
            "northwind",
            "contoso",
            "fabrikam",
            "tailspin",
            "wingtip",
            "raw",
            "renders",
        ],
        stems: &[
            "interview",
            "broll",
            "drone",
            "master",
            "final_cut",
            "shot",
            "grade",
        ],
        file_types: &[
            ("mov", 200 * MB, 40 * GB, 30),
            ("mp4", 50 * MB, 8 * GB, 25),
            ("mxf", GB, 60 * GB, 10),
            ("wav", 10 * MB, 2 * GB, 15),
            ("psd", 20 * MB, 900 * MB, 10),
            ("jpg", 500 * KB, 25 * MB, 10),
        ],
        storage_classes: &[
            ("DEEP_ARCHIVE", 55),
            ("GLACIER", 20),
            ("STANDARD_IA", 15),
            ("STANDARD", 10),
        ],
        versioned_percent: 3,
        deleted_percent: 1,
        abandoned_uploads_per_mille: 12,
    },
    DemoBucket {
        name: "acme-app-logs",
        region: "us-west-2",
        weight: 36,
        top_prefixes: &["api", "checkout", "search", "auth", "cdn", "workers"],
        sub_prefixes: &[
            "2025-10", "2025-11", "2025-12", "2026-01", "2026-02", "2026-03", "2026-04",
        ],
        stems: &["part", "events", "access", "errors", "audit"],
        file_types: &[
            ("gz", 20 * KB, 80 * MB, 60),
            ("json", 2 * KB, 5 * MB, 25),
            ("log", KB, 20 * MB, 15),
        ],
        storage_classes: &[("STANDARD", 45), ("STANDARD_IA", 30), ("GLACIER_IR", 25)],
        versioned_percent: 0,
        deleted_percent: 0,
        abandoned_uploads_per_mille: 0,
    },
    DemoBucket {
        name: "acme-backups",
        region: "eu-central-1",
        weight: 3,
        top_prefixes: &["databases", "servers", "laptops", "configs"],
        sub_prefixes: &[
            "orders-db",
            "crm",
            "fileserver-01",
            "build-agent",
            "hr",
            "wiki",
        ],
        stems: &["nightly", "weekly", "full", "incremental", "snapshot"],
        file_types: &[
            ("bak", GB, 180 * GB, 30),
            ("gz", 100 * MB, 40 * GB, 30),
            ("vhdx", 20 * GB, 400 * GB, 10),
            ("sql", 10 * MB, 8 * GB, 20),
            ("zip", 50 * MB, 10 * GB, 10),
        ],
        storage_classes: &[
            ("GLACIER_IR", 35),
            ("DEEP_ARCHIVE", 35),
            ("STANDARD_IA", 20),
            ("STANDARD", 10),
        ],
        versioned_percent: 45,
        deleted_percent: 8,
        abandoned_uploads_per_mille: 25,
    },
    DemoBucket {
        name: "acme-web-assets",
        region: "us-east-1",
        weight: 15,
        top_prefixes: &["images", "css", "js", "fonts", "downloads", "video"],
        sub_prefixes: &["home", "products", "blog", "docs", "careers", "legacy"],
        stems: &["hero", "thumb", "banner", "icon", "app", "vendor", "guide"],
        file_types: &[
            ("png", 5 * KB, 4 * MB, 25),
            ("jpg", 20 * KB, 6 * MB, 20),
            ("webp", 5 * KB, 2 * MB, 15),
            ("svg", KB, 200 * KB, 10),
            ("js", 2 * KB, 3 * MB, 10),
            ("css", KB, 500 * KB, 5),
            ("woff2", 20 * KB, 300 * KB, 5),
            ("pdf", 100 * KB, 40 * MB, 5),
            ("mp4", 5 * MB, 400 * MB, 5),
        ],
        storage_classes: &[("STANDARD", 65), ("INTELLIGENT_TIERING", 35)],
        versioned_percent: 20,
        deleted_percent: 5,
        abandoned_uploads_per_mille: 1,
    },
    DemoBucket {
        name: "acme-data-lake",
        region: "eu-west-1",
        weight: 22,
        top_prefixes: &["raw", "curated", "exports", "tmp"],
        sub_prefixes: &[
            "orders",
            "clickstream",
            "inventory",
            "customers",
            "telemetry",
            "payments",
        ],
        stems: &["part", "batch", "snapshot", "extract"],
        file_types: &[
            ("parquet", 5 * MB, 2 * GB, 50),
            ("csv", 100 * KB, 900 * MB, 20),
            ("json", 10 * KB, 300 * MB, 15),
            ("avro", MB, 700 * MB, 15),
        ],
        storage_classes: &[
            ("INTELLIGENT_TIERING", 50),
            ("STANDARD", 30),
            ("STANDARD_IA", 20),
        ],
        versioned_percent: 5,
        deleted_percent: 2,
        abandoned_uploads_per_mille: 4,
    },
    DemoBucket {
        name: "acme-user-uploads",
        region: "ap-southeast-2",
        weight: 16,
        top_prefixes: &["avatars", "documents", "photos", "attachments"],
        sub_prefixes: &["2024", "2025", "2026", "quarantine"],
        stems: &["img", "scan", "receipt", "contract", "photo", "upload"],
        file_types: &[
            ("jpg", 50 * KB, 12 * MB, 35),
            ("heic", 500 * KB, 8 * MB, 15),
            ("png", 20 * KB, 6 * MB, 15),
            ("pdf", 50 * KB, 30 * MB, 20),
            ("docx", 20 * KB, 10 * MB, 10),
            ("mp4", 2 * MB, 900 * MB, 5),
        ],
        storage_classes: &[("STANDARD", 60), ("ONEZONE_IA", 25), ("STANDARD_IA", 15)],
        versioned_percent: 10,
        deleted_percent: 6,
        abandoned_uploads_per_mille: 3,
    },
];

impl DemoBucket {
    /// About `objects` made-up objects (plus their older versions and delete markers
    /// when `with_versions`), with keys relative to the bucket. Keys come out grouped
    /// by folder, like a real listing.
    pub fn entries(
        &self,
        objects: u64,
        with_versions: bool,
        seed: u64,
    ) -> impl Iterator<Item = Entry> + '_ {
        let mut rng = Rng::new(seed ^ hash_name(self.name));
        let mut counter = 0u64;
        let mut produced = 0u64;
        let mut pending: Vec<Entry> = Vec::new();

        std::iter::from_fn(move || {
            while pending.is_empty() {
                if produced >= objects {
                    return None;
                }
                let folder = self.folder(&mut rng);
                // Folders hold very different numbers of objects, like real buckets.
                let largest_batch = 1 + rng.below(400);
                let batch = 1 + rng.below(largest_batch);
                for _ in 0..batch.min(objects - produced) {
                    counter += 1;
                    produced += 1;
                    self.object_versions(&mut rng, &folder, counter, with_versions, &mut pending);
                }
                pending.reverse();
            }
            pending.pop()
        })
    }

    fn folder(&self, rng: &mut Rng) -> String {
        let top = rng.pick(self.top_prefixes);
        let sub = rng.pick(self.sub_prefixes);
        format!("{top}/{sub}/")
    }

    fn object_versions(
        &self,
        rng: &mut Rng,
        folder: &str,
        counter: u64,
        with_versions: bool,
        out: &mut Vec<Entry>,
    ) {
        let &(extension, smallest, largest, _) = rng.weighted(self.file_types, |t| t.3);
        let key = format!("{folder}{}_{counter:06}.{extension}", rng.pick(self.stems));
        let size = rng.log_uniform(smallest, largest);
        let class = rng.weighted(self.storage_classes, |c| c.1).0;
        // Dates come from their own generator so the rest of the data stays the same
        // as before dates existed. Most files are recent; a few are years old.
        let mut dates = Rng::new(counter ^ hash_name(self.name));
        let modified = DEMO_TODAY - dates.log_uniform(1, 1500) * DAY - dates.below(DAY);
        let entry = |size, kind, modified| Entry {
            key: key.clone(),
            size,
            storage_class: class.into(),
            kind,
            last_modified: Some(modified),
        };

        if (rng.below(1000) as u32) < self.abandoned_uploads_per_mille {
            let uploaded = size / 100 * (10 + rng.below(80));
            out.push(entry(uploaded, EntryKind::IncompleteUpload, modified));
            return;
        }

        let roll = rng.below(100) as u32;
        let deleted = with_versions && roll < self.deleted_percent;
        let versioned = with_versions && roll < self.deleted_percent + self.versioned_percent;

        if !deleted {
            out.push(entry(size, EntryKind::Current, modified));
        }
        if versioned {
            let mut version_modified = modified;
            for _ in 0..1 + rng.below(3) {
                let older = size / 100 * (80 + rng.below(40));
                version_modified -= (1 + dates.below(120)) * DAY;
                out.push(entry(older, EntryKind::Noncurrent, version_modified));
            }
        }
        if deleted {
            out.push(entry(0, EntryKind::DeleteMarker, modified));
        }
    }
}

/// About `objects` made-up objects spread over all [`BUCKETS`], with keys prefixed by
/// `bucket/`, like an all-buckets scan.
pub fn all_buckets(objects: u64, with_versions: bool, seed: u64) -> impl Iterator<Item = Entry> {
    BUCKETS.iter().flat_map(move |bucket| {
        bucket
            .entries(share(objects, bucket), with_versions, seed)
            .map(move |mut entry| {
                entry.key.insert_str(0, &format!("{}/", bucket.name));
                entry
            })
    })
}

/// How many of `objects` go to `bucket` in [`all_buckets`].
pub fn share(objects: u64, bucket: &DemoBucket) -> u64 {
    let total_weight: u64 = BUCKETS.iter().map(|bucket| u64::from(bucket.weight)).sum();
    (objects * u64::from(bucket.weight) / total_weight).max(1)
}

pub fn bucket(name: &str) -> Option<&'static DemoBucket> {
    BUCKETS.iter().find(|bucket| bucket.name == name)
}

/// SplitMix64: small, fast, and good enough for made-up data.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn below(&mut self, limit: u64) -> u64 {
        if limit == 0 { 0 } else { self.next() % limit }
    }

    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }

    fn pick<T: Copy>(&mut self, items: &[T]) -> T {
        items[self.below(items.len() as u64) as usize]
    }

    fn weighted<'a, T>(&mut self, items: &'a [T], weight: impl Fn(&T) -> u32) -> &'a T {
        let total: u64 = items.iter().map(|item| u64::from(weight(item))).sum();
        let mut roll = self.below(total);
        for item in items {
            let weight = u64::from(weight(item));
            if roll < weight {
                return item;
            }
            roll -= weight;
        }
        &items[items.len() - 1]
    }

    /// Sizes spread evenly on a log scale, so small files are common and huge ones rare.
    fn log_uniform(&mut self, smallest: u64, largest: u64) -> u64 {
        let (low, high) = ((smallest.max(1)) as f64, largest.max(smallest) as f64);
        (low.ln() + self.unit() * (high.ln() - low.ln())).exp() as u64
    }
}

fn hash_name(name: &str) -> u64 {
    name.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Tree;

    #[test]
    fn same_seed_gives_same_data() {
        let first: Vec<_> = all_buckets(500, true, 7).collect();
        let second: Vec<_> = all_buckets(500, true, 7).collect();
        assert_eq!(first, second);
        assert_ne!(first, all_buckets(500, true, 8).collect::<Vec<_>>());
    }

    #[test]
    fn spreads_objects_over_every_bucket() {
        let mut tree = Tree::new();
        for entry in all_buckets(5_000, false, 1) {
            tree.insert(&entry);
        }
        assert_eq!(tree.children(Tree::ROOT).count(), BUCKETS.len());
        assert!(tree.total().objects >= 4_900);
        assert!(tree.storage_classes().len() >= 5);
        assert!(tree.file_types().len() >= 10);
    }

    #[test]
    fn includes_some_abandoned_uploads() {
        let entries: Vec<_> = bucket("acme-backups")
            .unwrap()
            .entries(2_000, false, 5)
            .collect();
        assert!(
            entries
                .iter()
                .any(|entry| entry.kind == EntryKind::IncompleteUpload)
        );
    }

    #[test]
    fn versions_only_when_asked() {
        let without: Vec<_> = bucket("acme-backups")
            .unwrap()
            .entries(300, false, 3)
            .collect();
        assert!(
            without.iter().all(|entry| matches!(
                entry.kind,
                EntryKind::Current | EntryKind::IncompleteUpload
            ))
        );

        let with: Vec<_> = bucket("acme-backups")
            .unwrap()
            .entries(300, true, 3)
            .collect();
        assert!(with.iter().any(|entry| entry.kind == EntryKind::Noncurrent));
        assert!(
            with.iter()
                .any(|entry| entry.kind == EntryKind::DeleteMarker)
        );
    }
}
