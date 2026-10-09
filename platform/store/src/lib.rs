//! In-app purchases and subscriptions through the platform store.
//!
//! | Platform | Backend | Notes |
//! | --- | --- | --- |
//! | Android | Play Billing Library 9 | needs the Play Store; the app declares each product's [`ProductKind`] because Play does not distinguish consumables from non-consumables |
//! | iOS / macOS | StoreKit 2 | iOS 15 / macOS 12 floor; below it the store reports unavailable |
//! | Windows | — | `Windows.Services.Store` does not expose a per-purchase signed proof or a transaction update stream, so the store reports unavailable |
//! | Linux / wasm | — | unavailable |
//!
//! ```no_run
//! use waterkit_store::{Catalog, ProductId, ProductKind, Store};
//!
//! # async fn example() -> Result<(), waterkit_store::StoreError> {
//! let catalog = Catalog::new()
//!     .with(ProductId::new("app.coins.small"), ProductKind::Consumable)
//!     .with(ProductId::new("app.pro"), ProductKind::NonConsumable)
//!     .with(ProductId::new("app.sub.monthly"), ProductKind::Subscription);
//!
//! let (store, _events) = Store::connect(catalog).await?;
//! for product in store.products().await? {
//!     tracing::info!("{}: {}", product.id().as_str(), product.price().formatted);
//! }
//! # Ok(())
//! # }
//! ```

#![warn(missing_docs)]
#![warn(missing_debug_implementations)]

mod sys;

use std::collections::BTreeMap;

use futures::Stream;
use waterkit_core::Capabilities;

pub use waterkit_core::Timestamp;

/// A product identifier as configured in the store console.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ProductId(String);

impl ProductId {
    /// Wraps a console product identifier.
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// The identifier text, exactly as the console knows it.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ProductId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// How the app sells a product.
///
/// Declared by the app, because Play does not distinguish consumables from
/// non-consumables: the app consumes or acknowledges at finish time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ProductKind {
    /// Consumed on use — coins, lives, fuel.
    Consumable,
    /// Bought once, kept forever — a one-time unlock.
    NonConsumable,
    /// Auto-renewing subscription.
    Subscription,
}

/// The products the app sells.
///
/// `Catalog::new().with(id, kind)...` — the catalog declares every product
/// the app can query or buy, along with the kind the app sells it as.
#[derive(Debug, Clone, Default)]
pub struct Catalog {
    products: BTreeMap<ProductId, ProductKind>,
}

impl Catalog {
    /// An empty catalog.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            products: BTreeMap::new(),
        }
    }

    /// Adds a product and returns the catalog for chaining.
    #[must_use]
    pub fn with(mut self, id: ProductId, kind: ProductKind) -> Self {
        self.products.insert(id, kind);
        self
    }

    /// The kind the catalog declares for `id`, if the product is in it.
    #[must_use]
    pub fn kind(&self, id: &ProductId) -> Option<ProductKind> {
        self.products.get(id).copied()
    }

    /// Every product id in the catalog, sorted.
    pub fn ids(&self) -> impl Iterator<Item = &ProductId> {
        self.products.keys()
    }
}

/// Whether store purchases are available on this device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct StoreCapabilities {
    /// Whether this device can make purchases at all: Play Store present on
    /// Android, `canMakePayments` on Apple, never on other platforms.
    pub purchases: bool,
}

impl Capabilities for StoreCapabilities {
    fn available(&self) -> bool {
        self.purchases
    }
}

/// Whether store purchases are available on this device.
///
/// Does not connect to the store; a device that reports available can still
/// fail [`Store::connect`] when the store itself is unreachable. A device
/// without a store — no Play Store app, an OS below the `StoreKit` 2 floor —
/// answers `Ok` with [`StoreCapabilities::purchases`] unset: that is a real
/// answer, not a failure.
///
/// # Errors
/// The platform's probe error (`platform`).
pub async fn capabilities() -> Result<StoreCapabilities, StoreError> {
    sys::capabilities().await
}

