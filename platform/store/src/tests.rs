//! Host-side unit tests: catalog/kind checks, wire decoding, and the
//! outcome/entitlement mapping.

use crate::sys::wire;
use crate::{Catalog, PeriodUnit, PhaseMode, ProductId, ProductKind, ProofKind, StoreError};

fn catalog() -> Catalog {
    Catalog::new()
        .with(ProductId::new("app.coins"), ProductKind::Consumable)
        .with(ProductId::new("app.pro"), ProductKind::NonConsumable)
        .with(ProductId::new("app.sub"), ProductKind::Subscription)
}

#[test]
fn catalog_tracks_declared_kinds() {
    let catalog = catalog();
    assert_eq!(
        catalog.kind(&ProductId::new("app.coins")),
        Some(ProductKind::Consumable)
    );
    assert_eq!(
        catalog.kind(&ProductId::new("app.pro")),
        Some(ProductKind::NonConsumable)
    );
    assert_eq!(
        catalog.kind(&ProductId::new("app.sub")),
        Some(ProductKind::Subscription)
    );
    assert_eq!(catalog.kind(&ProductId::new("app.missing")), None);
    assert_eq!(catalog.ids().count(), 3);
}

#[test]
fn encode_catalog_emits_ids_and_kinds() {
    let json = wire::encode_catalog(&catalog());
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    let products = value["products"].as_array().unwrap();
    assert_eq!(products.len(), 3);
    assert!(
        products
            .iter()
            .any(|p| p["id"] == "app.coins" && p["kind"] == "consumable")
    );
    assert!(
        products
            .iter()
            .any(|p| p["id"] == "app.pro" && p["kind"] == "non_consumable")
    );
    assert!(
        products
            .iter()
            .any(|p| p["id"] == "app.sub" && p["kind"] == "subscription")
    );
}

