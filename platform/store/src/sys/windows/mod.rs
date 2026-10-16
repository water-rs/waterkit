//! Windows backend: the Microsoft Store through
//! `Windows.Services.Store.StoreContext`.
//!
//! The Microsoft Store has no per-purchase signed proof, no pending
//! purchase state, and no transaction feed — `OfflineLicensesChanged` is a
//! bare license invalidation. The event stream therefore re-reads the
//! add-on licenses on each change and reports catalog add-ons whose
//! license is new or renewed (an advanced `ExpirationDate`), holding the
//! signals while a purchase call is in flight so a purchase's own license
//! never lands on the stream — the same contract the Apple backend keeps.
//! Consumables hold no license and stay silent. Server-side verification
//! goes through [`Store::store_id_key`] and the Microsoft Store
//! collections API.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;

use futures::channel::mpsc;
use futures::{StreamExt, stream};
use windows::Foundation::{DateTime, TypedEventHandler};
use windows::Globalization::NumberFormatting::CurrencyFormatter;
use windows::Services::Store::{
    StoreCollectionData, StoreConsumableStatus, StoreContext, StoreDurationUnit, StorePrice,
    StoreProduct, StorePurchaseStatus, StoreSku,
};
use windows::Win32::Foundation::{APPMODEL_ERROR_NO_PACKAGE, E_POINTER, ERROR_INSUFFICIENT_BUFFER};
use windows::Win32::Storage::Packaging::Appx::GetCurrentPackageFullName;
use windows::Win32::UI::Shell::IInitializeWithWindow;
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};
use windows::core::{GUID, HRESULT, HSTRING, IInspectable, Interface};
use windows_collections::{IIterable, IMapView};

use crate::sys::{EventStream, mapping};
use crate::{
    Catalog, Entitlement, Offer, OfferToken, PhaseMode, Price, PricingPhase, Product, ProductId,
    ProductKind, PurchaseOutcome, StoreCapabilities, StoreError, Subscription, Timestamp,
    Transaction,
};

/// `FILETIME` epoch offset: seconds between 1601-01-01 and 1970-01-01.
const FILETIME_UNIX_DIFF: i64 = 11_644_473_600;

/// The add-on product kinds the store queries accept; a subscription
/// add-on reports as `Durable`.
const PRODUCT_KINDS: [&str; 3] = ["Durable", "Consumable", "UnmanagedConsumable"];

#[expect(
    clippy::needless_pass_by_value,
    reason = "a `map_err` adapter: callers hand the error over"
)]
fn platform(error: windows::core::Error) -> StoreError {
    StoreError::Platform(error.to_string())
}

/// The `ExtendedError` message, when the result carries a failure HRESULT.
fn extended_message(error: HRESULT) -> Option<String> {
    error.is_err().then(|| error.message())
}

/// Fails on a query result's `ExtendedError` (`ERROR_NO_SUCH_USER` when
/// the user is not signed in to the Store, among others).
fn check_extended(error: HRESULT) -> Result<(), StoreError> {
    extended_message(error).map_or(Ok(()), |message| Err(StoreError::Platform(message)))
}

/// Whether this process is packaged for the Microsoft Store: an unpackaged
/// process has no `StoreContext` association at all.
fn packaged() -> Result<bool, StoreError> {
    let mut length = 0u32;
    let error = unsafe { GetCurrentPackageFullName(&raw mut length, None) };
    if error == ERROR_INSUFFICIENT_BUFFER {
        Ok(true)
    } else if error == APPMODEL_ERROR_NO_PACKAGE {
        Ok(false)
    } else {
        Err(StoreError::Platform(format!(
            "GetCurrentPackageFullName failed: {}",
            error.to_hresult().message()
        )))
    }
}

/// A `Windows.Foundation.DateTime` — 100 ns ticks since 1601-01-01 — as a
/// [`Timestamp`].
fn timestamp(datetime: DateTime) -> Result<Timestamp, StoreError> {
    let unix_100ns =
        i128::from(datetime.UniversalTime) - i128::from(FILETIME_UNIX_DIFF) * 10_000_000;
    Timestamp::from_nanosecond(unix_100ns * 100)
        .map_err(|error| StoreError::Platform(format!("bad acquisition date: {error}")))
}

