use std::collections::HashMap;

use clouddirstat_core::{Cost, Entry, Pricing};

use super::prices::{CLASS_A_PER_1000, MULTI_REGIONS, PUBLISHED, REGIONS};

const FALLBACK_REGION: &str = "us-central1";
const BYTES_PER_GIB: f64 = 1024.0 * 1024.0 * 1024.0;

/// List prices for one location, as generated into `prices.rs`.
#[derive(Debug)]
pub struct LocationPrices {
    pub location: &'static str,
    pub standard: Option<f64>,
    pub nearline: Option<f64>,
    pub coldline: Option<f64>,
    pub archive: Option<f64>,
}

/// A price for each storage class.
#[derive(Debug)]
pub struct ClassPrices {
    pub standard: f64,
    pub nearline: f64,
    pub coldline: f64,
    pub archive: f64,
}

impl ClassPrices {
    fn for_class(&self, class: &str) -> f64 {
        match class {
            "NEARLINE" => self.nearline,
            "COLDLINE" => self.coldline,
            "ARCHIVE" => self.archive,
            _ => self.standard,
        }
    }
}

/// Where a bucket keeps its data, as its metadata reports it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BucketPlacement {
    /// `US-CENTRAL1`, `US`, `NAM4`, ... (any case).
    pub location: String,
    /// `region`, `dual-region`, or `multi-region`.
    pub location_type: String,
    /// The regions of a configurable dual-region.
    pub data_locations: Vec<String>,
}

impl BucketPlacement {
    fn prices(&self) -> Option<&'static LocationPrices> {
        let find = |table: &'static [LocationPrices], location: &str| {
            let location = location.to_ascii_lowercase();
            table.iter().find(|prices| prices.location == location)
        };
        match self.location_type.as_str() {
            "region" => find(REGIONS, &self.location),
            // Configurable dual-regions (location "US" or "EU" plus the regions they
            // use) are priced by their regions; predefined dual-regions and
            // multi-regions have prices of their own.
            _ => match self.data_locations.first() {
                Some(region) => find(MULTI_REGIONS, region),
                None => find(MULTI_REGIONS, &self.location),
            },
        }
    }

    fn label(&self) -> String {
        match self.location_type.as_str() {
            "" => FALLBACK_REGION.to_owned(),
            kind => format!("{} {kind}", self.location.to_ascii_lowercase()),
        }
    }
}

/// Estimated Cloud Storage cost per month from public list prices. Minimum storage
/// durations, operations, retrieval, and network charges are not included.
#[derive(Debug)]
pub struct GcsPricing {
    default: &'static LocationPrices,
    default_label: String,
    fallback: &'static LocationPrices,
    /// For all-buckets scans, whose keys start with `bucket/`: each bucket's prices.
    by_bucket: HashMap<String, &'static LocationPrices>,
}

impl GcsPricing {
    /// Prices for one bucket. Unknown placements (e.g. without permission to read the
    /// bucket's metadata) use us-central1 prices.
    pub fn for_bucket(placement: &BucketPlacement) -> Self {
        let fallback = fallback();
        Self {
            default: placement.prices().unwrap_or(fallback),
            default_label: placement.label(),
            fallback,
            by_bucket: HashMap::new(),
        }
    }

    /// Prices for an all-buckets scan: each bucket at its own location's prices.
    pub fn for_buckets<'a>(
        buckets: impl IntoIterator<Item = (&'a str, &'a BucketPlacement)>,
    ) -> Self {
        let fallback = fallback();
        Self {
            default: fallback,
            default_label: "each bucket's location".to_owned(),
            fallback,
            by_bucket: buckets
                .into_iter()
                .map(|(bucket, placement)| {
                    (bucket.to_owned(), placement.prices().unwrap_or(fallback))
                })
                .collect(),
        }
    }

    /// What Cloud Storage charges per 1,000 object listings in a bucket whose default
    /// storage class is `class`.
    pub fn list_price_per_1000(class: &str) -> f64 {
        CLASS_A_PER_1000.for_class(class)
    }

    fn prices_for(&self, key: &str) -> &'static LocationPrices {
        if self.by_bucket.is_empty() {
            return self.default;
        }
        let bucket = key.split_once('/').map_or(key, |(bucket, _)| bucket);
        self.by_bucket.get(bucket).copied().unwrap_or(self.default)
    }

    fn rate(&self, prices: &LocationPrices, class: &str) -> f64 {
        let field: fn(&LocationPrices) -> Option<f64> = match class {
            "NEARLINE" => |p| p.nearline,
            "COLDLINE" => |p| p.coldline,
            "ARCHIVE" => |p| p.archive,
            // STANDARD, and the legacy MULTI_REGIONAL, REGIONAL, and
            // DURABLE_REDUCED_AVAILABILITY classes, which bill like Standard.
            _ => |p| p.standard,
        };
        field(prices)
            .or_else(|| field(self.fallback))
            .unwrap_or(0.0)
    }
}

