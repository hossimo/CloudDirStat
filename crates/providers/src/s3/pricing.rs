use std::collections::HashMap;

use clouddirstat_core::{Cost, Entry, EntryKind, Pricing};

use super::prices::{PUBLISHED, REGIONS};

const FALLBACK_REGION: &str = "us-east-1";
const BYTES_PER_GB: f64 = 1024.0 * 1024.0 * 1024.0;
/// IA, One Zone-IA and Glacier Instant Retrieval bill small objects as this size.
const MINIMUM_BILLABLE_SIZE: u64 = 128 * 1024;
/// Intelligent-Tiering does not monitor (or charge monitoring for) smaller objects.
const MONITORED_SIZE: u64 = 128 * 1024;
/// Glacier Flexible Retrieval and Deep Archive add per-object metadata: 32 KB billed at
/// the archive rate and 8 KB at the Standard rate.
const ARCHIVE_OVERHEAD: u64 = 32 * 1024;
const ARCHIVE_INDEX_OVERHEAD: u64 = 8 * 1024;

/// Intelligent-Tiering's lower tiers. Listings only say INTELLIGENT_TIERING (priced at
/// Frequent Access), but CloudWatch reports bytes per tier, so estimates price each one.
pub(super) const INT_INFREQUENT: &str = "INTELLIGENT_TIERING:IA";
pub(super) const INT_ARCHIVE_INSTANT: &str = "INTELLIGENT_TIERING:AIA";
pub(super) const INT_ARCHIVE: &str = "INTELLIGENT_TIERING:AA";
pub(super) const INT_DEEP_ARCHIVE: &str = "INTELLIGENT_TIERING:DAA";

/// List prices for one region, as generated into `prices.rs`.
#[derive(Debug)]
pub struct RegionPrices {
    pub region: &'static str,
    pub standard: Option<f64>,
    pub intelligent_tiering: Option<f64>,
    pub int_infrequent: Option<f64>,
    pub int_archive_instant: Option<f64>,
    pub int_archive: Option<f64>,
    pub int_deep_archive: Option<f64>,
    pub standard_ia: Option<f64>,
    pub onezone_ia: Option<f64>,
    pub glacier_ir: Option<f64>,
    pub glacier: Option<f64>,
    pub reduced_redundancy: Option<f64>,
    pub express_onezone: Option<f64>,
    pub int_monitoring_per_object: Option<f64>,
    pub deep_archive: Option<f64>,
}

/// Estimated S3 storage cost per month from public list prices.
///
/// Uses the first volume tier and prices Intelligent-Tiering at its Frequent Access
/// tier, so it leans high. Requests, retrieval, transfer, and minimum-duration charges
/// are not included.
#[derive(Debug)]
pub struct S3Pricing {
    default: &'static RegionPrices,
    fallback: &'static RegionPrices,
    /// For all-buckets scans, whose keys start with `bucket/`: each bucket's prices.
    by_bucket: HashMap<String, &'static RegionPrices>,
}

impl S3Pricing {
    /// Prices for `region`, or us-east-1 prices when the region is not in the table.
    pub fn for_region(region: &str) -> Self {
        let fallback = fallback();
        Self {
            default: find(region).unwrap_or(fallback),
            fallback,
            by_bucket: HashMap::new(),
        }
    }