/// The product-kind iterable every add-on query takes.
fn product_kinds() -> IIterable<HSTRING> {
    IIterable::from(PRODUCT_KINDS.map(HSTRING::from).to_vec())
}

#[expect(
    clippy::unused_async,
    reason = "the sys contract is async on every platform; this backend's probe is synchronous"
)]
pub async fn capabilities() -> Result<StoreCapabilities, StoreError> {
    Ok(StoreCapabilities {
        purchases: packaged()?,
    })
}

/// What the `OfflineLicensesChanged` handler and purchase calls report to
/// the events task.
#[derive(Debug)]
enum Signal {
    /// A purchase call started; license changes are held until it ends.
    PurchaseBegan,
    /// A purchase call ended; carries the offer token it returned to its
    /// caller, if it produced a purchase.
    PurchaseEnded(Option<String>),
    /// `OfflineLicensesChanged` fired; re-read the add-on licenses.
    LicensesChanged,
}

/// A purchase call's span in the events channel: sends `PurchaseBegan` on
/// creation and `PurchaseEnded` on drop, so a purchase future the caller
/// dropped mid-call still releases the held license signals.
struct PurchaseSpan {
    signals: mpsc::UnboundedSender<Signal>,
    /// The offer token the call returned to its caller; `None` unless it
    /// produced a purchase.
    returned: Option<String>,
}

impl PurchaseSpan {
    fn begin(signals: &mpsc::UnboundedSender<Signal>) -> Self {
        let _ = signals.unbounded_send(Signal::PurchaseBegan);
        Self {
            signals: signals.clone(),
            returned: None,
        }
    }

    /// The call produced a purchase; record its token so the drop tells
    /// the events task which license it must not report.
    fn returned(&mut self, token: String) {
        self.returned = Some(token);
    }
}

impl Drop for PurchaseSpan {
    fn drop(&mut self) {
        let _ = self
            .signals
            .unbounded_send(Signal::PurchaseEnded(self.returned.take()));
    }
}

/// The Microsoft Store session: the `StoreContext` handle.
pub struct Store {
    context: StoreContext,
    /// Sends purchase lifecycle signals to the events task; the
    /// `OfflineLicensesChanged` handler holds a clone.
    signals: mpsc::UnboundedSender<Signal>,
    /// The `OfflineLicensesChanged` registration, removed on drop.
    licenses_changed: i64,
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Store").finish_non_exhaustive()
    }
}

impl Drop for Store {
    fn drop(&mut self) {
        if let Err(error) = self
            .context
            .RemoveOfflineLicensesChanged(self.licenses_changed)
        {
            tracing::warn!("store: removing OfflineLicensesChanged failed: {error}");
        }
    }
}

/// The platform's finish handle: the context plus the add-on's Store ID
/// and purchased units, for `ReportConsumableFulfillmentAsync`.
#[derive(Debug)]
pub struct Purchase {
    context: StoreContext,
    store_id: String,
    quantity: u32,
}

impl Store {
    pub async fn connect(catalog: &Arc<Catalog>) -> Result<(Self, EventStream), StoreError> {
        if !packaged()? {
            return Err(StoreError::Unavailable);
        }
        let context = StoreContext::GetDefault().map_err(platform)?;

        let (signals, receiver) = mpsc::unbounded();
        let handler = signals.clone();
        let licenses_changed = context
            .OfflineLicensesChanged(&TypedEventHandler::<StoreContext, IInspectable>::new(
                move |_, _| {
                    let _ = handler.unbounded_send(Signal::LicensesChanged);
                    Ok(())
                },
            ))
            .map_err(platform)?;

        // Licenses already active at connect must not be reported as new.
        let known = licensed_addons(&context).await?;
        let events = event_stream(context.clone(), receiver, catalog, known);
        Ok((
            Self {
                context,
                signals,
                licenses_changed,
            },
            events,
        ))
    }

