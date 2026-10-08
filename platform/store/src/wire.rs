//! The JSON wire format between Rust and the platform stores.
//!
//! Every call crosses the bridge as one JSON document. Rust → platform
//! carries the catalog; platform → Rust replies are an envelope:
//! `{"ok": <payload>}` or `{"error": {"kind": ..., ...}}`. Decoding lives
//! here, shared by the Android and Apple backends and exercised by the
//! host-side unit tests.

use serde::{Deserialize, Serialize};

use crate::{
    Catalog, Entitlement, Offer, OfferToken, Period, PeriodUnit, PhaseMode, Price, PricingPhase,
    Product, ProductId, ProductKind, ProofKind, Purchase, PurchaseProof, PurchaseOutcome,
    StoreError, Subscription, Transaction,
};

/// Serializes the catalog the platform helper was constructed with.
#[derive(Serialize)]
struct CatalogJson<'a> {
    products: Vec<CatalogProductJson<'a>>,
}

#[derive(Serialize)]
struct CatalogProductJson<'a> {
    id: &'a str,
    kind: &'static str,
}

/// The catalog document a platform helper reads.
pub fn encode_catalog(catalog: &Catalog) -> String {
    let products = catalog
        .ids()
        .map(|id| CatalogProductJson {
            id: id.as_str(),
            kind: kind_wire(catalog.kind(id).expect("ids() yields catalog keys")),
        })
        .collect();
    serde_json::to_string(&CatalogJson { products })
        .expect("serializing a catalog of strings cannot fail")
}

fn kind_wire(kind: ProductKind) -> &'static str {
    match kind {
        ProductKind::Consumable => "consumable",
        ProductKind::NonConsumable => "non_consumable",
        ProductKind::Subscription => "subscription",
    }
}

/// A reply envelope: `{"ok": ...}` or `{"error": {...}}`.
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Envelope<T> {
    Ok(T),
    Error(ErrorJson),
}

/// The platform's side of a failed call.
#[derive(Debug, Deserialize)]
pub struct ErrorJson {
    kind: String,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    product: Option<String>,
    #[serde(default)]
    proof: Option<ProofJson>,
}

/// A proof as the platform reports it.
#[derive(Debug, Deserialize)]
pub struct ProofJson {
    kind: String,
    value: String,
}

/// Decodes a reply envelope into a payload or a [`StoreError`].
pub fn decode_reply<T: for<'de> Deserialize<'de>>(json: &str) -> Result<T, StoreError> {
    let envelope: Envelope<T> = serde_json::from_str(json)
        .map_err(|error| StoreError::Platform(format!("malformed store reply: {error}")))?;
    match envelope {
        Envelope::Ok(payload) => Ok(payload),
        Envelope::Error(error) => Err(error.into()),
    }
}

impl From<ErrorJson> for StoreError {
    fn from(error: ErrorJson) -> Self {
        let message = || error.message.clone().unwrap_or_else(|| error.kind.clone());
        let product = || ProductId::new(error.product.clone().unwrap_or_default());
        match error.kind.as_str() {
            "unavailable" => StoreError::Unavailable,
            "product_not_found" => StoreError::ProductNotFound(product()),
            "kind_mismatch" => StoreError::KindMismatch {
                product: product(),
                // The platform reports the store's own product type; the
                // declared kind is the catalog's, which only the caller knows.
                declared: ProductKind::Consumable,
            },
            "unverified" => StoreError::Unverified(
                error
                    .proof
                    .map(ProofJson::into_proof)
                    .unwrap_or_else(|| PurchaseProof::new(ProofKind::AppStoreJws, String::new())),
            ),
            "already_owned" => StoreError::AlreadyOwned(product()),
            "network" => StoreError::Network(message()),
            _ => StoreError::Platform(message()),
        }
    }
}

/// `kind_mismatch` needs the catalog's declared kind, which the wire error
/// does not carry; the caller rewrites the placeholder.
pub fn with_declared_kind(error: StoreError, declared: ProductKind) -> StoreError {
    match error {
        StoreError::KindMismatch { product, .. } => StoreError::KindMismatch { product, declared },
        other => other,
    }
}

/// The store-reported product type, decoded back into a [`ProductKind`] the
/// catalog's declaration is checked against.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StoreKind {
    /// Play `INAPP` or StoreKit `consumable`.
    Consumable,
    /// StoreKit `nonConsumable`.
    NonConsumable,
    /// Play `SUBS` or StoreKit `autoRenewable`.
    Subscription,
    /// StoreKit `nonRenewable`: a subscription-shaped product this API does
    /// not model — it has no renewal period or offers.
    NonRenewableSubscription,
}