    /// Prices for an all-buckets scan: each `(bucket, region)` at its region's prices.
    pub fn for_buckets<'a>(buckets: impl IntoIterator<Item = (&'a str, &'a str)>) -> Self {
        let fallback = fallback();
        Self {
            default: fallback,
            fallback,
            by_bucket: buckets
                .into_iter()
                .map(|(bucket, region)| (bucket.to_owned(), find(region).unwrap_or(fallback)))
                .collect(),
        }
    }

    /// Where the prices come from, for display: a region, or "each bucket's region".
    pub fn region_label(&self) -> String {
        if self.by_bucket.is_empty() {
            self.default.region.to_owned()
        } else {
            "each bucket's region".to_owned()
        }
    }

    /// Date of the AWS price list the table was generated from.
    pub fn published() -> &'static str {
        PUBLISHED
    }

    fn prices_for(&self, key: &str) -> &'static RegionPrices {
        if self.by_bucket.is_empty() {
            return self.default;
        }
        let bucket = key.split_once('/').map_or(key, |(bucket, _)| bucket);
        self.by_bucket.get(bucket).copied().unwrap_or(self.default)
    }

    fn price(&self, prices: &RegionPrices, field: fn(&RegionPrices) -> Option<f64>) -> f64 {
        field(prices)
            .or_else(|| field(self.fallback))
            .unwrap_or(0.0)
    }

    /// The per-GB-month list price of `class` (Standard for unknown classes).
    fn rate(&self, prices: &RegionPrices, class: &str) -> f64 {
        let field: fn(&RegionPrices) -> Option<f64> = match class {
            "STANDARD_IA" => |p| p.standard_ia,
            "ONEZONE_IA" => |p| p.onezone_ia,
            "GLACIER_IR" => |p| p.glacier_ir,
            "INTELLIGENT_TIERING" => |p| p.intelligent_tiering,
            INT_INFREQUENT => |p| p.int_infrequent,
            INT_ARCHIVE_INSTANT => |p| p.int_archive_instant,
            INT_ARCHIVE => |p| p.int_archive,
            INT_DEEP_ARCHIVE => |p| p.int_deep_archive,
            "GLACIER" => |p| p.glacier,
            "DEEP_ARCHIVE" => |p| p.deep_archive,
            "REDUCED_REDUNDANCY" => |p| p.reduced_redundancy,
            "EXPRESS_ONEZONE" => |p| p.express_onezone,
            _ => |p| p.standard,
        };
        self.price(prices, field)
    }

    /// Cost of `bytes` stored as `class` at list price, with no minimum sizes, per-object
    /// overheads, or monitoring fees. For totals that already include overheads, like
    /// CloudWatch's.
    pub fn bytes_cost(&self, class: &str, bytes: u64) -> Cost {
        Cost::from_usd(bytes as f64 / BYTES_PER_GB * self.rate(self.default, class))
    }
}

impl Pricing for S3Pricing {
    fn source(&self) -> String {
        format!("{} list prices from {}", self.region_label(), PUBLISHED)
    }

    fn notes(&self) -> String {
        format!(
            "Estimated storage cost from {} list prices (AWS Price List, {PUBLISHED}).\n\
             First volume tier; Intelligent-Tiering at Frequent Access rates.\n\
             Includes minimum billable sizes and archive overhead; excludes requests, \
             retrieval, data transfer, and minimum storage duration charges.",
            self.region_label()
        )
    }

    fn monthly_cost(&self, entry: &Entry) -> Cost {
        if entry.kind == EntryKind::DeleteMarker {
            return Cost::ZERO;
        }
        let size = entry.size;
        let prices = self.prices_for(&entry.key);
        let class = entry.storage_class.as_str();
        let rate = self.rate(prices, class);
        let per_gb = |bytes: u64| bytes as f64 / BYTES_PER_GB * rate;

        let usd = match class {
            "STANDARD_IA" | "ONEZONE_IA" | "GLACIER_IR" => per_gb(size.max(MINIMUM_BILLABLE_SIZE)),
            "INTELLIGENT_TIERING" if size >= MONITORED_SIZE => {
                per_gb(size) + self.price(prices, |p| p.int_monitoring_per_object)
            }
            "GLACIER" | "DEEP_ARCHIVE" => archive(size, rate, self.rate(prices, "STANDARD")),
            _ => per_gb(size),
        };
        Cost::from_usd(usd)
    }
}