    pub async fn products(&self, catalog: &Catalog) -> Result<Vec<Product>, StoreError> {
        let associated = associated_products(&self.context).await?;
        catalog
            .ids()
            .map(|id| {
                let product = associated
                    .get(id.as_str())
                    .ok_or_else(|| StoreError::ProductNotFound(id.clone()))?;
                build_product(
                    product,
                    id,
                    catalog.kind(id).expect("ids() yields catalog keys"),
                )
            })
            .collect()
    }

    pub async fn purchase(
        &self,
        product: &ProductId,
        _offer: Option<&OfferToken>,
        catalog: &Catalog,
    ) -> Result<PurchaseOutcome, StoreError> {
        let declared = catalog
            .kind(product)
            .ok_or_else(|| StoreError::ProductNotFound(product.clone()))?;
        let associated = associated_products(&self.context).await?;
        let store_product = associated
            .get(product.as_str())
            .ok_or_else(|| StoreError::ProductNotFound(product.clone()))?;
        check_kind(store_product, product, declared)?;

        let mut span = PurchaseSpan::begin(&self.signals);
        let outcome = self
            .request_purchase(store_product, product, declared)
            .await;
        if matches!(outcome, Ok(PurchaseOutcome::Purchased(_))) {
            span.returned(product.as_str().to_owned());
        }
        outcome
    }

    /// Associates the context with the app's foreground top-level window
    /// and requests the purchase; the Microsoft Store shows modal UI owned
    /// by that window.
    async fn request_purchase(
        &self,
        product: &StoreProduct,
        id: &ProductId,
        declared: ProductKind,
    ) -> Result<PurchaseOutcome, StoreError> {
        self.set_owner_window()?;
        let store_id = product.StoreId().map_err(platform)?.to_string();
        let result = self
            .context
            .RequestPurchaseAsync(&HSTRING::from(&store_id))
            .map_err(platform)?
            .await
            .map_err(platform)?;
        let status = purchase_status(result.Status().map_err(platform)?)?;
        let extended = extended_message(result.ExtendedError().map_err(platform)?);
        match mapping::purchase_verdict(status, id, extended)? {
            mapping::PurchaseVerdict::Cancelled => Ok(PurchaseOutcome::Cancelled),
            mapping::PurchaseVerdict::Purchased => {
                // A store-managed consumable grants the quantity configured
                // in Partner Center; the new balance is what was bought.
                let quantity = match mapping::store_product_kind(
                    &product.ProductKind().map_err(platform)?.to_string(),
                ) {
                    mapping::StoreProductKind::Consumable => {
                        consumable_balance(&self.context, &store_id).await?
                    }
                    _ => 1,
                };
                let collection = user_collection(&self.context).await?;
                Ok(PurchaseOutcome::Purchased(build_purchase(
                    &self.context,
                    &collection,
                    id,
                    declared,
                    quantity,
                    &store_id,
                )?))
            }
        }
    }

    /// Points the context at this process's foreground top-level window —
    /// `RequestPurchaseAsync` fails with `ERROR_INVALID_WINDOW_HANDLE`
    /// when the context has no owner window for its modal dialogs.
    fn set_owner_window(&self) -> Result<(), StoreError> {
        let window = unsafe { GetForegroundWindow() };
        let mut process = 0u32;
        unsafe {
            GetWindowThreadProcessId(window, Some(&raw mut process));
        }
        if window.is_invalid() || process != std::process::id() {
            return Err(StoreError::Platform(
                "the purchase needs a foreground window of this app".into(),
            ));
        }
        let init: IInitializeWithWindow = self.context.cast().map_err(platform)?;
        unsafe { init.Initialize(window) }.map_err(platform)
    }

