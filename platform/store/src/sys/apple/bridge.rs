#[swift_bridge::bridge]
pub mod ffi {
    extern "Swift" {
        // `AppleStore`, the StoreKit session object. `Sendable` because every
        // call hands work to a `Task`; shared state lives in an actor.
        #[swift_bridge(Sendable)]
        type AppleStore;

        // `{"purchases": bool}` — StoreKit 2 availability and whether the
        // device may make payments.
        fn store_capabilities() -> String;

        // Creates the session; callers check `store_capabilities` first, so
        // the StoreKit 2 floor is already met here.
        fn store_connect(catalog_json: &str) -> AppleStore;

        // Retains the session for a second owner (the `events()` stream).
        fn store_retain(&self) -> AppleStore;

        fn store_products(&self, callback: Box<dyn FnOnce(String) -> ()>);
        fn store_purchase(&self, product_id: &str, callback: Box<dyn FnOnce(String) -> ()>);
        fn store_next_event(&self, callback: Box<dyn FnOnce(String) -> ()>);
        fn store_entitlements(&self, callback: Box<dyn FnOnce(String) -> ()>);
        fn store_finish(transaction_id: &str, callback: Box<dyn FnOnce(String) -> ()>);
    }
}
