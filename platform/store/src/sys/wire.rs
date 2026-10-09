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
    Product, ProductId, ProductKind, ProofKind, Purchase, PurchaseOutcome, PurchaseProof,
    StoreError, Subscription, Timestamp, Transaction,
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

const fn kind_wire(kind: ProductKind) -> &'static str {
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

/// The platform's side of a failed call: one variant per [`StoreError`]
/// case, each carrying exactly the fields that error needs. An unknown kind
/// or a missing field is a decode error and surfaces as
/// `StoreError::Platform("malformed store reply: …")`.
#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ErrorJson {
    /// No store on this device.
    Unavailable,
    /// The store does not know the product.
    ProductNotFound {
        /// The product id.
        product: String,
    },
    /// The store's product type contradicts the declared kind.
    KindMismatch {
        /// The product id.
        product: String,
        /// The kind the app's catalog declared.
        declared: ProductKind,
    },
    /// Transaction verification failed; the proof is kept for inspection.
    Unverified {
        /// The transaction's signed proof.
        proof: ProofJson,
    },
    /// The product is already owned.
    AlreadyOwned {
        /// The product id.
        product: String,
    },
    /// The store was unreachable or its backend errored.
    Network {
        /// The platform's message.
        message: String,
    },
    /// Any other platform-side failure.
    Platform {
        /// The platform's message.
        message: String,
    },
}

/// A proof as the platform reports it.
#[derive(Debug, Deserialize)]
pub struct ProofJson {
    /// Which platform signature the proof carries.
    pub kind: ProofKind,
    /// The signed payload.
    pub value: String,
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
        match error {
            ErrorJson::Unavailable => Self::Unavailable,
            ErrorJson::ProductNotFound { product } => {
                Self::ProductNotFound(ProductId::new(product))
            }
            ErrorJson::KindMismatch { product, declared } => Self::KindMismatch {
                product: ProductId::new(product),
                declared,
            },
            ErrorJson::Unverified { proof } => Self::Unverified(proof.into_proof()),
            ErrorJson::AlreadyOwned { product } => Self::AlreadyOwned(ProductId::new(product)),
            ErrorJson::Network { message } => Self::Network(message),
            ErrorJson::Platform { message } => Self::Platform(message),
        }
    }
}

/// The store-reported product type, decoded to compare against the
/// catalog's declared kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StoreKind {
    /// Play `INAPP`: Play does not distinguish consumables, so this matches
    /// either in-app [`ProductKind`].
    InApp,
    /// `StoreKit` `consumable`.
    Consumable,
    /// `StoreKit` `nonConsumable`.
    NonConsumable,
    /// Play `SUBS` or `StoreKit` `autoRenewable`.
    Subscription,
    /// `StoreKit` `nonRenewable`: a subscription-shaped product this API does
    /// not model — it has no renewal period or offers.
    NonRenewableSubscription,
}