impl Pricing for GcsPricing {
    fn monthly_cost(&self, entry: &Entry) -> Cost {
        let rate = self.rate(self.prices_for(&entry.key), &entry.storage_class);
        Cost::from_usd(entry.size as f64 / BYTES_PER_GIB * rate)
    }

    fn source(&self) -> String {
        format!("{} list prices from {PUBLISHED}", self.default_label)
    }

    fn notes(&self) -> String {
        format!(
            "Estimated storage cost from Cloud Storage list prices ({}, {PUBLISHED}).
             Excludes operations, retrieval, network, and minimum storage duration
             charges (30, 90, and 365 days for Nearline, Coldline, and Archive).",
            self.default_label
        )
    }
}

fn fallback() -> &'static LocationPrices {
    REGIONS
        .iter()
        .find(|prices| prices.location == FALLBACK_REGION)
        .unwrap_or(&REGIONS[0])
}

#[cfg(test)]
mod tests {
    use clouddirstat_core::EntryKind;

    use super::*;

    fn placement(location: &str, location_type: &str) -> BucketPlacement {
        BucketPlacement {
            location: location.to_owned(),
            location_type: location_type.to_owned(),
            data_locations: Vec::new(),
        }
    }

    fn cost(pricing: &GcsPricing, key: &str, class: &str) -> f64 {
        pricing
            .monthly_cost(&Entry {
                key: key.to_owned(),
                size: 1024 * 1024 * 1024,
                storage_class: class.to_owned(),
                kind: EntryKind::Current,
                last_modified: None,
            })
            .usd()
    }

    #[test]
    fn prices_regions_multi_regions_and_dual_regions() {
        let region = GcsPricing::for_bucket(&placement("US-CENTRAL1", "region"));
        let multi = GcsPricing::for_bucket(&placement("US", "multi-region"));
        let dual = GcsPricing::for_bucket(&placement("NAM4", "dual-region"));
        assert!(cost(&region, "a", "STANDARD") < cost(&multi, "a", "STANDARD"));
        assert!(cost(&multi, "a", "STANDARD") < cost(&dual, "a", "STANDARD"));
        assert!(cost(&region, "a", "ARCHIVE") < cost(&region, "a", "NEARLINE"));
        assert_eq!(
            cost(&region, "a", "REGIONAL"),
            cost(&region, "a", "STANDARD")
        );
    }

    #[test]
    fn configurable_dual_regions_use_their_first_region() {
        let custom = BucketPlacement {
            data_locations: vec!["US-CENTRAL1".to_owned(), "US-EAST1".to_owned()],
            ..placement("US", "dual-region")
        };
        assert_eq!(
            custom.prices().map(|prices| prices.location),
            Some("us-central1")
        );
    }

    #[test]
    fn unknown_placements_fall_back_to_us_central1() {
        let unknown = GcsPricing::for_bucket(&BucketPlacement::default());
        let iowa = GcsPricing::for_bucket(&placement("us-central1", "region"));
        assert_eq!(
            cost(&unknown, "a", "STANDARD"),
            cost(&iowa, "a", "STANDARD")
        );
        assert!(unknown.source().starts_with("us-central1"));
    }

    #[test]
    fn listing_costs_more_in_colder_default_classes() {
        assert!(
            GcsPricing::list_price_per_1000("STANDARD")
                < GcsPricing::list_price_per_1000("ARCHIVE")
        );
    }
}
