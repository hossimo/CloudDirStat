//! Works out the version and commit hash shown in the apps. The version comes from the
//! nearest git tag (`v1.2.0` -> `1.2.0`, or `1.2.0+3` three commits later), falling back
//! to the Cargo package version when there is no tag or no git.

use std::env;
use std::process::Command;

fn main() {
    watch_git_state();

    let version = version_from_tag()
        .unwrap_or_else(|| env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "unknown".to_owned()));
    let hash = git(&["rev-parse", "--short=7", "HEAD"]).unwrap_or_else(|| "unknown".to_owned());

    println!("cargo:rustc-env=CLOUDDIRSTAT_VERSION={version}");
    println!("cargo:rustc-env=CLOUDDIRSTAT_GIT_HASH={hash}");
}

/// Rebuild the version info only when the checked-out commit or the tags change.
fn watch_git_state() {
    let Some(git_dir) = git(&["rev-parse", "--absolute-git-dir"]) else {
        return;
    };
    for path in ["HEAD", "packed-refs", "refs/tags"] {
        println!("cargo:rerun-if-changed={git_dir}/{path}");
    }
    if let Some(branch) = git(&["symbolic-ref", "-q", "HEAD"]) {
        println!("cargo:rerun-if-changed={git_dir}/{branch}");
    }
}

fn version_from_tag() -> Option<String> {
    // e.g. "v1.2.0-3-gabc1234": tag, commits since the tag, hash.
    let described = git(&["describe", "--tags", "--long"])?;
    let mut parts = described.rsplitn(3, '-');
    let _hash = parts.next()?;
    let commits_since = parts.next()?;
    let tag = parts.next()?;
    let tag = tag.strip_prefix('v').unwrap_or(tag);
    Some(if commits_since == "0" {
        tag.to_owned()
    } else {
        format!("{tag}+{commits_since}")
    })
}

fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?.trim().to_owned();
    (!text.is_empty()).then_some(text)
}
