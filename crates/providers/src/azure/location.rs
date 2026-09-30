use std::fmt;
use std::str::FromStr;

use crate::Error;

/// `az://account/container/prefix`, `az://account` for every container in a storage
/// account, or `az://` for every account the signed-in user can see.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AzureLocation {
    /// Empty for every account.
    pub account: String,
    /// Empty for every container of the account.
    pub container: String,
    pub prefix: String,
}

impl AzureLocation {
    pub fn is_all_accounts(&self) -> bool {
        self.account.is_empty()
    }

    pub fn is_all_containers(&self) -> bool {
        self.container.is_empty()
    }
}

impl FromStr for AzureLocation {
    type Err = Error;

    fn from_str(input: &str) -> Result<Self, Error> {
        let path = input
            .strip_prefix("az://")
            .ok_or_else(|| Error::InvalidLocation(input.to_owned()))?;
        let mut parts = path.splitn(3, '/');
        let account = parts.next().unwrap_or_default();
        let container = parts.next().unwrap_or_default();
        let prefix = parts.next().unwrap_or_default();
        if account.is_empty() && !path.is_empty() {
            return Err(Error::InvalidLocation(input.to_owned()));
        }
        if !account.is_empty() && !is_account_name(account) {
            return Err(Error::InvalidBucketName(account.to_owned()));
        }
        if !container.is_empty() && !is_container_name(container) {
            return Err(Error::InvalidBucketName(container.to_owned()));
        }
        if container.is_empty() && !prefix.is_empty() {
            return Err(Error::InvalidLocation(input.to_owned()));
        }
        Ok(Self {
            account: account.to_owned(),
            container: container.to_owned(),
            prefix: prefix.to_owned(),
        })
    }
}

/// Storage account names: 3 to 24 lowercase letters and digits.
fn is_account_name(name: &str) -> bool {
    (3..=24).contains(&name.len())
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
}

/// Container names: 3 to 63 lowercase letters, digits, and hyphens, starting with a
/// letter or digit; or the special `$root`, `$web`, and `$logs` containers.
fn is_container_name(name: &str) -> bool {
    if matches!(name, "$root" | "$web" | "$logs") {
        return true;
    }
    (3..=63).contains(&name.len())
        && name
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_alphanumeric())
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

impl fmt::Display for AzureLocation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (self.account.is_empty(), self.container.is_empty()) {
            (true, _) => write!(f, "az://"),
            (false, true) => write!(f, "az://{}/", self.account),
            (false, false) => write!(
                f,
                "az://{}/{}/{}",
                self.account, self.container, self.prefix
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(input: &str) -> (String, String, String) {
        let location: AzureLocation = input.parse().unwrap();
        (location.account, location.container, location.prefix)
    }

    #[test]
    fn parses_account_container_and_prefix() {
        assert_eq!(
            parse("az://acct/photos/2024/"),
            ("acct".into(), "photos".into(), "2024/".into())
        );
        assert_eq!(
            parse("az://acct/photos"),
            ("acct".into(), "photos".into(), "".into())
        );
        assert_eq!(parse("az://acct"), ("acct".into(), "".into(), "".into()));
        assert_eq!(parse("az://acct/"), ("acct".into(), "".into(), "".into()));
        assert_eq!(parse("az://"), ("".into(), "".into(), "".into()));
        assert_eq!(
            parse("az://acct/$web/"),
            ("acct".into(), "$web".into(), "".into())
        );
    }

    #[test]
    fn displays_each_form() {
        for text in [
            "az://",
            "az://acct/",
            "az://acct/photos/",
            "az://acct/photos/2024/",
        ] {
            assert_eq!(text.parse::<AzureLocation>().unwrap().to_string(), text);
        }
    }

    #[test]
    fn rejects_impossible_names() {
        for input in [
            "az://Acct/c",
            "az://ab/c",
            "az://acct/Photos",
            "az://acct/-c",
        ] {
            assert!(
                matches!(
                    input.parse::<AzureLocation>(),
                    Err(Error::InvalidBucketName(_))
                ),
                "{input}"
            );
        }
        assert!("az:///c".parse::<AzureLocation>().is_err());
    }
}
