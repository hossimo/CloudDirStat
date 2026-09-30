mod estimate;
mod report;

use std::io::Write;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use clap::{Args, Parser, Subcommand};
use clouddirstat_core::{Tree, format_count, format_counted};
use clouddirstat_providers::azure::AzureCredentials;
use clouddirstat_providers::gcs::GcsCredentials;
use clouddirstat_providers::s3::CredentialSource;
use clouddirstat_providers::{Credentials, Location, ScanOptions, Scanner};
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
    /// Scan a bucket, a prefix, or all buckets and print a usage report
    Scan(ScanArgs),
    /// Show S3 bucket totals from CloudWatch and what a full scan would cost, without
    /// listing anything (requires cloudwatch:GetMetricData)
    Estimate(TargetArgs),
}

/// Where to look, and as whom.
#[derive(Args)]
struct TargetArgs {
    /// s3://bucket/prefix/ (Amazon S3), gs://bucket/prefix/ (Google Cloud Storage), or
    /// az://account/container/prefix/ (Azure Blob Storage); s3://, gs://, or az:// alone
    /// for every bucket (az://account for every container of an account)
    location: Location,

    /// AWS profile from ~/.aws/config (including IAM Identity Center and `aws login`
    /// profiles). Access keys can be given with the AWS_ACCESS_KEY_ID,
    /// AWS_SECRET_ACCESS_KEY, and AWS_SESSION_TOKEN environment variables.
    #[arg(long)]
    profile: Option<String>,

    /// Bucket region; looked up automatically when omitted
    #[arg(long)]
    region: Option<String>,

    /// Google Cloud project whose buckets gs:// lists; defaults to gcloud's project.
    /// Google sign-in uses Application Default Credentials (`gcloud auth
    /// application-default login`), or an access token in GOOGLE_OAUTH_ACCESS_TOKEN.
    /// Azure sign-in uses the Azure CLI (`az login`), a SAS token in
    /// AZURE_STORAGE_SAS_TOKEN, or an account key in AZURE_STORAGE_KEY (or a connection
    /// string in AZURE_STORAGE_CONNECTION_STRING). Variables can also be set in a .env
    /// file in the current directory.
    #[arg(long)]
    project: Option<String>,
}

impl TargetArgs {
    async fn connect(&self) -> Result<Scanner> {
        let credentials = Credentials {
            aws: CredentialSource::Chain {
                profile: self.profile.clone(),
            },
            gcs: GcsCredentials {
                project: self.project.clone(),
                ..GcsCredentials::from_env()
            },
            azure: AzureCredentials::from_env(),
        };
        Ok(Scanner::connect(&self.location, &credentials, self.region.as_deref()).await?)
    }
}

#[derive(Args)]
struct ScanArgs {
    #[command(flatten)]
    target: TargetArgs,

    /// Include old versions: noncurrent versions and delete markers in S3 (requires
    /// s3:ListBucketVersions), noncurrent and soft-deleted objects in Cloud Storage,
    /// and previous versions, snapshots, and soft-deleted blobs in Azure
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
    // Secrets such as AZURE_STORAGE_KEY can live in a .env file; a missing file is fine.
    let _ = dotenvy::dotenv();
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

    let pricing = scanner.pricing();
    let options = ScanOptions {
        include_versions: args.versions,
        concurrency: args.concurrency,
    };
    let (sender, mut receiver) = mpsc::channel(256);
    // CloudWatch's object count, for progress. It covers whole buckets only.
    let mut expected = args.target.location.prefix().is_empty().then(|| {
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
                None => eprint!("\rScanned {}...", format_counted(scanned, "object")),
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
        pricing: pricing.as_ref(),
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
