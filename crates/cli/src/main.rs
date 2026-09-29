mod report;

use std::io::Write;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use clap::{Args, Parser, Subcommand};
use clouddirstat_core::{Tree, format_count};
use clouddirstat_providers::s3::{CredentialSource, S3Location, S3Scanner, ScanOptions};
use tokio::sync::mpsc;

use crate::report::{Report, ReportOptions};

const PROGRESS_INTERVAL: u64 = 10_000;

#[derive(Parser)]
#[command(
    version = clouddirstat_core::LONG_VERSION,
    about = "Disk usage statistics for cloud object storage"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Scan an S3 bucket, a prefix, or all buckets and print a usage report
    Scan(ScanArgs),
}

#[derive(Args)]
struct ScanArgs {
    /// s3://bucket, s3://bucket/prefix/, or s3:// for every bucket
    /// (all buckets needs s3:ListAllMyBuckets)
    location: S3Location,

    /// AWS profile from ~/.aws/config (including IAM Identity Center and `aws login`
    /// profiles). Access keys can be given with the AWS_ACCESS_KEY_ID,
    /// AWS_SECRET_ACCESS_KEY, and AWS_SESSION_TOKEN environment variables.
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
    let credentials = CredentialSource::Chain {
        profile: args.profile.clone(),
    };
    let scanner = S3Scanner::connect(&args.location, &credentials, args.region.as_deref()).await?;

    let pricing = Arc::new(scanner.pricing());
    let options = ScanOptions {
        include_versions: args.versions,
        concurrency: args.concurrency,
    };
    let (sender, mut receiver) = mpsc::channel(256);
    let scan = tokio::spawn(async move { scanner.scan(&options, sender).await });

    let mut tree = Tree::with_pricing(pricing.clone());
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
    for skipped in &stats.skipped_buckets {
        eprintln!(
            "warning: skipped bucket {}: {}",
            skipped.bucket, skipped.reason
        );
    }

    Report {
        tree: &tree,
        location: &args.location,
        pricing: &pricing,
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