    pub async fn entitlements(&self, catalog: &Catalog) -> Result<Vec<Entitlement>, StoreError> {
        let app_license = self
            .context
            .GetAppLicenseAsync()
            .map_err(platform)?
            .await
            .map_err(platform)?;
        let collection = user_collection(&self.context).await?;
        let mut entitlements = Vec::new();

        // AddOnLicenses holds every valid durable add-on license, keyed by
        // the add-on SKU's Store ID; consumables never appear in it.
        for pair in &app_license.AddOnLicenses().map_err(platform)? {
            let license = pair.Value().map_err(platform)?;
            let id = ProductId::new(license.InAppOfferToken().map_err(platform)?.to_string());
            if let Some(declared @ (ProductKind::NonConsumable | ProductKind::Subscription)) =
                catalog.kind(&id)
            {
                entitlements.push(Entitlement::Finished(Transaction::new(
                    id.clone(),
                    declared,
                    1,
                    acquired_date(&collection, &id)?,
                    None,
                )));
            }
        }

        // Store-managed consumables with a remaining balance are unfinished
        // purchases; a developer-managed consumable in the user collection
        // is unfulfilled.
        let associated = associated_products(&self.context).await?;
        for id in catalog.ids() {
            if catalog.kind(id) != Some(ProductKind::Consumable) {
                continue;
            }
            let Some(product) = associated.get(id.as_str()) else {
                continue;
            };
            match mapping::store_product_kind(&product.ProductKind().map_err(platform)?.to_string())
            {
                mapping::StoreProductKind::Consumable => {
                    let store_id = product.StoreId().map_err(platform)?.to_string();
                    let balance = consumable_balance(&self.context, &store_id).await?;
                    if balance > 0 {
                        entitlements.push(Entitlement::Unfinished(build_purchase(
                            &self.context,
                            &collection,
                            id,
                            ProductKind::Consumable,
                            balance,
                            &store_id,
                        )?));
                    }
                }
                mapping::StoreProductKind::UnmanagedConsumable => {
                    let Some(owned) = collection.get(id.as_str()) else {
                        continue;
                    };
                    if owned.IsInUserCollection().map_err(platform)? {
                        let store_id = owned.StoreId().map_err(platform)?.to_string();
                        entitlements.push(Entitlement::Unfinished(build_purchase(
                            &self.context,
                            &collection,
                            id,
                            ProductKind::Consumable,
                            1,
                            &store_id,
                        )?));
                    }
                }
                _ => {
                    return Err(StoreError::KindMismatch {
                        product: id.clone(),
                        declared: ProductKind::Consumable,
                    });
                }
            }
        }
        Ok(entitlements)
    }

    /// The Microsoft Store ID key the app's backend uses against the
    /// collections API.
    pub async fn store_id_key(
        &self,
        service_ticket: &str,
        publisher_user_id: &str,
    ) -> Result<String, StoreError> {
        Ok(self
            .context
            .GetCustomerCollectionsIdAsync(
                &HSTRING::from(service_ticket),
                &HSTRING::from(publisher_user_id),
            )
            .map_err(platform)?
            .await
            .map_err(platform)?
            .to_string())
    }
}

/// Reports a consumable's units as fulfilled; durables and subscriptions
/// need no acknowledgement on the Microsoft Store, so finishing them makes
/// no store call.
pub async fn finish(purchase: Purchase, kind: ProductKind) -> Result<(), StoreError> {
    if kind != ProductKind::Consumable {
        return Ok(());
    }
    let result = purchase
        .context
        .ReportConsumableFulfillmentAsync(
            &HSTRING::from(&purchase.store_id),
            purchase.quantity,
            GUID::new().map_err(platform)?,
        )
        .map_err(platform)?
        .await
        .map_err(platform)?;
    let status = consumable_status(result.Status().map_err(platform)?)?;
    mapping::fulfillment_verdict(
        status,
        extended_message(result.ExtendedError().map_err(platform)?),
    )
}

/// The remaining balance of a store-managed consumable by its Store ID.
async fn consumable_balance(context: &StoreContext, store_id: &str) -> Result<u32, StoreError> {
    let result = context
        .GetConsumableBalanceRemainingAsync(&HSTRING::from(store_id))
        .map_err(platform)?
        .await
        .map_err(platform)?;
    check_extended(result.ExtendedError().map_err(platform)?)?;
    result.BalanceRemaining().map_err(platform)
}

