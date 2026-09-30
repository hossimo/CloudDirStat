//! The S3 · Google · Azure buttons next to Location, so nobody has to remember which
//! scheme (`s3://`, `gs://`, `az://`) goes with which cloud. The location text is the
//! only state: the selected button always follows it, so typing or pasting a location
//! selects its cloud, and clicking a cloud rewrites the scheme and keeps the rest.

use clouddirstat_providers::Provider;
use eframe::egui::{self, Color32, CornerRadius, Sense, StrokeKind, Vec2};

const ICON_SIZE: f32 = 14.0;
const ICON_GAP: f32 = 4.0;
const CORNER: u8 = 4;

/// What a location's text starts with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scheme {
    Known(Provider),
    /// No `://` at all; locations without a scheme are read as S3.
    Missing,
    /// A `://` scheme that isn't `s3`, `gs`, or `az`.
    Unknown,
}

pub fn scheme_of(location: &str) -> Scheme {
    match location.trim().split_once("://") {
        None => Scheme::Missing,
        Some((scheme, _)) => Provider::from_scheme(scheme).map_or(Scheme::Unknown, Scheme::Known),
    }
}

/// The cloud a location is for, as it would be scanned; `None` for an unknown scheme.
pub fn provider_of(location: &str) -> Option<Provider> {
    match scheme_of(location) {
        Scheme::Known(provider) => Some(provider),
        Scheme::Missing => Some(Provider::S3),
        Scheme::Unknown => None,
    }
}

/// `location` with `provider`'s scheme in place of its own (or in front, if it has none).
pub fn with_scheme(location: &str, provider: Provider) -> String {
    let location = location.trim();
    let rest = location
        .split_once("://")
        .map_or(location, |(_, rest)| rest);
    format!("{}{rest}", provider.scheme())
}

/// Shows the buttons. Returns whether `location` was changed.
pub fn show(ui: &mut egui::Ui, location: &mut String) -> bool {
    let current = provider_of(location);
    let mut changed = false;
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        let last = Provider::ALL.len() - 1;
        for (index, provider) in Provider::ALL.into_iter().enumerate() {
            let selected = current == Some(provider);
            let response = segment(ui, provider, selected, index == 0, index == last);
            // Clicking the cloud that is only implied (no scheme typed yet) writes it in.
            if response.clicked() && scheme_of(location) != Scheme::Known(provider) {
                *location = with_scheme(location, provider);
                changed = true;
            }
        }
    });
    changed
}

fn label(provider: Provider) -> &'static str {
    match provider {
        Provider::S3 => "S3",
        Provider::Gcs => "Google",
        Provider::Azure => "Azure",
    }
}

fn example(provider: Provider) -> &'static str {
    match provider {
        Provider::S3 => "s3://bucket/folder/",
        Provider::Gcs => "gs://bucket/folder/",
        Provider::Azure => "az://account/container/folder/",
    }
}

/// The colors of a cloud icon's left, top, and right bumps and its base.
struct CloudColors {
    left: Color32,
    top: Color32,
    right: Color32,
    base: Color32,
}

impl CloudColors {
    const fn plain(color: Color32) -> Self {
        Self {
            left: color,
            top: color,
            right: color,
            base: color,
        }
    }
}

/// A generic cloud in each provider's familiar colors (not their logos, which are
/// trademarks): AWS orange, Google's four colors, Azure blue.
fn icon_colors(provider: Provider) -> CloudColors {
    match provider {
        Provider::S3 => CloudColors::plain(Color32::from_rgb(0xff, 0x99, 0x00)),
        Provider::Gcs => CloudColors {
            left: Color32::from_rgb(0x42, 0x85, 0xf4),
            top: Color32::from_rgb(0xea, 0x43, 0x35),
            right: Color32::from_rgb(0xfb, 0xbc, 0x05),
            base: Color32::from_rgb(0x34, 0xa8, 0x53),
        },
        Provider::Azure => CloudColors::plain(Color32::from_rgb(0x00, 0x89, 0xd6)),
    }
}