#[test]
fn decode_reply_reads_ok_payload() {
    let caps: wire::CapabilitiesJson = wire::decode_reply(r#"{"ok":{"purchases":true}}"#).unwrap();
    assert!(caps.purchases);
}

#[test]
fn decode_reply_maps_unavailable() {
    let error = wire::decode_reply::<serde_json::Value>(
        r#"{"error":{"kind":"unavailable","message":"no store"}}"#,
    )
    .unwrap_err();
    assert!(matches!(error, StoreError::Unavailable));
}

#[test]
fn decode_reply_maps_product_not_found() {
    let error = wire::decode_reply::<serde_json::Value>(
        r#"{"error":{"kind":"product_not_found","product":"app.gone"}}"#,
    )
    .unwrap_err();
    assert!(matches!(
        error,
        StoreError::ProductNotFound(ref id) if id.as_str() == "app.gone"
    ));
}

#[test]
fn decode_reply_maps_kind_mismatch() {
    let error = wire::decode_reply::<serde_json::Value>(
        r#"{"error":{"kind":"kind_mismatch","product":"app.sub","declared":"subscription"}}"#,
    )
    .unwrap_err();
    assert!(matches!(
        error,
        StoreError::KindMismatch {
            ref product,
            declared: ProductKind::Subscription
        } if product.as_str() == "app.sub"
    ));
}

#[test]
fn decode_reply_maps_unverified() {
    let error = wire::decode_reply::<serde_json::Value>(
        r#"{"error":{"kind":"unverified","proof":{"kind":"app_store_jws","value":"abc"}}}"#,
    )
    .unwrap_err();
    assert!(matches!(
        error,
        StoreError::Unverified(ref proof)
            if proof.kind() == ProofKind::AppStoreJws && proof.value() == "abc"
    ));
}

#[test]
fn decode_reply_maps_already_owned() {
    let error = wire::decode_reply::<serde_json::Value>(
        r#"{"error":{"kind":"already_owned","product":"app.pro"}}"#,
    )
    .unwrap_err();
    assert!(matches!(
        error,
        StoreError::AlreadyOwned(ref id) if id.as_str() == "app.pro"
    ));
}

#[test]
fn decode_reply_maps_network() {
    let error = wire::decode_reply::<serde_json::Value>(
        r#"{"error":{"kind":"network","message":"offline"}}"#,
    )
    .unwrap_err();
    assert!(matches!(error, StoreError::Network(ref m) if m == "offline"));
}

#[test]
fn decode_reply_maps_platform() {
    let error = wire::decode_reply::<serde_json::Value>(
        r#"{"error":{"kind":"platform","message":"billing error"}}"#,
    )
    .unwrap_err();
    assert!(matches!(
        error,
        StoreError::Platform(ref m) if m == "billing error"
    ));
}

#[test]
fn decode_reply_rejects_malformed_errors() {
    let malformed = |json: &str| {
        let error = wire::decode_reply::<serde_json::Value>(json).unwrap_err();
        assert!(
            matches!(error, StoreError::Platform(ref m) if m.starts_with("malformed store reply")),
            "{json} decoded to {error:?}"
        );
    };

    // An unknown kind is not a store error.
    malformed(r#"{"error":{"kind":"bogus","message":"x"}}"#);
    // A missing required field fails the decode.
    malformed(r#"{"error":{"kind":"product_not_found"}}"#);
    malformed(r#"{"error":{"kind":"kind_mismatch","product":"app.sub"}}"#);
    malformed(r#"{"error":{"kind":"unverified"}}"#);
    malformed(r#"{"error":{"kind":"network"}}"#);
    // An unknown proof kind fails the decode.
    malformed(r#"{"error":{"kind":"unverified","proof":{"kind":"bogus","value":"x"}}}"#);
}

fn product_json(kind: &str, id: &str) -> String {
    format!(
        r#"{{"id":"{id}","store_kind":"{kind}","title":"T","description":"D","price":{{"formatted":"$1","micros":1000000,"currency":"USD"}}}}"#
    )
}

#[test]
fn product_decoding_enforces_declared_kind() {
    let catalog = catalog();
    let product: wire::ProductJson =
        serde_json::from_str(&product_json("in_app", "app.coins")).unwrap();
    assert_eq!(
        product.into_product(&catalog).unwrap().kind(),
        ProductKind::Consumable
    );

    // Play reports `inapp` for both in-app kinds.
    let product: wire::ProductJson =
        serde_json::from_str(&product_json("in_app", "app.pro")).unwrap();
    assert_eq!(
        product.into_product(&catalog).unwrap().kind(),
        ProductKind::NonConsumable
    );

    let product: wire::ProductJson =
        serde_json::from_str(&product_json("consumable", "app.pro")).unwrap();
    assert!(matches!(
        product.into_product(&catalog),
        Err(StoreError::KindMismatch {
            declared: ProductKind::NonConsumable,
            ..
        })
    ));

    let product: wire::ProductJson =
        serde_json::from_str(&product_json("non_renewable_subscription", "app.sub")).unwrap();
    assert!(matches!(
        product.into_product(&catalog),
        Err(StoreError::KindMismatch {
            declared: ProductKind::Subscription,
            ..
        })
    ));

    let product: wire::ProductJson =
        serde_json::from_str(&product_json("in_app", "app.missing")).unwrap();
    assert!(matches!(
        product.into_product(&catalog),
        Err(StoreError::ProductNotFound(_))
    ));
}

#[test]
fn subscription_wire_decodes_offers() {
    let catalog = catalog();
    let json = r#"{"id":"app.sub","store_kind":"subscription","title":"Pro","description":"D","price":{"formatted":"$4.99","micros":4990000,"currency":"USD"},"subscription":{"period":{"unit":"month","count":1},"offers":[{"token":"base","phases":[{"price":{"formatted":"$0","micros":0,"currency":"USD"},"period":{"unit":"week","count":1},"cycles":1,"mode":"free_trial"},{"price":{"formatted":"$4.99","micros":4990000,"currency":"USD"},"period":{"unit":"month","count":1},"cycles":null,"mode":"recurring"}]}]}}"#;
    let product: wire::ProductJson = serde_json::from_str(json).unwrap();
    let product = product.into_product(&catalog).unwrap();
    let subscription = product.subscription().expect("subscription info");
    assert_eq!(subscription.period.unit, PeriodUnit::Month);
    assert_eq!(subscription.period.count, 1);
    let offer = subscription.offers.first().expect("one offer");
    assert_eq!(offer.token().as_str(), "base");
    assert_eq!(offer.phases().len(), 2);
    assert_eq!(offer.phases()[0].mode, PhaseMode::FreeTrial);
    assert_eq!(offer.phases()[0].cycles, Some(1));
    assert_eq!(offer.phases()[1].mode, PhaseMode::Recurring);
    assert_eq!(offer.phases()[1].cycles, None);
}

#[test]
fn outcome_decoding_maps_purchased_pending_cancelled() {
    let purchased: wire::OutcomeJson = serde_json::from_str(
        r#"{"outcome":"purchased","purchase":{"product_id":"app.coins","quantity":1,"purchased_ms":1700000000000,"transaction_id":"tok","proof":{"kind":"play_purchase_token","value":"tok"}}}"#,
    )
    .unwrap();
    assert!(matches!(purchased, wire::OutcomeJson::Purchased { .. }));

    let pending: wire::OutcomeJson = serde_json::from_str(r#"{"outcome":"pending"}"#).unwrap();
    assert!(matches!(pending, wire::OutcomeJson::Pending));

    let cancelled: wire::OutcomeJson = serde_json::from_str(r#"{"outcome":"cancelled"}"#).unwrap();
    assert!(matches!(cancelled, wire::OutcomeJson::Cancelled));
}

#[test]
fn purchase_fields_come_from_the_catalog() {
    let catalog = catalog();
    let json = r#"{"product_id":"app.sub","quantity":2,"purchased_ms":1700000000000,"transaction_id":"42","proof":{"kind":"app_store_jws","value":"jws"}}"#;
    let purchase: wire::PurchaseJson = serde_json::from_str(json).unwrap();
    let fields = purchase.into_fields(&catalog).unwrap();
    assert_eq!(fields.product_id.as_str(), "app.sub");
    assert_eq!(fields.kind, ProductKind::Subscription);
    assert_eq!(fields.quantity, 2);
    assert_eq!(fields.transaction_id, "42");
    assert_eq!(fields.proof.kind(), ProofKind::AppStoreJws);
    assert_eq!(fields.purchased_at.as_millisecond(), 1_700_000_000_000);
}

#[test]
fn entitlement_decoding_splits_finished() {
    let unfinished: wire::EntitlementJson = serde_json::from_str(
        r#"{"product_id":"app.sub","quantity":1,"purchased_ms":1700000000000,"transaction_id":"42","proof":{"kind":"app_store_jws","value":"jws"},"finished":false}"#,
    )
    .unwrap();
    assert!(!unfinished.finished);

    let finished: wire::EntitlementJson = serde_json::from_str(
        r#"{"product_id":"app.pro","quantity":1,"purchased_ms":1700000000000,"transaction_id":"9","proof":{"kind":"app_store_jws","value":"jws"},"finished":true}"#,
    )
    .unwrap();
    assert!(finished.finished);
}

#[test]
fn event_decoding_covers_purchase_and_end() {
    let event: wire::EventJson = serde_json::from_str(
        r#"{"purchase":{"product_id":"app.sub","quantity":1,"purchased_ms":1700000000000,"transaction_id":"43","proof":{"kind":"app_store_jws","value":"jws"}}}"#,
    )
    .unwrap();
    assert!(matches!(event, wire::EventJson::Purchase(_)));
    let end: wire::EventJson = serde_json::from_str(r#"{"end":true}"#).unwrap();
    assert!(matches!(end, wire::EventJson::End(true)));
}

// ---- Windows pure mappings ----

use crate::sys::mapping::{
    BillingUnit, ConsumableStatus, PurchaseStatus, PurchaseVerdict, StoreProductKind,
    billing_period, fulfillment_verdict, kind_matches, price_micros, purchase_verdict,
    store_product_kind,
};

#[test]
fn store_product_kind_parses_the_documented_values() {
    assert_eq!(
        store_product_kind("Consumable"),
        StoreProductKind::Consumable
    );
    assert_eq!(
        store_product_kind("UnmanagedConsumable"),
        StoreProductKind::UnmanagedConsumable
    );
    assert_eq!(store_product_kind("Durable"), StoreProductKind::Durable);
    assert_eq!(store_product_kind("Application"), StoreProductKind::Other);
    assert_eq!(store_product_kind("whatever-next"), StoreProductKind::Other);
}

#[test]
fn windows_kind_matching_uses_the_subscription_sku() {
    use ProductKind::{Consumable, NonConsumable, Subscription};
    use StoreProductKind::{Consumable as C, Durable, Other, UnmanagedConsumable as U};

    for store in [C, U] {
        assert!(kind_matches(Consumable, store, false));
        assert!(!kind_matches(NonConsumable, store, false));
        assert!(!kind_matches(Subscription, store, false));
    }
    // A durable without a subscription SKU is a non-consumable.
    assert!(kind_matches(NonConsumable, Durable, false));
    assert!(!kind_matches(Consumable, Durable, false));
    assert!(!kind_matches(Subscription, Durable, false));
    // A durable with a subscription SKU is a subscription.
    assert!(kind_matches(Subscription, Durable, true));
    assert!(!kind_matches(NonConsumable, Durable, true));
    assert!(!kind_matches(Consumable, Durable, true));

    for declared in [Consumable, NonConsumable, Subscription] {
        assert!(!kind_matches(declared, Other, false));
        assert!(!kind_matches(declared, Other, true));
    }
}

#[test]
fn billing_period_maps_calendar_units() {
    assert_eq!(
        billing_period(3, BillingUnit::Month).unwrap(),
        crate::Period {
            unit: PeriodUnit::Month,
            count: 3
        }
    );
    for (unit, expected) in [
        (BillingUnit::Day, PeriodUnit::Day),
        (BillingUnit::Week, PeriodUnit::Week),
        (BillingUnit::Year, PeriodUnit::Year),
    ] {
        assert_eq!(billing_period(1, unit).unwrap().unit, expected);
    }
    // Minute and Hour have no PeriodUnit counterpart; they fail rather than
    // misreport a subscription's period.
    for unit in [BillingUnit::Minute, BillingUnit::Hour] {
        assert!(matches!(
            billing_period(1, unit),
            Err(StoreError::Platform(_))
        ));
    }
}

#[test]
fn purchase_verdict_maps_each_status() {
    let id = ProductId::new("app.pro");
    assert_eq!(
        purchase_verdict(PurchaseStatus::Succeeded, &id, None).unwrap(),
        PurchaseVerdict::Purchased
    );
    assert_eq!(
        purchase_verdict(PurchaseStatus::NotPurchased, &id, None).unwrap(),
        PurchaseVerdict::Cancelled
    );
    assert!(matches!(
        purchase_verdict(PurchaseStatus::AlreadyPurchased, &id, None),
        Err(StoreError::AlreadyOwned(ref product)) if product == &id
    ));
    assert!(matches!(
        purchase_verdict(PurchaseStatus::NetworkError, &id, None),
        Err(StoreError::Network(_))
    ));
    assert!(matches!(
        purchase_verdict(
            PurchaseStatus::ServerError,
            &id,
            Some("store backend said no".into())
        ),
        Err(StoreError::Platform(ref message)) if message == "store backend said no"
    ));
}

#[test]
fn fulfillment_verdict_maps_each_status() {
    assert!(fulfillment_verdict(ConsumableStatus::Succeeded, None).is_ok());
    assert!(matches!(
        fulfillment_verdict(ConsumableStatus::NetworkError, None),
        Err(StoreError::Network(_))
    ));
    assert!(matches!(
        fulfillment_verdict(ConsumableStatus::InsufficientQuantity, None),
        Err(StoreError::Platform(_))
    ));
    assert!(matches!(
        fulfillment_verdict(ConsumableStatus::ServerError, Some("x".into())),
        Err(StoreError::Platform(ref message)) if message == "x"
    ));
}

#[test]
fn price_micros_parses_decimal_prices() {
    assert_eq!(price_micros("4.99").unwrap(), 4_990_000);
    assert_eq!(price_micros("0").unwrap(), 0);
    assert_eq!(price_micros("12.345").unwrap(), 12_345_000);
    assert!(matches!(
        price_micros("not a price"),
        Err(StoreError::Platform(_))
    ));
}
