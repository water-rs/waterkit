import Foundation
import StoreKit

// The wire format mirrors src/sys/wire.rs: every reply is one JSON document,
// `{"ok": <payload>}` or `{"error": {"kind": ..., ...}}`. The `AppleStore`
// class cannot be `@available`-gated — the generated bridge glue references
// it unconditionally — so StoreKit use lives behind `#available` guards in
// the method bodies, and `connect` is only reached after `capabilities`
// confirms the iOS 15 / macOS 12 floor.

private struct WireError: Encodable {
    let kind: String
    let message: String?
    let product: String?
    let declared: String?
    let proof: WireProof?

    init(
        _ kind: String,
        _ message: String? = nil,
        product: String? = nil,
        declared: String? = nil,
        proof: WireProof? = nil
    ) {
        self.kind = kind
        self.message = message
        self.product = product
        self.declared = declared
        self.proof = proof
    }
}

private struct WireProof: Encodable {
    let kind: String
    let value: String
}

private struct WireErrorReply: Encodable {
    let error: WireError
}

private struct WireReply<Payload: Encodable>: Encodable {
    private let payload: Payload

    private enum CodingKeys: String, CodingKey {
        case ok
    }

    init(ok payload: Payload) {
        self.payload = payload
    }

    func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encode(payload, forKey: .ok)
    }
}

private struct WireCapabilities: Encodable {
    let purchases: Bool
}

private struct WirePrice: Encodable {
    let formatted: String
    let micros: Int64
    let currency: String
}

private struct WirePeriod: Encodable {
    let unit: String
    let count: UInt32
}

private struct WirePhase: Encodable {
    let price: WirePrice
    let period: WirePeriod
    let cycles: UInt32?
    let mode: String
}

private struct WireOffer: Encodable {
    let token: String
    let phases: [WirePhase]
}

private struct WireSubscription: Encodable {
    let period: WirePeriod
    let offers: [WireOffer]
}

private struct WireProduct: Encodable {
    let id: String
    let storeKind: String
    let title: String
    let description: String
    let price: WirePrice
    let subscription: WireSubscription?
}

private struct WirePurchase: Encodable {
    let productId: String
    let quantity: UInt32
    let purchasedMs: Int64
    let transactionId: String
    let proof: WireProof
}

private struct WireOutcome: Encodable {
    let outcome: String
    let purchase: WirePurchase?

    init(_ outcome: String, purchase: WirePurchase? = nil) {
        self.outcome = outcome
        self.purchase = purchase
    }
}

private struct WireEntitlement: Encodable {
    let productId: String
    let quantity: UInt32
    let purchasedMs: Int64
    let transactionId: String
    let proof: WireProof
    let finished: Bool
}

// Event items decode as an externally tagged enum on the Rust side, so each
// shape must serialize with exactly one key.
private struct WireEventPurchase: Encodable {
    let purchase: WirePurchase
}

private struct WireEventEnd: Encodable {
    let end: Bool
}

/// A callback crossing the bridge is a `Box<dyn FnOnce(String)>` on the Rust
/// side; this wrapper makes it safe to capture inside a `Task`.
private final class ResponseCallback: @unchecked Sendable {
    private let callback: (String) -> Void

    init(_ callback: @escaping (String) -> Void) {
        self.callback = callback
    }

    func call(_ response: String) {
        callback(response)
    }
}

private let jsonEncoder: JSONEncoder = {
    let encoder = JSONEncoder()
    encoder.keyEncodingStrategy = .convertToSnakeCase
    return encoder
}()

private func encoded<Payload: Encodable>(_ reply: WireReply<Payload>) -> String {
    do {
        let data = try jsonEncoder.encode(reply)
        guard let value = String(data: data, encoding: .utf8) else {
            return errorReply(WireError("platform", "store reply was not UTF-8"))
        }
        return value
    } catch {
        return errorReply(WireError("platform", error.localizedDescription))
    }
}

private func errorReply(_ error: WireError) -> String {
    guard let data = try? jsonEncoder.encode(WireErrorReply(error: error)),
        let value = String(data: data, encoding: .utf8)
    else {
        return #"{"error":{"kind":"platform","message":"store reply encoding failed"}}"#
    }
    return value
}

private func unavailableReply() -> String {
    errorReply(WireError("unavailable", "StoreKit 2 requires iOS 15 / macOS 12"))
}

