//! How much memory the tree needs per object.
//!
//!     cargo run --release -p clouddirstat-core --features demo --example memory -- 5000000

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use clouddirstat_core::{Tree, demo, format_bytes, format_count};

/// Wraps the system allocator to count the bytes currently allocated.
struct Counting;

static ALLOCATED: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let now = ALLOCATED.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
        PEAK.fetch_max(now, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        ALLOCATED.fetch_sub(layout.size(), Ordering::Relaxed);
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

fn main() {
    let objects: u64 = std::env::args()
        .nth(1)
        .and_then(|arg| arg.parse().ok())
        .unwrap_or(1_000_000);

    let before = ALLOCATED.load(Ordering::Relaxed);
    let started = Instant::now();
    let mut tree = Tree::new();
    for entry in demo::all_buckets(objects, false, 42) {
        tree.insert(&entry);
    }
    let elapsed = started.elapsed();
    let used = ALLOCATED.load(Ordering::Relaxed) - before;
    let inserted = tree.total().objects;

    println!("objects inserted   {}", format_count(inserted));
    println!("tree memory        {}", format_bytes(used as u64));
    println!(
        "peak memory        {}",
        format_bytes(PEAK.load(Ordering::Relaxed) as u64)
    );
    println!("bytes per object   {:.1}", used as f64 / inserted as f64);
    println!(
        "insert speed       {:.2} M objects/s",
        inserted as f64 / elapsed.as_secs_f64() / 1e6
    );
}