impl StoreKind {
    /// Whether the catalog-declared `declared` describes this store type.
    pub fn matches(self, declared: ProductKind) -> bool {
        matches!(
            (declared, self),
            (ProductKind::Consumable, StoreKind::Consumable)
                | (ProductKind::NonConsumable, StoreKind::NonConsumable)
                | (ProductKind::Subscription, StoreKind::Subscription)
        )
    }
}

/// The capabilities reply: `{"purchases": bool}`.
#[derive(Deserialize)]
pub struct CapabilitiesJson {
    /// Whether the platform store can take purchases.
    pub purchases: bool,
}

/// A product as the platform reports it.
#[derive(Debug, Deserialize)]
pub struct ProductJson {
    /// The product's console identifier.
    pub id: String,
    /// The store's own product type.
    pub store_kind: StoreKind,
    /// Localized product name.
    pub title: String,
    /// Localized product description.
    #[serde(default)]
    pub description: String,
    /// One-time price, or the subscription's base price.
    pub price: PriceJson,
    /// Subscription terms, present when the product is one.
    #[serde(default)]
    pub subscription: Option<SubscriptionJson>,
}

/// A price as the platform reports it.
#[derive(Debug, Clone, Deserialize)]
pub struct PriceJson {
    /// The localized price string.
    pub formatted: String,
    /// Millionths of a currency unit.
    pub micros: i64,
    /// ISO 4217 currency code.
    pub currency: String,
}

/// A period as the platform reports it.
#[derive(Debug, Clone, Copy, Deserialize)]
pub struct PeriodJson {
    /// `day` | `week` | `month` | `year`.
    pub unit: PeriodUnitJson,
    /// How many units.
    pub count: u32,
}

/// A period unit as the platform reports it.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PeriodUnitJson {
    /// Days.
    Day,
    /// Weeks.
    Week,
    /// Months.
    Month,
    /// Years.
    Year,
}

/// Subscription terms as the platform reports them.
#[derive(Debug, Deserialize)]
pub struct SubscriptionJson {
    /// The renewal period.
    pub period: PeriodJson,
    /// The offers the subscription sells under.
    #[serde(default)]
    pub offers: Vec<OfferJson>,
}

/// An offer as the platform reports it.
#[derive(Debug, Deserialize)]
pub struct OfferJson {
    /// The platform's opaque offer token.
    pub token: String,
    /// The pricing phases, ending in a recurring one.
    #[serde(default)]
    pub phases: Vec<PhaseJson>,
}

/// A pricing phase as the platform reports it.
#[derive(Debug, Deserialize)]
pub struct PhaseJson {
    /// What it charges.
    pub price: PriceJson,
    /// How long one cycle lasts.
    pub period: PeriodJson,
    /// Cycle count; absent recurs forever.
    #[serde(default)]
    pub cycles: Option<u32>,
    /// `free_trial` | `pay_as_you_go` | `pay_up_front` | `recurring`.
    pub mode: PhaseModeJson,
}

/// A phase mode as the platform reports it.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PhaseModeJson {
    /// Free cycles.
    FreeTrial,
    /// Discounted cycles.
    PayAsYouGo,
    /// One discounted charge.
    PayUpFront,
    /// Standard renewal price.
    Recurring,
}

/// A purchase as the platform reports it.
#[derive(Debug, Deserialize)]
pub struct PurchaseJson {
    /// The product bought.
    pub product_id: String,
    /// Units bought.
    #[serde(default = "one")]
    pub quantity: u32,
    /// Purchase time, epoch milliseconds.
    pub purchased_ms: i64,
    /// The platform's transaction handle: the StoreKit transaction id or the
    /// Play purchase token.
    pub transaction_id: String,
    /// The signed proof.
    pub proof: ProofJson,
}

const fn one() -> u32 {
    1
}

/// A purchase-call outcome as the platform reports it.
#[derive(Debug, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum OutcomeJson {
    /// Paid.
    Purchased {
        /// The purchase record.
        purchase: Box<PurchaseJson>,
    },
    /// Waiting on external action.
    Pending,
    /// The user cancelled.
    Cancelled,
}

/// An entitlement as the platform reports it.
#[derive(Debug, Deserialize)]
pub struct EntitlementJson {
    /// The product record.
    #[serde(flatten)]
    pub purchase: PurchaseJson,
    /// Whether the purchase was already consumed / acknowledged / finished.
    pub finished: bool,
}

