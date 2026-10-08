import Foundation

// StoreKit Test session for the `store` harness cases. The session lives in
// the app target because `SKTestSession` must be created by the process that
// runs StoreKit — the test library's own Swift cannot reach it.
#if canImport(StoreKitTest)
    import StoreKitTest

    private var storeTestSession: SKTestSession?
#endif

/// Starts the session from the committed configuration and returns "" on
/// success or the failure message. Dialogs are disabled so purchases
/// complete headless.
func store_test_begin() -> RustString {
    #if canImport(StoreKitTest)
        guard
            let url = Bundle.main.url(forResource: "WaterKitTest", withExtension: "storekit")
        else {
            return "WaterKitTest.storekit is missing from the app bundle".intoRustString()
        }
        do {
            let session = try SKTestSession(contentsOf: url)
            session.disableDialogs = true
            session.clearTransactions()
            storeTestSession = session
            return "".intoRustString()
        } catch {
            return "SKTestSession failed: \(error.localizedDescription)".intoRustString()
        }
    #else
        return "the StoreKitTest framework is unavailable".intoRustString()
    #endif
}

/// Forces a renewal of the subscription — the renewed transaction lands on
/// `Transaction.updates` like a real out-of-band one.
func store_test_force_renewal(product_id: RustStr) {
    #if canImport(StoreKitTest)
        do {
            try storeTestSession?.forceRenewalOfSubscription(
                productIdentifier: product_id.toString())
        } catch {
            // The renewal case records the failure when no event arrives.
        }
    #endif
}

/// Ends the session.
func store_test_end() {
    #if canImport(StoreKitTest)
        storeTestSession = nil
    #endif
}
