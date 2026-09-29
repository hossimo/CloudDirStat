#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod credentials_form;
mod cushion;
mod help;
mod legend;
mod palette;
mod scan;
mod tree_view;
mod treemap_view;

use anyhow::{Result, anyhow};
use clap::Parser;
use clouddirstat_providers::s3::{CredentialSource, S3Location};
use eframe::egui;

use crate::app::App;
use crate::scan::ScanRequest;

#[derive(Parser)]
#[command(
    version = clouddirstat_core::LONG_VERSION,
    about = "Treemap view of cloud object storage usage"
)]
struct Cli {
    /// s3://bucket or s3://bucket/prefix/ to scan on startup
    location: Option<S3Location>,

    /// AWS profile from ~/.aws/config (including IAM Identity Center and `aws login`
    /// profiles). Access keys can be entered in the app instead.
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
        credentials: CredentialSource::Chain {
            profile: cli.profile.clone(),
        },
        include_versions: cli.versions,
    });

    let options = eframe::NativeOptions {
        viewport: with_app_icon(
            egui::ViewportBuilder::default()
                .with_title("CloudDirStat")
                .with_inner_size([1280.0, 800.0]),
        ),
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

/// Sets the window and taskbar icon. A broken icon file only costs the icon.
pub fn with_app_icon(builder: egui::ViewportBuilder) -> egui::ViewportBuilder {
    const ICON: &[u8] = include_bytes!("../../../icons/png/icon-256.png");
    match eframe::icon_data::from_png_bytes(ICON) {
        Ok(icon) => builder.with_icon(icon),
        Err(_) => builder,
    }
}
