use std::time::Duration;

use clouddirstat_core::Pricing;
use clouddirstat_core::{
    EntryKind, NodeId, NodeKind, Tree, Usage, escape_control, format_bytes, format_count,
    format_counted, format_usd,
};
use clouddirstat_providers::{Location, ScanStats};

pub struct ReportOptions {
    pub depth: usize,
    pub top: usize,
    pub show_versions: bool,
}

pub struct Report<'a> {
    pub tree: &'a Tree,
    pub location: &'a Location,
    pub pricing: &'a dyn Pricing,
    pub stats: ScanStats,
    pub elapsed: Duration,
    pub options: ReportOptions,
}

impl Report<'_> {
    pub fn print(&self) {
        self.print_summary();
        self.print_storage_classes();
        if self.options.show_versions {
            self.print_versions();
        }
        self.print_tree();
        self.print_largest_objects();
    }

    fn print_summary(&self) {
        let total = self.tree.total();
        println!(
            "{}  {} in {}",
            self.location,
            format_bytes(total.bytes),
            format_counted(total.objects, "object")
        );
        println!(
            "Scanned in {:.1}s using {} LIST requests (~${:.4})",
            self.elapsed.as_secs_f64(),
            format_count(self.stats.list_requests),
            self.stats.estimated_cost_usd()
        );
        println!(
            "Estimated storage cost ~{}/month ({})",
            format_usd(total.monthly_cost),
            self.pricing.source()
        );

        let uploads = self.tree.usage_by_kind(EntryKind::IncompleteUpload);
        if uploads.objects > 0 {
            println!(
                "Incomplete multipart uploads: {} in {} (~{}/month), not shown by normal listings",
                format_bytes(uploads.bytes),
                format_counted(uploads.objects, "upload"),
                format_usd(uploads.monthly_cost)
            );
        }
    }

    fn print_storage_classes(&self) {
        section("By storage class");
        for (class, usage) in self.tree.storage_classes() {
            self.print_usage_row(&escape_control(class), usage);
        }
    }

    fn print_versions(&self) {
        section("By version state");
        for &kind in self.location.provider().entry_kinds() {
            self.print_usage_row(kind.label(), self.tree.usage_by_kind(kind));
        }
    }

    fn print_usage_row(&self, label: &str, usage: Usage) {
        println!(
            "  {:<22} {:>10} {:>6.1}% {:>12}/mo  {:>12} objects",
            label,
            format_bytes(usage.bytes),
            self.percent_of_total(usage.bytes),
            format_usd(usage.monthly_cost),
            format_count(usage.objects)
        );
    }

    fn print_tree(&self) {
        section("Largest directories");
        self.print_tree_row(Tree::ROOT, 0);
        self.print_children(Tree::ROOT, 1);
    }

    fn print_children(&self, parent: NodeId, level: usize) {
        if level > self.options.depth {
            return;
        }

        let children = self.tree.children_by_size(parent);
        for &child in children.iter().take(self.options.top) {
            self.print_tree_row(child, level);
            if self.tree.node(child).kind() == NodeKind::Directory {
                self.print_children(child, level + 1);
            }
        }

        let hidden = children.iter().skip(self.options.top);
        let hidden_count = hidden.len();
        if hidden_count > 0 {
            let mut hidden_usage = Usage::default();
            for &id in hidden {
                hidden_usage += self.tree.node(id).usage();
            }
            println!(
                "  {:>10} {:>6.1}% {:>12}/mo  {}... {} more",
                format_bytes(hidden_usage.bytes),
                self.percent_of_total(hidden_usage.bytes),
                format_usd(hidden_usage.monthly_cost),
                indent(level),
                format_count(hidden_count as u64)
            );
        }
    }

    fn print_tree_row(&self, id: NodeId, level: usize) {
        let usage = self.tree.node(id).usage();
        println!(
            "  {:>10} {:>6.1}% {:>12}/mo  {}{}",
            format_bytes(usage.bytes),
            self.percent_of_total(usage.bytes),
            format_usd(usage.monthly_cost),
            indent(level),
            self.display_name(id)
        );
    }

    /// The largest objects overall, or the largest in each bucket for `s3://`.
    fn print_largest_objects(&self) {
        if !self.location.is_all() {
            section("Largest objects");
            self.print_objects(self.tree.largest_objects(self.options.top));
            return;
        }
        section("Largest objects per bucket");
        for bucket in self.tree.children_by_size(Tree::ROOT) {
            if self.tree.node(bucket).kind() == NodeKind::Directory {
                println!("  {}", escape_control(self.tree.name(bucket)));
                self.print_objects(self.tree.largest_objects_in(bucket, self.options.top));
            }
        }
    }

    fn print_objects(&self, objects: Vec<NodeId>) {
        for id in objects {
            let usage = self.tree.node(id).usage();
            println!(
                "  {:>10} {:>12}/mo  {}",
                format_bytes(usage.bytes),
                format_usd(usage.monthly_cost),
                escape_control(&self.tree.path(id))
            );
        }
    }

    fn display_name(&self, id: NodeId) -> String {
        if id == Tree::ROOT {
            return "/".to_owned();
        }
        let name = match self.tree.name(id) {
            "" => "(empty)".into(),
            name => escape_control(name),
        };
        match self.tree.node(id).kind() {
            NodeKind::Directory => format!("{name}/"),
            NodeKind::Object => name.into_owned(),
        }
    }

    fn percent_of_total(&self, bytes: u64) -> f64 {
        match self.tree.total().bytes {
            0 => 0.0,
            total => bytes as f64 / total as f64 * 100.0,
        }
    }
}

fn section(title: &str) {
    println!("\n{title}");
}

fn indent(level: usize) -> String {
    "  ".repeat(level)
}
