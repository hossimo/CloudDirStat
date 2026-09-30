use clouddirstat_providers::azure::AzureCredentials;
use clouddirstat_providers::gcs::GcsCredentials;
use clouddirstat_providers::s3::{AccessKey, CredentialSource};
use clouddirstat_providers::{Credentials, Provider};
use eframe::egui;

const PROFILE_HELP: &str = "A profile from ~/.aws/config. Leave empty for the default profile \
     or environment variables.\n\nRecommended: short-lived credentials from `aws login` \
     (console sign-in, AWS CLI v2) or `aws sso login` (IAM Identity Center).";
const ACCESS_KEY_HELP: &str = "Keys are kept in memory only: never saved to disk or logged.\n\n\
     Prefer temporary credentials (with a session token), or a profile set up with \
     `aws login` or `aws sso login`, over long-term access keys.";
const GOOGLE_LOGIN_HELP: &str = "Application Default Credentials: sign in once with \
     `gcloud auth application-default login`. A service account key file set in \
     GOOGLE_APPLICATION_CREDENTIALS also works.";
const GOOGLE_TOKEN_HELP: &str = "Paste the output of `gcloud auth print-access-token`. It is \
     valid for about an hour and kept in memory only: never saved to disk or logged.";
const AZURE_CLI_HELP: &str = "Tokens from the Azure CLI: sign in once with `az login`. \
     Reading blobs needs the Storage Blob Data Reader role on the account or container \
     (being the subscription's owner is not enough).";
const SAS_HELP: &str = "A shared access signature for the account or container (with list \
     permission), as generated in the portal or with `az storage account \
     generate-sas`. Kept in memory only: never saved to disk or logged.";
const ACCOUNT_KEY_HELP: &str = "key1 or key2 from the storage account's Access keys page, or \
     its connection string. It grants full access to the account, so prefer the Azure CLI or \
     a read-only SAS where you can. Kept in memory only: never saved to disk or logged.";
