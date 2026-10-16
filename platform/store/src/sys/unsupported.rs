//! Platforms without a store backend: Linux and wasm.

#![expect(
    clippy::unused_async,
    reason = "the sys contract is async on every platform"
)]

use crate::sys::EventStream;
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
    pub async fn connect(_catalog: &Catalog) -> Result<(Self, EventStream), StoreError> {
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

    pub async fn entitlements(&self, _catalog: &Catalog) -> Result<Vec<Entitlement>, StoreError> {
        Err(StoreError::Unavailable)
    }
}

pub async fn finish(_purchase: Purchase, _kind: ProductKind) -> Result<(), StoreError> {
    Err(StoreError::Unavailable)
}