impl StoreKind {
    /// Whether the catalog-declared `declared` describes this store type.
    ///
    /// `InApp` matches either in-app kind: Play reports `INAPP` for both, so
    /// the catalog's declaration is the source of truth there.
    pub const fn matches(self, declared: ProductKind) -> bool {
        matches!(
            (declared, self),
            (ProductKind::Consumable, Self::Consumable | Self::InApp)
                | (
                    ProductKind::NonConsumable,
                    Self::NonConsumable | Self::InApp
                )
                | (ProductKind::Subscription, Self::Subscription)
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

impl ProductJson {
    /// Converts into the public [`Product`]; the store's product type must
    /// agree with the catalog's declared kind.
    pub fn into_product(self, catalog: &Catalog) -> Result<Product, StoreError> {
        let id = ProductId::new(self.id);
        let kind = catalog
            .kind(&id)
            .ok_or_else(|| StoreError::ProductNotFound(id.clone()))?;
        if !self.store_kind.matches(kind) {
            return Err(StoreError::KindMismatch {
                product: id,
                declared: kind,
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
        if kind == ProductKind::Subscription && subscription.is_none() {
            return Err(StoreError::Platform(format!(
                "subscription product {} carries no terms",
                id.as_str()
            )));
        }
        Ok(Product::new(
            id,
            kind,
            self.title,
            self.description,
            self.price.into_price(),
            subscription,
        ))
    }
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
    /// The platform's transaction handle: the `StoreKit` transaction id or the
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
    /// The purchase record.
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
    #[cfg_attr(
        not(any(target_os = "android", target_os = "ios", target_os = "macos")),
        expect(
            dead_code,
            reason = "only the platform backends consume event payloads"
        )
    )]
    Purchase(Box<PurchaseJson>),
    /// The platform's update stream ended; the payload is `true`.
    End(bool),
}

// ---- wire → public types ----

impl ProofJson {
    fn into_proof(self) -> PurchaseProof {
        PurchaseProof::new(self.kind, self.value)
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
    pub const fn into_period(self) -> Period {
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

/// The decoded fields of a [`Purchase`] — everything except the platform's
/// finish handle, which the sys backend supplies.
#[derive(Debug)]
pub struct PurchaseFields {
    /// The product bought.
    pub product_id: ProductId,
    /// The catalog-declared kind.
    pub kind: ProductKind,
    /// Units bought.
    pub quantity: u32,
    /// When the store recorded it.
    pub purchased_at: Timestamp,
    /// The signed proof.
    pub proof: PurchaseProof,
    /// The platform's transaction handle (`StoreKit` id / Play token).
    pub transaction_id: String,
}

impl PurchaseJson {
    /// Decodes the purchase fields; the product must be in the catalog.
    pub fn into_fields(self, catalog: &Catalog) -> Result<PurchaseFields, StoreError> {
        let product_id = ProductId::new(self.product_id);
        let kind = catalog
            .kind(&product_id)
            .ok_or_else(|| StoreError::ProductNotFound(product_id.clone()))?;
        let purchased_at = Timestamp::from_millisecond(self.purchased_ms)
            .map_err(|error| StoreError::Platform(format!("bad purchase timestamp: {error}")))?;
        Ok(PurchaseFields {
            product_id,
            kind,
            quantity: self.quantity,
            purchased_at,
            proof: self.proof.into_proof(),
            transaction_id: self.transaction_id,
        })
    }
}

impl OutcomeJson {
    /// Maps the outcome; `build` turns a purchased record into a [`Purchase`]
    /// with the platform's finish handle.
    #[cfg_attr(
        not(any(target_os = "android", target_os = "ios", target_os = "macos")),
        expect(dead_code, reason = "only the platform backends map purchases")
    )]
    pub fn into_outcome(
        self,
        build: impl FnOnce(PurchaseJson) -> Result<Purchase, StoreError>,
    ) -> Result<PurchaseOutcome, StoreError> {
        Ok(match self {
            Self::Purchased { purchase } => PurchaseOutcome::Purchased(build(*purchase)?),
            Self::Pending => PurchaseOutcome::Pending,
            Self::Cancelled => PurchaseOutcome::Cancelled,
        })
    }
}

impl EntitlementJson {
    /// Maps the entitlement; `build_purchase`/`build_transaction` attach the
    /// platform's finish handle / produce the public records.
    #[cfg_attr(
        not(any(target_os = "android", target_os = "ios", target_os = "macos")),
        expect(dead_code, reason = "only the platform backends map purchases")
    )]
    pub fn into_entitlement(
        self,
        build_purchase: impl FnOnce(PurchaseJson) -> Result<Purchase, StoreError>,
        build_transaction: impl FnOnce(PurchaseJson) -> Result<Transaction, StoreError>,
    ) -> Result<Entitlement, StoreError> {
        if self.finished {
            Ok(Entitlement::Finished(build_transaction(self.purchase)?))
        } else {
            Ok(Entitlement::Unfinished(build_purchase(self.purchase)?))
        }
    }
}