@available(iOS 15.0, macOS 12.0, *)
private func wirePrice(_ price: Decimal, formatted: String, currency: String) -> WirePrice {
    WirePrice(
        formatted: formatted,
        micros: NSDecimalNumber(decimal: price)
            .multiplying(by: NSDecimalNumber(value: 1_000_000)).int64Value,
        currency: currency
    )
}

@available(iOS 15.0, macOS 12.0, *)
private func wirePeriod(_ period: Product.SubscriptionPeriod) -> WirePeriod {
    let unit: String
    switch period.unit {
    case .day: unit = "day"
    case .week: unit = "week"
    case .month: unit = "month"
    case .year: unit = "year"
    default:
        fatalError("waterkit-store: unknown subscription period unit \(period.unit)")
    }
    return WirePeriod(unit: unit, count: UInt32(period.value))
}

@available(iOS 15.0, macOS 12.0, *)
private func storeKind(of product: Product) -> String {
    switch product.type {
    case .consumable: return "consumable"
    case .nonConsumable: return "non_consumable"
    case .autoRenewable: return "subscription"
    case .nonRenewable: return "non_renewable_subscription"
    default:
        fatalError("waterkit-store: unknown product type \(product.type)")
    }
}

@available(iOS 15.0, macOS 12.0, *)
private func wirePhase(_ offer: Product.SubscriptionOffer, currency: String) -> WirePhase {
    let mode: String
    switch offer.paymentMode {
    case .freeTrial: mode = "free_trial"
    case .payAsYouGo: mode = "pay_as_you_go"
    case .payUpFront: mode = "pay_up_front"
    default:
        fatalError("waterkit-store: unknown offer payment mode \(offer.paymentMode)")
    }
    return WirePhase(
        price: wirePrice(offer.price, formatted: offer.displayPrice, currency: currency),
        period: wirePeriod(offer.period),
        cycles: UInt32(offer.periodCount),
        mode: mode
    )
}

@available(iOS 15.0, macOS 12.0, *)
private func wireProduct(_ product: Product) async -> WireProduct {
    let currency = product.priceFormatStyle.currencyCode
    let price = wirePrice(product.price, formatted: product.displayPrice, currency: currency)

    var subscription: WireSubscription? = nil
    if let info = product.subscription {
        let period = wirePeriod(info.subscriptionPeriod)
        var phases: [WirePhase] = []
        // The standard offer: the base price, led by any introductory offer
        // the user is eligible for. Promotional and win-back offers need a
        // server signature and stay out of scope.
        if await info.isEligibleForIntroOffer, let intro = info.introductoryOffer {
            phases.append(wirePhase(intro, currency: currency))
        }
        phases.append(WirePhase(price: price, period: period, cycles: nil, mode: "recurring"))
        subscription = WireSubscription(
            period: period,
            offers: [WireOffer(token: product.id, phases: phases)]
        )
    }
    return WireProduct(
        id: product.id,
        storeKind: storeKind(of: product),
        title: product.displayName,
        description: product.description,
        price: price,
        subscription: subscription
    )
}

@available(iOS 15.0, macOS 12.0, *)
private func purchasedQuantity(of transaction: StoreKit.Transaction) -> UInt32 {
    if #available(iOS 17.0, macOS 14.0, *) {
        return UInt32(transaction.purchasedQuantity)
    }
    return 1
}

@available(iOS 15.0, macOS 12.0, *)
private func wirePurchase(
    _ transaction: StoreKit.Transaction,
    jws: String
) -> WirePurchase {
    WirePurchase(
        productId: transaction.productID,
        quantity: purchasedQuantity(of: transaction),
        purchasedMs: Int64(transaction.purchaseDate.timeIntervalSince1970 * 1000),
        transactionId: String(transaction.id),
        proof: WireProof(kind: "app_store_jws", value: jws)
    )
}

@available(iOS 15.0, macOS 12.0, *)
private func unverifiedReply(
    _ result: VerificationResult<StoreKit.Transaction>
) -> String {
    errorReply(
        WireError(
            "unverified",
            "the transaction failed StoreKit verification",
            proof: WireProof(kind: "app_store_jws", value: result.jwsRepresentation)
        )
    )
}

