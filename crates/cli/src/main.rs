mod report;

use std::io::Write;
use std::time::Instant;

use anyhow::Result;
use clap::{Args, Parser, Subcommand};
use clouddirstat_core::{Tree, format_count};
use clouddirstat_providers::s3::{S3Location, S3Scanner, ScanOptions};
use tokio::sync::mpsc;

use crate::report::{Report, ReportOptions};

const PROGRESS_INTERVAL: u64 = 10_000;

#[derive(Parser)]
#[command(version, about = "Disk usage statistics for cloud object storage")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Scan an S3 bucket (or prefix) and print a usage report
    Scan(ScanArgs),
}

#[derive(Args)]
struct ScanArgs {
    /// s3://bucket or s3://bucket/prefix/
    location: S3Location,

    /// AWS profile from ~/.aws/config
    #[arg(long)]
    profile: Option<String>,

    /// Bucket region; looked up automatically when omitted
    #[arg(long)]
    region: Option<String>,

    /// Include noncurrent versions and delete markers (requires s3:ListBucketVersions)
    #[arg(long)]
    versions: bool,

    /// Directory levels to print
    #[arg(long, default_value_t = 2)]
    depth: usize,

    /// Entries to print per directory and in the largest-objects list
    #[arg(long, default_value_t = 10)]
    top: usize,

    /// Maximum concurrent LIST requests
    #[arg(long, default_value_t = 32)]
    concurrency: usize,
}

#[tokio::main]
async fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Scan(args) => scan(args).await,
    }
}

async fn scan(args: ScanArgs) -> Result<()> {
    let started = Instant::now();
    let scanner = S3Scanner::connect(
        &args.location.bucket,
        args.profile.as_deref(),
        args.region.as_deref(),
    )
    .await?;

    let options = ScanOptions {
        include_versions: args.versions,
        concurrency: args.concurrency,
    };
    let location = args.location.clone();
    let (sender, mut receiver) = mpsc::channel(256);
    let scan = tokio::spawn(async move { scanner.scan(&location, &options, sender).await });

    let mut tree = Tree::new();
    let mut next_progress = PROGRESS_INTERVAL;
    while let Some(entries) = receiver.recv().await {
        for entry in &entries {
            tree.insert(entry);
        }
        if tree.total().objects >= next_progress {
            eprint!(
                "\rScanned {} objects...",
                format_count(tree.total().objects)
            );
            std::io::stderr().flush()?;
            next_progress = tree.total().objects + PROGRESS_INTERVAL;
        }
    }
    let stats = scan.await??;
    if next_progress > PROGRESS_INTERVAL {
        eprint!("\r\x1b[2K");
    }

    Report {
        tree: &tree,
        location: &args.location,
        stats,
        elapsed: started.elapsed(),
        options: ReportOptions {
            depth: args.depth,
            top: args.top,
            show_versions: args.versions,
        },
    }
    .print();
    Ok(())
}