/// The offer tokens of the add-ons holding a currently valid license,
/// each mapped to its `ExpirationDate` in `FILETIME` ticks — durable
/// add-ons only; consumables hold no license. A subscription renewal
/// shows up as an advanced expiration for a token already in the map.
async fn licensed_addons(context: &StoreContext) -> Result<BTreeMap<String, i64>, StoreError> {
    let license = context
        .GetAppLicenseAsync()
        .map_err(platform)?
        .await
        .map_err(platform)?;
    let mut licenses = BTreeMap::new();
    for pair in &license.AddOnLicenses().map_err(platform)? {
        let license = pair.Value().map_err(platform)?;
        licenses.insert(
            license.InAppOfferToken().map_err(platform)?.to_string(),
            license.ExpirationDate().map_err(platform)?.UniversalTime,
        );
    }
    Ok(licenses)
}

/// Every add-on the Store associates with this app, keyed by the
/// Partner Center product ID (`InAppOfferToken`).
async fn associated_products(
    context: &StoreContext,
) -> Result<BTreeMap<String, StoreProduct>, StoreError> {
    // `IIterable` is not `Send`; scope it so it drops before the await.
    let operation = {
        let kinds = product_kinds();
        context
            .GetAssociatedStoreProductsAsync(&kinds)
            .map_err(platform)?
    };
    let result = operation.await.map_err(platform)?;
    check_extended(result.ExtendedError().map_err(platform)?)?;
    product_map(&result.Products().map_err(platform)?)
}

/// Every add-on the user currently owns, keyed by `InAppOfferToken`.
async fn user_collection(
    context: &StoreContext,
) -> Result<BTreeMap<String, StoreProduct>, StoreError> {
    // `IIterable` is not `Send`; scope it so it drops before the await.
    let operation = {
        let kinds = product_kinds();
        context.GetUserCollectionAsync(&kinds).map_err(platform)?
    };
    let result = operation.await.map_err(platform)?;
    check_extended(result.ExtendedError().map_err(platform)?)?;
    product_map(&result.Products().map_err(platform)?)
}

fn product_map(
    products: &IMapView<HSTRING, StoreProduct>,
) -> Result<BTreeMap<String, StoreProduct>, StoreError> {
    let mut map = BTreeMap::new();
    for pair in products {
        let product = pair.Value().map_err(platform)?;
        map.insert(
            product.InAppOfferToken().map_err(platform)?.to_string(),
            product,
        );
    }
    Ok(map)
}

/// The store's product shape must agree with the catalog's declared kind.
fn check_kind(
    product: &StoreProduct,
    id: &ProductId,
    declared: ProductKind,
) -> Result<(), StoreError> {
    let kind = mapping::store_product_kind(&product.ProductKind().map_err(platform)?.to_string());
    if mapping::kind_matches(declared, kind, subscription_sku(product)?.is_some()) {
        Ok(())
    } else {
        Err(StoreError::KindMismatch {
            product: id.clone(),
            declared,
        })
    }
}

/// The product's subscription SKU, if it has one; a subscription add-on
/// reports `ProductKind` `Durable`, so the SKU tells it apart.
fn subscription_sku(product: &StoreProduct) -> Result<Option<StoreSku>, StoreError> {
    for sku in &product.Skus().map_err(platform)? {
        if sku.IsSubscription().map_err(platform)? {
            return Ok(Some(sku));
        }
    }
    Ok(None)
}

fn build_product(
    product: &StoreProduct,
    id: &ProductId,
    declared: ProductKind,
) -> Result<Product, StoreError> {
    check_kind(product, id, declared)?;
    let price = product_price(&product.Price().map_err(platform)?)?;
    let subscription = match declared {
        ProductKind::Subscription => Some(subscription(
            &subscription_sku(product)?.ok_or_else(|| {
                StoreError::Platform(format!("subscription product {id} has no subscription SKU"))
            })?,
            id,
        )?),
        ProductKind::Consumable | ProductKind::NonConsumable => None,
    };
    Ok(Product::new(
        id.clone(),
        declared,
        product.Title().map_err(platform)?.to_string(),
        product.Description().map_err(platform)?.to_string(),
        price,
        subscription,
    ))
}