fn archive(size: u64, archive_price: f64, standard_price: f64) -> f64 {
    let gb = |bytes: u64| bytes as f64 / BYTES_PER_GB;
    gb(size + ARCHIVE_OVERHEAD) * archive_price + gb(ARCHIVE_INDEX_OVERHEAD) * standard_price
}

fn fallback() -> &'static RegionPrices {
    find(FALLBACK_REGION).unwrap_or(&REGIONS[0])
}

fn find(region: &str) -> Option<&'static RegionPrices> {
    REGIONS.iter().find(|prices| prices.region == region)
}

#[cfg(test)]
mod tests {
    use super::*;

    const GB: u64 = 1024 * 1024 * 1024;

    fn cost(pricing: &S3Pricing, class: &str, size: u64) -> f64 {
        pricing
            .monthly_cost(&Entry {
                key: "k".to_owned(),
                size,
                storage_class: class.to_owned(),
                kind: EntryKind::Current,
                last_modified: None,
            })
            .usd()
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn prices_standard_by_the_gigabyte() {
        let pricing = S3Pricing::for_region("us-east-1");
        let standard = pricing.price(pricing.default, |p| p.standard);
        assert!(standard > 0.0);
        assert!(close(cost(&pricing, "STANDARD", 10 * GB), 10.0 * standard));
    }

    #[test]
    fn small_infrequent_access_objects_bill_as_128_kib() {
        let pricing = S3Pricing::for_region("us-east-1");
        assert!(close(
            cost(&pricing, "STANDARD_IA", 1),
            cost(&pricing, "STANDARD_IA", MINIMUM_BILLABLE_SIZE)
        ));
    }

    #[test]
    fn archive_classes_add_per_object_overhead() {
        let pricing = S3Pricing::for_region("us-east-1");
        assert!(cost(&pricing, "DEEP_ARCHIVE", 0) > 0.0);
        assert!(cost(&pricing, "GLACIER", 0) > 0.0);
    }

    #[test]
    fn delete_markers_are_free() {
        let pricing = S3Pricing::for_region("us-east-1");
        let marker = Entry {
            key: "k".to_owned(),
            size: 0,
            storage_class: "STANDARD".to_owned(),
            kind: EntryKind::DeleteMarker,
            last_modified: None,
        };
        assert_eq!(pricing.monthly_cost(&marker), Cost::ZERO);
    }

    #[test]
    fn unknown_regions_fall_back_to_us_east_1() {
        let pricing = S3Pricing::for_region("xx-nowhere-1");
        assert_eq!(pricing.region_label(), "us-east-1");
        assert!(close(
            cost(&pricing, "STANDARD", GB),
            cost(&S3Pricing::for_region("us-east-1"), "STANDARD", GB)
        ));
    }

    #[test]
    fn prices_each_bucket_at_its_own_region() {
        let pricing = S3Pricing::for_buckets([("east", "us-east-1"), ("brazil", "sa-east-1")]);
        let key_cost = |key: &str| {
            pricing
                .monthly_cost(&Entry {
                    key: key.to_owned(),
                    size: GB,
                    storage_class: "STANDARD".to_owned(),
                    kind: EntryKind::Current,
                    last_modified: None,
                })
                .usd()
        };

        let virginia = cost(&S3Pricing::for_region("us-east-1"), "STANDARD", GB);
        let sao_paulo = cost(&S3Pricing::for_region("sa-east-1"), "STANDARD", GB);
        assert!(close(key_cost("east/a.bin"), virginia));
        assert!(close(key_cost("brazil/a.bin"), sao_paulo));
        assert!(close(key_cost("unknown/a.bin"), virginia));
        assert_eq!(pricing.region_label(), "each bucket's region");
    }

    #[test]
    fn regional_prices_differ() {
        let virginia = cost(&S3Pricing::for_region("us-east-1"), "STANDARD", GB);
        let sao_paulo = cost(&S3Pricing::for_region("sa-east-1"), "STANDARD", GB);
        assert!(sao_paulo > virginia);
    }
}
