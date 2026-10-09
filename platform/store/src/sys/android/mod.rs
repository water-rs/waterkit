//! Android backend: Play Billing through a per-connection Kotlin object.
//!
//! `StoreConnection` (in `StoreHelper.kt`) owns the `BillingClient`; the Rust
//! `Store` keeps it as a JNI global reference and ends the connection on
//! drop. Its `PurchasesUpdatedListener` feeds one `NativeChannel` backing the
//! [`EventStream`], except results belonging to an in-flight
//! `launchBillingFlow`, which complete that call's `NativeCallback`. All
//! replies are JSON decoded by `crate::sys::wire`.

use std::sync::Arc;

use futures::{StreamExt, channel::mpsc};
use jni::objects::{Global, JObject, JString, JValue, JValueOwned};
use jni::{Env, jni_sig, jni_str};
use waterkit_build::{
    AndroidError, DexHelper, NativeCallback, NativeChannel, PeerError, decode_string,
    describe_jni_error, dex_helper, with_android_context,
};

use crate::sys::{EventStream, wire};
use crate::{
    Catalog, Entitlement, OfferToken, Product, ProductId, ProductKind, PurchaseOutcome,
    StoreCapabilities, StoreError, Transaction,
};

/// `waterkit.store.StoreHelper`, the static entry points.
static HELPER: DexHelper = dex_helper!("waterkit.store.StoreHelper");
/// `waterkit.store.StoreConnection`, the per-connection object owning the
/// `BillingClient`.
static CONNECTION: DexHelper = dex_helper!("waterkit.store.StoreConnection");

impl From<AndroidError> for StoreError {
    fn from(error: AndroidError) -> Self {
        Self::Platform(error.to_string())
    }
}

impl From<PeerError> for StoreError {
    fn from(error: PeerError) -> Self {
        Self::Platform(error.to_string())
    }
}

fn jni_error(env: &Env<'_>, call: &str, error: jni::errors::Error) -> StoreError {
    StoreError::Platform(format!("{call}: {}", describe_jni_error(env, error)))
}

/// Awaits a `NativeCallback` reply and decodes its JSON envelope.
async fn await_reply(
    receiver: futures::channel::oneshot::Receiver<Result<String, PeerError>>,
) -> Result<String, StoreError> {
    let reply = receiver
        .await
        .map_err(|_| StoreError::Platform("store call abandoned".into()))??;
    Ok(reply)
}

#[expect(
    clippy::unused_async,
    reason = "the sys contract is async on every platform; this backend's probe is synchronous"
)]
pub async fn capabilities() -> Result<StoreCapabilities, StoreError> {
    let reply = with_android_context(|env, context| {
        let class = HELPER.class(env, context)?;
        let object = env
            .call_static_method(
                class,
                jni_str!("capabilities"),
                &jni_sig!("(Landroid/content/Context;)Ljava/lang/String;"),
                &[JValue::Object(context)],
            )
            .and_then(JValueOwned::l)
            .map_err(|error| jni_error(env, "StoreHelper.capabilities", error))?;
        Ok::<String, StoreError>(decode_string(env, &object)?)
    })?;
    let capabilities: wire::CapabilitiesJson = wire::decode_reply(&reply)?;
    Ok(StoreCapabilities {
        purchases: capabilities.purchases,
    })
}

/// The Play Billing connection: the Kotlin `StoreConnection` global
/// reference.
pub struct Store {
    connection: Global<JObject<'static>>,
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Store").finish_non_exhaustive()
    }
}

/// The Play finish handle: the connection plus the purchase token.
pub struct Purchase {
    connection: Global<JObject<'static>>,
    token: String,
}

impl std::fmt::Debug for Purchase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Purchase")
            .field("token", &self.token)
            .finish_non_exhaustive()
    }
}

