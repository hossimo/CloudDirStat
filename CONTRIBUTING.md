# Contributing to CloudDirStat

Thanks for your interest! Bug reports, test results from real Google Cloud and Azure buckets, and pull requests are all welcome. For anything bigger than a small fix, please [open an issue](https://github.com/hossimo/CloudDirStat/issues) first so we can talk it over.

## Ground rules

- **Never write to a bucket.** CloudDirStat only lists. It must never add, change, or delete data in any bucket or container, or read file contents. A feature that would need a write is out of scope, even as an opt-in.
- **Least privilege.** A scan needs only permission to list. When an optional permission is missing, the feature it enables is skipped with a warning; the scan doesn't fail. A new API call means updating the permission tables in the README, [`docs/iam-policy.json`](docs/iam-policy.json), and the app's Help window (`crates/gui/src/help.rs`).
- **No secrets in logs, errors, or the repository.** Keys and tokens are never printed, logged, or saved. Error messages that mention a URL show only its host, because SAS tokens travel in the query string. There is no telemetry.
- **Made-up data only.** The repository is public: never commit real bucket, account, or object names. Use the [demo data](#demo-data-for-screenshots) for screenshots.

## Building from source

1. Install Rust with [rustup](https://rustup.rs). On Windows you also need the Visual Studio C++ Build Tools.
2. Build:

   ```sh
   cargo build --release --locked
   ```

   `--locked` uses exactly the dependency versions in `Cargo.lock`, the ones the releases are built with. `rust-toolchain.toml` picks the Rust version; rustup installs it the first time.

   The programs are `target/release/clouddirstat` and `target/release/clouddirstat-gui` (with `.exe` on Windows).

## Development commands

```sh
cargo run -p clouddirstat -- scan s3://my-bucket
cargo run -p clouddirstat-gui
cargo test --workspace --all-features
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all
```

CI runs `cargo fmt`, both clippy commands, and the tests on Linux, Windows, and macOS. Please run them before opening a pull request. Tests that talk to a real cloud are marked `#[ignore]` and only run when asked for.

## Project layout

| Crate | Purpose |
|---|---|
| `crates/core` | Folder tree, size and cost totals, filters, treemap layout. No network access. |
| `crates/providers` | Listing for S3 (with CloudWatch estimates), Google Cloud Storage, and Azure, plus price tables. |
| `crates/cli` | The `clouddirstat` command-line program. |
| `crates/gui` | The `clouddirstat-gui` app (egui). |

Scanners send objects in batches while they list, and the tree is built as they arrive, so the app can draw while a scan runs. Each object takes a 48-byte node plus its name, stored in fixed-size blocks; full object keys are never kept. That is about 66 bytes per object. Measure it with:

```sh
cargo run --release -p clouddirstat-core --features demo --example memory -- 10000000
```

## Price tables

The built-in prices are generated from the providers' public price lists (no credentials needed):

```sh
python scripts/update_s3_prices.py
python scripts/update_gcs_prices.py
python scripts/update_azure_prices.py
```

## Demo data (for screenshots)

Build the app with the `demo` feature to scan made-up buckets instead of a real cloud, so screenshots never show real data. Release builds never include it.

```sh
cargo run --release -p clouddirstat-gui --features demo -- s3://
```

`s3://` shows six fictional `acme-*` buckets; `s3://acme-backups/` shows one. The data is the same every time; set `CLOUDDIRSTAT_DEMO_OBJECTS` for more or fewer objects (default 120,000).

## Version numbers

Versions come from git tags: after `git tag v0.2.0`, builds report `0.2.0`, and builds from later commits report `0.2.0+N` (N commits since the tag). Without a tag, the version in `Cargo.toml` is used. `--version` and the app's Help window also show the commit.

## Making a release

Create a release on GitHub with a new `vX.Y.Z` tag. Publishing it runs the **Release** workflow, which:

- builds all six targets (Windows, macOS, and Linux, each on x64 and ARM64);
- joins the two Mac builds into a universal `CloudDirStat.app` in a DMG (`scripts/macos_app.sh`, which also runs on any Mac) and a universal command line;
- attaches everything: two files for Mac, and one archive per Windows and Linux target;
- records a build provenance attestation for every file (see [Checking a download](README.md#checking-a-download)).

A release's files can't be replaced by publishing again. To rebuild them, run the workflow by hand from the Actions tab with that tag, which replaces them. With no tag it only builds, which is useful for testing. Actions are pinned to commit SHAs, which Dependabot keeps up to date.

## License

By contributing, you agree that your contributions are dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), like the rest of the project.
