//! Errors shown in the window: a short title for the status bar, and a window with what
//! to do about it and the full details. Colors stay readable (pure red text on a dark
//! background is hard to read).

use clouddirstat_providers::Error;
use eframe::egui::{self, Color32, Label, Margin, RichText};

const TROUBLESHOOTING_URL: &str = "https://github.com/hossimo/CloudDirStat#troubleshooting";
const WINDOW_WIDTH: f32 = 520.0;

/// An error put into words for the user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    /// A few words, e.g. "Access denied".
    pub title: String,
    /// What it means and what to try.
    pub hint: Option<String>,
    /// The message from the cloud service or the parser, for a bug report.
    pub details: Option<String>,
}

impl Problem {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            hint: None,
            details: None,
        }
    }

    fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    fn details(mut self, details: impl Into<String>) -> Self {
        self.details = Some(details.into());
        self
    }

    /// A message that is already meant for the user, such as a missing sign-in field.
    pub fn from_message(message: &str) -> Self {
        Self::new(sentence(message))
    }

    pub fn from_error(error: &Error) -> Self {
        match error {
            Error::Request {
                operation,
                permission,
                message,
            } if is_denied(message) => Self::new("Access denied")
                .hint(format!(
                    "You are signed in, but not allowed to do this. {operation} needs \
                     {permission}. Help lists the permissions CloudDirStat uses."
                ))
                .details(message.as_str()),
            Error::Request {
                operation,
                permission,
                message,
            } => Self::new(format!("{operation} failed"))
                .hint(format!(
                    "The cloud service refused the request. It needs {permission}."
                ))
                .details(message.as_str()),
            Error::Credentials(message) => Self::new("Could not sign in")
                .hint(
                    "Check the sign-in choice next to Location (profile, key, or token). \
                     Help explains each one.",
                )
                .details(message.as_str()),
            Error::InvalidLocation(input) => Self::new("Location not recognized")
                .hint(
                    "Use s3://bucket/folder/, gs://bucket/folder/, or \
                     az://account/container/folder/. Leave out the bucket (just s3://, \
                     gs://, or az://) to scan everything you can see.",
                )
                .details(format!("You entered: {input}")),
            Error::InvalidBucketName(name) => Self::new("Name not valid")
                .hint(
                    "Bucket and container names are 3 to 63 lowercase letters, numbers, \
                     dots, and hyphens. Storage account names are 3 to 24 lowercase \
                     letters and numbers.",
                )
                .details(format!("You entered: {name}")),
            Error::NoSuchBucket(bucket) => Self::new("Bucket not found").hint(format!(
                "There is no bucket named {bucket}. Check the spelling, and that you are \
                 signed in to the right account."
            )),
            Error::Network(message) => Self::new("Could not reach the cloud service")
                .hint("Check your internet connection, then try again.")
                .details(sentence(message)),
            Error::Unsupported(message) => Self::new("Not available").hint(sentence(message)),
            Error::NoProject => Self::new("No Google Cloud project").hint(error.to_string()),
            Error::Cancelled => Self::new("Scan stopped"),
            Error::Task(error) => Self::new("Something went wrong")
                .hint("CloudDirStat hit an internal error. Please report it on GitHub.")
                .details(error.to_string()),
        }
    }

    /// Everything, for the clipboard.
    fn text(&self) -> String {
        [Some(&self.title), self.hint.as_ref(), self.details.as_ref()]
            .into_iter()
            .flatten()
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Whether a service's error message says the request was not allowed.
fn is_denied(message: &str) -> bool {
    [
        "AccessDenied",
        "Access Denied",
        "AuthorizationPermissionMismatch",
        "AuthorizationFailure",
        "PERMISSION_DENIED",
        "Forbidden",
        "HTTP 403",
    ]
    .iter()
    .any(|marker| message.contains(marker))
}

/// What the user did in the error window.
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

/// The title for the status bar, with a link to the details. Returns whether the
/// link was clicked.
pub fn chip(ui: &mut egui::Ui, problem: &Problem) -> bool {
    let (_, text) = colors(ui.visuals());
    let mut clicked = false;
    ui.horizontal(|ui| {
        frame(ui.visuals(), Margin::symmetric(6, 1)).show(ui, |ui| {
            ui.add(Label::new(RichText::new(&problem.title).color(text)).truncate());
        });
        clicked = ui.link("Details…").clicked();
    });
    clicked
}

/// The title and hint, for places with room for them (e.g. the Estimate window).
pub fn block(ui: &mut egui::Ui, problem: &Problem) {
    let (_, text) = colors(ui.visuals());
    frame(ui.visuals(), Margin::same(8)).show(ui, |ui| {
        ui.vertical(|ui| {
            ui.label(RichText::new(&problem.title).color(text).strong());
            if let Some(hint) = &problem.hint {
                ui.add(Label::new(RichText::new(hint).color(text)).wrap());
            }
        });
    });
    details(ui, problem);
}

/// A window in the middle of the main window with what went wrong, what to try, and
/// the full details. `open` turns false when it is closed.
pub fn window(ctx: &egui::Context, problem: &Problem, open: &mut bool) -> Option<Action> {
    let mut action = None;
    let modal = egui::Modal::new(egui::Id::new("error window")).show(ctx, |ui| {
        ui.set_width(WINDOW_WIDTH.min(ctx.content_rect().width() - 64.0));
        let (_, text) = colors(ui.visuals());
        ui.label(
            RichText::new(&problem.title)
                .color(text)
                .strong()
                .size(18.0),
        );
        ui.add_space(6.0);
        if let Some(hint) = &problem.hint {
            ui.add(Label::new(hint).wrap());
            ui.add_space(6.0);
        }
        details(ui, problem);
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            if ui
                .button("Copy")
                .on_hover_text("Copy the error, e.g. for a bug report")
                .clicked()
            {
                ui.ctx().copy_text(problem.text());
            }
            if ui
                .button("Help")
                .on_hover_text("Permissions and how to sign in")
                .clicked()
            {
                action = Some(Action::Help);
            }
            ui.hyperlink_to("Troubleshooting", TROUBLESHOOTING_URL);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Close").clicked() {
                    *open = false;
                }
            });
        });
    });
    if modal.should_close() || action.is_some() {
        *open = false;
    }
    action
}

/// The service's own message, folded away; most people never need it.
fn details(ui: &mut egui::Ui, problem: &Problem) {
    let Some(details) = &problem.details else {
        return;
    };
    egui::CollapsingHeader::new("Details")
        .id_salt("error details")
        .show(ui, |ui| {
            ui.add(Label::new(RichText::new(details).weak()).wrap());
        });
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

    #[test]
    fn refused_requests_read_as_access_denied() {
        let error = Error::Request {
            operation: "ListObjectsV2",
            permission: "s3:ListBucket",
            message: "AccessDenied: User is not authorized".to_owned(),
        };
        let problem = Problem::from_error(&error);
        assert_eq!(problem.title, "Access denied");
        assert!(problem.hint.unwrap().contains("s3:ListBucket"));
    }

    #[test]
    fn invalid_locations_keep_the_input_out_of_the_title() {
        let problem = Problem::from_error(&Error::InvalidLocation("ftp://x".to_owned()));
        assert_eq!(problem.title, "Location not recognized");
        assert_eq!(problem.details.as_deref(), Some("You entered: ftp://x"));
    }
}
