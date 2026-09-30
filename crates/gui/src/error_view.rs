//! Errors shown in the window: readable colors (pure red text on a dark background is
//! hard to read), and long messages kept from running off the edge.

use eframe::egui::{self, Color32, Label, Margin, RichText};

const TROUBLESHOOTING_URL: &str = "https://github.com/hossimo/CloudDirStat#troubleshooting";
const CARD_WIDTH: f32 = 640.0;

/// What the user did on the error card.
pub enum Action {
    Help,
}

/// Background and text colors for errors, for the current light or dark theme.
fn colors(visuals: &egui::Visuals) -> (Color32, Color32) {
    if visuals.dark_mode {
        (
            Color32::from_rgb(0x5c, 0x1f, 0x1f),
            Color32::from_rgb(0xff, 0xdc, 0xdc),
        )
    } else {
        (
            Color32::from_rgb(0xfd, 0xe4, 0xe4),
            Color32::from_rgb(0x8a, 0x16, 0x16),
        )
    }
}

fn frame(visuals: &egui::Visuals, margin: Margin) -> egui::Frame {
    let (fill, _) = colors(visuals);
    egui::Frame::new()
        .fill(fill)
        .corner_radius(4.0)
        .inner_margin(margin)
}

/// One line for the status bar, cut off with "…" to fit; hovering shows all of it.
pub fn chip(ui: &mut egui::Ui, message: &str) {
    let (_, text) = colors(ui.visuals());
    frame(ui.visuals(), Margin::symmetric(6, 1)).show(ui, |ui| {
        ui.add(Label::new(RichText::new(sentence(message)).color(text)).truncate());
    });
}

/// The whole message, wrapped, for places with room for it (e.g. a window).
pub fn block(ui: &mut egui::Ui, message: &str) {
    let (_, text) = colors(ui.visuals());
    frame(ui.visuals(), Margin::same(8)).show(ui, |ui| {
        ui.add(Label::new(RichText::new(sentence(message)).color(text)).wrap());
    });
}

/// A card in the middle of the window for a scan that failed before finding anything:
/// the whole message, a way to copy it, and where to get help.
pub fn card(ui: &mut egui::Ui, title: &str, message: &str) -> Option<Action> {
    let mut action = None;
    ui.vertical_centered(|ui| {
        ui.add_space((ui.available_height() * 0.25).min(160.0));
        ui.set_max_width(CARD_WIDTH.min(ui.available_width()));
        let (_, text) = colors(ui.visuals());
        frame(ui.visuals(), Margin::same(16)).show(ui, |ui| {
            ui.vertical(|ui| {
                ui.label(RichText::new(title).color(text).strong().size(18.0));
                ui.add_space(6.0);
                ui.add(Label::new(RichText::new(sentence(message)).color(text)).wrap());
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui.button("Copy error").clicked() {
                        ui.ctx().copy_text(message.to_owned());
                    }
                    if ui
                        .button("Help")
                        .on_hover_text("Permissions and how to sign in")
                        .clicked()
                    {
                        action = Some(Action::Help);
                    }
                    ui.hyperlink_to("Troubleshooting", TROUBLESHOOTING_URL);
                });
            });
        });
    });
    action
}

/// Error messages start lowercase so they read well after "Error: " in the CLI; on
/// their own they start with a capital.
fn sentence(message: &str) -> String {
    let mut chars = message.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capitalizes_the_first_letter() {
        assert_eq!(sentence("could not sign in"), "Could not sign in");
        assert_eq!(sentence(""), "");
    }
}