fn subscription(sku: &StoreSku, id: &ProductId) -> Result<Subscription, StoreError> {
    let info = sku.SubscriptionInfo().map_err(platform)?;
    let period = mapping::billing_period(
        info.BillingPeriod().map_err(platform)?,
        billing_unit(info.BillingPeriodUnit().map_err(platform)?)?,
    )?;
    let sku_price = sku.Price().map_err(platform)?;
    let currency = sku_price.CurrencyCode().map_err(platform)?.to_string();
    let mut phases = Vec::new();
    if info.HasTrialPeriod().map_err(platform)? {
        phases.push(PricingPhase {
            price: free_price(&currency)?,
            period: mapping::billing_period(
                info.TrialPeriod().map_err(platform)?,
                billing_unit(info.TrialPeriodUnit().map_err(platform)?)?,
            )?,
            cycles: Some(1),
            mode: PhaseMode::FreeTrial,
        });
    }
    phases.push(PricingPhase {
        price: recurrence_price(&sku_price, &currency)?,
        period,
        cycles: None,
        mode: PhaseMode::Recurring,
    });
    Ok(Subscription {
        period,
        offers: vec![Offer::new(OfferToken::new(id.as_str().to_owned()), phases)],
    })
}

fn product_price(price: &StorePrice) -> Result<Price, StoreError> {
    Ok(Price {
        formatted: price.FormattedPrice().map_err(platform)?.to_string(),
        amount_micros: mapping::price_micros(
            &price.UnformattedPrice().map_err(platform)?.to_string(),
        )?,
        currency: price.CurrencyCode().map_err(platform)?.to_string(),
    })
}

/// The subscription's renewal price; `FormattedRecurrencePrice` is empty
/// on SKUs with no recurrence.
fn recurrence_price(price: &StorePrice, currency: &str) -> Result<Price, StoreError> {
    let formatted = price
        .FormattedRecurrencePrice()
        .map_err(platform)?
        .to_string();
    if formatted.is_empty() {
        Ok(Price {
            formatted: price.FormattedPrice().map_err(platform)?.to_string(),
            amount_micros: mapping::price_micros(
                &price.UnformattedPrice().map_err(platform)?.to_string(),
            )?,
            currency: currency.to_owned(),
        })
    } else {
        Ok(Price {
            formatted,
            amount_micros: mapping::price_micros(
                &price
                    .UnformattedRecurrencePrice()
                    .map_err(platform)?
                    .to_string(),
            )?,
            currency: currency.to_owned(),
        })
    }
}

/// A free-trial phase's zero price, formatted in the product's currency.
fn free_price(currency: &str) -> Result<Price, StoreError> {
    let formatter = CurrencyFormatter::CreateCurrencyFormatterCode(&HSTRING::from(currency))
        .map_err(platform)?;
    Ok(Price {
        formatted: formatter.FormatInt(0).map_err(platform)?.to_string(),
        amount_micros: 0,
        currency: currency.to_owned(),
    })
}

/// The acquisition date the Store reports for an owned add-on — the SKU
/// collection data's `AcquiredDate`.
fn acquired_date(
    collection: &BTreeMap<String, StoreProduct>,
    id: &ProductId,
) -> Result<Timestamp, StoreError> {
    let product = collection.get(id.as_str()).ok_or_else(|| {
        StoreError::Platform(format!(
            "the store reports {id} outside the user's collection"
        ))
    })?;
    let mut data: Option<StoreCollectionData> = None;
    for sku in &product.Skus().map_err(platform)? {
        // `CollectionData` is null on a SKU outside the user's collection;
        // windows-rs surfaces the null object as `E_POINTER`. Any other
        // error is real and propagates.
        let candidate = match sku.CollectionData() {
            Ok(candidate) => candidate,
            Err(error) if error.code() == E_POINTER => continue,
            Err(error) => return Err(platform(error)),
        };
        if sku.IsInUserCollection().map_err(platform)? {
            data = Some(candidate);
            break;
        }
        data.get_or_insert(candidate);
    }
    timestamp(
        data.ok_or_else(|| {
            StoreError::Platform(format!("the store reports no acquisition date for {id}"))
        })?
        .AcquiredDate()
        .map_err(platform)?,
    )
}

