<img src="icons/png/icon-128.png" alt="CloudDirStat icon" width="96" align="right">

# CloudDirStat

[![CI](https://github.com/hossimo/CloudDirStat/actions/workflows/ci.yml/badge.svg)](https://github.com/hossimo/CloudDirStat/actions/workflows/ci.yml)

WinDirStat for cloud object storage. Find out what is using the space (and the money) in your buckets.

CloudDirStat lists a bucket, rebuilds a folder tree from the object keys, and shows where the bytes are: by directory, by storage class, and by version state (current, noncurrent, delete markers).

- **Least privilege:** only needs `s3:ListBucket`. It never reads or writes object contents.
- **Native and small:** single binaries for Windows, macOS, and Linux (CLI ~9 MB, GUI ~14 MB). No runtime, no browser.
- **Fast:** lists prefixes in parallel.
- **Cost-aware:** estimates the monthly storage cost of every folder and object from your bucket region's list prices, and reports what the scan itself cost in LIST requests.

> **Status:** early development. The command-line scanner for AWS S3 works, and a first treemap GUI is in progress. Azure Blob Storage and Google Cloud Storage come later (see [Roadmap](#roadmap)).

## Usage

```sh
clouddirstat scan s3://my-bucket
clouddirstat scan s3://my-bucket/logs/ --profile prod --depth 3
clouddirstat scan s3://my-bucket --versions
clouddirstat scan s3://                      # every bucket, each as a top-level folder
```

| Option | Default | Description |
|---|---|---|
| `--profile <NAME>` | default chain | AWS profile from `~/.aws/config` / `~/.aws/credentials` |
| `--region <REGION>` | auto | Bucket region. Detected automatically when omitted |
| `--versions` | off | Include noncurrent versions and delete markers (needs `s3:ListBucketVersions`) |
| `--depth <N>` | 2 | Directory levels to print |
| `--top <N>` | 10 | Entries per directory and in the largest-objects list |
| `--concurrency <N>` | 32 | Maximum parallel LIST requests |

Example output:

```
s3://my-backups/  6.8 GiB in 1,024 objects
Scanned in 0.5s using 3 LIST requests (~$0.0000)
Estimated storage cost ~$0.16/month (us-east-1 list prices from 2026-09-28)

By storage class
  STANDARD                  6.8 GiB  100.0%        $0.16/mo         1,024 objects

By version state
  Current versions          6.8 GiB  100.0%        $0.16/mo         1,024 objects
  Noncurrent versions           0 B    0.0%        $0.00/mo             0 objects
  Delete markers                0 B    0.0%        $0.00/mo             0 objects

Largest directories
     6.8 GiB  100.0%        $0.16/mo  /
     6.6 GiB   98.3%        $0.15/mo    backups/
    11.2 MiB    0.2%       <$0.01/mo      db-snapshot-0412.tar.gz
     6.6 GiB   97.5%        $0.15/mo      ... 1,016 more
    83.5 MiB    1.2%       <$0.01/mo    archive.zip

Largest objects
    83.5 MiB       <$0.01/mo  archive.zip
    11.2 MiB       <$0.01/mo  backups/db-snapshot-0412.tar.gz
```

### GUI

```sh
clouddirstat-gui
clouddirstat-gui s3://my-bucket --profile prod
```

Enter a location (and optionally a profile) and press **Scan**, or pass them on the command line to scan on startup. Use `s3://` as the location to scan every bucket at once (needs `s3:ListAllMyBuckets`); each bucket appears as a top-level folder, priced at its own region's rates, and buckets you can't list are skipped with a warning.

- **Folder list:** every prefix and object, sorted by size, with its share of the parent folder. It fills in while the scan runs. Click the arrow or double-click a folder to expand it.
- **Treemap:** appears when the scan finishes. Each rectangle is an object sized by bytes; shading shows which folder it belongs to. Hover to see the object and outline its folder; click to select it in the list.
- **Color by:** the tabs on the right switch the treemap colors and legend between **Storage classes**, **Versions** (objects with old versions, or deleted objects whose old versions are still billed; needs **Versions** checked), **Prefixes** (the largest top-level folders; click one to select it in the folder list), and **File types** (grouped by extension, like WinDirStat).
- **Stop** ends the scan and keeps the partial result.
- **Help** explains the permissions a scan needs and shows a minimal IAM policy for the bucket in the Location field, ready to copy.

### Credentials

CloudDirStat uses the standard AWS credential chain, the same as the AWS CLI: environment variables, `~/.aws/credentials`, `~/.aws/config` profiles, and EC2/ECS/EKS roles. Pick a profile with `--profile` (CLI) or the **Profile** field (GUI).

Prefer short-lived credentials over long-term access keys. Both of these work out of the box:

| Method | Set up | Then |
|---|---|---|
| Console sign-in (`aws login`, AWS CLI v2) | `aws login --profile myprofile` | `clouddirstat scan s3://my-bucket --profile myprofile` |
| IAM Identity Center (SSO) | `aws configure sso` | `aws sso login --profile myprofile`, then scan with `--profile myprofile` |

**Access keys without an AWS config.** In the GUI, choose **Access key** and enter the access key ID, secret access key, and (for temporary credentials) session token. Keys are kept in memory for the session only: they are never written to disk or logged. The CLI reads keys from the standard environment variables instead of command-line flags, so they don't end up in shell history:

```sh
export AWS_ACCESS_KEY_ID=...
export AWS_SECRET_ACCESS_KEY=...
export AWS_SESSION_TOKEN=...   # only for temporary credentials
clouddirstat scan s3://my-bucket
```

If you do create an access key, give it only the permissions below.

### Cost of a scan

S3 charges for LIST requests (about $0.005 per 1,000 in most regions). Each request returns up to 1,000 objects, so scanning 10 million objects costs roughly $0.05.

### Storage cost estimates

The **Cost/mo** figures (GUI columns, tooltips, and status bar; CLI report) estimate what keeping the objects stored costs per month, using the S3 list prices for the bucket's region:

- Each storage class at its own per-GB-month price, including the 128 KB minimum billable size for Standard-IA, One Zone-IA, and Glacier Instant Retrieval, the 40 KB per-object overhead for Glacier Flexible Retrieval and Deep Archive, and the Intelligent-Tiering monitoring fee.
- Noncurrent versions count (scan with `--versions` / **Versions** to include them); delete markers are free.
- Approximations: first volume tier only (large buckets pay slightly less), Intelligent-Tiering priced at its Frequent Access tier (the highest), and no request, retrieval, data transfer, minimum-duration, or discount charges.

Prices are compiled into the binary from the public AWS Price List ([S3 pricing](https://aws.amazon.com/s3/pricing/)), so estimating needs no extra permissions or network calls. The date of the price list is shown with the estimate. To refresh the prices (no AWS credentials needed):

```sh
python scripts/update_s3_prices.py
```

## Permissions

The minimum IAM policy is in [`docs/iam-policy.json`](docs/iam-policy.json).

| Permission | Required | Used for |
|---|---|---|
| `s3:ListBucket` | yes | Listing objects and detecting the bucket region |
| `s3:ListBucketVersions` | only with `--versions` | Noncurrent versions and delete markers |
| `s3:ListAllMyBuckets` | only for `s3://` (all buckets) | Finding every bucket to scan them together |

CloudDirStat never calls `GetObject`, `PutObject`, or `DeleteObject`.

## Building from source

1. Install Rust with [rustup](https://rustup.rs). On Windows you also need the Visual Studio C++ Build Tools.
2. Build:

```sh
cargo build --release
```

The binaries are `target/release/clouddirstat` and `target/release/clouddirstat-gui` (with `.exe` on Windows).

**Versions** come from git tags: after `git tag v0.2.0`, builds report `0.2.0`, and builds from later commits report `0.2.0+N` (N commits since the tag). Without a tag the version in `Cargo.toml` is used. `--version` and the GUI's Help window also show the short commit hash.

Development commands:

```sh
cargo run -p clouddirstat -- scan s3://my-bucket
cargo run -p clouddirstat-gui
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
```

### Project layout

| Crate | Purpose |
|---|---|
| `crates/core` | Provider-agnostic folder tree, size aggregation, formatting. No I/O. |
| `crates/providers` | Cloud scanners. Currently S3. |
| `crates/cli` | The `clouddirstat` command-line tool. |
| `crates/gui` | The `clouddirstat-gui` treemap app (egui). |

Scanners stream objects in batches over a channel, and the tree is built while the scan is still running. This is what will let the GUI draw while a scan is in progress. To keep memory low on big buckets, the tree stores each path segment only once and does not keep full object keys.

## Troubleshooting

**`could not load AWS credentials ... could not parse profile file`**

The AWS SDK for Rust is stricter than the AWS CLI about the format of `~/.aws/config` and `~/.aws/credentials`. The usual cause is an indented setting directly under a profile header:

```ini
[default]
    region = us-east-1     # indented: treated as a continuation line
```

Remove the leading spaces:

```ini
[default]
region = us-east-1
```

Indentation is only valid for nested settings, for example under `s3 =`.

**Credentials expired / `aws login` or SSO session errors**

Sessions from `aws login` and `aws sso login` expire. Run the login command again for that profile, then rescan.

## Roadmap

- [x] S3 scanner CLI: directory tree, storage classes, versions, LIST cost
- [ ] Treemap GUI (egui): folder list, shaded treemap, color by class/versions/prefix done; next per-folder scan progress, zoom, largest-files list
- [ ] Incomplete multipart uploads
- [x] Estimated monthly storage cost per directory and object
- [ ] Instant bucket totals from CloudWatch
- [ ] Read S3 Inventory reports for billion-object buckets
- [ ] Azure Blob Storage
- [ ] Google Cloud Storage

## AI Disclosure

Parts of this project are developed with AI assistance (Claude). All code is reviewed, tested, and owned by the maintainer.

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
