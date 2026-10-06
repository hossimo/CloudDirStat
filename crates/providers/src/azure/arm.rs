//! Azure Resource Manager: the storage accounts the signed-in user can see, with their
//! regions and redundancy. Only the Azure CLI signs in to it.

use std::sync::atomic::Ordering;

use futures_util::{StreamExt, stream};
use serde::Deserialize;

use super::auth::MANAGEMENT;
use super::location::is_account_name;
use super::{Account, AccountPlacement, AzureScanner, LOOKUPS_AT_ONCE};
use crate::http::{Response, encode};
use crate::{Error, Result};

const ARM: &str = "https://management.azure.com";

#[derive(Deserialize)]
struct ArmList<T> {
    #[serde(default = "Vec::new")]
    value: Vec<T>,
    #[serde(rename = "nextLink")]
    next_link: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Subscription {
    subscription_id: String,
}

#[derive(Deserialize)]
struct StorageAccount {
    name: String,
    #[serde(default)]
    location: String,
    sku: Option<Sku>,
}

#[derive(Deserialize)]
struct Sku {
    name: String,
}

impl AzureScanner {
    /// Every storage account in every subscription the user can read (needs the Reader
    /// role, or any role with Microsoft.Storage/storageAccounts/read). A subscription
    /// that can't be read is warned about and left out; if none can, that's an error.
    pub(super) async fn resource_manager_accounts(&mut self) -> Result<Vec<Account>> {
        let subscriptions: Vec<Subscription> = self
            .arm_list(format!("{ARM}/subscriptions?api-version=2022-12-01"))
            .await?;
        let this = &*self;
        let lookups: Vec<(String, Result<Vec<StorageAccount>>)> = stream::iter(subscriptions)
            .map(|subscription| async move {
                let url = format!(
                    "{ARM}/subscriptions/{}/providers/Microsoft.Storage/storageAccounts?api-version=2023-05-01",
                    encode(&subscription.subscription_id)
                );
                (subscription.subscription_id, this.arm_list(url).await)
            })
            .buffered(LOOKUPS_AT_ONCE)
            .collect()
            .await;

        let mut accounts = Vec::new();
        let mut warnings = Vec::new();
        let mut first_error = None;
        for (subscription, found) in lookups {
            match found {
                Ok(found) => accounts.extend(
                    found
                        .into_iter()
                        .filter(|account| is_account_name(&account.name))
                        .map(|account| Account {
                            name: account.name,
                            placement: AccountPlacement {
                                region: account.location,
                                sku: account.sku.map(|sku| sku.name).unwrap_or_default(),
                            },
                        }),
                ),
                Err(error) => {
                    warnings.push(format!(
                        "subscription {subscription}: storage accounts not listed. {error}"
                    ));
                    first_error.get_or_insert(error);
                }
            }
        }
        if accounts.is_empty()
            && let Some(error) = first_error
        {
            return Err(error);
        }
        self.warnings.extend(warnings);
        Ok(accounts)
    }

    async fn arm_list<T: for<'de> Deserialize<'de>>(&self, url: String) -> Result<Vec<T>> {
        let mut items = Vec::new();
        let mut next = Some(url);
        while let Some(url) = next {
            let token = self.tokens.token(MANAGEMENT).await?;
            let authorization = format!("Bearer {token}");
            let response = self
                .http
                .get(&url, &[("authorization", &authorization)])
                .await?;
            self.setup_requests.fetch_add(1, Ordering::Relaxed);
            if !response.status.is_success() {
                return Err(Error::Request {
                    operation: "List storage accounts",
                    permission: "the Reader role",
                    message: arm_error(&response),
                });
            }
            let page: ArmList<T> = serde_json::from_slice(&response.body).map_err(|error| {
                Error::Network(format!(
                    "unexpected Azure Resource Manager response: {error}"
                ))
            })?;
            items.extend(page.value);
            next = page.next_link.map(arm_link).transpose()?;
        }
        Ok(items)
    }
}

/// A next-page link from Resource Manager, which must stay on Resource Manager: the
/// request that follows it carries the management token.
fn arm_link(link: String) -> Result<String> {
    if link.starts_with(&format!("{ARM}/")) {
        Ok(link)
    } else {
        Err(Error::Network(format!(
            "Azure Resource Manager sent a next page link to another host ({})",
            crate::http::host(&link)
        )))
    }
}

/// The message of an Azure Resource Manager error (`{"error": {"message": ...}}`).
fn arm_error(response: &Response) -> String {
    #[derive(Deserialize)]
    struct Body {
        error: Detail,
    }
    #[derive(Deserialize)]
    struct Detail {
        code: String,
        message: String,
    }
    match serde_json::from_slice::<Body>(&response.body) {
        Ok(body) => format!("{}: {}", body.error.code, body.error.message),
        Err(_) => format!("HTTP {}", response.status),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_page_links_stay_on_resource_manager() {
        let link = format!("{ARM}/subscriptions?api-version=2022-12-01&$skiptoken=x");
        assert_eq!(arm_link(link.clone()).unwrap(), link);
        for other in [
            "https://evil.example/subscriptions",
            "https://management.azure.com.evil.example/x",
            "http://management.azure.com/x",
        ] {
            assert!(arm_link(other.to_owned()).is_err(), "{other}");
        }
    }
}