/// Buffers `Transaction.updates` replies for the event stream: the
/// listener enqueues them, each `store_next_event` call dequeues exactly
/// one.
private actor EventQueue {
    private var buffer: [String] = []
    private var waiters: [CheckedContinuation<String, Never>] = []
    private var ended = false

    func enqueue(_ json: String) {
        guard !ended else { return }
        if let waiter = waiters.first {
            waiters.removeFirst()
            waiter.resume(returning: json)
        } else {
            buffer.append(json)
        }
    }

    func end() {
        ended = true
        let reply = endReply()
        for waiter in waiters {
            waiter.resume(returning: reply)
        }
        waiters.removeAll()
    }

    /// The next out-of-band transaction reply: a purchase envelope, an
    /// `unverified` error, or `{"ok":{"end":true}}` when updates finish.
    func next() async -> String {
        if let json = buffer.first {
            buffer.removeFirst()
            return json
        }
        if ended {
            return endReply()
        }
        return await withCheckedContinuation { continuation in
            waiters.append(continuation)
        }
    }

    private func endReply() -> String {
        encoded(WireReply(ok: WireEventEnd(end: true)))
    }
}

/// The StoreKit session: owns the `Transaction.updates` listener that feeds
/// the event stream from `connect` until the store drops. Thread-safe by
/// construction: every mutating call dispatches a `Task`, and the only
/// mutable state lives in the `EventQueue` actor.
final class AppleStore: @unchecked Sendable {
    private let catalog: [String: String]
    private let queue: EventQueue
    private let updatesTask: Task<Void, Never>

