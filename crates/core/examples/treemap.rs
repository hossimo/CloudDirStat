//! How long a treemap layout takes, for a demo scan plus one flat folder of tiny objects.
//!
//!     cargo run --release -p clouddirstat-core --features demo --example treemap -- 5000000 1000000

use std::time::{Duration, Instant};

use clouddirstat_core::{Entry, EntryKind, Rect, Tree, demo, format_count, squarify};

/// A treemap panel on a 1080p screen.
const BOUNDS: Rect = Rect {
    x: 0.0,
    y: 0.0,
    w: 1900.0,
    h: 600.0,
};
const MIN_AREA: f32 = 1.5;
const RUNS: usize = 5;

fn main() {
    let mut args = std::env::args().skip(1).map(|arg| arg.parse().ok());
    let objects: u64 = args.next().flatten().unwrap_or(1_000_000);
    let flat: u64 = args.next().flatten().unwrap_or(1_000_000);

    let mut tree = Tree::new();
    for entry in demo::all_buckets(objects, false, 42) {
        tree.insert(&entry);
    }
    for index in 0..flat {
        tree.insert(&Entry {
            key: format!("acme-flat/thumbnails/{index:08}.jpg"),
            size: 20_000,
            storage_class: "STANDARD".into(),
            kind: EntryKind::Current,
            last_modified: None,
        });
    }

    let mut fastest = Duration::MAX;
    let mut rectangles = 0;
    for _ in 0..RUNS {
        let started = Instant::now();
        rectangles = squarify(&tree, Tree::ROOT, BOUNDS, MIN_AREA).len();
        fastest = fastest.min(started.elapsed());
    }

    println!("objects            {}", format_count(tree.total().objects));
    println!("rectangles         {}", format_count(rectangles as u64));
    println!("layout time        {:.1} ms", fastest.as_secs_f64() * 1e3);
}
