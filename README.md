# CloudDirStat

WinDirStat for cloud object storage. Find out what is using the space (and the money) in your buckets.

CloudDirStat lists a bucket, rebuilds a folder tree from the object keys, and shows where the bytes are: by directory, by storage class, and by version state (current, noncurrent, delete markers).

- **Least privilege:** only needs `s3:ListBucket`. It never reads or writes object contents.
- **Native and small:** a single ~10 MB binary for Windows, macOS, and Linux. No runtime, no browser.
- **Fast:** lists prefixes in parallel.
- **Cost-aware:** reports how many LIST requests a scan used and what they cost.

> **Status:** early development. The command-line scanner for AWS S3 works. The treemap GUI, Azure Blob Storage, and Google Cloud Storage are next (see [Roadmap](#roadmap)).

## Usage

```sh
clouddirstat scan s3://my-bucket
clouddirstat scan s3://my-bucket/logs/ --profile prod --depth 3
clouddirstat scan s3://my-bucket --versions
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

By storage class
  STANDARD                  6.8 GiB  100.0%         1,024 objects

By version state
  Current versions          6.8 GiB  100.0%         1,024 objects
  Noncurrent versions           0 B    0.0%             0 objects
  Delete markers                0 B    0.0%             0 objects

Largest directories
     6.8 GiB  100.0%  /
     6.6 GiB   98.3%    backups/
    11.2 MiB    0.2%      db-snapshot-0412.tar.gz
     6.6 GiB   97.5%      ... 1,016 more
    83.5 MiB    1.2%    archive.zip

Largest objects
    83.5 MiB  archive.zip
    11.2 MiB  backups/db-snapshot-0412.tar.gz
```

### Credentials

CloudDirStat uses the standard AWS credential chain, the same as the AWS CLI: environment variables, `~/.aws/credentials`, `~/.aws/config` profiles (including SSO), and EC2/ECS/EKS roles.

### Cost

S3 charges for LIST requests (about $0.005 per 1,000 in most regions). Each request returns up to 1,000 objects, so scanning 10 million objects costs roughly $0.05.

## Permissions

The minimum IAM policy is in [`docs/iam-policy.json`](docs/iam-policy.json).

| Permission | Required | Used for |
|---|---|---|
| `s3:ListBucket` | yes | Listing objects and detecting the bucket region |
| `s3:ListBucketVersions` | only with `--versions` | Noncurrent versions and delete markers |

CloudDirStat never calls `GetObject`, `PutObject`, or `DeleteObject`.

## Building from source

1. Install Rust with [rustup](https://rustup.rs). On Windows you also need the Visual Studio C++ Build Tools.
2. Build:

```sh
cargo build --release
```

The binary is `target/release/clouddirstat` (`clouddirstat.exe` on Windows).

Development commands:

```sh
cargo run -p clouddirstat -- scan s3://my-bucket
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

## Roadmap

- [x] S3 scanner CLI: directory tree, storage classes, versions, LIST cost
- [ ] Treemap GUI (egui)
- [ ] Incomplete multipart uploads
- [ ] Estimated monthly storage cost per directory
- [ ] Instant bucket totals from CloudWatch
- [ ] Read S3 Inventory reports for billion-object buckets
- [ ] Azure Blob Storage
- [ ] Google Cloud Storage

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
