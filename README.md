<img src="icons/png/icon-128.png" alt="CloudDirStat icon" width="96" align="right">

# CloudDirStat

[![CI](https://github.com/hossimo/CloudDirStat/actions/workflows/ci.yml/badge.svg)](https://github.com/hossimo/CloudDirStat/actions/workflows/ci.yml)

**CloudDirStat is like the amazing [WinDirStat](https://windirstat.net/) (not affiliated) for cloud storage.** See what is using the space, and the money, in your Amazon S3, Google Cloud Storage, and Azure Blob Storage buckets.

CloudDirStat lists a bucket (or all of them), rebuilds the folder tree from the object names, and shows it as a sortable folder list and a treemap. Every folder and file shows its size and an estimated monthly cost. It breaks usage down by storage class, file type, and version, and finds storage that costs money but hides from normal listings: old versions, deleted files that are still billed, and abandoned uploads.

![CloudDirStat scanning six demo buckets: folder list, file types legend, and treemap colored by file type](screenshots/demo-1.png)

*A scan of made-up demo buckets.*

## Status

| Provider | Status |
|---|---|
| **Amazon S3** | **Well tested** on real buckets. |
| **Google Cloud Storage** | **New; needs more testing.** Works with Google login, service accounts, and access tokens, but has only been tested on a small test bucket. |
| **Azure Blob Storage** | **New; needs more testing.** Works with the Azure CLI, SAS tokens, and account keys, but has only been tested on a small test account. |

The project is in early development. If a Google or Azure scan fails or shows wrong numbers, please [open an issue](https://github.com/hossimo/CloudDirStat/issues). This is a side project made possible with Claude. It's been something that I needed for years but until now I could not make a reality. 

## Safe by design

- **Read-only.** CloudDirStat never adds, changes, or deletes anything in your buckets, and never reads the contents of your files. It only lists them.
- **Least privilege.** A scan needs nothing more than permission to list. Optional permissions add features; when one is missing, that feature is skipped with a warning.
- **Your keys stay yours.** Keys and tokens typed into the app are kept in memory only: never saved to disk, logged, or sent anywhere but the provider. There is no telemetry.

## Features

- **Folder list and treemap**, sorted by size, with share of the parent folder, cost per month, object count, and last modified date.
- **Largest files** list, per bucket when scanning several.
- **Color and filter** by storage class, version state, top-level folder, or file type.
- **Monthly cost estimates** from each bucket's own regional list prices.
- **Hidden costs:** old versions, deleted files whose old versions are still billed, and incomplete multipart uploads (S3).
- **Every bucket at once:** `s3://`, `gs://`, or `az://` scans them all into one tree.
- **Scan cost up front:** reports what the listing itself cost; for S3, **Estimate** shows sizes and costs in seconds without listing anything.
- **Small and fast:** one native app per platform, no runtime or browser. Lists in parallel and uses about 66 bytes of memory per object (roughly 650 MB for 10 million objects).
- **Written in Rust:** this one is only possible for me due to using AI to build the project. I'm still learning Rust and it normally not a language I would reach for but I wanted to make this future proof and as safe as I could.

## Quick start

1. **Download** the latest [release](https://github.com/hossimo/CloudDirStat/releases) for your system (Windows, macOS, or Linux; x64 or ARM64) and unzip it. It contains two programs: `clouddirstat-gui` (the app) and `clouddirstat` (the command line).
2. **Sign in** to your cloud with its own command-line tool. This is a one-time step; see [Connect to your cloud](#connect-to-your-cloud) for details and for key-based sign-in.

   | Cloud | Sign in once with |
   |---|---|
   | AWS | `aws login` or `aws sso login` |
   | Google Cloud | `gcloud auth application-default login` |
   | Azure | `az login` |

3. **Scan.** Start `clouddirstat-gui`, choose **S3**, **Google**, or **Azure**, type the bucket (for example `my-bucket`), and press **Scan**. Or from a terminal:

   ```sh
   clouddirstat scan s3://my-bucket
   clouddirstat scan gs://my-bucket
   clouddirstat scan az://mystorageaccount/mycontainer
   ```

The binaries are not code-signed yet. On macOS, clear the download quarantine before the first run with `xattr -d com.apple.quarantine clouddirstat clouddirstat-gui`. On Windows, SmartScreen may warn about an unrecognized app; choose **More info → Run anyway**.

## Locations

A location says what to scan. Everything after the bucket (or container) is an optional folder prefix.

| Location | Scans |
|---|---|
| `s3://my-bucket` | One S3 bucket |
| `s3://my-bucket/logs/2024/` | One folder of a bucket |
| `s3://` | Every bucket your AWS account can see |
| `gs://my-bucket` | One Google Cloud Storage bucket |
| `gs://` | Every bucket of one Google Cloud project |
| `az://mystorageaccount/mycontainer` | One Azure container |
| `az://mystorageaccount` | Every container of a storage account |
| `az://` | Every storage account you can see (Azure CLI sign-in only) |

In the app you don't need to type the `s3://`, `gs://`, or `az://` part: the **S3**, **Google**, and **Azure** buttons next to **Location** put it in for you, and switching between them keeps the rest of the location. Typing or pasting a full location selects the matching button, and an unknown scheme outlines the field in red. A location without a scheme is taken as S3. When scanning several buckets, each one appears as a top-level folder and is priced at its own region's rates; buckets you can't list are skipped with a warning.

## Connect to your cloud

In the app, the sign-in choices to the right of **Location** change with the cloud you choose: **Profile / Access key** for `s3://`, **Google login / Access token** for `gs://`, and **Azure CLI / SAS token / Account key** for `az://`. The **Help** button repeats the essentials.

Signing in with your cloud's own login is recommended: nothing secret is typed into CloudDirStat, and sessions expire on their own. Keys and tokens work too, for when you can't use a login.

### Amazon S3 (AWS)

**Option 1: sign in with the AWS CLI (recommended)**

1. Install the [AWS CLI v2](https://aws.amazon.com/cli/).
2. Sign in, using either the AWS console login or IAM Identity Center (SSO):

   ```sh
   aws login --profile myprofile          # console sign-in
   aws configure sso                      # SSO: once, to set up the profile
   aws sso login --profile myprofile      # SSO: each time the session expires
   ```

3. Scan with that profile:
   - **App:** choose **Profile** and type `myprofile` (leave it empty for the default profile).
   - **Command line:** `clouddirstat scan s3://my-bucket --profile myprofile`

Any other profile in `~/.aws/config` or `~/.aws/credentials` works the same way, as do the standard `AWS_*` environment variables and EC2/ECS/EKS roles. CloudDirStat uses the same credential chain as the AWS CLI.

**Option 2: access keys**

- **App:** choose **Access key** and enter the access key ID and secret access key, plus the session token if the keys are temporary.
- **Command line:** set the standard environment variables, so the keys stay out of your shell history:

  ```sh
  export AWS_ACCESS_KEY_ID=...
  export AWS_SECRET_ACCESS_KEY=...
  export AWS_SESSION_TOKEN=...   # only for temporary keys
  clouddirstat scan s3://my-bucket
  ```

  (On Windows PowerShell: `$env:AWS_ACCESS_KEY_ID = "..."`, and so on.)

Create keys for an IAM user or role that has only the permissions below.

**AWS permissions**

A ready-to-use minimal IAM policy is in [`docs/iam-policy.json`](docs/iam-policy.json); the app's **Help** window fills in the bucket name for you.

| Permission | Needed | For |
|---|---|---|
| `s3:ListBucket` | **Always** | Listing objects and finding the bucket's region |
| `s3:ListBucketVersions` | With **Versions** | Old versions and delete markers |
| `s3:ListAllMyBuckets` | For `s3://` | Finding every bucket |
| `s3:ListBucketMultipartUploads` | Optional | Finding incomplete uploads |
| `s3:ListMultipartUploadParts` | Optional | Sizing incomplete uploads (resource `arn:aws:s3:::my-bucket/*`) |
| `cloudwatch:GetMetricData` | Optional | **Estimate** and the scan progress bar |

### Google Cloud Storage

**Option 1: sign in with gcloud (recommended)**

1. Install the [Google Cloud CLI](https://cloud.google.com/sdk/docs/install) (on Windows: `winget install Google.CloudSDK`).
2. Sign in once:

   ```sh
   gcloud auth application-default login
   ```

3. Scan:
   - **App:** choose **Google login**.
   - **Command line:** `clouddirstat scan gs://my-bucket`

To scan every bucket with `gs://`, CloudDirStat needs to know which project to look in. Type the project ID in the app's **Project** field or pass `--project my-project` on the command line, or set a default once with `gcloud config set project my-project`. `gcloud projects list` shows your project IDs.

**Option 2: an access token**

Tokens last about an hour.

1. Get one with `gcloud auth print-access-token` (or from the [OAuth Playground](https://developers.google.com/oauthplayground) with the scope `https://www.googleapis.com/auth/devstorage.read_only`).
2. Use it:
   - **App:** choose **Access token** and paste it.
   - **Command line:** set `GOOGLE_OAUTH_ACCESS_TOKEN` to the token.

**Option 3: a service account key file**

Set `GOOGLE_APPLICATION_CREDENTIALS` to the path of the key file, then use **Google login** in the app (or no option on the command line). CloudDirStat asks Google for read-only storage access with it. Workload identity federation and impersonated credentials are not supported yet.

**Google permissions**

The **Storage Object Viewer** role covers scanning a bucket. For the optional parts, add the permissions to a custom role.

| Permission | Needed | For |
|---|---|---|
| `storage.objects.list` | **Always** | Listing objects (in Storage Object Viewer) |
| `storage.buckets.get` | Optional | The bucket's location, for prices (otherwise us-central1 prices, with a warning) |
| `storage.buckets.list` | For `gs://` | Finding every bucket of the project |

### Azure Blob Storage

**Option 1: sign in with the Azure CLI (recommended)**

1. Install the [Azure CLI](https://learn.microsoft.com/cli/azure/install-azure-cli) (on Windows: `winget install Microsoft.AzureCLI`).
2. Sign in once:

   ```sh
   az login
   ```

3. Give yourself the **Storage Blob Data Reader** role on the storage account (or container). Being Owner or Contributor of the subscription is **not** enough to read blobs. Role changes can take a few minutes to apply.
4. Scan:
   - **App:** choose **Azure CLI**.
   - **Command line:** `clouddirstat scan az://mystorageaccount/mycontainer`

If the storage account is in a different Azure tenant from your default one, sign in to that tenant with `az login --tenant TENANT_ID`. CloudDirStat's error message names the tenant.

**Option 2: a SAS token**

A shared access signature covers one storage account (or container). Create one in the portal on the storage account's **Shared access signature** page:
- **Allowed services:** Blob.
- **Allowed resource types:** Service and Container.
- **Allowed permissions:** Read and List. Nothing more is needed.

Then use it:
- **App:** choose **SAS token** and paste it.
- **Command line:** set `AZURE_STORAGE_SAS_TOKEN` to it.

Enter the account in the location (`az://mystorageaccount/...`): a SAS can't be used with `az://` alone.

**Option 3: an account key or connection string**

Copy **key1**, **key2**, or a **connection string** from the storage account's **Access keys** page. An account key gives full control of the account; CloudDirStat only lists with it, but a SAS or the Azure CLI is safer where you can use them.

- **App:** choose **Account key** and paste it.
- **Command line:** set `AZURE_STORAGE_KEY` (a key) or `AZURE_STORAGE_CONNECTION_STRING` (a connection string).

A key covers one account, so enter it in the location. A connection string names its account, so `az://` alone scans that account. SAS connection strings (with `SharedAccessSignature=`) work in the same places.

**Which location works with which Azure sign-in**

| Sign-in | `az://account/container` | `az://account` | `az://` |
|---|---|---|---|
| Azure CLI | Yes | Yes | Yes (needs the Reader role) |
| Account key | Yes | Yes | No |
| Connection string | Yes | Yes | Yes: the account it names |
| SAS (resource types Service + Container) | Yes | Yes | No |
| SAS (resource type Container only) | Yes | No | No |

**Azure roles**

| Role | Needed | For |
|---|---|---|
| Storage Blob Data Reader | **Always** with the Azure CLI | Listing containers and blobs |
| Reader (on the subscription or account) | Optional; needed for `az://` | Finding storage accounts, and each account's region and redundancy for prices |

The region lookup always goes through the Azure CLI, even when blobs are listed with a key or SAS. Without it, costs use eastus LRS prices, with a warning.

### Keeping keys in a `.env` file

Instead of setting environment variables in every terminal, you can put them in a file named `.env` in the folder you start CloudDirStat from. Both programs read it at startup, and the app fills in its sign-in fields from it:

```sh
AZURE_STORAGE_CONNECTION_STRING=DefaultEndpointsProtocol=https;AccountName=...;AccountKey=...
GOOGLE_OAUTH_ACCESS_TOKEN=ya29....
```

A `.env` file holds secrets: keep it private, and never commit it to version control.

## Using the app

```sh
clouddirstat-gui                                   # then type a location and press Scan
clouddirstat-gui s3://my-bucket --profile prod     # or scan on startup
clouddirstat-gui s3://my-bucket --versions         # include old versions
```

- **Folders:** every folder and file, largest first, with its share of the parent folder, size, cost per month, object count, and last modified date (for a folder, its newest file; dates are UTC). It fills in while the scan runs. Click the arrow or double-click to open a folder. Very large folders show their largest 1,000 items and a **… N more** row; click it to show more.
- **Largest files:** the largest files, 50 by default (change the number at the top). When scanning several buckets, it lists the largest in each bucket. Double-click a file to find it in the folder list.
- **Treemap:** appears when the scan finishes. Each rectangle is a file, sized by bytes. Hover to see what it is, click to select it, double-click to zoom into a folder, and right-click (or the mouse back button) to zoom out. You can also right-click a folder in the list and choose **Zoom treemap here**.
- **Legend tabs:** color the treemap by **Storage classes**, **Versions**, **Prefixes** (top-level folders), or **File types**.
- **Filters:** click a legend row to show only those files everywhere, with sizes and costs recalculated. Click it again, press **Clear filter**, or press Esc to show everything.
- **Versions** (checkbox): also lists old versions. In S3 these are noncurrent versions and delete markers; in Google Cloud, noncurrent and soft-deleted objects; in Azure, previous versions, snapshots, and soft-deleted blobs. Incomplete S3 uploads are always found.
- **Estimate** (S3 only): bucket sizes, costs, and what a full scan would cost, from CloudWatch, without listing anything. See [Estimates](#s3-estimates-without-scanning).
- **Stop** ends a scan and keeps what was found so far.
- **Status bar:** the total cost per month, what the scan cost, and warnings (hover for details). While a whole S3 bucket is scanned, a progress bar compares the objects listed so far with CloudWatch's count.

## Using the command line

```sh
clouddirstat scan <location> [options]       # scan and print a report
clouddirstat estimate <location> [options]   # S3 only: totals from CloudWatch, without listing
```

Examples:

```sh
clouddirstat scan s3://my-bucket/logs/ --profile prod --depth 3
clouddirstat scan s3:// --versions
clouddirstat scan gs:// --project my-project
clouddirstat scan az://mystorageaccount
clouddirstat estimate s3://
```

| Option | Applies to | Default | What it does |
|---|---|---|---|
| `--profile <NAME>` | S3 | default profile | AWS profile to sign in with |
| `--region <REGION>` | S3 | found automatically | The bucket's region |
| `--project <ID>` | Google | gcloud's project | Project whose buckets `gs://` scans |
| `--versions` | `scan` | off | Also list old versions (see **Versions** above) |
| `--depth <N>` | `scan` | 2 | Folder levels to print |
| `--top <N>` | `scan` | 10 | Entries to print per folder and in the largest-files list |
| `--concurrency <N>` | `scan` | 32 | Maximum listing requests at once |

Keys and tokens are read from environment variables (or a [`.env` file](#keeping-keys-in-a-env-file)), never from options, so they don't end up in your shell history.

Example report:

```
s3://my-backups/  6.8 GiB in 1,024 objects
Scanned in 0.5s using 3 LIST requests (~$0.0000)
Estimated storage cost ~$0.16/month (us-east-1 list prices from 2026-09-28)

By storage class
  STANDARD                  6.8 GiB  100.0%        $0.16/mo         1,024 objects

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

## Costs

### What a scan costs

Cloud providers charge a small fee for listing requests. Each request returns up to 1,000 objects (5,000 on Azure), so scanning 10 million S3 objects costs roughly $0.05. Every scan reports how many requests it made and what they cost at list prices.

### How storage costs are estimated

The **Cost/mo** figures estimate what keeping the files stored costs per month, from each provider's public list prices for the bucket's region (and, on Azure, the account's redundancy). The prices are built into the program, so estimating needs no extra permissions or network calls; the price list's date is shown with every estimate.

- Each storage class is priced at its own rate. For S3 this includes the 128 KB minimum billed size of the Infrequent Access classes, the per-object overhead of Glacier and Deep Archive, and the Intelligent-Tiering monitoring fee.
- Old versions count when **Versions** is on; S3 delete markers are free.
- Not included: request, retrieval, data transfer, early deletion, and minimum storage duration charges, discounts, and volume tiers beyond the first. Intelligent-Tiering files are priced at the Frequent Access rate, since a listing doesn't say which tier a file is in. Treat the figures as estimates, not a bill.

### S3 estimates without scanning

S3 reports each bucket's size per storage class and object count to CloudWatch once a day. **Estimate** in the app, or `clouddirstat estimate`, reads these figures, so you can see how big a bucket is, what it costs, and what a full scan would cost before running one:

```
s3://  2.9 TiB in 4,210,332 objects (CloudWatch, 2026-09-27)
Estimated storage cost ~$41.18/month (list prices from 2026-09-28)
A full scan needs about 4,225 LIST requests (~$0.0211); this estimate cost ~$0.0016
```

- It needs the `cloudwatch:GetMetricData` permission and costs about $0.0003 per bucket.
- The figures are a day or two old and cover whole buckets, not folders. Object counts include every version, so a scan without **Versions** may list fewer.
- Intelligent-Tiering is priced per access tier here, since CloudWatch reports bytes per tier.

### Incomplete multipart uploads (S3)

When a large upload fails or is abandoned, the parts already uploaded stay in the bucket and are billed every month, but don't appear in normal listings or the S3 console. CloudDirStat finds them on every scan and shows each as an **Incomplete upload** at its path, with the total in the status bar and report. To clean them up automatically, add a lifecycle rule with `AbortIncompleteMultipartUpload` to the bucket; CloudDirStat itself never deletes anything.

## Troubleshooting

### AWS

**`could not parse profile file`.** The AWS SDK used by CloudDirStat is stricter than the AWS CLI about `~/.aws/config` and `~/.aws/credentials`. The usual cause is a setting indented directly under a profile header:

```ini
[default]
    region = us-east-1     # wrong: indented
```

Remove the leading spaces (`region = us-east-1`). Indentation is only valid for nested settings, for example under `s3 =`.

**Session expired.** Sessions from `aws login` and `aws sso login` expire. Run the login command again, then scan again.

### Google Cloud

**`no Google credentials found`.** Run `gcloud auth application-default login`, or use an access token.

**`gs:// scans every bucket of one Google Cloud project, but no project is set`.** Enter a project ID in **Project** (or `--project`), or run `gcloud config set project PROJECT_ID`. `gcloud projects list` shows your project IDs.

**`Google rejected the credentials`.** The sign-in or token has expired or been revoked. Sign in again, or get a fresh token.

### Azure

**`AuthorizationPermissionMismatch`.** You can manage the storage account but not read its data. Give yourself **Storage Blob Data Reader** on the account or container, and wait a few minutes.

**`InvalidAuthenticationInfo` / "in a different Azure tenant".** The account belongs to another tenant than the one `az login` signed in to. Run the `az login --tenant ...` command from the message.

**`AuthenticationFailed` / `Azure rejected the credentials`.** The key or SAS is for a different storage account than the one in the location, has expired, or was revoked (rotating an account key revokes every SAS signed with it).

**`AuthorizationResourceTypeMismatch`.** The SAS doesn't allow listing containers. Scan one container (`az://account/container`), or create a SAS with the resource types **Service** and **Container**.

**`the account key is not valid`.** The field holds neither a key, a connection string, nor a SAS. Check that nothing was cut off when copying.

## Building from source

1. Install Rust with [rustup](https://rustup.rs). On Windows you also need the Visual Studio C++ Build Tools.
2. Build:

   ```sh
   cargo build --release
   ```

   The programs are `target/release/clouddirstat` and `target/release/clouddirstat-gui` (with `.exe` on Windows).

Development commands:

```sh
cargo run -p clouddirstat -- scan s3://my-bucket
cargo run -p clouddirstat-gui
cargo test --workspace --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all
```

<details>
<summary>Project layout, memory use, demo data, versions, and releases</summary>

### Project layout

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

### Price tables

The built-in prices are generated from the providers' public price lists (no credentials needed):

```sh
python scripts/update_s3_prices.py
python scripts/update_gcs_prices.py
python scripts/update_azure_prices.py
```

### Demo data (for screenshots)

Build the app with the `demo` feature to scan made-up buckets instead of a real cloud, so screenshots never show real data. Release builds never include it.

```sh
cargo run --release -p clouddirstat-gui --features demo -- s3://
```

`s3://` shows six fictional `acme-*` buckets; `s3://acme-backups/` shows one. The data is the same every time; set `CLOUDDIRSTAT_DEMO_OBJECTS` for more or fewer objects (default 120,000).

### Version numbers

Versions come from git tags: after `git tag v0.2.0`, builds report `0.2.0`, and builds from later commits report `0.2.0+N` (N commits since the tag). Without a tag, the version in `Cargo.toml` is used. `--version` and the app's Help window also show the commit.

### Making a release

Create a release on GitHub with a new `vX.Y.Z` tag. Publishing it runs the **Release** workflow, which builds all six targets and attaches the archives. To rebuild the files of an existing release, run the workflow by hand from the Actions tab with that tag; with no tag it only builds, which is useful for testing.

</details>

## Roadmap

Done: S3, Google Cloud Storage, and Azure scanning; folder list and treemap with zoom; cost estimates; old versions and incomplete uploads; scanning every bucket at once; largest files; filters; last modified dates; S3 estimates from CloudWatch; release builds for six platforms.

Next:

- [ ] More testing of Google Cloud Storage and Azure on real, larger buckets
- [ ] Estimates without scanning for Google Cloud Storage and Azure
- [ ] Read S3 Inventory reports for billion-object buckets
- [ ] macOS app bundle and code signing

## AI disclosure

Parts of this project are developed with AI assistance (Claude). All code is reviewed, tested, and owned by the maintainer.

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