impl Store {
    pub async fn connect(catalog: &Arc<Catalog>) -> Result<(Self, EventStream), StoreError> {
        let catalog_json = wire::encode_catalog(catalog);
        let (connection, events_receiver) = with_android_context(|env, context| {
            let (channel, receiver) = NativeChannel::<String>::new(env)?;
            let class = CONNECTION.class(env, context)?;
            let catalog_arg = env
                .new_string(&catalog_json)
                .map_err(|error| jni_error(env, "new_string catalog", error))?;
            let connection = env
                .new_object(
                    class,
                    jni_sig!(
                        "(Landroid/content/Context;Ljava/lang/String;Lwaterkit/build/NativeChannel;)V"
                    ),
                    &[
                        JValue::Object(context),
                        JValue::Object(&catalog_arg),
                        JValue::Object(channel.as_obj()),
                    ],
                )
                .map_err(|error| jni_error(env, "StoreConnection.<init>", error))?;
            let connection = env
                .new_global_ref(connection)
                .map_err(|error| jni_error(env, "retain StoreConnection", error))?;
            Ok::<_, StoreError>((connection, receiver))
        })?;

        let receiver = with_android_context(|env, _context| {
            let (callback, receiver) = NativeCallback::<String>::new(env)?;
            env.call_method(
                connection.as_obj(),
                jni_str!("connect"),
                jni_sig!("(Lwaterkit/build/NativeCallback;)V"),
                &[JValue::Object(callback.as_obj())],
            )
            .map_err(|error| jni_error(env, "StoreConnection.connect", error))?;
            Ok::<_, StoreError>(receiver)
        })?;
        let json = await_reply(receiver).await?;
        wire::decode_reply::<serde::de::IgnoredAny>(&json)?;

        // The events feed needs its own global reference on the connection;
        // a JNI failure here is a connect error.
        let events_connection = with_android_context(|env, _context| {
            env.new_global_ref(&connection)
                .map_err(|error| jni_error(env, "retain StoreConnection", error))
        })?;
        let events = event_stream(events_receiver, events_connection, catalog);

        Ok((Self { connection }, events))
    }

    pub async fn products(&self, catalog: &Catalog) -> Result<Vec<Product>, StoreError> {
        let receiver = with_android_context(|env, _context| {
            let (callback, receiver) = NativeCallback::<String>::new(env)?;
            env.call_method(
                self.connection.as_obj(),
                jni_str!("products"),
                jni_sig!("(Lwaterkit/build/NativeCallback;)V"),
                &[JValue::Object(callback.as_obj())],
            )
            .map_err(|error| jni_error(env, "StoreConnection.products", error))?;
            Ok::<_, StoreError>(receiver)
        })?;
        let json = await_reply(receiver).await?;
        let products: Vec<wire::ProductJson> = wire::decode_reply(&json)?;
        products
            .into_iter()
            .map(|product| product.into_product(catalog))
            .collect()
    }

    pub async fn purchase(
        &self,
        product: &ProductId,
        offer: Option<&OfferToken>,
        catalog: &Catalog,
    ) -> Result<PurchaseOutcome, StoreError> {
        let receiver = with_android_context(|env, _context| {
            let (callback, receiver) = NativeCallback::<String>::new(env)?;
            let product_arg = env
                .new_string(product.as_str())
                .map_err(|error| jni_error(env, "new_string product", error))?;
            let offer_arg = match offer {
                Some(token) => env
                    .new_string(token.as_str())
                    .map_err(|error| jni_error(env, "new_string offer", error))?,
                None => JString::null(),
            };
            env.call_method(
                self.connection.as_obj(),
                jni_str!("purchase"),
                jni_sig!("(Ljava/lang/String;Ljava/lang/String;Lwaterkit/build/NativeCallback;)V"),
                &[
                    JValue::Object(&product_arg),
                    JValue::Object(&offer_arg),
                    JValue::Object(callback.as_obj()),
                ],
            )
            .map_err(|error| jni_error(env, "StoreConnection.purchase", error))?;
            Ok::<_, StoreError>(receiver)
        })?;
        let json = await_reply(receiver).await?;
        let outcome: wire::OutcomeJson = wire::decode_reply(&json)?;
        outcome.into_outcome(|purchase| self.build_purchase(purchase, catalog))
    }

