#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod cushion;
mod legend;
mod palette;
mod scan;
mod tree_view;
mod treemap_view;

use anyhow::{Result, anyhow};
use clap::Parser;
use clouddirstat_providers::s3::S3Location;
use eframe::egui;

use crate::app::App;
use crate::scan::ScanRequest;

#[derive(Parser)]
#[command(version, about = "Treemap view of cloud object storage usage")]
struct Cli {
    /// s3://bucket or s3://bucket/prefix/ to scan on startup
    location: Option<S3Location>,

    /// AWS profile from ~/.aws/config
    #[arg(long)]
    profile: Option<String>,

    /// Include noncurrent versions and delete markers (requires s3:ListBucketVersions)
    #[arg(long)]
    versions: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let runtime = tokio::runtime::Runtime::new()?;
    let initial_scan = cli.location.map(|location| ScanRequest {
        location,
        profile: cli.profile.clone(),
        include_versions: cli.versions,
    });

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("CloudDirStat")
            .with_inner_size([1280.0, 800.0]),
        ..Default::default()
    };
    eframe::run_native(
        "CloudDirStat",
        options,
        Box::new(move |creation| {
            Ok(Box::new(App::new(
                runtime,
                &creation.egui_ctx,
                cli.profile,
                cli.versions,
                initial_scan,
            )))
        }),
    )
    .map_err(|error| anyhow!("could not start the GUI: {error}"))
}
