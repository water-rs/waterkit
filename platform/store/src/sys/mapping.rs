//! Pure mappings between the Microsoft Store's reported shapes and the
//! crate's own types — free of `windows` crate types so the host unit tests
//! exercise them on every platform.

use crate::{Period, PeriodUnit, ProductId, ProductKind, StoreError};

/// A `StoreProduct.ProductKind` value, as the Microsoft Store reports it.
///
/// <https://learn.microsoft.com/en-us/uwp/api/windows.services.store.storeproduct.productkind>
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreProductKind {
    /// `Consumable`: a store-managed consumable; the Store tracks the
    /// user's balance.
    Consumable,
    /// `UnmanagedConsumable`: a developer-managed consumable; the app
    /// tracks the balance and reports fulfillment.
    UnmanagedConsumable,
    /// `Durable`: a durable add-on — also how a subscription add-on
    /// reports; a subscription SKU tells them apart.
    Durable,
    /// `Application`, `Game`, or a kind newer than this table.
    Other,
}

/// Parses the `StoreProduct.ProductKind` string.
pub fn store_product_kind(kind: &str) -> StoreProductKind {
    match kind {
        "Consumable" => StoreProductKind::Consumable,
        "UnmanagedConsumable" => StoreProductKind::UnmanagedConsumable,
        "Durable" => StoreProductKind::Durable,
        _ => StoreProductKind::Other,
    }
}

/// Whether the store-reported product shape matches the catalog's declared
/// kind: a consumable is either consumable kind, a non-consumable is a
/// durable without a subscription SKU, and a subscription is a durable
/// whose SKUs include a subscription.
pub const fn kind_matches(
    declared: ProductKind,
    store: StoreProductKind,
    has_subscription_sku: bool,
) -> bool {
    match declared {
        ProductKind::Consumable => matches!(
            store,
            StoreProductKind::Consumable | StoreProductKind::UnmanagedConsumable
        ),
        ProductKind::NonConsumable => {
            matches!(store, StoreProductKind::Durable) && !has_subscription_sku
        }
        ProductKind::Subscription => {
            matches!(store, StoreProductKind::Durable) && has_subscription_sku
        }
    }
}

/// The unit a `StoreSubscriptionInfo` period counts. `Minute` and `Hour`
/// exist in `StoreDurationUnit` but have no [`PeriodUnit`] counterpart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BillingUnit {
    /// Minutes.
    Minute,
    /// Hours.
    Hour,
    /// Days.
    Day,
    /// Weeks.
    Week,
    /// Months.
    Month,
    /// Years.
    Year,
}

/// A billing period from the store's count and unit; fails when the unit
/// has no [`PeriodUnit`] counterpart rather than misrepresenting it.
pub fn billing_period(count: u32, unit: BillingUnit) -> Result<Period, StoreError> {
    let unit = match unit {
        BillingUnit::Day => PeriodUnit::Day,
        BillingUnit::Week => PeriodUnit::Week,
        BillingUnit::Month => PeriodUnit::Month,
        BillingUnit::Year => PeriodUnit::Year,
        BillingUnit::Minute | BillingUnit::Hour => {
            return Err(StoreError::Platform(format!(
                "subscription period unit {unit:?} has no PeriodUnit counterpart"
            )));
        }
    };
    Ok(Period { unit, count })
}

/// The status `RequestPurchaseAsync` reports, mirroring
/// `StorePurchaseStatus`. There is no pending state.
///
/// <https://learn.microsoft.com/en-us/uwp/api/windows.services.store.storepurchasestatus>
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PurchaseStatus {
    /// The purchase succeeded.
    Succeeded,
    /// The user already owns the product.
    AlreadyPurchased,
    /// The user cancelled the flow.
    NotPurchased,
    /// A network failure.
    NetworkError,
    /// A server-side failure.
    ServerError,
}

/// What a `RequestPurchaseAsync` status resolves to, absent the purchase
/// record the caller builds on [`PurchaseVerdict::Purchased`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PurchaseVerdict {
    /// Paid; the caller builds the purchase record.
    Purchased,
    /// The user cancelled the flow.
    Cancelled,
}

/// Maps a purchase status to its verdict or error; `extended` is the
/// `ExtendedError` message when one is attached.
pub fn purchase_verdict(
    status: PurchaseStatus,
    product: &ProductId,
    extended: Option<String>,
) -> Result<PurchaseVerdict, StoreError> {
    match status {
        PurchaseStatus::Succeeded => Ok(PurchaseVerdict::Purchased),
        PurchaseStatus::NotPurchased => Ok(PurchaseVerdict::Cancelled),
        PurchaseStatus::AlreadyPurchased => Err(StoreError::AlreadyOwned(product.clone())),
        PurchaseStatus::NetworkError => {
            Err(StoreError::Network(extended.unwrap_or_else(|| {
                "the purchase failed with a network error".into()
            })))
        }
        PurchaseStatus::ServerError => {
            Err(StoreError::Platform(extended.unwrap_or_else(|| {
                "the purchase failed with a server error".into()
            })))
        }
    }
}

/// The status `ReportConsumableFulfillmentAsync` reports, mirroring
/// `StoreConsumableStatus`.
///
/// <https://learn.microsoft.com/en-us/uwp/api/windows.services.store.storeconsumablestatus>
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsumableStatus {
    /// The fulfillment was reported.
    Succeeded,
    /// The remaining balance is lower than the reported quantity.
    InsufficientQuantity,
    /// A network failure.
    NetworkError,
    /// A server-side failure.
    ServerError,
}

/// Maps a fulfillment status to its error; `extended` is the
/// `ExtendedError` message when one is attached.
pub fn fulfillment_verdict(
    status: ConsumableStatus,
    extended: Option<String>,
) -> Result<(), StoreError> {
    match status {
        ConsumableStatus::Succeeded => Ok(()),
        ConsumableStatus::NetworkError => {
            Err(StoreError::Network(extended.unwrap_or_else(|| {
                "the fulfillment failed with a network error".into()
            })))
        }
        ConsumableStatus::InsufficientQuantity | ConsumableStatus::ServerError => {
            Err(StoreError::Platform(extended.unwrap_or_else(|| {
                "the consumable fulfillment failed".into()
            })))
        }
    }
}

/// Parses a `StorePrice.UnformattedPrice` ("4.99") into micros.
pub fn price_micros(unformatted: &str) -> Result<i64, StoreError> {
    let value: f64 = unformatted.parse().map_err(|_| {
        StoreError::Platform(format!("unformatted price {unformatted:?} is not a number"))
    })?;
    #[expect(
        clippy::cast_possible_truncation,
        reason = "a store price fits an i64 in micros"
    )]
    Ok((value * 1_000_000.0).round() as i64)
}
