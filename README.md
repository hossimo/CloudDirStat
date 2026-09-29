<img src="icons/png/icon-128.png" alt="CloudDirStat icon" width="96" align="right">

# CloudDirStat

[![CI](https://github.com/hossimo/CloudDirStat/actions/workflows/ci.yml/badge.svg)](https://github.com/hossimo/CloudDirStat/actions/workflows/ci.yml)

WinDirStat for cloud object storage. Find out what is using the space (and the money) in your buckets.

CloudDirStat lists a bucket (or all of them), rebuilds a folder tree from the object keys, and shows where the bytes and the money are: by folder, storage class, file type, and version state, in a sortable folder list and a treemap.

- **Least privilege:** only needs `s3:ListBucket`. Optional permissions add features and are skipped with a warning when missing. It never reads or writes object contents.
- **Cost-aware:** estimates the monthly storage cost of every folder and object from each bucket region's list prices, and reports what the scan itself cost in LIST requests.
- **Finds hidden costs:** noncurrent versions, deleted objects whose old versions are still billed, and incomplete multipart uploads that normal listings don't show.
- **All buckets at once:** scan `s3://` to see every bucket in one tree.
- **Native and small:** single binaries for Windows, macOS, and Linux, x64 and ARM64 (CLI ~9 MB, GUI ~14 MB). No runtime, no browser.
- **Fast and lean:** lists prefixes and buckets in parallel; about 66 bytes of memory per object (roughly 650 MB for 10 million).

> **Status:** early development, but usable: the CLI and GUI both work for AWS S3, and [prebuilt releases](#download) are available. Azure Blob Storage and Google Cloud Storage come later (see [Roadmap](#roadmap)).

![CloudDirStat scanning six demo buckets: folder list, file types legend, and treemap colored by file type](screenshots/demo-1.png)

*Scan of made-up demo buckets (see [Demo data](#demo-data-for-screenshots)).*

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
| `--top <N>` | 10 | Entries per directory and in the largest-objects list (per bucket for `s3://`) |
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
  Incomplete uploads            0 B    0.0%        $0.00/mo             0 objects

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
- **Largest files:** the tab next to **Folders** lists the largest files (50 by default; change the number at the top), with size, cost, storage class, and folder. When scanning all buckets it lists the largest files in each bucket, under headings you can collapse one by one or all at once. Click a file to select it; double-click to show it in the folder list.
- **Treemap:** appears when the scan finishes. Each rectangle is an object sized by bytes; shading shows which folder it belongs to. Hover to see the object and outline its folder; click to select it in the list.
- **Color by:** the tabs on the right switch the treemap colors and legend between **Storage classes**, **Versions** (objects with old versions and deleted objects whose old versions are still billed, which need **Versions** checked, plus incomplete uploads, which are always found), **Prefixes** (the largest top-level folders; click one to select it in the folder list), and **File types** (grouped by extension, like WinDirStat).
- **Status bar:** total estimated cost per month, incomplete uploads when there are any, scan time and LIST cost, and any warnings (hover for details).
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

### Incomplete multipart uploads

When a large upload fails or is abandoned, the parts already uploaded stay in the bucket and are billed every month, but they don't appear in normal listings or the S3 console's object list. CloudDirStat finds them on every scan: each shows up at its key's path as an **Incomplete upload** (colored in the Versions tab), the status bar and CLI report show the total, and costs are included in the estimates. A lifecycle rule with `AbortIncompleteMultipartUpload` cleans them up automatically.

This uses two optional permissions. Without `s3:ListBucketMultipartUploads` the check is skipped; without `s3:ListMultipartUploadParts` uploads are listed with unknown (zero) size. Either way the scan completes and shows a warning.

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
| `s3:ListBucketMultipartUploads` | optional | Finding incomplete multipart uploads (hidden, billed storage) |
| `s3:ListMultipartUploadParts` | optional | Sizing those uploads (on `arn:aws:s3:::bucket/*`) |
| `s3:ListAllMyBuckets` | only for `s3://` (all buckets) | Finding every bucket to scan them together |

CloudDirStat never calls `GetObject`, `PutObject`, or `DeleteObject`.

## Download

Prebuilt binaries for Windows, macOS, and Linux (x64 and ARM64) are attached to each [release](https://github.com/hossimo/CloudDirStat/releases). Each archive contains `clouddirstat` (CLI) and `clouddirstat-gui`.

The binaries are not code-signed yet. On macOS, clear the download quarantine before the first run:

```sh
xattr -d com.apple.quarantine clouddirstat clouddirstat-gui
```

On Windows, SmartScreen may warn about an unrecognized app; choose **More info → Run anyway**.

### Making a release

Create a release on GitHub with a new `vX.Y.Z` tag. Publishing it runs the **Release** workflow, which builds all six targets and attaches the archives to the release. To rebuild the files for an existing release, run the workflow manually from the Actions tab with that tag; with no tag it only builds, which is handy for testing.

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

Scanners stream objects in batches over a channel, and the tree is built while the scan is still running, which lets the GUI draw while a scan is in progress. To keep memory low on big buckets, each object is a 48-byte node plus its name, stored in fixed-size blocks: about 66 bytes per object (roughly 650 MB per 10 million objects), and full object keys are never kept. Measure it with:

```sh
cargo run --release -p clouddirstat-core --features demo --example memory -- 10000000
```

### Demo data (for screenshots)

Build the GUI with the `demo` feature to scan made-up buckets instead of AWS, so screenshots never show real data. Release builds never include it.

```sh
cargo run --release -p clouddirstat-gui --features demo -- s3://
```

`s3://` shows six fictional `acme-*` buckets in different regions, including some incomplete uploads; `s3://acme-backups/` (or any demo bucket name) shows one. Check **Versions** for noncurrent versions and delete markers. The data is generated from a fixed seed, so it looks the same every time; set `CLOUDDIRSTAT_DEMO_OBJECTS` for more or fewer objects (default 120,000).

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

Done:

- [x] S3 scanner CLI: folder tree, storage classes, versions, LIST cost
- [x] Treemap GUI: folder list, cushion-shaded treemap, color by storage class, versions, prefix, or file type
- [x] Estimated monthly storage cost per folder and object, by region
- [x] Incomplete multipart uploads
- [x] Scan all buckets at once (`s3://`)
- [x] Largest files list (per bucket when scanning all buckets)
- [x] Credentials: profiles, `aws login`, IAM Identity Center, access keys
- [x] Low memory for large buckets (~66 bytes per object)
- [x] Release builds for Windows, macOS, and Linux (x64 and ARM64)

Next:

- [ ] Click to zoom into a folder in the treemap
- [ ] Last modified column
- [ ] Instant bucket totals and scan-cost estimate from CloudWatch
- [ ] macOS app bundle and code signing
- [ ] Read S3 Inventory reports for billion-object buckets
- [ ] Azure Blob Storage
- [ ] Google Cloud Storage

## AI Disclosure

Parts of this project are developed with AI assistance (Claude). All code is reviewed, tested, and owned by the maintainer.

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