/// An event-stream item: a purchase, or a terminal `{"end": true}`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventJson {
    /// A transaction completed outside a purchase call.
    Purchase(Box<PurchaseJson>),
    /// The platform's update stream ended.
    End,
}

// ---- wire → public types ----

impl ProofJson {
    fn into_proof(self) -> PurchaseProof {
        let kind = match self.kind.as_str() {
            "play_purchase_token" => ProofKind::PlayPurchaseToken,
            _ => ProofKind::AppStoreJws,
        };
        PurchaseProof::new(kind, self.value)
    }
}

impl PriceJson {
    /// Converts into the public [`Price`].
    pub fn into_price(self) -> Price {
        Price {
            formatted: self.formatted,
            amount_micros: self.micros,
            currency: self.currency,
        }
    }
}

impl PeriodJson {
    /// Converts into the public [`Period`].
    pub fn into_period(self) -> Period {
        Period {
            unit: match self.unit {
                PeriodUnitJson::Day => PeriodUnit::Day,
                PeriodUnitJson::Week => PeriodUnit::Week,
                PeriodUnitJson::Month => PeriodUnit::Month,
                PeriodUnitJson::Year => PeriodUnit::Year,
            },
            count: self.count,
        }
    }
}

impl PhaseJson {
    /// Converts into the public [`PricingPhase`].
    pub fn into_phase(self) -> PricingPhase {
        PricingPhase {
            price: self.price.into_price(),
            period: self.period.into_period(),
            cycles: self.cycles,
            mode: match self.mode {
                PhaseModeJson::FreeTrial => PhaseMode::FreeTrial,
                PhaseModeJson::PayAsYouGo => PhaseMode::PayAsYouGo,
                PhaseModeJson::PayUpFront => PhaseMode::PayUpFront,
                PhaseModeJson::Recurring => PhaseMode::Recurring,
            },
        }
    }
}

impl OfferJson {
    /// Converts into the public [`Offer`].
    pub fn into_offer(self) -> Offer {
        Offer::new(
            OfferToken::new(self.token),
            self.phases.into_iter().map(PhaseJson::into_phase).collect(),
        )
    }
}

impl ProductJson {
    /// Checks the store's product type against the catalog's declared kind
    /// and converts into the public [`Product`].
    pub fn into_product(self, catalog: &Catalog) -> Result<Product, StoreError> {
        let id = ProductId::new(self.id);
        let declared = catalog
            .kind(&id)
            .ok_or_else(|| StoreError::ProductNotFound(id.clone()))?;
        if !self.store_kind.matches(declared) {
            return Err(StoreError::KindMismatch {
                product: id,
                declared,
            });
        }
        let subscription = self.subscription.map(|subscription| Subscription {
            period: subscription.period.into_period(),
            offers: subscription
                .offers
                .into_iter()
                .map(OfferJson::into_offer)
                .collect(),
        });
        Ok(Product::new(
            id,
            declared,
            self.title,
            self.description,
            self.price.into_price(),
            subscription,
        ))
    }
}

impl PurchaseJson {
    /// The purchase fields, without the platform's finish handle.
    pub fn into_parts(
        self,
        catalog: &Catalog,
    ) -> Result<(ProductId, ProductKind, u32, crate::Timestamp, PurchaseProof, String), StoreError>
    {
        let product_id = ProductId::new(self.product_id);
        let kind = catalog
            .kind(&product_id)
            .ok_or_else(|| StoreError::ProductNotFound(product_id.clone()))?;
        let purchased_at = crate::Timestamp::from_millisecond(self.purchased_ms)
            .map_err(|error| StoreError::Platform(format!("bad purchase timestamp: {error}")))?;
        Ok((
            product_id,
            kind,
            self.quantity,
            purchased_at,
            self.proof.into_proof(),
            self.transaction_id,
        ))
    }
}

impl OutcomeJson {
    /// Converts the outcome, giving the sys layer the purchase record when
    /// the outcome is `Purchased`.
    pub fn map_purchase<T>(self, build: impl FnOnce(PurchaseJson) -> T) -> (Option<T>, Outcome) {
        match self {
            OutcomeJson::Purchased { purchase } => (Some(build(*purchase)), Outcome::Purchased),
            OutcomeJson::Pending => (None, Outcome::Pending),
            OutcomeJson::Cancelled => (None, Outcome::Cancelled),
        }
    }
}

/// The non-purchase half of an outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Paid.
    Purchased,
    /// Waiting on external action.
    Pending,
    /// The user cancelled.
    Cancelled,
}
