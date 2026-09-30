use eframe::egui::{self, RichText};

/// The same policy as docs/iam-policy.json, so the app and the docs never disagree.
const POLICY_TEMPLATE: &str = include_str!("../../../docs/iam-policy.json");
const BUCKET_PLACEHOLDER: &str = "YOUR-BUCKET";

const TITLE: &str = "CloudDirStat: permissions and credentials";
/// Drawn at half size so it stays sharp on high-DPI screens.
const LOGO_PNG: &[u8] = include_bytes!("../../../icons/png/icon-128.png");
const LOGO_SIZE: f32 = 64.0;

/// The Help window: which permissions a scan needs and how to sign in. Opens as its own
/// OS window, so it can be moved and sized independently of the main window.
#[derive(Default)]
pub struct HelpWindow {
    open: bool,
    logo: Option<egui::TextureHandle>,
}

impl HelpWindow {
    pub fn toggle(&mut self) {
        self.open = !self.open;
    }

    pub fn open(&mut self) {
        self.open = true;
    }

    /// `bucket` fills in the policy when the location field holds one (`*` for all).
    pub fn show(&mut self, ctx: &egui::Context, bucket: Option<&str>) {
        if !self.open {
            return;
        }
        let policy = policy_for(bucket);
        if self.logo.is_none() {
            self.logo = load_logo(ctx);
        }
        let logo = self.logo.as_ref();
        let builder = crate::with_app_icon(egui::ViewportBuilder::default())
            .with_title(TITLE)
            .with_inner_size([600.0, 640.0])
            .with_min_inner_size([360.0, 240.0]);

        let closed = ctx.show_viewport_immediate(
            egui::ViewportId::from_hash_of("help"),
            builder,
            |ui, class| {
                if class == egui::ViewportClass::EmbeddedWindow {
                    // No OS windows on this platform: egui shows it inside the main window.
                    egui::ScrollArea::vertical().show(ui, |ui| contents(ui, &policy, logo));
                    return false;
                }
                egui::CentralPanel::default().show(ui, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| contents(ui, &policy, logo));
                });
                ui.input(|input| input.viewport().close_requested())
            },
        );
        if closed {
            self.open = false;
        }
    }
}

fn contents(ui: &mut egui::Ui, policy: &str, logo: Option<&egui::TextureHandle>) {
    about(ui, logo);
    ui.separator();
    ui.heading("Amazon S3: permissions");
    ui.label(
        "CloudDirStat only lists your bucket (and reads its storage metrics from \
         CloudWatch). It never reads object contents and never writes or deletes anything.",
    );
    ui.add_space(4.0);
    egui::Grid::new("permissions")
        .num_columns(3)
        .striped(true)
        .show(ui, |ui| {
            ui.strong("Permission");
            ui.strong("Needed");
            ui.strong("Used for");
            ui.end_row();

            ui.monospace("s3:ListBucket");
            ui.label("Always");
            ui.label("Listing objects and finding the bucket's region");
            ui.end_row();

            ui.monospace("s3:ListBucketVersions");
            ui.label("With Versions");
            ui.label("Noncurrent versions and delete markers");
            ui.end_row();

            ui.monospace("s3:ListBucketMultipartUploads");
            ui.label("Optional");
            ui.label("Finding incomplete multipart uploads (billed but hidden)");
            ui.end_row();

            ui.monospace("s3:ListMultipartUploadParts");
            ui.label("Optional");
            ui.label("Sizing incomplete uploads (resource: bucket/*)");
            ui.end_row();

            ui.monospace("s3:ListAllMyBuckets");
            ui.label("For s3://");
            ui.label("Finding every bucket to scan them all at once");
            ui.end_row();

            ui.monospace("cloudwatch:GetMetricData");
            ui.label("Optional");
            ui.label("Estimate, and progress while scanning (daily S3 storage metrics)");
            ui.end_row();
        });

    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.strong("Minimal IAM policy");
        if ui.button("Copy").clicked() {
            ui.ctx().copy_text(policy.to_owned());
        }
    });
    let mut shown = policy.to_owned();
    ui.add(
        egui::TextEdit::multiline(&mut shown)
            .code_editor()
            .desired_width(f32::INFINITY)
            .interactive(false),
    );
    ui.weak(
        "Statements whose Sid ends in Optional can be dropped: without them, that \
         feature is skipped with a warning. To scan all buckets (s3://), set the bucket \
         resources to arn:aws:s3:::* (and arn:aws:s3:::*/*) or list each bucket.",
    );

    ui.add_space(12.0);
    ui.heading("Amazon S3: signing in");
    ui.label(RichText::new("Recommended: short-lived credentials in a profile").strong());
    ui.label("• Console sign-in (AWS CLI v2):  aws login --profile NAME");
    ui.label("• IAM Identity Center (SSO):  aws configure sso, then aws sso login --profile NAME");
    ui.label("Then choose Profile and enter NAME (or leave it empty for the default profile).");
    ui.add_space(6.0);
    ui.label(RichText::new("Access keys").strong());
    ui.label(
        "Choose Access key and paste the key ID and secret, plus the session token for \
         temporary credentials. Keys stay in memory for this session only; they are never \
         saved to disk or logged. Create keys for an IAM user or role that has only the \
         policy above.",
    );

    ui.add_space(12.0);
    google(ui);
    ui.add_space(12.0);
    azure(ui);
}