/// A connection to the platform store.
///
/// On Android it owns the `BillingClient` (connected in [`Store::connect`],
/// ended on drop); on Apple it is a thin handle — `StoreKit` needs no session.
#[derive(Debug)]
pub struct Store {
    catalog: std::sync::Arc<Catalog>,
    sys: sys::Store,
}

impl Store {
    /// Connects to the platform store.
    ///
    /// Returns the store and its [`StoreEvents`] transaction feed — the feed
    /// exists from connect, so a failure to set it up is a `connect` error.
    ///
    /// Fails with [`StoreError::Unavailable`] when the platform reports no
    /// store: no Play Store app on Android, an OS below the `StoreKit` 2
    /// floor, or an unsupported platform.
    ///
    /// # Errors
    /// [`StoreError::Unavailable`] when the device cannot purchase, or the
    /// store's own error (`network`, `platform`).
    pub async fn connect(catalog: Catalog) -> Result<(Self, StoreEvents), StoreError> {
        let catalog = std::sync::Arc::new(catalog);
        let (sys, events) = sys::Store::connect(&catalog).await?;
        Ok((Self { catalog, sys }, StoreEvents { inner: events }))
    }

    /// Queries every product in the catalog.
    ///
    /// A product the store does not know is [`StoreError::ProductNotFound`].
    /// On Apple, a `StoreKit` product type that contradicts the declared kind
    /// is [`StoreError::KindMismatch`] — fail fast, the app declared the wrong
    /// kind.
    ///
    /// # Errors
    /// [`StoreError::ProductNotFound`], [`StoreError::KindMismatch`], or the
    /// store's own error (`network`, `platform`, `unavailable`).
    pub async fn products(&self) -> Result<Vec<Product>, StoreError> {
        self.sys.products(&self.catalog).await
    }

    /// Consumable or non-consumable purchase.
    ///
    /// A subscription product passed here is [`StoreError::KindMismatch`].
    ///
    /// # Errors
    /// [`StoreError::KindMismatch`] for a subscription product, or the store's
    /// own error (`network`, `platform`, `unavailable`, `unverified`,
    /// `already_owned`).
    pub async fn purchase(&self, product: &Product) -> Result<PurchaseOutcome, StoreError> {
        if product.kind() == ProductKind::Subscription {
            return Err(StoreError::KindMismatch {
                product: product.id().clone(),
                declared: product.kind(),
            });
        }
        self.sys.purchase(product.id(), None, &self.catalog).await
    }

    /// Subscription purchase of one of the product's offers.
    ///
    /// Play requires the offer token: it selects the base plan and pricing
    /// phases the subscription runs under. On Apple the token is unused —
    /// the standard price and any introductory offer the user is eligible
    /// for apply automatically.
    ///
    /// # Errors
    /// [`StoreError::KindMismatch`] for a non-subscription product, or the
    /// store's own error (`network`, `platform`, `unavailable`, `unverified`).
    pub async fn subscribe(
        &self,
        product: &Product,
        offer: &Offer,
    ) -> Result<PurchaseOutcome, StoreError> {
        if product.kind() != ProductKind::Subscription {
            return Err(StoreError::KindMismatch {
                product: product.id().clone(),
                declared: product.kind(),
            });
        }
        self.sys
            .purchase(product.id(), Some(offer.token()), &self.catalog)
            .await
    }

    fn catalog(&self) -> &Catalog {
        &self.catalog
    }

    /// What the user currently owns: non-consumables and active
    /// subscriptions, each either still unfinished or already finished.
    ///
    /// A consumed consumable is never an entitlement.
    ///
    /// # Errors
    /// The store's own error (`network`, `platform`, `unavailable`,
    /// `unverified`).
    pub async fn entitlements(&self) -> Result<Vec<Entitlement>, StoreError> {
        self.sys.entitlements(self.catalog()).await
    }
}

/// The transaction feed [`Store::connect`] returns with the store.
///
/// It carries transactions completed outside a purchase call — pending
/// purchases that settle, Ask to Buy, purchases from another device, and
/// renewals where the platform delivers them to the client.
///
/// On Android, Play delivers subscription renewals only to the app's
/// backend through Real-time Developer Notifications — never to
/// `PurchasesUpdatedListener` — so renewals do not appear in this stream
/// there.
///
/// The stream ends when the [`Store`] drops.
pub struct StoreEvents {
    inner: futures::stream::BoxStream<'static, Result<Purchase, StoreError>>,
}

