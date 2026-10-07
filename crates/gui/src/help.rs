use eframe::egui;

const TITLE: &str = "About CloudDirStat";
const REPOSITORY_URL: &str = "https://github.com/hossimo/CloudDirStat";
const ISSUES_URL: &str = "https://github.com/hossimo/CloudDirStat/issues";
const README_URL: &str = "https://github.com/hossimo/CloudDirStat#readme";
/// Drawn at half size so it stays sharp on high-DPI screens.
const LOGO_PNG: &[u8] = include_bytes!("../../../icons/png/icon-128.png");
const LOGO_SIZE: f32 = 64.0;

/// The Help window: the app's name, version, and links to the project. Permissions and
/// sign-in are documented in the README. Opens as its own OS window.
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

    pub fn show(&mut self, ctx: &egui::Context, version_in_title: &mut bool) {
        if !self.open {
            return;
        }
        if self.logo.is_none() {
            self.logo = load_logo(ctx);
        }
        let logo = self.logo.as_ref();
        let builder = crate::with_app_icon(egui::ViewportBuilder::default())
            .with_title(TITLE)
            .with_inner_size([360.0, 200.0]);

        let closed = ctx.show_viewport_immediate(
            egui::ViewportId::from_hash_of("help"),
            builder,
            |ui, class| {
                if class == egui::ViewportClass::EmbeddedWindow {
                    // No OS windows on this platform: egui shows it inside the main window.
                    contents(ui, logo, version_in_title);
                    return false;
                }
                egui::CentralPanel::default().show(ui, |ui| contents(ui, logo, version_in_title));
                ui.input(|input| input.viewport().close_requested())
            },
        );
        if closed {
            self.open = false;
        }
    }
}

fn contents(ui: &mut egui::Ui, logo: Option<&egui::TextureHandle>, version_in_title: &mut bool) {
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
            ui.label(format!("Version {}", clouddirstat_core::LONG_VERSION));
            ui.add_space(6.0);
            ui.hyperlink_to("Help (README)", README_URL);
            ui.hyperlink_to("Report an issue", ISSUES_URL);
            ui.hyperlink_to("Source code on GitHub", REPOSITORY_URL);
        });
    });
    ui.add_space(8.0);
    ui.checkbox(version_in_title, "Show the version in the title bar");
}

/// A broken logo file only costs the logo.
fn load_logo(ctx: &egui::Context) -> Option<egui::TextureHandle> {
    let icon = eframe::icon_data::from_png_bytes(LOGO_PNG).ok()?;
    let size = [icon.width as usize, icon.height as usize];
    let image = egui::ColorImage::from_rgba_unmultiplied(size, &icon.rgba);
    Some(ctx.load_texture("logo", image, egui::TextureOptions::LINEAR))
}
