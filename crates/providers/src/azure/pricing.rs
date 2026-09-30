use std::collections::HashMap;

use clouddirstat_core::{Cost, Entry, Pricing};

use super::prices::{PRICES, PUBLISHED};

const FALLBACK_REGION: &str = "eastus";
const FALLBACK_REDUNDANCY: &str = "LRS";
const BYTES_PER_GB: f64 = 1024.0 * 1024.0 * 1024.0;

/// List prices for one region and redundancy, as generated into `prices.rs`.
#[derive(Debug)]
pub struct RedundancyPrices {
    pub region: &'static str,
    pub redundancy: &'static str,
    pub hot: Option<f64>,
    pub cool: Option<f64>,
    pub cold: Option<f64>,
    pub archive: Option<f64>,
    pub list_per_10k: Option<f64>,
}

/// Where a storage account keeps its data, as Azure Resource Manager reports it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AccountPlacement {
    /// `eastus`, `westeurope`, ...
    pub region: String,
    /// The account's SKU: `Standard_LRS`, `Standard_RAGRS`, ...
    pub sku: String,
}

impl AccountPlacement {
    /// `LRS`, `RA-GRS`, ... as the price list names them.
    fn redundancy(&self) -> &'static str {
        match self.sku.as_str() {
            "Standard_ZRS" | "Premium_ZRS" => "ZRS",
            "Standard_GRS" => "GRS",
            "Standard_RAGRS" => "RA-GRS",
            "Standard_GZRS" => "GZRS",
            "Standard_RAGZRS" => "RA-GZRS",
            _ => FALLBACK_REDUNDANCY,
        }
    }

    fn prices(&self) -> Option<&'static RedundancyPrices> {
        let redundancy = self.redundancy();
        PRICES
            .iter()
            .find(|prices| prices.region == self.region && prices.redundancy == redundancy)
    }

    fn label(&self) -> String {
        if self.region.is_empty() {
            format!("{FALLBACK_REGION} {FALLBACK_REDUNDANCY}")
        } else {
            format!("{} {}", self.region, self.redundancy())
        }
    }
}

/// Estimated Blob Storage cost per month from public list prices (first volume tier).
/// Operations, retrieval, early deletion, and data transfer are not included.
#[derive(Debug)]
pub struct AzurePricing {
    default: &'static RedundancyPrices,
    default_label: String,
    fallback: &'static RedundancyPrices,
    /// For all-accounts scans, whose keys start with `account/`: each account's prices.
    by_account: HashMap<String, &'static RedundancyPrices>,
}

impl AzurePricing {
    /// Prices for one account. An unknown placement (e.g. without access to Azure
    /// Resource Manager) uses eastus LRS prices.
    pub fn for_account(placement: &AccountPlacement) -> Self {
        let fallback = fallback();
        Self {
            default: placement.prices().unwrap_or(fallback),
            default_label: placement.label(),
            fallback,
            by_account: HashMap::new(),
        }
    }

    /// Prices for an all-accounts scan: each account at its own region and redundancy.
    pub fn for_accounts<'a>(
        accounts: impl IntoIterator<Item = (&'a str, &'a AccountPlacement)>,
    ) -> Self {
        let fallback = fallback();
        Self {
            default: fallback,
            default_label: "each account's region and redundancy".to_owned(),
            fallback,
            by_account: accounts
                .into_iter()
                .map(|(account, placement)| {
                    (account.to_owned(), placement.prices().unwrap_or(fallback))
                })
                .collect(),
        }
    }

    /// What listing costs per 1,000 requests in an account.
    pub fn list_price_per_1000(placement: &AccountPlacement) -> f64 {
        let prices = placement.prices().unwrap_or_else(fallback);
        prices
            .list_per_10k
            .or(fallback().list_per_10k)
            .unwrap_or(0.0)
            / 10.0
    }

    fn prices_for(&self, key: &str) -> &'static RedundancyPrices {
        if self.by_account.is_empty() {
            return self.default;
        }
        let account = key.split_once('/').map_or(key, |(account, _)| account);
        self.by_account
            .get(account)
            .copied()
            .unwrap_or(self.default)
    }

    fn rate(&self, prices: &RedundancyPrices, tier: &str) -> f64 {
        let field: fn(&RedundancyPrices) -> Option<f64> = match tier {
            "Cool" => |p| p.cool,
            "Cold" => |p| p.cold,
            "Archive" => |p| p.archive,
            _ => |p| p.hot,
        };
        field(prices)
            .or_else(|| field(self.fallback))
            .unwrap_or(0.0)
    }
}

