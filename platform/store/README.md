# waterkit-store

In-app purchases and subscriptions through the platform's store backend:

- **Android**: Play Billing Library 9 through a Kotlin helper compiled into
  the host app. The app declares its products and their kinds in a
  [`Catalog`] — Play does not distinguish consumables from non-consumables,
  so the app decides at finish time by consuming or acknowledging.
- **iOS / macOS**: StoreKit 2 (no Objective-C API exists). Requires
  iOS 15 / macOS 12 or newer; earlier OS versions report the store as
  unavailable.
- **Windows, Linux, wasm**: report as unavailable.

## Usage

```rust
use waterkit_store::{Catalog, ProductId, ProductKind, Store};

let catalog = Catalog::new()
    .with(ProductId::new("app.coins.small"), ProductKind::Consumable)
    .with(ProductId::new("app.pro"), ProductKind::NonConsumable)
    .with(ProductId::new("app.sub.monthly"), ProductKind::Subscription);

let (store, mut events) = Store::connect(catalog).await?;
let products = store.products().await?;
```

`events` is a [`StoreEvents`] stream of discrete transactions that complete
outside a purchase call: Ask to Buy settlements, purchases made on another
device, and — where the platform delivers them to the client — renewals. It
ends when the `Store` drops. [`capabilities()`] reports whether the device
can purchase at all, without connecting; a device with no store answers
`Ok` with `purchases` unset, and only a probe failure is a `StoreError`.

Every paid purchase is a [`Purchase`] and stays *unfinished* until the app
calls [`Purchase::finish`]: finishing consumes a consumable or acknowledges a
non-consumable / subscription. Play refunds a purchase that is never
acknowledged within three days, so finishing is explicit.

Subscriptions are bought through one of the product's [`Offer`]s. On Play the
offer token selects the base plan / pricing phase set the purchase runs
under; on Apple the standard price plus any introductory offer the user is
eligible for applies automatically, so the offer token carries no handle into
StoreKit. Promotional and win-back offers that need a server signature are
out of scope.

[`Entitlement`] reports what the user currently owns — non-consumables and
active subscriptions — split into `Unfinished` and `Finished` records.

## Platform notes

- **Renewals on Android are not delivered to the app.** Play Billing only
  pushes subscription renewals to a backend via Real-time Developer
  Notifications; `PurchasesUpdatedListener` does not fire for them, so
  `StoreEvents` cannot report renewals on Android. It does report pending
  purchases that settle and purchases initiated outside an in-flight
  `launchBillingFlow` the listener sees.
- **`launchBillingFlow` needs an `Activity`.** The published Android context
  must be an `Activity` (or `ComponentActivity`) when `purchase` or
  `subscribe` is called; otherwise the call fails with a platform error.
- **Transaction verification**: App Store transactions arrive signed (JWS);
  `VerificationResult.unverified` surfaces as [`StoreError::Unverified`]
  carrying the proof for inspection — an unverified transaction is never
  returned as a `Purchase`. On Play the proof is the purchase token, which a
  backend verifies through the Play Developer API.