/// One button of the group; only the outer ends have rounded corners.
fn segment(
    ui: &mut egui::Ui,
    provider: Provider,
    selected: bool,
    first: bool,
    last: bool,
) -> egui::Response {
    let text = label(provider);
    let font = egui::TextStyle::Button.resolve(ui.style());
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_owned(), font, Color32::PLACEHOLDER);
    let padding = ui.spacing().button_padding;
    let size = Vec2::new(
        padding.x * 2.0 + ICON_SIZE + ICON_GAP + galley.size().x,
        ui.spacing()
            .interact_size
            .y
            .max(galley.size().y + padding.y * 2.0),
    );
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, selected, text)
    });

    if ui.is_rect_visible(rect) {
        let visuals = ui.style().interact_selectable(&response, selected);
        let fill = if selected {
            ui.visuals().selection.bg_fill
        } else {
            visuals.weak_bg_fill
        };
        let round = |rounded: bool| if rounded { CORNER } else { 0 };
        let corners = CornerRadius {
            nw: round(first),
            sw: round(first),
            ne: round(last),
            se: round(last),
        };
        let painter = ui.painter();
        painter.rect(rect, corners, fill, visuals.bg_stroke, StrokeKind::Inside);

        let icon_center = egui::pos2(rect.left() + padding.x + ICON_SIZE / 2.0, rect.center().y);
        cloud(painter, icon_center, &icon_colors(provider));
        let text_pos = egui::pos2(
            rect.left() + padding.x + ICON_SIZE + ICON_GAP,
            rect.center().y - galley.size().y / 2.0,
        );
        painter.galley(text_pos, galley, visuals.text_color());
    }

    response.on_hover_text(format!(
        "{}: locations look like {}",
        provider.label(),
        example(provider)
    ))
}

/// A small cloud, about 14 points wide, centered on `center`: three bumps on a flat
/// base. The base is drawn last, so with several colors each bump keeps its top and
/// the base shows as a band along the bottom.
fn cloud(painter: &egui::Painter, center: egui::Pos2, colors: &CloudColors) {
    let at = |x: f32, y: f32| center + Vec2::new(x, y);
    painter.circle_filled(at(-3.0, 1.5), 3.0, colors.left);
    painter.circle_filled(at(0.5, -1.0), 4.0, colors.top);
    painter.circle_filled(at(4.0, 2.0), 2.5, colors.right);
    painter.rect_filled(
        egui::Rect::from_min_max(at(-3.0, 1.5), at(4.0, 4.5)),
        0.0,
        colors.base,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_scheme() {
        assert_eq!(scheme_of("gs://bucket/x"), Scheme::Known(Provider::Gcs));
        assert_eq!(scheme_of("  az://acct "), Scheme::Known(Provider::Azure));
        assert_eq!(scheme_of("bucket/logs/"), Scheme::Missing);
        assert_eq!(scheme_of(""), Scheme::Missing);
        assert_eq!(scheme_of("ftp://host/x"), Scheme::Unknown);
        assert_eq!(provider_of("bucket/logs/"), Some(Provider::S3));
        assert_eq!(provider_of("s4://bucket"), None);
    }

    #[test]
    fn switching_keeps_the_path() {
        assert_eq!(
            with_scheme("s3://my-bucket/logs/", Provider::Gcs),
            "gs://my-bucket/logs/"
        );
        assert_eq!(
            with_scheme("my-bucket/logs/", Provider::Azure),
            "az://my-bucket/logs/"
        );
        assert_eq!(with_scheme("ftp://host/x", Provider::S3), "s3://host/x");
        assert_eq!(with_scheme("", Provider::Gcs), "gs://");
        assert_eq!(with_scheme("gs://", Provider::S3), "s3://");
    }
}
