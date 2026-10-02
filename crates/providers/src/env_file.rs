use std::io::Read;

/// The variables a `.env` file may set: the sign-in variables the README documents.
/// Others, such as AWS_ENDPOINT_URL or GOOGLE_APPLICATION_CREDENTIALS, could send
/// requests or credentials somewhere else, so they must come from the real environment.
pub const DOTENV_VARIABLES: &[&str] = &[
    "AWS_ACCESS_KEY_ID",
    "AWS_SECRET_ACCESS_KEY",
    "AWS_SESSION_TOKEN",
    "AWS_PROFILE",
    "AWS_REGION",
    "GOOGLE_OAUTH_ACCESS_TOKEN",
    "GOOGLE_CLOUD_PROJECT",
    "CLOUDSDK_CORE_PROJECT",
    "AZURE_STORAGE_KEY",
    "AZURE_STORAGE_CONNECTION_STRING",
    "AZURE_STORAGE_SAS_TOKEN",
];

/// Sets the allowed variables in `.env` in the current folder (not its parents) that
/// aren't set already, and returns the names of the variables it ignored. A missing or
/// unreadable file sets nothing.
///
/// # Safety
///
/// Changes the process environment, so no other thread may be running.
pub unsafe fn load_dotenv() -> Vec<String> {
    let Ok(file) = std::fs::File::open(".env") else {
        return Vec::new();
    };
    let (allowed, ignored) = read_dotenv(file);
    for (name, value) in allowed {
        if std::env::var_os(&name).is_none() {
            // SAFETY: the caller guarantees no other thread is running.
            unsafe { std::env::set_var(name, value) };
        }
    }
    ignored
}

/// The allowed `(name, value)` pairs, and the names of the others. Lines that don't
/// parse are skipped.
fn read_dotenv(reader: impl Read) -> (Vec<(String, String)>, Vec<String>) {
    let mut allowed = Vec::new();
    let mut ignored = Vec::new();
    for (name, value) in dotenvy::from_read_iter(reader).flatten() {
        if DOTENV_VARIABLES.contains(&name.as_str()) {
            allowed.push((name, value));
        } else {
            ignored.push(name);
        }
    }
    (allowed, ignored)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_only_sign_in_variables() {
        let file = "AZURE_STORAGE_KEY=abc\n\
                    # a comment\n\
                    AWS_ENDPOINT_URL=https://example.invalid\n\
                    export GOOGLE_CLOUD_PROJECT=my-project\n\
                    GOOGLE_APPLICATION_CREDENTIALS=/tmp/key.json\n";
        let (allowed, ignored) = read_dotenv(file.as_bytes());
        assert_eq!(
            allowed,
            [
                ("AZURE_STORAGE_KEY".to_owned(), "abc".to_owned()),
                ("GOOGLE_CLOUD_PROJECT".to_owned(), "my-project".to_owned()),
            ]
        );
        assert_eq!(
            ignored,
            ["AWS_ENDPOINT_URL", "GOOGLE_APPLICATION_CREDENTIALS"]
        );
    }
}