impl Stream for StoreEvents {
    type Item = Result<Purchase, StoreError>;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        self.inner.as_mut().poll_next(cx)
    }
}

impl std::fmt::Debug for StoreEvents {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoreEvents").finish_non_exhaustive()
    }
}

/// A product the store knows, priced and described.
#[derive(Debug)]
#[non_exhaustive]
pub struct Product {
    id: ProductId,
    kind: ProductKind,
    title: String,
    description: String,
    price: Price,
    subscription: Option<Subscription>,
}

impl Product {
    #[cfg_attr(
        not(any(target_os = "android", target_os = "ios", target_os = "macos", test)),
        expect(dead_code, reason = "only the platform wire decoder builds these")
    )]
    pub(crate) const fn new(
        id: ProductId,
        kind: ProductKind,
        title: String,
        description: String,
        price: Price,
        subscription: Option<Subscription>,
    ) -> Self {
        Self {
            id,
            kind,
            title,
            description,
            price,
            subscription,
        }
    }

    /// The product's console identifier.
    #[must_use]
    pub const fn id(&self) -> &ProductId {
        &self.id
    }

    /// The kind the app's catalog declared for this product.
    #[must_use]
    pub const fn kind(&self) -> ProductKind {
        self.kind
    }

    /// The store-localized product name.
    #[must_use]
    pub fn title(&self) -> &str {
        &self.title
    }

    /// The store-localized product description.
    #[must_use]
    pub fn description(&self) -> &str {
        &self.description
    }

    /// The product's one-time price, or the subscription's base price.
    #[must_use]
    pub const fn price(&self) -> &Price {
        &self.price
    }

    /// Subscription terms, when the product is a subscription.
    #[must_use]
    pub const fn subscription(&self) -> Option<&Subscription> {
        self.subscription.as_ref()
    }
}

/// A price: the localized string and its structured parts.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Price {
    /// The localized price string, e.g. `$4.99`.
    pub formatted: String,
    /// The price in millionths of a currency unit (micros).
    pub amount_micros: i64,
    /// The ISO 4217 currency code, e.g. `USD`.
    pub currency: String,
}

/// Subscription terms: the renewal period and the offers it is sold under.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Subscription {
    /// How often the subscription renews.
    pub period: Period,
    /// The offers this subscription can be purchased under. Non-empty.
    pub offers: Vec<Offer>,
}

/// A billing period: a count of a calendar unit, e.g. `1 month`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Period {
    /// The calendar unit the period counts.
    pub unit: PeriodUnit,
    /// How many units the period lasts.
    pub count: u32,
}

/// The calendar unit of a [`Period`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PeriodUnit {
    /// Days.
    Day,
    /// Weeks.
    Week,
    /// Months.
    Month,
    /// Years.
    Year,
}

/// A purchasable subscription offer: the base plan — or, on Apple, the
/// standard price with any introductory offer the user is eligible for —
/// and its pricing phases.
///
/// The token is opaque and platform-specific: on Play it is the offer token
/// `launchBillingFlow` requires; on Apple it identifies the base offer the
/// product carries and is not presented to `StoreKit`. Apple promotional and
/// win-back offers, which need a server-generated signature, are out of
/// scope and never appear here.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Offer {
    token: OfferToken,
    phases: Vec<PricingPhase>,
}

impl Offer {
    #[cfg_attr(
        not(any(target_os = "android", target_os = "ios", target_os = "macos", test)),
        expect(dead_code, reason = "only the platform wire decoder builds these")
    )]
    pub(crate) const fn new(token: OfferToken, phases: Vec<PricingPhase>) -> Self {
        Self { token, phases }
    }

    /// The opaque platform offer handle [`Store::subscribe`] needs.
    #[must_use]
    pub const fn token(&self) -> &OfferToken {
        &self.token
    }

    /// The pricing phases the purchase runs through, in order. Ends with a
    /// [`PhaseMode::Recurring`] phase.
    #[must_use]
    pub fn phases(&self) -> &[PricingPhase] {
        &self.phases
    }
}