    pub async fn entitlements(&self, catalog: &Catalog) -> Result<Vec<Entitlement>, StoreError> {
        let receiver = with_android_context(|env, _context| {
            let (callback, receiver) = NativeCallback::<String>::new(env)?;
            env.call_method(
                self.connection.as_obj(),
                jni_str!("entitlements"),
                jni_sig!("(Lwaterkit/build/NativeCallback;)V"),
                &[JValue::Object(callback.as_obj())],
            )
            .map_err(|error| jni_error(env, "StoreConnection.entitlements", error))?;
            Ok::<_, StoreError>(receiver)
        })?;
        let json = await_reply(receiver).await?;
        let entitlements: Vec<wire::EntitlementJson> = wire::decode_reply(&json)?;
        entitlements
            .into_iter()
            .map(|entitlement| {
                entitlement.into_entitlement(
                    |purchase| self.build_purchase(purchase, catalog),
                    |purchase| build_transaction(purchase, catalog),
                )
            })
            .collect()
    }

    fn build_purchase(
        &self,
        wire: wire::PurchaseJson,
        catalog: &Catalog,
    ) -> Result<crate::Purchase, StoreError> {
        build_purchase(wire, catalog, &self.connection)
    }
}

impl Drop for Store {
    fn drop(&mut self) {
        let result: Result<(), StoreError> = with_android_context(|env, _context| {
            env.call_method(
                self.connection.as_obj(),
                jni_str!("disconnect"),
                jni_sig!("()V"),
                &[],
            )
            .map_err(|error| jni_error(env, "StoreConnection.disconnect", error))?;
            Ok(())
        });
        if let Err(error) = result {
            tracing::warn!("store disconnect failed: {error}");
        }
    }
}

pub async fn finish(purchase: Purchase, kind: ProductKind) -> Result<(), StoreError> {
    let kind = match kind {
        ProductKind::Consumable => "consumable",
        ProductKind::NonConsumable | ProductKind::Subscription => "non_consumable",
    };
    let receiver = with_android_context(|env, _context| {
        let (callback, receiver) = NativeCallback::<String>::new(env)?;
        let token = env
            .new_string(&purchase.token)
            .map_err(|error| jni_error(env, "new_string token", error))?;
        let kind_arg = env
            .new_string(kind)
            .map_err(|error| jni_error(env, "new_string kind", error))?;
        env.call_method(
            purchase.connection.as_obj(),
            jni_str!("finish"),
            jni_sig!("(Ljava/lang/String;Ljava/lang/String;Lwaterkit/build/NativeCallback;)V"),
            &[
                JValue::Object(&token),
                JValue::Object(&kind_arg),
                JValue::Object(callback.as_obj()),
            ],
        )
        .map_err(|error| jni_error(env, "StoreConnection.finish", error))?;
        Ok::<_, StoreError>(receiver)
    })?;
    let json = await_reply(receiver).await?;
    wire::decode_reply::<serde::de::IgnoredAny>(&json).map(|_| ())
}

/// The transaction feed the connection's `NativeChannel` carries, decoded
/// into purchases. Ends when the channel closes — when the [`Store`] drops
/// and `disconnect` runs.
fn event_stream(
    receiver: mpsc::UnboundedReceiver<Result<String, PeerError>>,
    connection: Global<JObject<'static>>,
    catalog: &Arc<Catalog>,
) -> EventStream {
    let catalog = Arc::clone(catalog);
    receiver
        .filter_map(move |item| {
            let catalog = Arc::clone(&catalog);
            let connection = &connection;
            std::future::ready(match item {
                Err(error) => Some(Err(StoreError::from(error))),
                Ok(json) => match wire::decode_reply::<wire::EventJson>(&json) {
                    Ok(wire::EventJson::Purchase(purchase)) => {
                        Some(build_purchase(*purchase, &catalog, connection))
                    }
                    Ok(wire::EventJson::End(ended)) => {
                        debug_assert!(ended, "store event stream end marker must be true");
                        None
                    }
                    Err(error) => Some(Err(error)),
                },
            })
        })
        .boxed()
}

fn build_purchase(
    wire: wire::PurchaseJson,
    catalog: &Catalog,
    connection: &Global<JObject<'static>>,
) -> Result<crate::Purchase, StoreError> {
    let fields = wire.into_fields(catalog)?;
    let connection = with_android_context(|env, _context| {
        env.new_global_ref(connection)
            .map_err(|error| jni_error(env, "retain StoreConnection", error))
    })?;
    Ok(crate::Purchase::new(
        fields.product_id,
        fields.kind,
        fields.quantity,
        fields.purchased_at,
        fields.proof,
        Purchase {
            connection,
            token: fields.transaction_id,
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
        fields.proof,
    ))
}