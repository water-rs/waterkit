//! Apple backend: `StoreKit` 2 through a `swift-bridge` module.
//!
//! `StoreKit` 2 has no Objective-C API, so the platform side is Swift:
//! `WaterkitStore`, an actor created at connect, owns the
//! `Transaction.updates` listener task that feeds the [`EventStream`]. Every
//! call replies as one JSON document decoded by `crate::sys::wire`.

use std::sync::Arc;

use futures::{StreamExt, channel::oneshot, stream};

use crate::sys::{EventStream, wire};
use crate::{
    Catalog, Entitlement, OfferToken, Product, ProductId, ProductKind, PurchaseOutcome,
    StoreCapabilities, StoreError, Transaction,
};

mod bridge;
use bridge::ffi;

/// Runs a bridge call whose reply arrives through a `FnOnce` callback.
async fn reply(call: impl FnOnce(Box<dyn FnOnce(String)>)) -> Result<String, StoreError> {
    let (sender, receiver) = oneshot::channel::<String>();
    call(Box::new(move |json| {
        let _ = sender.send(json);
    }));
    receiver
        .await
        .map_err(|cancelled| StoreError::Platform(format!("store call abandoned: {cancelled}")))
}

#[expect(
    clippy::unused_async,
    reason = "the sys contract is async on every platform; this backend's probe is synchronous"
)]
pub async fn capabilities() -> Result<StoreCapabilities, StoreError> {
    let capabilities: wire::CapabilitiesJson = wire::decode_reply(&ffi::store_capabilities())?;
    Ok(StoreCapabilities {
        purchases: capabilities.purchases,
    })
}

/// The `StoreKit` session: the opaque `WaterkitStore` actor handle.
pub struct Store {
    handle: ffi::AppleStore,
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Store").finish_non_exhaustive()
    }
}

impl Drop for Store {
    fn drop(&mut self) {
        // The events stream keeps its own retain on the session, so
        // `deinit` would not run here — disconnect ends the updates
        // listener and closes the event queue, ending the stream.
        self.handle.store_disconnect();
    }
}

/// The platform's finish handle: the `StoreKit` transaction id.
#[derive(Debug)]
pub struct Purchase {
    transaction_id: String,
}

impl Store {
    pub async fn connect(catalog: &Arc<Catalog>) -> Result<(Self, EventStream), StoreError> {
        // `canMakePayments` and the OS floor are the platform's own answers —
        // the same probe `capabilities()` reports, checked again so a device
        // that cannot pay fails connect with `Unavailable`.
        if !capabilities().await?.purchases {
            return Err(StoreError::Unavailable);
        }
        let handle = ffi::store_connect(&wire::encode_catalog(catalog));
        let events = event_stream(handle.store_retain(), catalog);
        Ok((Self { handle }, events))
    }

    pub async fn products(&self, catalog: &Catalog) -> Result<Vec<Product>, StoreError> {
        let json = reply(|callback| self.handle.store_products(callback)).await?;
        let products: Vec<wire::ProductJson> = wire::decode_reply(&json)?;
        products
            .into_iter()
            .map(|product| product.into_product(catalog))
            .collect()
    }

    pub async fn purchase(
        &self,
        product: &ProductId,
        _offer: Option<&OfferToken>,
        catalog: &Catalog,
    ) -> Result<PurchaseOutcome, StoreError> {
        let product_id = product.as_str().to_owned();
        let json = reply(|callback| self.handle.store_purchase(&product_id, callback)).await?;
        let outcome: wire::OutcomeJson = wire::decode_reply(&json)?;
        outcome.into_outcome(|purchase| build_purchase(purchase, catalog))
    }

    pub async fn entitlements(&self, catalog: &Catalog) -> Result<Vec<Entitlement>, StoreError> {
        let json = reply(|callback| self.handle.store_entitlements(callback)).await?;
        let entitlements: Vec<wire::EntitlementJson> = wire::decode_reply(&json)?;
        entitlements
            .into_iter()
            .map(|entitlement| {
                entitlement.into_entitlement(
                    |purchase| build_purchase(purchase, catalog),
                    |purchase| build_transaction(purchase, catalog),
                )
            })
            .collect()
    }
}

/// The transaction feed: dequeues one `Transaction.updates` reply per poll
/// from the session's event queue. Ends when the updates stream finishes.
fn event_stream(handle: ffi::AppleStore, catalog: &Arc<Catalog>) -> EventStream {
    let catalog = Arc::clone(catalog);
    stream::unfold(
        (handle, catalog, false),
        |(handle, catalog, done)| async move {
            if done {
                return None;
            }
            let next = reply(|callback| handle.store_next_event(callback)).await;
            let event = next.and_then(|json| wire::decode_reply::<wire::EventJson>(&json));
            match event {
                Ok(wire::EventJson::Purchase(purchase)) => Some((
                    build_purchase(*purchase, &catalog),
                    (handle, catalog, false),
                )),
                Ok(wire::EventJson::End(ended)) => {
                    debug_assert!(ended, "store event stream end marker must be true");
                    None
                }
                Err(error) => Some((Err(error), (handle, catalog, true))),
            }
        },
    )
    .boxed()
}

pub async fn finish(purchase: Purchase, _kind: ProductKind) -> Result<(), StoreError> {
    let json = reply(|callback| ffi::store_finish(&purchase.transaction_id, callback)).await?;
    wire::decode_reply::<serde::de::IgnoredAny>(&json).map(|_| ())
}

fn build_purchase(
    wire: wire::PurchaseJson,
    catalog: &Catalog,
) -> Result<crate::Purchase, StoreError> {
    let fields = wire.into_fields(catalog)?;
    Ok(crate::Purchase::new(
        fields.product_id,
        fields.kind,
        fields.quantity,
        fields.purchased_at,
        Some(fields.proof),
        Purchase {
            transaction_id: fields.transaction_id,
        },
    ))
}

fn build_transaction(
    wire: wire::PurchaseJson,
    catalog: &Catalog,
) -> Result<Transaction, StoreError> {
    let fields = wire.into_fields(catalog)?;
    Ok(Transaction::new(
        fields.product_id,
        fields.kind,
        fields.quantity,
        fields.purchased_at,
        Some(fields.proof),
    ))
}