    init(catalogJson: String) {
        catalog = AppleStore.decodeCatalog(catalogJson)
        let queue = EventQueue()
        self.queue = queue
        updatesTask = Task {
            guard #available(iOS 15.0, macOS 12.0, *) else {
                await queue.end()
                return
            }
            for await result in StoreKit.Transaction.updates {
                await queue.enqueue(AppleStore.encodeUpdate(result))
            }
            await queue.end()
        }
    }

    deinit {
        updatesTask.cancel()
    }

    private static func decodeCatalog(_ json: String) -> [String: String] {
        struct CatalogDocument: Decodable {
            struct ProductEntry: Decodable {
                let id: String
                let kind: String
            }
            let products: [ProductEntry]
        }
        guard let data = json.data(using: .utf8),
            let document = try? JSONDecoder().decode(CatalogDocument.self, from: data)
        else {
            fatalError("waterkit-store: catalog JSON is malformed: \(json)")
        }
        var catalog: [String: String] = [:]
        for product in document.products {
            catalog[product.id] = product.kind
        }
        return catalog
    }

    @available(iOS 15.0, macOS 12.0, *)
    private static func encodeUpdate(
        _ result: VerificationResult<StoreKit.Transaction>
    ) -> String {
        switch result {
        case .verified(let transaction):
            return encoded(
                WireReply(
                    ok: WireEventPurchase(
                        purchase: wirePurchase(
                            transaction, jws: result.jwsRepresentation))))
        case .unverified:
            return unverifiedReply(result)
        }
    }

    private func products() async -> String {
        guard #available(iOS 15.0, macOS 12.0, *) else {
            return unavailableReply()
        }
        do {
            let products = try await Product.products(for: Array(catalog.keys))
            let found = Set(products.map(\.id))
            if let missing = catalog.keys.first(where: { !found.contains($0) }) {
                return errorReply(
                    WireError(
                        "product_not_found",
                        "the store does not know this product",
                        product: missing
                    ))
            }
            var items: [WireProduct] = []
            for product in products.sorted(by: { $0.id < $1.id }) {
                items.append(await wireProduct(product))
            }
            return encoded(WireReply(ok: items))
        } catch {
            return errorReply(WireError("platform", error.localizedDescription))
        }
    }

    private func purchase(productId: String) async -> String {
        guard #available(iOS 15.0, macOS 12.0, *) else {
            return unavailableReply()
        }
        guard let declared = catalog[productId] else {
            return errorReply(
                WireError("product_not_found", "not in the catalog", product: productId))
        }
        do {
            guard let product = try await Product.products(for: [productId]).first else {
                return errorReply(
                    WireError(
                        "product_not_found",
                        "the store does not know this product",
                        product: productId
                    ))
            }
            let actual = storeKind(of: product)
            let matches =
                (declared == "consumable" && actual == "consumable")
                || (declared == "non_consumable" && actual == "non_consumable")
                || (declared == "subscription" && actual == "subscription")
            guard matches else {
                return errorReply(
                    WireError(
                        "kind_mismatch",
                        "the store's product type contradicts the declared kind",
                        product: productId,
                        declared: declared
                    ))
            }
            let result = try await product.purchase()
            switch result {
            case .success(let verification):
                switch verification {
                case .verified(let transaction):
                    return encoded(
                        WireReply(
                            ok: WireOutcome(
                                "purchased",
                                purchase: wirePurchase(
                                    transaction, jws: verification.jwsRepresentation)
                            )))
                case .unverified:
                    return unverifiedReply(verification)
                }
            case .userCancelled:
                return encoded(WireReply(ok: WireOutcome("cancelled")))
            case .pending:
                return encoded(WireReply(ok: WireOutcome("pending")))
            default:
                return errorReply(
                    WireError("platform", "StoreKit returned an unknown purchase result"))
            }
        } catch let error as Product.PurchaseError {
            switch error {
            case .productUnavailable:
                return errorReply(
                    WireError(
                        "product_not_found", error.localizedDescription, product: productId))
            default:
                return errorReply(WireError("platform", error.localizedDescription))
            }
        } catch {
            return errorReply(WireError("platform", error.localizedDescription))
        }
    }

    private func entitlements() async -> String {
        guard #available(iOS 15.0, macOS 12.0, *) else {
            return unavailableReply()
        }
        var unfinished = Set<UInt64>()
        for await result in StoreKit.Transaction.unfinished {
            if case .verified(let transaction) = result {
                unfinished.insert(transaction.id)
            }
        }
        var items: [WireEntitlement] = []
        for await result in StoreKit.Transaction.currentEntitlements {
            switch result {
            case .verified(let transaction):
                let purchase = wirePurchase(
                    transaction, jws: result.jwsRepresentation)
                items.append(
                    WireEntitlement(
                        productId: purchase.productId,
                        quantity: purchase.quantity,
                        purchasedMs: purchase.purchasedMs,
                        transactionId: purchase.transactionId,
                        proof: purchase.proof,
                        finished: !unfinished.contains(transaction.id)
                    ))
            case .unverified:
                return unverifiedReply(result)
            }
        }
        return encoded(WireReply(ok: items))
    }

    // Bridge entry points. The generated glue calls `some_method(...)` with
    // the Rust argument names as labels, so the labels match the bridge
    // declarations exactly.

    func store_retain() -> AppleStore {
        self
    }

    // Ends the session: cancels the updates listener and closes the queue,
    // so the event stream ends even while it retains this session.
    func store_disconnect() {
        updatesTask.cancel()
        let queue = queue
        Task { await queue.end() }
    }

    func store_products(callback: @escaping (String) -> Void) {
        let callback = ResponseCallback(callback)
        Task { callback.call(await products()) }
    }

    func store_purchase(product_id: RustStr, callback: @escaping (String) -> Void) {
        let callback = ResponseCallback(callback)
        let productId = product_id.toString()
        Task { callback.call(await purchase(productId: productId)) }
    }

    func store_next_event(callback: @escaping (String) -> Void) {
        let callback = ResponseCallback(callback)
        Task { callback.call(await queue.next()) }
    }

    func store_entitlements(callback: @escaping (String) -> Void) {
        let callback = ResponseCallback(callback)
        Task { callback.call(await entitlements()) }
    }
}

func store_capabilities() -> RustString {
    if #available(iOS 15.0, macOS 12.0, *) {
        return encoded(WireReply(ok: WireCapabilities(purchases: AppStore.canMakePayments)))
            .intoRustString()
    }
    return encoded(WireReply(ok: WireCapabilities(purchases: false))).intoRustString()
}

func store_connect(catalog_json: RustStr) -> AppleStore {
    AppleStore(catalogJson: catalog_json.toString())
}

func store_finish(transaction_id: RustStr, callback: @escaping (String) -> Void) {
    let callback = ResponseCallback(callback)
    let transactionId = transaction_id.toString()
    Task {
        guard #available(iOS 15.0, macOS 12.0, *) else {
            callback.call(unavailableReply())
            return
        }
        for await result in StoreKit.Transaction.unfinished {
            if case .verified(let transaction) = result, String(transaction.id) == transactionId {
                await transaction.finish()
                callback.call(encoded(WireReply(ok: true)))
                return
            }
        }
        callback.call(
            errorReply(
                WireError(
                    "platform",
                    "no unfinished transaction with id \(transactionId)")))
    }
}
