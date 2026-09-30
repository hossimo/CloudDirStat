mod estimate;
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
    /// Show bucket totals from CloudWatch and what a full scan would cost, without
    /// listing anything (requires cloudwatch:GetMetricData)
    Estimate(TargetArgs),
}

/// Where to look, and as whom.
#[derive(Args)]
struct TargetArgs {
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
}

impl TargetArgs {
    async fn connect(&self) -> Result<S3Scanner> {
        let credentials = CredentialSource::Chain {
            profile: self.profile.clone(),
        };
        Ok(S3Scanner::connect(&self.location, &credentials, self.region.as_deref()).await?)
    }
}

#[derive(Args)]
struct ScanArgs {
    #[command(flatten)]
    target: TargetArgs,

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
        Command::Estimate(args) => {
            let estimate = args.connect().await?.estimate().await?;
            for skipped in &estimate.skipped {
                eprintln!(
                    "warning: skipped bucket {}: {}",
                    skipped.bucket, skipped.reason
                );
            }
            estimate::print(&args.location, &estimate);
            Ok(())
        }
    }
}

async fn scan(args: ScanArgs) -> Result<()> {
    let started = Instant::now();
    let scanner = Arc::new(args.target.connect().await?);

    let pricing = Arc::new(scanner.pricing());
    let options = ScanOptions {
        include_versions: args.versions,
        concurrency: args.concurrency,
    };
    let (sender, mut receiver) = mpsc::channel(256);
    // CloudWatch's object count, for progress. It covers whole buckets only.
    let mut expected = args.target.location.prefix.is_empty().then(|| {
        let scanner = Arc::clone(&scanner);
        tokio::spawn(async move { scanner.object_counts().await })
    });
    let mut expected_objects = None;
    let scan = {
        let scanner = Arc::clone(&scanner);
        tokio::spawn(async move { scanner.scan(&options, sender).await })
    };

    let mut tree = Tree::with_pricing(pricing.clone());
    let mut next_progress = PROGRESS_INTERVAL;
    while let Some(entries) = receiver.recv().await {
        for entry in &entries {
            tree.insert(entry);
        }
        if expected.as_ref().is_some_and(|task| task.is_finished())
            && let Some(task) = expected.take()
        {
            expected_objects = task
                .await
                .ok()
                .and_then(Result::ok)
                .map(|estimate| estimate.objects())
                .filter(|&objects| objects > 0);
        }
        let scanned = tree.total().objects;
        if scanned >= next_progress {
            match expected_objects {
                Some(total) => eprint!(
                    "\rScanned {} of ~{} objects ({}%)...",
                    format_count(scanned),
                    format_count(total),
                    progress_percent(scanned, total)
                ),
                None => eprint!("\rScanned {} objects...", format_count(scanned)),
            }
            std::io::stderr().flush()?;
            next_progress = scanned + PROGRESS_INTERVAL;
        }
    }
    if let Some(task) = expected {
        task.abort();
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
    for warning in &stats.warnings {
        eprintln!("warning: {warning}");
    }

    Report {
        tree: &tree,
        location: &args.target.location,
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

/// Share of the expected objects scanned so far. CloudWatch's count is a day old and
/// includes versions, so it stops at 99% rather than claiming to be done.
fn progress_percent(scanned: u64, expected: u64) -> u64 {
    (scanned * 100 / expected.max(1)).min(99)
}