/// A purchase record for `id` from the collection data the Store reports
/// for the add-on.
fn build_purchase(
    context: &StoreContext,
    collection: &BTreeMap<String, StoreProduct>,
    id: &ProductId,
    declared: ProductKind,
    quantity: u32,
    store_id: &str,
) -> Result<crate::Purchase, StoreError> {
    Ok(crate::Purchase::new(
        id.clone(),
        declared,
        quantity,
        acquired_date(collection, id)?,
        None,
        Purchase {
            context: context.clone(),
            store_id: store_id.to_owned(),
            quantity,
        },
    ))
}

fn purchase_status(status: StorePurchaseStatus) -> Result<mapping::PurchaseStatus, StoreError> {
    Ok(if status == StorePurchaseStatus::Succeeded {
        mapping::PurchaseStatus::Succeeded
    } else if status == StorePurchaseStatus::AlreadyPurchased {
        mapping::PurchaseStatus::AlreadyPurchased
    } else if status == StorePurchaseStatus::NotPurchased {
        mapping::PurchaseStatus::NotPurchased
    } else if status == StorePurchaseStatus::NetworkError {
        mapping::PurchaseStatus::NetworkError
    } else if status == StorePurchaseStatus::ServerError {
        mapping::PurchaseStatus::ServerError
    } else {
        return Err(StoreError::Platform(format!(
            "unknown purchase status {}",
            status.0
        )));
    })
}

fn consumable_status(
    status: StoreConsumableStatus,
) -> Result<mapping::ConsumableStatus, StoreError> {
    Ok(if status == StoreConsumableStatus::Succeeded {
        mapping::ConsumableStatus::Succeeded
    } else if status == StoreConsumableStatus::InsufficentQuantity {
        mapping::ConsumableStatus::InsufficientQuantity
    } else if status == StoreConsumableStatus::NetworkError {
        mapping::ConsumableStatus::NetworkError
    } else if status == StoreConsumableStatus::ServerError {
        mapping::ConsumableStatus::ServerError
    } else {
        return Err(StoreError::Platform(format!(
            "unknown consumable status {}",
            status.0
        )));
    })
}

fn billing_unit(unit: StoreDurationUnit) -> Result<mapping::BillingUnit, StoreError> {
    Ok(if unit == StoreDurationUnit::Minute {
        mapping::BillingUnit::Minute
    } else if unit == StoreDurationUnit::Hour {
        mapping::BillingUnit::Hour
    } else if unit == StoreDurationUnit::Day {
        mapping::BillingUnit::Day
    } else if unit == StoreDurationUnit::Week {
        mapping::BillingUnit::Week
    } else if unit == StoreDurationUnit::Month {
        mapping::BillingUnit::Month
    } else if unit == StoreDurationUnit::Year {
        mapping::BillingUnit::Year
    } else {
        return Err(StoreError::Platform(format!(
            "unknown duration unit {}",
            unit.0
        )));
    })
}

/// The event task's state: the signal channel it drains and the offer
/// tokens it already reported or must suppress.
struct Events {
    context: StoreContext,
    signals: mpsc::UnboundedReceiver<Signal>,
    catalog: Arc<Catalog>,
    /// Purchase calls in flight; license changes are held while nonzero.
    in_flight: u32,
    /// A license change arrived while a purchase was in flight.
    held: bool,
    /// Offer token → expiration of the license last seen, in `FILETIME`
    /// ticks; seeded at connect and replaced by every refresh. A token
    /// absent from the map is newly active, an advanced expiration is a
    /// renewal, and a dropped token has lapsed.
    known: BTreeMap<String, i64>,
    /// Offer tokens a purchase call already returned to its caller —
    /// suppressed until a refresh first sees them licensed, then consumed
    /// so a later renewal reports.
    returned: BTreeSet<String>,
    /// Items waiting to be yielded.
    queue: VecDeque<Result<crate::Purchase, StoreError>>,
}

