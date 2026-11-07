#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod cloud_picker;
mod credentials_form;
mod cushion;
#[cfg(feature = "demo")]
mod demo_scan;
mod error_view;
mod estimate_view;
mod filter;
mod help;
mod largest_files;
mod legend;
mod palette;
mod scan;
mod tree_view;
mod treemap_view;

use std::cell::Cell;

use anyhow::{Result, anyhow};
use clap::Parser;
use clouddirstat_providers::azure::AzureCredentials;
use clouddirstat_providers::gcs::GcsCredentials;
use clouddirstat_providers::s3::CredentialSource;
use clouddirstat_providers::{Credentials, Location, load_dotenv};
use eframe::egui;

use crate::app::App;
use crate::scan::ScanRequest;

#[derive(Parser)]
#[command(
    version = clouddirstat_core::LONG_VERSION,
    about = "Treemap view of cloud object storage usage"
)]
struct Cli {
    /// s3://bucket/prefix/, gs://bucket/prefix/, or az://account/container/prefix/ to scan
    /// on startup
    location: Option<Location>,

    /// AWS profile from ~/.aws/config (including IAM Identity Center and `aws login`
    /// profiles). Access keys can be entered in the app instead.
    #[arg(long)]
    profile: Option<String>,

    /// Include old versions: noncurrent versions and delete markers in S3 (requires
    /// s3:ListBucketVersions), noncurrent and soft-deleted objects in Cloud Storage,
    /// and previous versions, snapshots, and soft-deleted blobs in Azure
    #[arg(long)]
    versions: bool,
}

fn main() -> Result<()> {
    // Sign-in variables such as AZURE_STORAGE_KEY can live in a .env file. A release
    // build has no console, so variables the file may not set are ignored silently.
    // SAFETY: no other thread has started yet; the tokio runtime is built below.
    let _ = unsafe { load_dotenv() };
    let cli = Cli::parse();
    let runtime = tokio::runtime::Runtime::new()?;
    let initial_scan = cli.location.map(|location| ScanRequest {
        location,
        credentials: Credentials {
            aws: CredentialSource::Chain {
                profile: cli.profile.clone(),
            },
            gcs: GcsCredentials::from_env(),
            azure: AzureCredentials::from_env(),
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
    let started = Cell::new(false);
    let result = eframe::run_native(
        "CloudDirStat",
        options,
        Box::new(|creation| {
            started.set(true);
            Ok(Box::new(App::new(
                runtime,
                &creation.egui_ctx,
                cli.profile,
                cli.versions,
                initial_scan,
            )))
        }),
    );
    match result {
        Ok(()) => Ok(()),
        Err(error) if started.get() => Err(anyhow!("the GUI stopped: {error}")),
        Err(error) => {
            let message = format!(
                "CloudDirStat could not open its window.\n\n{error}\n\nIt needs OpenGL 2.0 \
                 or newer. Virtual machines and remote desktop sessions without a graphics \
                 driver often lack it; updating the graphics driver usually helps.\n\n\
                 Report a problem: https://github.com/hossimo/CloudDirStat/issues"
            );
            #[cfg(windows)]
            show_error_box(&message);
            Err(anyhow!(message))
        }
    }
}

/// A release build on Windows has no console, so an error would otherwise vanish.
#[cfg(windows)]
fn show_error_box(message: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        MB_ICONERROR, MB_OK, MB_SETFOREGROUND, MessageBoxW,
    };

    let wide = |text: &str| text.encode_utf16().chain([0]).collect::<Vec<u16>>();
    let (text, caption) = (wide(message), wide("CloudDirStat"));
    // SAFETY: both strings are NUL-terminated and outlive the call; no owner window.
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            caption.as_ptr(),
            MB_OK | MB_ICONERROR | MB_SETFOREGROUND,
        )
    };
}

/// Sets the window and taskbar icon. A broken icon file only costs the icon.
pub fn with_app_icon(builder: egui::ViewportBuilder) -> egui::ViewportBuilder {
    const ICON: &[u8] = include_bytes!("../../../icons/png/icon-256.png");
    match eframe::icon_data::from_png_bytes(ICON) {
        Ok(icon) => builder.with_icon(icon),
        Err(_) => builder,
    }
}
