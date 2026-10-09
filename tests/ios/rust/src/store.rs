//! The `store` case: real `StoreKit` 2 purchases against a `StoreKit` Test
//! session the app seeded from the committed `WaterKitTest.storekit`
//! configuration (fast renewals, dialogs disabled, all headless).

use futures::StreamExt;
use waterkit::store::{
    Catalog, Entitlement, Product, ProductId, ProductKind, PurchaseOutcome, Store, StoreEvents,
};
use waterkit_test_report::{TestCase, TestReport};

use crate::ffi;

const COINS: &str = "waterkit.test.coins";
const PRO: &str = "waterkit.test.pro";
const SUB: &str = "waterkit.test.sub";

fn catalog() -> Catalog {
    Catalog::new()
        .with(ProductId::new(COINS), ProductKind::Consumable)
        .with(ProductId::new(PRO), ProductKind::NonConsumable)
        .with(ProductId::new(SUB), ProductKind::Subscription)
}

fn find<'a>(products: &'a [Product], id: &str) -> &'a Product {
    products
        .iter()
        .find(|product| product.id().as_str() == id)
        .unwrap_or_else(|| panic!("the store must know {id}"))
}

/// The `store` case set.
pub async fn record(report: &mut TestReport) {
    let session_error = ffi::store_test_begin();
    if !session_error.is_empty() {
        report.push(TestCase::failed("store.session", session_error));
        return;
    }
    report.push(TestCase::passed("store.session"));

    match waterkit::store::capabilities().await {
        Ok(capabilities) if capabilities.purchases => {
            report.push(TestCase::passed("store.capabilities"));
        }
        Ok(_) => {
            report.push(TestCase::failed(
                "store.capabilities",
                "the StoreKit Test session reports purchases unavailable",
            ));
            ffi::store_test_end();
            return;
        }
        Err(error) => {
            report.push(TestCase::failed(
                "store.capabilities",
                format!("capabilities probe failed: {error}"),
            ));
            ffi::store_test_end();
            return;
        }
    }

    let (store, events) = match Store::connect(catalog()).await {
        Ok(connected) => {
            report.push(TestCase::passed("store.connect"));
            connected
        }
        Err(error) => {
            report.push(TestCase::failed(
                "store.connect",
                format!("connect failed: {error}"),
            ));
            ffi::store_test_end();
            return;
        }
    };

    let products = match store.products().await {
        Ok(products) => {
            report.push(TestCase::passed("store.products"));
            products
        }
        Err(error) => {
            report.push(TestCase::failed(
                "store.products",
                format!("product query failed: {error}"),
            ));
            ffi::store_test_end();
            return;
        }
    };

    purchases(&store, &products, report).await;
    subscribe(&store, &products, report).await;
    entitlements(&store, report).await;
    renewal(events, report).await;

    ffi::store_test_end();
}

async fn purchases(store: &Store, products: &[Product], report: &mut TestReport) {
    for (case, id) in [
        ("store.purchase_consumable", COINS),
        ("store.purchase_nonconsumable", PRO),
    ] {
        match store.purchase(find(products, id)).await {
            Ok(outcome) => match outcome {
                PurchaseOutcome::Purchased(purchase) => match purchase.finish().await {
                    Ok(_transaction) => report.push(TestCase::passed(case)),
                    Err(error) => {
                        report.push(TestCase::failed(case, format!("finish failed: {error}")));
                    }
                },
                PurchaseOutcome::Pending => {
                    report.push(TestCase::failed(case, "the test purchase stayed pending"));
                }
                PurchaseOutcome::Cancelled => {
                    report.push(TestCase::failed(case, "the test purchase was cancelled"));
                }
                _ => report.push(TestCase::failed(case, "unexpected purchase outcome")),
            },
            Err(error) => report.push(TestCase::failed(case, format!("{error}"))),
        }
    }
}

async fn subscribe(store: &Store, products: &[Product], report: &mut TestReport) {
    let product = find(products, SUB);
    let offer = product
        .subscription()
        .expect("the subscription carries its terms")
        .offers
        .first()
        .expect("the subscription has an offer");
    match store.subscribe(product, offer).await {
        Ok(PurchaseOutcome::Purchased(purchase)) => match purchase.finish().await {
            Ok(_transaction) => report.push(TestCase::passed("store.subscribe")),
            Err(error) => report.push(TestCase::failed(
                "store.subscribe",
                format!("finish failed: {error}"),
            )),
        },
        Ok(PurchaseOutcome::Pending) => {
            report.push(TestCase::failed(
                "store.subscribe",
                "the subscription stayed pending",
            ));
        }
        Ok(PurchaseOutcome::Cancelled) => {
            report.push(TestCase::failed(
                "store.subscribe",
                "the subscription was cancelled",
            ));
        }
        Err(error) => report.push(TestCase::failed("store.subscribe", format!("{error}"))),
        Ok(_) => report.push(TestCase::failed(
            "store.subscribe",
            "unexpected purchase outcome",
        )),
    }
}

async fn entitlements(store: &Store, report: &mut TestReport) {
    match store.entitlements().await {
        Ok(entitlements) => {
            let owned = entitlements
                .iter()
                .any(|entitlement| matches!(entitlement, Entitlement::Finished(transaction) if transaction.product_id().as_str() == SUB));
            if owned {
                report.push(TestCase::passed("store.entitlements"));
            } else {
                report.push(TestCase::failed(
                    "store.entitlements",
                    "the finished subscription is missing from entitlements",
                ));
            }
        }
        Err(error) => report.push(TestCase::failed("store.entitlements", format!("{error}"))),
    }
}

async fn renewal(mut events: StoreEvents, report: &mut TestReport) {
    // A renewal is an out-of-band transaction: it must arrive through the
    // `StoreEvents` stream. StoreKit Test renews the subscription on demand.
    ffi::store_test_force_renewal(SUB);
    match tokio::time::timeout(std::time::Duration::from_secs(30), events.next()).await {
        Ok(Some(Ok(purchase))) if purchase.product_id().as_str() == SUB => {
            report.push(TestCase::passed("store.renewal_event"));
        }
        Ok(Some(Ok(purchase))) => report.push(TestCase::failed(
            "store.renewal_event",
            format!(
                "the renewal carried the wrong product {}",
                purchase.product_id().as_str()
            ),
        )),
        Ok(Some(Err(error))) => {
            report.push(TestCase::failed("store.renewal_event", format!("{error}")));
        }
        Ok(None) => report.push(TestCase::failed(
            "store.renewal_event",
            "the events stream ended before the renewal",
        )),
        Err(_elapsed) => report.push(TestCase::failed(
            "store.renewal_event",
            "no renewal transaction arrived within 30 s",
        )),
    }
}