const PROJECT_HELP: &str = "Only needed for gs:// (every bucket of a project). Leave empty \
     to use the project from gcloud (`gcloud config set project ...`).";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum AwsMode {
    #[default]
    Profile,
    AccessKey,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum AzureMode {
    #[default]
    Cli,
    Sas,
    AccountKey,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum GoogleMode {
    #[default]
    Login,
    AccessToken,
}

/// The credential inputs in the toolbar, for whichever provider the location is at.
/// Each provider keeps its own inputs, so switching locations loses nothing.
#[derive(Default)]
pub struct CredentialsForm {
    aws_mode: AwsMode,
    profile: String,
    access_key_id: String,
    secret_access_key: String,
    session_token: String,
    google_mode: GoogleMode,
    google_token: String,
    google_project: String,
    azure_mode: AzureMode,
    azure_sas: String,
    azure_account_key: String,
}

impl CredentialsForm {
    /// Starts with `profile`, and with the Azure key or SAS from the environment (or
    /// .env) when one is set.
    pub fn new(profile: Option<String>) -> Self {
        let azure = AzureCredentials::from_env();
        let azure_mode = if azure.account_key.is_some() {
            AzureMode::AccountKey
        } else if azure.sas.is_some() {
            AzureMode::Sas
        } else {
            AzureMode::Cli
        };
        Self {
            profile: profile.unwrap_or_default(),
            azure_mode,
            azure_sas: azure.sas.unwrap_or_default(),
            azure_account_key: azure.account_key.unwrap_or_default(),
            ..Self::default()
        }
    }

    /// The mode switch and the short fields for `provider`. Returns whether Enter was
    /// pressed in a field.
    pub fn show_main_row(&mut self, ui: &mut egui::Ui, provider: Provider) -> bool {
        match provider {
            Provider::S3 => {
                ui.selectable_value(&mut self.aws_mode, AwsMode::Profile, "Profile")
                    .on_hover_text(PROFILE_HELP);
                ui.selectable_value(&mut self.aws_mode, AwsMode::AccessKey, "Access key")
                    .on_hover_text(ACCESS_KEY_HELP);
                match self.aws_mode {
                    AwsMode::Profile => submitted(
                        ui,
                        egui::TextEdit::singleline(&mut self.profile)
                            .hint_text("default")
                            .desired_width(120.0),
                    ),
                    AwsMode::AccessKey => false,
                }
            }
            Provider::Gcs => {
                ui.selectable_value(&mut self.google_mode, GoogleMode::Login, "Google login")
                    .on_hover_text(GOOGLE_LOGIN_HELP);
                ui.selectable_value(
                    &mut self.google_mode,
                    GoogleMode::AccessToken,
                    "Access token",
                )
                .on_hover_text(GOOGLE_TOKEN_HELP);
                ui.label("Project").on_hover_text(PROJECT_HELP);
                submitted(
                    ui,
                    egui::TextEdit::singleline(&mut self.google_project)
                        .hint_text("from gcloud")
                        .desired_width(140.0),
                )
            }
            Provider::Azure => {
                ui.selectable_value(&mut self.azure_mode, AzureMode::Cli, "Azure CLI")
                    .on_hover_text(AZURE_CLI_HELP);
                ui.selectable_value(&mut self.azure_mode, AzureMode::Sas, "SAS token")
                    .on_hover_text(SAS_HELP);
                ui.selectable_value(&mut self.azure_mode, AzureMode::AccountKey, "Account key")
                    .on_hover_text(ACCOUNT_KEY_HELP);
                false
            }
        }
    }

    /// Secrets, shown on their own row when that mode is chosen. Returns whether Enter
    /// was pressed.
    pub fn show_secret_row(&mut self, ui: &mut egui::Ui, provider: Provider) -> bool {
        let mut entered = false;
        match provider {
            Provider::S3 if self.aws_mode == AwsMode::AccessKey => {
                ui.horizontal_wrapped(|ui| {
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
            }
            Provider::Azure if self.azure_mode == AzureMode::Sas => {
                ui.horizontal(|ui| {
                    ui.label("SAS token").on_hover_text(SAS_HELP);
                    entered |= submitted(
                        ui,
                        egui::TextEdit::singleline(&mut self.azure_sas)
                            .password(true)
                            .hint_text("sv=...&sig=...")
                            .desired_width(320.0),
                    );
                });
            }
            Provider::Azure if self.azure_mode == AzureMode::AccountKey => {
                ui.horizontal(|ui| {
                    ui.label("Account key").on_hover_text(ACCOUNT_KEY_HELP);
                    entered |= submitted(
                        ui,
                        egui::TextEdit::singleline(&mut self.azure_account_key)
                            .password(true)
                            .hint_text("key or connection string")
                            .desired_width(320.0),
                    );
                });
            }
            Provider::Gcs if self.google_mode == GoogleMode::AccessToken => {
                ui.horizontal(|ui| {
                    ui.label("Access token").on_hover_text(GOOGLE_TOKEN_HELP);
                    entered |= submitted(
                        ui,
                        egui::TextEdit::singleline(&mut self.google_token)
                            .password(true)
                            .hint_text("gcloud auth print-access-token")
                            .desired_width(320.0),
                    );
                });
            }
            _ => {}
        }
        entered
    }

    /// The credentials entered for `provider`, or what is missing. Other providers get
    /// their defaults.
    pub fn credentials(&self, provider: Provider) -> Result<Credentials, String> {
        let mut credentials = Credentials::default();
        match provider {
            Provider::S3 => credentials.aws = self.aws()?,
            Provider::Gcs => credentials.gcs = self.google()?,
            Provider::Azure => credentials.azure = self.azure()?,
        }
        Ok(credentials)
    }

    fn aws(&self) -> Result<CredentialSource, String> {
        match self.aws_mode {
            AwsMode::Profile => {
                let profile = self.profile.trim();
                Ok(CredentialSource::Chain {
                    profile: (!profile.is_empty()).then(|| profile.to_owned()),
                })
            }
            AwsMode::AccessKey => {
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

    fn google(&self) -> Result<GcsCredentials, String> {
        let project = self.google_project.trim();
        let access_token = match self.google_mode {
            GoogleMode::Login => None,
            GoogleMode::AccessToken => {
                let token = self.google_token.trim();
                if token.is_empty() {
                    return Err(
                        "Paste an access token from `gcloud auth print-access-token`.".to_owned(),
                    );
                }
                Some(token.to_owned())
            }
        };
        Ok(GcsCredentials {
            access_token,
            project: (!project.is_empty()).then(|| project.to_owned()),
        })
    }

    fn azure(&self) -> Result<AzureCredentials, String> {
        match self.azure_mode {
            AzureMode::Cli => Ok(AzureCredentials::default()),
            AzureMode::Sas => {
                let sas = self.azure_sas.trim();
                if sas.is_empty() {
                    return Err("Paste a SAS token for the account or container.".to_owned());
                }
                Ok(AzureCredentials {
                    sas: Some(sas.to_owned()),
                    account_key: None,
                })
            }
            AzureMode::AccountKey => {
                let key = self.azure_account_key.trim();
                if key.is_empty() {
                    return Err(
                        "Paste an access key or connection string for the account.".to_owned()
                    );
                }
                Ok(AzureCredentials {
                    sas: None,
                    account_key: Some(key.to_owned()),
                })
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
            form.aws(),
            Ok(CredentialSource::Chain { profile: None })
        ));
    }

    #[test]
    fn access_key_needs_id_and_secret() {
        let mut form = CredentialsForm {
            aws_mode: AwsMode::AccessKey,
            access_key_id: "AKIDEXAMPLE".to_owned(),
            ..CredentialsForm::default()
        };
        assert!(form.credentials(Provider::S3).is_err());

        form.secret_access_key = " secret ".to_owned();
        let Ok(CredentialSource::AccessKey(key)) = form.aws() else {
            panic!("expected an access key");
        };
        assert_eq!(key.secret_access_key, "secret");
        assert_eq!(key.session_token, None);
    }

    #[test]
    fn only_the_scanned_provider_is_checked() {
        let form = CredentialsForm {
            aws_mode: AwsMode::AccessKey,
            google_mode: GoogleMode::AccessToken,
            google_token: " ya29.token ".to_owned(),
            google_project: "my-project".to_owned(),
            ..CredentialsForm::default()
        };
        // Missing AWS keys don't matter for a Google scan.
        let credentials = form.credentials(Provider::Gcs).unwrap();
        assert_eq!(credentials.gcs.access_token.as_deref(), Some("ya29.token"));
        assert_eq!(credentials.gcs.project.as_deref(), Some("my-project"));
        assert!(form.credentials(Provider::S3).is_err());
    }
}