/// An opaque platform handle selecting a subscription offer.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct OfferToken(String);

impl OfferToken {
    #[cfg_attr(
        not(any(target_os = "android", target_os = "ios", target_os = "macos", test)),
        expect(dead_code, reason = "only the platform wire decoder builds these")
    )]
    pub(crate) const fn new(value: String) -> Self {
        Self(value)
    }

    /// The token text, as the platform issued it.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// One pricing phase of a subscription [`Offer`].
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct PricingPhase {
    /// What the phase charges.
    pub price: Price,
    /// How long one cycle of the phase lasts.
    pub period: Period,
    /// How many cycles the phase runs; `None` recurs forever.
    pub cycles: Option<u32>,
    /// How the phase bills.
    pub mode: PhaseMode,
}

/// How a [`PricingPhase`] bills.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PhaseMode {
    /// Free for the phase's cycles.
    FreeTrial,
    /// A discounted price each cycle.
    PayAsYouGo,
    /// One discounted charge covering the phase.
    PayUpFront,
    /// The standard renewal price.
    Recurring,
}

/// The result of a purchase call.
#[derive(Debug)]
#[non_exhaustive]
pub enum PurchaseOutcome {
    /// Paid; the purchase is unfinished until [`Purchase::finish`] runs.
    Purchased(Purchase),
    /// Waiting on external action (e.g. Ask to Buy); the purchase completes
    /// or fails through [`StoreEvents`].
    Pending,
    /// The user cancelled the flow.
    Cancelled,
}

/// A paid purchase the app has not finished.
///
/// It cannot be confused with a finished one: only [`Purchase::finish`]
/// produces a [`Transaction`]. Play refunds a purchase that is not
/// acknowledged within three days, so finishing is explicit.
#[must_use = "an unfinished purchase is refunded by Play after three days"]
pub struct Purchase {
    product_id: ProductId,
    kind: ProductKind,
    quantity: u32,
    purchased_at: Timestamp,
    proof: PurchaseProof,
    sys: sys::Purchase,
}

impl std::fmt::Debug for Purchase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Purchase")
            .field("product_id", &self.product_id)
            .field("kind", &self.kind)
            .field("quantity", &self.quantity)
            .field("purchased_at", &self.purchased_at)
            .field("proof", &self.proof)
            .finish_non_exhaustive()
    }
}

impl Purchase {
    /// The product that was bought.
    #[must_use]
    pub const fn product_id(&self) -> &ProductId {
        &self.product_id
    }

    /// The kind the catalog declared for the product.
    #[must_use]
    pub const fn kind(&self) -> ProductKind {
        self.kind
    }

    /// How many units were bought.
    #[must_use]
    pub const fn quantity(&self) -> u32 {
        self.quantity
    }

    /// When the store recorded the purchase.
    #[must_use]
    pub const fn purchased_at(&self) -> Timestamp {
        self.purchased_at
    }

    /// The store's signed proof of purchase, for server verification.
    #[must_use]
    pub const fn proof(&self) -> &PurchaseProof {
        &self.proof
    }

    /// Consumes a consumable, or acknowledges a non-consumable or
    /// subscription (`StoreKit`: `transaction.finish()`).
    ///
    /// Consumed items leave the user's entitlements; acknowledged ones stay.
    ///
    /// # Errors
    /// The store's own error (`network`, `platform`, `unavailable`).
    pub async fn finish(self) -> Result<Transaction, StoreError> {
        let kind = self.kind;
        sys::finish(self.sys, kind).await?;
        Ok(Transaction::new(
            self.product_id,
            kind,
            self.quantity,
            self.purchased_at,
            self.proof,
        ))
    }
}

