//! Platforms without a store backend: Linux, wasm, and Windows.
//!
//! Windows is deliberately unsupported rather than partial: the
//! `Windows.Services.Store` surface has no per-purchase signed proof
//! (`GetCustomerPurchaseIdAsync` yields a user-level collections token, not
//! a transaction signature), no pending-purchase state, and no transaction
//! update stream — `OfflineLicensesChanged` is a license invalidation, not a
//! purchase feed. The public API's `Purchase::proof` and `Store::events`
//! cannot be honored there.

#![expect(
    clippy::unused_async,
    reason = "the sys contract is async on every platform"
)]

use futures::{Stream, stream};

use crate::{
    Catalog, Entitlement, OfferToken, Product, ProductId, ProductKind, PurchaseOutcome,
    StoreCapabilities, StoreError,
};

pub async fn capabilities() -> Result<StoreCapabilities, StoreError> {
    Ok(StoreCapabilities { purchases: false })
}

/// No store connects on this platform.
#[derive(Debug)]
pub struct Store {
    _private: (),
}

/// No purchase is ever created on this platform.
#[derive(Debug)]
pub struct Purchase {
    _private: (),
}

impl Store {
    pub async fn connect(_catalog: &Catalog) -> Result<Self, StoreError> {
        Err(StoreError::Unavailable)
    }

    pub async fn products(&self, _catalog: &Catalog) -> Result<Vec<Product>, StoreError> {
        Err(StoreError::Unavailable)
    }

    pub async fn purchase(
        &self,
        _id: &ProductId,
        _offer: Option<&OfferToken>,
        _catalog: &Catalog,
    ) -> Result<PurchaseOutcome, StoreError> {
        Err(StoreError::Unavailable)
    }

    #[expect(
        clippy::unused_self,
        reason = "the sys contract keeps &self on every platform"
    )]
    pub fn events(
        &self,
        _catalog: &Catalog,
    ) -> impl Stream<Item = Result<crate::Purchase, StoreError>> + Send + use<> {
        stream::empty()
    }

    pub async fn entitlements(&self, _catalog: &Catalog) -> Result<Vec<Entitlement>, StoreError> {
        Err(StoreError::Unavailable)
    }
}

pub async fn finish(_purchase: Purchase, _kind: ProductKind) -> Result<(), StoreError> {
    Err(StoreError::Unavailable)
}