impl Events {
    /// Re-reads the add-on licenses and queues a purchase for every catalog
    /// add-on whose license is new or renewed.
    async fn refresh(&mut self) {
        match self.pending().await {
            Ok((items, licenses, consumed)) => {
                for token in consumed {
                    self.returned.remove(&token);
                }
                self.known = licenses;
                self.queue.extend(items);
            }
            Err(error) => self.queue.push_back(Err(error)),
        }
    }

    /// Purchases for catalog add-ons whose license newly appeared or whose
    /// expiration advanced — a renewal. Also returns the live license map
    /// that replaces `known`, and the `returned` tokens a refresh now sees
    /// licensed: their expiration goes on record without an emission, and
    /// they leave `returned` so a later renewal reports — whichever order
    /// the purchase return and the license arrived in.
    async fn pending(
        &self,
    ) -> Result<
        (
            Vec<Result<crate::Purchase, StoreError>>,
            BTreeMap<String, i64>,
            Vec<String>,
        ),
        StoreError,
    > {
        let licenses = licensed_addons(&self.context).await?;
        let mut changed = Vec::new();
        let mut consumed = Vec::new();
        for (token, expiration) in &licenses {
            if self.returned.contains(token) {
                consumed.push(token.clone());
                continue;
            }
            let emit = match self.known.get(token) {
                None => true,
                Some(&last) => *expiration > last,
            };
            let id = ProductId::new(token.clone());
            if emit && self.catalog.kind(&id).is_some() {
                changed.push(token.clone());
            }
        }
        if changed.is_empty() {
            return Ok((Vec::new(), licenses, consumed));
        }
        let collection = user_collection(&self.context).await?;
        let items = changed
            .into_iter()
            .map(|token| {
                let id = ProductId::new(token);
                let declared = self.catalog.kind(&id).expect("filtered to catalog ids");
                let store_id = collection
                    .get(id.as_str())
                    .ok_or_else(|| {
                        StoreError::Platform(format!(
                            "the store reports {id} outside the user's collection"
                        ))
                    })
                    .and_then(|product| {
                        product
                            .StoreId()
                            .map(|store_id| store_id.to_string())
                            .map_err(platform)
                    });
                store_id.and_then(|store_id| {
                    build_purchase(&self.context, &collection, &id, declared, 1, &store_id)
                })
            })
            .collect();
        Ok((items, licenses, consumed))
    }
}

/// The license-change feed: re-reads the add-on licenses on each
/// `OfflineLicensesChanged` signal and emits a purchase for every catalog
/// add-on whose license is new or renewed. Ends when the channel closes —
/// when the [`Store`] drops.
fn event_stream(
    context: StoreContext,
    signals: mpsc::UnboundedReceiver<Signal>,
    catalog: &Arc<Catalog>,
    known: BTreeMap<String, i64>,
) -> EventStream {
    stream::unfold(
        Events {
            context,
            signals,
            catalog: Arc::clone(catalog),
            in_flight: 0,
            held: false,
            known,
            returned: BTreeSet::new(),
            queue: VecDeque::new(),
        },
        |mut events| async move {
            loop {
                if let Some(item) = events.queue.pop_front() {
                    return Some((item, events));
                }
                match events.signals.next().await {
                    None => return None,
                    Some(Signal::PurchaseBegan) => events.in_flight += 1,
                    Some(Signal::PurchaseEnded(token)) => {
                        events.in_flight -= 1;
                        if let Some(token) = token {
                            events.returned.insert(token);
                        }
                        if events.in_flight == 0 && events.held {
                            events.held = false;
                            events.refresh().await;
                        }
                    }
                    Some(Signal::LicensesChanged) => {
                        if events.in_flight > 0 {
                            events.held = true;
                        } else {
                            events.refresh().await;
                        }
                    }
                }
            }
        },
    )
    .boxed()
}
