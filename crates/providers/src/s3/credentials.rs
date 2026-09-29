use std::fmt;

use aws_config::ConfigLoader;
use aws_sdk_s3::config::Credentials;

/// Where the AWS credentials for a scan come from.
#[derive(Clone, Debug)]
pub enum CredentialSource {
    /// The standard AWS credential chain, like the AWS CLI: environment variables,
    /// `~/.aws` profiles (including IAM Identity Center and `aws login` sessions),
    /// and EC2/ECS/EKS roles. `None` uses the default profile.
    Chain { profile: Option<String> },
    /// An access key typed in by the user.
    AccessKey(AccessKey),
}

/// An access key, optionally with the session token of temporary credentials.
/// Held in memory only; `Debug` never shows the secret or the token.
#[derive(Clone)]
pub struct AccessKey {
    pub access_key_id: String,
    pub secret_access_key: String,
    pub session_token: Option<String>,
}

impl fmt::Debug for AccessKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AccessKey")
            .field("access_key_id", &self.access_key_id)
            .field("secret_access_key", &"** redacted **")
            .field(
                "session_token",
                &self.session_token.as_ref().map(|_| "** redacted **"),
            )
            .finish()
    }
}

impl CredentialSource {
    pub(super) fn configure(&self, loader: ConfigLoader) -> ConfigLoader {
        match self {
            Self::Chain { profile: None } => loader,
            Self::Chain {
                profile: Some(profile),
            } => loader.profile_name(profile),
            Self::AccessKey(key) => loader.credentials_provider(Credentials::new(
                &key.access_key_id,
                &key.secret_access_key,
                key.session_token.clone(),
                None,
                "CloudDirStat",
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_output_hides_secrets() {
        let key = AccessKey {
            access_key_id: "AKIDEXAMPLE".to_owned(),
            secret_access_key: "super-secret".to_owned(),
            session_token: Some("token-value".to_owned()),
        };

        let shown = format!("{:?}", CredentialSource::AccessKey(key));

        assert!(shown.contains("AKIDEXAMPLE"));
        assert!(!shown.contains("super-secret"));
        assert!(!shown.contains("token-value"));
    }
}