impl Pricing for AzurePricing {
    fn monthly_cost(&self, entry: &Entry) -> Cost {
        let rate = self.rate(self.prices_for(&entry.key), &entry.storage_class);
        Cost::from_usd(entry.size as f64 / BYTES_PER_GB * rate)
    }

    fn source(&self) -> String {
        format!("{} list prices from {PUBLISHED}", self.default_label)
    }

    fn notes(&self) -> String {
        format!(
            "Estimated storage cost from Azure Blob Storage list prices ({}, {PUBLISHED}).
             First volume tier. Excludes operations, retrieval, data transfer, and early
             deletion charges (30, 90, and 180 days for Cool, Cold, and Archive).",
            self.default_label
        )
    }
}

fn fallback() -> &'static RedundancyPrices {
    PRICES
        .iter()
        .find(|prices| prices.region == FALLBACK_REGION && prices.redundancy == FALLBACK_REDUNDANCY)
        .unwrap_or(&PRICES[0])
}

#[cfg(test)]
mod tests {
    use clouddirstat_core::EntryKind;

    use super::*;

    fn placement(region: &str, sku: &str) -> AccountPlacement {
        AccountPlacement {
            region: region.to_owned(),
            sku: sku.to_owned(),
        }
    }

    fn cost(pricing: &AzurePricing, key: &str, tier: &str) -> f64 {
        pricing
            .monthly_cost(&Entry {
                key: key.to_owned(),
                size: 1024 * 1024 * 1024,
                storage_class: tier.to_owned(),
                kind: EntryKind::Current,
                last_modified: None,
            })
            .usd()
    }

    #[test]
    fn colder_tiers_and_less_redundancy_cost_less() {
        let lrs = AzurePricing::for_account(&placement("eastus", "Standard_LRS"));
        let ragrs = AzurePricing::for_account(&placement("eastus", "Standard_RAGRS"));
        assert!(cost(&lrs, "a", "Hot") < cost(&ragrs, "a", "Hot"));
        assert!(cost(&lrs, "a", "Archive") < cost(&lrs, "a", "Cold"));
        assert!(cost(&lrs, "a", "Cold") < cost(&lrs, "a", "Cool"));
        assert!(cost(&lrs, "a", "Cool") < cost(&lrs, "a", "Hot"));
        assert!(lrs.source().starts_with("eastus LRS"));
    }

    #[test]
    fn prices_each_account_in_an_all_accounts_scan() {
        let east = placement("eastus", "Standard_LRS");
        let geo = placement("eastus", "Standard_RAGRS");
        let pricing = AzurePricing::for_accounts([("east", &east), ("geo", &geo)]);
        assert!(cost(&pricing, "east/c/a", "Hot") < cost(&pricing, "geo/c/a", "Hot"));
    }

    #[test]
    fn unknown_accounts_use_eastus_lrs() {
        let unknown = AzurePricing::for_account(&AccountPlacement::default());
        let eastus = AzurePricing::for_account(&placement("eastus", "Standard_LRS"));
        assert_eq!(cost(&unknown, "a", "Hot"), cost(&eastus, "a", "Hot"));
        assert!(AzurePricing::list_price_per_1000(&AccountPlacement::default()) > 0.0);
    }
}