fn azure(ui: &mut egui::Ui) {
    ui.heading("Azure Blob Storage");
    ui.label(
        "Sign in once with az login (Azure CLI), then choose Azure CLI; CloudDirStat asks the \
         CLI for tokens as it goes. SAS token takes a shared access signature for one \
         account or container instead. az://ACCOUNT lists every container of an account; \
         az:// every account you can see.",
    );
    ui.add_space(4.0);
    egui::Grid::new("azure permissions")
        .num_columns(3)
        .striped(true)
        .show(ui, |ui| {
            ui.strong("Role");
            ui.strong("Needed");
            ui.strong("Used for");
            ui.end_row();

            ui.monospace("Storage Blob Data Reader");
            ui.label("Always");
            ui.label("Listing containers and blobs (on the account or container)");
            ui.end_row();

            ui.monospace("Reader");
            ui.label("Optional");
            ui.label("The account's region and redundancy, for prices; needed for az://");
            ui.end_row();
        });
    ui.weak(
        "Owner or Contributor of the subscription does not include reading blobs: add \
         Storage Blob Data Reader too. Without Reader, prices are eastus LRS rates. \
         Versions adds blob versions, snapshots, and soft-deleted blobs.",
    );
}

fn google(ui: &mut egui::Ui) {
    ui.heading("Google Cloud Storage");
    ui.label(
        "Sign in once with gcloud auth application-default login (or set \
         GOOGLE_APPLICATION_CREDENTIALS to a service account key file), then choose Google \
         login. Access token takes the output of gcloud auth print-access-token instead; \
         it lasts about an hour and is kept in memory only. gs:// on its own lists every \
         bucket of the project in Project (or gcloud's project).",
    );
    ui.add_space(4.0);
    egui::Grid::new("google permissions")
        .num_columns(3)
        .striped(true)
        .show(ui, |ui| {
            ui.strong("Permission");
            ui.strong("Needed");
            ui.strong("Used for");
            ui.end_row();

            ui.monospace("storage.objects.list");
            ui.label("Always");
            ui.label("Listing objects (in the Storage Object Viewer role)");
            ui.end_row();

            ui.monospace("storage.buckets.get");
            ui.label("Optional");
            ui.label("The bucket's location, for prices (otherwise us-central1 prices)");
            ui.end_row();

            ui.monospace("storage.buckets.list");
            ui.label("For gs://");
            ui.label("Finding every bucket of the project");
            ui.end_row();
        });
    ui.weak(
        "Storage Object Viewer covers listing. For the rest, give a custom role these \
         three permissions. Versions adds old versions and soft-deleted objects.",
    );
}

fn about(ui: &mut egui::Ui, logo: Option<&egui::TextureHandle>) {
    ui.horizontal(|ui| {
        if let Some(logo) = logo {
            let size = egui::vec2(LOGO_SIZE, LOGO_SIZE);
            ui.add(egui::Image::from_texture(egui::load::SizedTexture::new(
                logo.id(),
                size,
            )));
        }
        ui.vertical(|ui| {
            ui.heading("CloudDirStat");
            ui.label(format!("version {}", clouddirstat_core::VERSION));
            ui.horizontal(|ui| {
                ui.weak(format!("commit {}", clouddirstat_core::GIT_HASH));
                if ui
                    .small_button("Copy")
                    .on_hover_text("Copy the version and commit, e.g. for a bug report")
                    .clicked()
                {
                    ui.ctx()
                        .copy_text(format!("CloudDirStat {}", clouddirstat_core::LONG_VERSION));
                }
            });
        });
    });
}

/// A broken logo file only costs the logo.
fn load_logo(ctx: &egui::Context) -> Option<egui::TextureHandle> {
    let icon = eframe::icon_data::from_png_bytes(LOGO_PNG).ok()?;
    let size = [icon.width as usize, icon.height as usize];
    let image = egui::ColorImage::from_rgba_unmultiplied(size, &icon.rgba);
    Some(ctx.load_texture("logo", image, egui::TextureOptions::LINEAR))
}

fn policy_for(bucket: Option<&str>) -> String {
    let bucket = bucket.filter(|bucket| !bucket.is_empty());
    POLICY_TEMPLATE.replace(BUCKET_PLACEHOLDER, bucket.unwrap_or(BUCKET_PLACEHOLDER))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fills_in_the_bucket_name() {
        let policy = policy_for(Some("my-bucket"));
        assert!(policy.contains("arn:aws:s3:::my-bucket"));
        assert!(!policy.contains(BUCKET_PLACEHOLDER));
    }

    #[test]
    fn keeps_the_placeholder_without_a_bucket() {
        assert!(policy_for(None).contains("arn:aws:s3:::YOUR-BUCKET"));
        assert!(policy_for(Some("")).contains("arn:aws:s3:::YOUR-BUCKET"));
    }
}