impl Purchase {
    #[cfg_attr(
        not(any(target_os = "android", target_os = "ios", target_os = "macos")),
        expect(dead_code, reason = "only the platform wire decoder builds these")
    )]
    pub(crate) const fn new(
        product_id: ProductId,
        kind: ProductKind,
        quantity: u32,
        purchased_at: Timestamp,
        proof: PurchaseProof,
        sys: sys::Purchase,
    ) -> Self {
        Self {
            product_id,
            kind,
            quantity,
            purchased_at,
            proof,
            sys,
        }
    }
}

/// A finished purchase record: the product was consumed or acknowledged.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Transaction {
    product_id: ProductId,
    kind: ProductKind,
    quantity: u32,
    purchased_at: Timestamp,
    proof: PurchaseProof,
}

impl Transaction {
    pub(crate) const fn new(
        product_id: ProductId,
        kind: ProductKind,
        quantity: u32,
        purchased_at: Timestamp,
        proof: PurchaseProof,
    ) -> Self {
        Self {
            product_id,
            kind,
            quantity,
            purchased_at,
            proof,
        }
    }

    /// The product that was bought.
    #[must_use]
    pub const fn product_id(&self) -> &ProductId {
        &self.product_id
    }

    /// The kind the catalog declared for the product.
    #[must_use]
    pub const fn kind(&self) -> ProductKind {
        self.kind
    }

    /// How many units were bought.
    #[must_use]
    pub const fn quantity(&self) -> u32 {
        self.quantity
    }

    /// When the store recorded the purchase.
    #[must_use]
    pub const fn purchased_at(&self) -> Timestamp {
        self.purchased_at
    }

    /// The store's signed proof of purchase.
    #[must_use]
    pub const fn proof(&self) -> &PurchaseProof {
        &self.proof
    }
}

/// What the user owns: a purchase still unfinished, or a finished one.
#[derive(Debug)]
#[non_exhaustive]
pub enum Entitlement {
    /// Paid but not yet consumed or acknowledged.
    Unfinished(Purchase),
    /// Consumed or acknowledged.
    Finished(Transaction),
}

/// The store's signed proof of purchase, for server verification.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct PurchaseProof {
    kind: ProofKind,
    value: String,
}

impl PurchaseProof {
    #[cfg_attr(
        not(any(target_os = "android", target_os = "ios", target_os = "macos", test)),
        expect(dead_code, reason = "only the platform wire decoder builds these")
    )]
    pub(crate) const fn new(kind: ProofKind, value: String) -> Self {
        Self { kind, value }
    }

    /// Which platform signature this proof carries.
    #[must_use]
    pub const fn kind(&self) -> ProofKind {
        self.kind
    }

    /// The signed payload: a Play purchase token or an App Store JWS.
    #[must_use]
    pub fn value(&self) -> &str {
        &self.value
    }
}

/// Which platform signature a [`PurchaseProof`] carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ProofKind {
    /// A Play purchase token; verify through the Play Developer API.
    PlayPurchaseToken,
    /// A signed App Store transaction (JWS).
    AppStoreJws,
}

/// Store errors.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum StoreError {
    /// No store on this device: no Play Store app, an OS below the `StoreKit` 2
    /// floor, or an unsupported platform.
    #[error("the platform store is unavailable")]
    Unavailable,
    /// The store does not know this product id.
    #[error("product not found: {0}")]
    ProductNotFound(ProductId),
    /// The platform's product type contradicts the kind the catalog declared.
    #[error("product {product} is not a {declared:?} on the store")]
    KindMismatch {
        /// The product the mismatch is about.
        product: ProductId,
        /// The kind the app's catalog declared.
        declared: ProductKind,
    },
    /// `StoreKit` could not verify the transaction's JWS; the proof is kept
    /// for inspection. Unverified transactions are never returned as
    /// purchases.
    #[error("transaction verification failed ({} proof)", .0.value())]
    Unverified(PurchaseProof),
    /// The product is already owned and cannot be bought again.
    #[error("product already owned: {0}")]
    AlreadyOwned(ProductId),
    /// The store was unreachable or its backend returned a transport error.
    #[error("store network error: {0}")]
    Network(String),
    /// Any other platform-side failure, with the platform's own message.
    #[error("store platform error: {0}")]
    Platform(String),
}

#[cfg(test)]
mod tests;
