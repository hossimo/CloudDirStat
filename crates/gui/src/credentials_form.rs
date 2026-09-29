use clouddirstat_providers::s3::{AccessKey, CredentialSource};
use eframe::egui;

const PROFILE_HELP: &str = "A profile from ~/.aws/config. Leave empty for the default profile \
     or environment variables.\n\nRecommended: short-lived credentials from `aws login` \
     (console sign-in, AWS CLI v2) or `aws sso login` (IAM Identity Center).";
const ACCESS_KEY_HELP: &str = "Keys are kept in memory only: never saved to disk or logged.\n\n\
     Prefer temporary credentials (with a session token), or a profile set up with \
     `aws login` or `aws sso login`, over long-term access keys.";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Mode {
    #[default]
    Profile,
    AccessKey,
}

/// The credential inputs in the toolbar.
#[derive(Default)]
pub struct CredentialsForm {
    mode: Mode,
    profile: String,
    access_key_id: String,
    secret_access_key: String,
    session_token: String,
}

impl CredentialsForm {
    pub fn new(profile: Option<String>) -> Self {
        Self {
            profile: profile.unwrap_or_default(),
            ..Self::default()
        }
    }

    /// The mode switch and, in profile mode, the profile field. Returns whether Enter
    /// was pressed in a field.
    pub fn show_main_row(&mut self, ui: &mut egui::Ui) -> bool {
        ui.selectable_value(&mut self.mode, Mode::Profile, "Profile")
            .on_hover_text(PROFILE_HELP);
        ui.selectable_value(&mut self.mode, Mode::AccessKey, "Access key")
            .on_hover_text(ACCESS_KEY_HELP);
        match self.mode {
            Mode::Profile => submitted(
                ui,
                egui::TextEdit::singleline(&mut self.profile)
                    .hint_text("default")
                    .desired_width(120.0),
            ),
            Mode::AccessKey => false,
        }
    }

    /// The access key fields, shown on their own row. Returns whether Enter was pressed.
    pub fn show_access_key_row(&mut self, ui: &mut egui::Ui) -> bool {
        if self.mode != Mode::AccessKey {
            return false;
        }
        let mut entered = false;
        ui.horizontal(|ui| {
            ui.label("Access key ID");
            entered |= submitted(
                ui,
                egui::TextEdit::singleline(&mut self.access_key_id).desired_width(180.0),
            );
            ui.label("Secret");
            entered |= submitted(
                ui,
                egui::TextEdit::singleline(&mut self.secret_access_key)
                    .password(true)
                    .desired_width(220.0),
            );
            ui.label("Session token");
            entered |= submitted(
                ui,
                egui::TextEdit::singleline(&mut self.session_token)
                    .password(true)
                    .hint_text("optional")
                    .desired_width(160.0),
            );
        });
        entered
    }

    pub fn source(&self) -> Result<CredentialSource, String> {
        match self.mode {
            Mode::Profile => {
                let profile = self.profile.trim();
                Ok(CredentialSource::Chain {
                    profile: (!profile.is_empty()).then(|| profile.to_owned()),
                })
            }
            Mode::AccessKey => {
                let access_key_id = self.access_key_id.trim();
                let secret_access_key = self.secret_access_key.trim();
                if access_key_id.is_empty() || secret_access_key.is_empty() {
                    return Err("Enter both an access key ID and a secret access key.".to_owned());
                }
                let session_token = self.session_token.trim();
                Ok(CredentialSource::AccessKey(AccessKey {
                    access_key_id: access_key_id.to_owned(),
                    secret_access_key: secret_access_key.to_owned(),
                    session_token: (!session_token.is_empty()).then(|| session_token.to_owned()),
                }))
            }
        }
    }
}

fn submitted(ui: &mut egui::Ui, field: egui::TextEdit<'_>) -> bool {
    let response = ui.add(field);
    response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_profile_means_the_default_chain() {
        let form = CredentialsForm::new(None);
        assert!(matches!(
            form.source(),
            Ok(CredentialSource::Chain { profile: None })
        ));
    }

    #[test]
    fn access_key_needs_id_and_secret() {
        let mut form = CredentialsForm {
            mode: Mode::AccessKey,
            access_key_id: "AKIDEXAMPLE".to_owned(),
            ..CredentialsForm::default()
        };
        assert!(form.source().is_err());

        form.secret_access_key = " secret ".to_owned();
        let Ok(CredentialSource::AccessKey(key)) = form.source() else {
            panic!("expected an access key");
        };
        assert_eq!(key.secret_access_key, "secret");
        assert_eq!(key.session_token, None);
    }
}
