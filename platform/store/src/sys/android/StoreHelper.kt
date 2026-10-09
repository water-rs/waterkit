package waterkit.store

import android.app.Activity
import android.content.Context
import android.content.pm.PackageManager
import android.os.Handler
import android.os.Looper
import com.android.billingclient.api.AcknowledgePurchaseParams
import com.android.billingclient.api.BillingClient
import com.android.billingclient.api.BillingClient.BillingResponseCode
import com.android.billingclient.api.BillingClient.ProductType
import com.android.billingclient.api.BillingClientStateListener
import com.android.billingclient.api.BillingFlowParams
import com.android.billingclient.api.BillingResult
import com.android.billingclient.api.ConsumeParams
import com.android.billingclient.api.PendingPurchasesParams
import com.android.billingclient.api.ProductDetails
import com.android.billingclient.api.Purchase
import com.android.billingclient.api.PurchasesUpdatedListener
import com.android.billingclient.api.QueryProductDetailsParams
import com.android.billingclient.api.QueryPurchasesParams
import org.json.JSONArray
import org.json.JSONObject
import waterkit.build.NativeCallback
import waterkit.build.NativeChannel

/**
 * One store connection. Owns the `BillingClient`; the Rust `Store` holds this
 * object as a JNI global reference and calls [disconnect] when it drops.
 *
 * The [PurchasesUpdatedListener] on the client feeds [events], except the
 * result that belongs to an in-flight [purchase] call, which completes that
 * call's callback instead.
 */
class StoreConnection(
    context: Context,
    catalogJson: String,
    private val events: NativeChannel,
) {
    private val appContext = context.applicationContext
    private val activity = context as? Activity
    private val catalog = parseCatalog(catalogJson)
    private val client =
        BillingClient.newBuilder(appContext)
            .setListener(PurchasesUpdatedListener(::onPurchasesUpdated))
            .enablePendingPurchases(
                PendingPurchasesParams.newBuilder().enableOneTimeProducts().build()
            )
            .enableAutoServiceReconnection()
            .build()

    /** The `NativeCallback` waiting on the in-flight `launchBillingFlow`. */
    private var purchaseCallback: NativeCallback? = null
    private var connected = false
    private var closed = false

    fun connect(callback: NativeCallback) {
        runOnMain {
            if (closed) return@runOnMain
            client.startConnection(
                object : BillingClientStateListener {
                    override fun onBillingSetupFinished(result: BillingResult) {
                        if (closed) return
                        if (result.responseCode == BillingResponseCode.OK) {
                            connected = true
                            callback.complete(okReply(JSONObject()))
                        } else {
                            callback.complete(errorReply(result))
                        }
                    }

                    override fun onBillingServiceDisconnected() {}
                }
            )
        }
    }

    fun products(callback: NativeCallback) {
        runOnMain {
            if (!ready("products", callback)) return@runOnMain
            val inApp = catalog.filter { it.value != "subscription" }.keys.toList()
            val subs = catalog.filter { it.value == "subscription" }.keys.toList()
            queryProducts(inApp, ProductType.INAPP, mutableListOf()) { first ->
                if (first == null) {
                    callback.complete(errorReply("platform", "INAPP product query did not run"))
                    return@queryProducts
                }
                if (first.isError) {
                    callback.complete(first.errorJson!!)
                    return@queryProducts
                }
                queryProducts(subs, ProductType.SUBS, first.items) { second ->
                    if (second == null || second.isError) {
                        callback.complete(
                            second?.errorJson
                                ?: errorReply("platform", "SUBS product query did not run")
                        )
                        return@queryProducts
                    }
                    callback.complete(okReply(JSONArray(second.items)))
                }
            }
        }
    }

    fun purchase(productId: String, offerToken: String?, callback: NativeCallback) {
        runOnMain {
            if (!ready("purchase", callback)) return@runOnMain
            if (purchaseCallback != null) {
                callback.complete(
                    errorReply("platform", "a billing flow is already in flight")
                )
                return@runOnMain
            }
            val kind = catalog[productId]
            if (kind == null) {
                callback.complete(
                    errorReply("product_not_found", "not in the catalog", productId)
                )
                return@runOnMain
            }
            val productType =
                if (kind == "subscription") ProductType.SUBS else ProductType.INAPP
            queryProducts(listOf(productId), productType, mutableListOf()) { result ->
                if (result == null || result.isError) {
                    callback.complete(
                        result?.errorJson
                            ?: errorReply("platform", "product query did not run")
                    )
                    return@queryProducts
                }
                val details = result.details.firstOrNull()
                if (details == null) {
                    callback.complete(
                        errorReply(
                            "product_not_found",
                            "the store does not know this product",
                            productId
                        )
                    )
                    return@queryProducts
                }
                launchFlow(details, offerToken, productId, callback)
            }
        }
    }

    private fun launchFlow(
        details: ProductDetails,
        offerToken: String?,
        productId: String,
        callback: NativeCallback,
    ) {
        val activity = activity
        if (activity == null) {
            callback.complete(
                errorReply(
                    "platform",
                    "launchBillingFlow requires the Android context to be an Activity"
                )
            )
            return
        }
        val params =
            BillingFlowParams.ProductDetailsParams.newBuilder().setProductDetails(details).apply {
                if (details.productType == ProductType.SUBS) {
                    val token = offerToken ?: details.subscriptionOfferDetails?.firstOrNull()?.offerToken
                    if (token != null) setOfferToken(token)
                }
            }
                .build()
        val result =
            client.launchBillingFlow(
                activity,
                BillingFlowParams.newBuilder()
                    .setProductDetailsParamsList(listOf(params))
                    .build(),
            )
        if (result.responseCode != BillingResponseCode.OK) {
            callback.complete(flowErrorReply(result, productId))
            return
        }
        // The flow's outcome arrives on PurchasesUpdatedListener and completes
        // this callback there.
        purchaseCallback = callback
    }

    fun entitlements(callback: NativeCallback) {
        runOnMain {
            if (!ready("entitlements", callback)) return@runOnMain
            queryPurchases(ProductType.INAPP) { first ->
                if (first == null || first.isError) {
                    callback.complete(
                        first?.errorJson
                            ?: errorReply("platform", "INAPP purchase query did not run")
                    )
                    return@queryPurchases
                }
                queryPurchases(ProductType.SUBS) { second ->
                    if (second == null || second.isError) {
                        callback.complete(
                            second?.errorJson
                                ?: errorReply("platform", "SUBS purchase query did not run")
                        )
                        return@queryPurchases
                    }
                    val items = JSONArray()
                    for (purchase in first.purchases + second.purchases) {
                        items.put(entitlementJson(purchase))
                    }
                    callback.complete(okReply(items))
                }
            }
        }
    }

    fun finish(purchaseToken: String, kind: String, callback: NativeCallback) {
        runOnMain {
            if (!ready("finish", callback)) return@runOnMain
            if (kind == "consumable") {
                client.consumeAsync(
                    ConsumeParams.newBuilder().setPurchaseToken(purchaseToken).build()
                ) { result, _ ->
                    callback.complete(
                        if (result.responseCode == BillingResponseCode.OK) {
                            okReply(true)
                        } else {
                            errorReply(result)
                        }
                    )
                }
            } else {
                client.acknowledgePurchase(
                    AcknowledgePurchaseParams.newBuilder()
                        .setPurchaseToken(purchaseToken)
                        .build()
                ) { result ->
                    callback.complete(
                        if (result.responseCode == BillingResponseCode.OK) {
                            okReply(true)
                        } else {
                            errorReply(result)
                        }
                    )
                }
            }
        }
    }

    /** Ends the billing connection and closes the events stream. */
    fun disconnect() {
        runOnMain {
            if (closed) return@runOnMain
            closed = true
            purchaseCallback?.let { callback ->
                purchaseCallback = null
                callback.fail("the store connection was dropped")
            }
            client.endConnection()
            events.close()
        }
    }

    private fun ready(call: String, callback: NativeCallback): Boolean {
        if (closed) {
            callback.complete(errorReply("platform", "the store connection is closed"))
            return false
        }
        if (!connected) {
            callback.complete(errorReply("unavailable", "$call ran before connect"))
            return false
        }
        return true
    }

    /**
     * Routes a purchase update: while a `launchBillingFlow` is in flight its
     * result completes the call's callback; anything else — a pending purchase
     * settling, Ask to Buy — streams into `events`.
     */
    private fun onPurchasesUpdated(result: BillingResult, purchases: List<Purchase>?) {
        val pending = purchaseCallback
        if (pending != null) {
            purchaseCallback = null
            pending.complete(flowOutcomeJson(result, purchases))
            return
        }
        if (result.responseCode == BillingResponseCode.OK && !purchases.isNullOrEmpty()) {
            for (purchase in purchases) {
                if (purchase.purchaseState == Purchase.PurchaseState.PURCHASED) {
                    events.send(okReply(JSONObject().put("purchase", purchaseJson(purchase))))
                } else {
                    events.send(
                        errorReply(
                            "platform",
                            "a purchase settled without reaching PURCHASED state"
                        )
                    )
                }
            }
        } else if (result.responseCode != BillingResponseCode.USER_CANCELED) {
            events.send(errorReply(result))
        }
    }

    private fun flowOutcomeJson(result: BillingResult, purchases: List<Purchase>?): String {
        if (result.responseCode != BillingResponseCode.OK) {
            return flowErrorReply(result, null)
        }
        val purchase = purchases?.firstOrNull()
            ?: return errorReply("platform", "the billing flow returned no purchase")
        return when (purchase.purchaseState) {
            Purchase.PurchaseState.PURCHASED ->
                okReply(JSONObject().put("outcome", "purchased").put("purchase", purchaseJson(purchase)))
            Purchase.PurchaseState.PENDING -> okReply(JSONObject().put("outcome", "pending"))
            else -> errorReply("platform", "the purchase reached no terminal state")
        }
    }

    private fun flowErrorReply(result: BillingResult, productId: String?): String {
        if (result.responseCode == BillingResponseCode.USER_CANCELED) {
            return okReply(JSONObject().put("outcome", "cancelled"))
        }
        return errorReply(result, productId)
    }

    private class ProductQuery(
        val items: MutableList<JSONObject>,
        val details: List<ProductDetails>,
        val isError: Boolean,
        val errorJson: String?,
    )

    private fun queryProducts(
        ids: List<String>,
        productType: String,
        items: MutableList<JSONObject>,
        callback: (ProductQuery?) -> Unit,
    ) {
        if (ids.isEmpty()) {
            callback(ProductQuery(items, emptyList(), false, null))
            return
        }
        val products =
            ids.map { id ->
                QueryProductDetailsParams.Product.newBuilder()
                    .setProductId(id)
                    .setProductType(productType)
                    .build()
            }
        client.queryProductDetailsAsync(
            QueryProductDetailsParams.newBuilder().setProductList(products).build()
        ) { result, detailsResult ->
            if (result.responseCode != BillingResponseCode.OK) {
                callback(
                    ProductQuery(items, emptyList(), true, errorReply(result))
                )
                return@queryProductDetailsAsync
            }
            val unfetched = detailsResult.unfetchedProductList
            if (unfetched.isNotEmpty()) {
                callback(
                    ProductQuery(
                        items,
                        emptyList(),
                        true,
                        errorReply(
                            "product_not_found",
                            "the store does not know this product",
                            unfetched.first().productId,
                        ),
                    )
                )
                return@queryProductDetailsAsync
            }
            for (details in detailsResult.productDetailsList) {
                items.add(productJson(details))
            }
            callback(ProductQuery(items, detailsResult.productDetailsList, false, null))
        }
    }

    private class PurchaseQuery(
        val purchases: List<Purchase>,
        val isError: Boolean,
        val errorJson: String?,
    )

    private fun queryPurchases(productType: String, callback: (PurchaseQuery?) -> Unit) {
        client.queryPurchasesAsync(
            QueryPurchasesParams.newBuilder().setProductType(productType).build()
        ) { result, purchases ->
            if (result.responseCode == BillingResponseCode.OK) {
                callback(PurchaseQuery(purchases, false, null))
            } else {
                callback(PurchaseQuery(emptyList(), true, errorReply(result)))
            }
        }
    }

    // ---- wire encoding ----

    private fun productJson(details: ProductDetails): JSONObject {
        val json = JSONObject()
        json.put("id", details.productId)
        json.put(
            "store_kind",
            if (details.productType == ProductType.SUBS) "subscription" else "in_app"
        )
        json.put("title", details.title)
        json.put("description", details.description)
        // `OneTimePurchaseOfferDetails` and `PricingPhase` share the price
        // fields by name only, so each branch reads its own type.
        val price =
            if (details.productType == ProductType.SUBS) {
                details.subscriptionOfferDetails
                    ?.firstOrNull()
                    ?.pricingPhases
                    ?.pricingPhaseList
                    ?.lastOrNull()
                    ?.let {
                        Triple(it.formattedPrice, it.priceAmountMicros, it.priceCurrencyCode)
                    }
            } else {
                details.oneTimePurchaseOfferDetails?.let {
                    Triple(it.formattedPrice, it.priceAmountMicros, it.priceCurrencyCode)
                }
            }
        json.put(
            "price",
            priceJson(
                price?.first ?: "",
                price?.second ?: 0L,
                price?.third ?: ""
            ),
        )
        if (details.productType == ProductType.SUBS) {
            json.put("subscription", subscriptionJson(details))
        }
        return json
    }

    private fun subscriptionJson(details: ProductDetails): JSONObject {
        val offers = details.subscriptionOfferDetails ?: emptyList()
        val offersJson = JSONArray()
        for (offer in offers) {
            val phases = JSONArray()
            val list = offer.pricingPhases.pricingPhaseList
            for (phase in list) {
                val phaseJson = JSONObject()
                phaseJson.put(
                    "price",
                    priceJson(phase.formattedPrice, phase.priceAmountMicros, phase.priceCurrencyCode),
                )
                phaseJson.put("period", periodJson(phase.billingPeriod))
                phaseJson.put(
                    "cycles",
                    when (phase.recurrenceMode) {
                        ProductDetails.RecurrenceMode.INFINITE_RECURRING -> JSONObject.NULL
                        // A non-recurring phase is a single charge.
                        ProductDetails.RecurrenceMode.NON_RECURRING -> 1
                        else -> phase.billingCycleCount
                    }
                )
                phaseJson.put("mode", phaseMode(phase))
                phases.put(phaseJson)
            }
            offersJson.put(
                JSONObject().put("token", offer.offerToken).put("phases", phases)
            )
        }
        // The subscription's period is the base plan's recurring period; the
        // first offer's last (recurring) phase carries it.
        val recurring =
            offers.firstOrNull()?.pricingPhases?.pricingPhaseList?.lastOrNull()
        return JSONObject()
            .put("period", periodJson(recurring?.billingPeriod ?: "P1M"))
            .put("offers", offersJson)
    }

    private fun phaseMode(phase: ProductDetails.PricingPhase): String {
        return when (phase.recurrenceMode) {
            ProductDetails.RecurrenceMode.FINITE_RECURRING ->
                if (phase.priceAmountMicros == 0L) "free_trial" else "pay_as_you_go"
            ProductDetails.RecurrenceMode.NON_RECURRING -> "pay_up_front"
            else -> "recurring"
        }
    }

    /** ISO-8601 billing period ("P1W", "P3M", ...) to `{unit, count}`. */
    private fun periodJson(period: String): JSONObject {
        val match = Regex("^P(\\d+)([DWMY])$").matchEntire(period)
            ?: return JSONObject().put("unit", "month").put("count", 1)
        val unit =
            when (match.groupValues[2]) {
                "D" -> "day"
                "W" -> "week"
                "M" -> "month"
                "Y" -> "year"
                else -> "month"
            }
        return JSONObject().put("unit", unit).put("count", match.groupValues[1].toInt())
    }

    private fun priceJson(formatted: String, micros: Long, currency: String): JSONObject {
        return JSONObject()
            .put("formatted", formatted)
            .put("micros", micros)
            .put("currency", currency)
    }

    private fun purchaseJson(purchase: Purchase): JSONObject {
        return JSONObject()
            .put("product_id", purchase.products.firstOrNull() ?: "")
            .put("quantity", purchase.quantity)
            .put("purchased_ms", purchase.purchaseTime)
            .put("transaction_id", purchase.purchaseToken)
            .put(
                "proof",
                JSONObject()
                    .put("kind", "play_purchase_token")
                    .put("value", purchase.purchaseToken),
            )
    }

    private fun entitlementJson(purchase: Purchase): JSONObject {
        return purchaseJson(purchase).put("finished", purchase.isAcknowledged)
    }

    // ---- replies ----

    private fun okReply(payload: Any): String {
        return JSONObject().put("ok", payload).toString()
    }

    private fun errorReply(kind: String, message: String, product: String? = null): String {
        val error =
            JSONObject().put("kind", kind).put("message", message)
        if (product != null) error.put("product", product)
        return JSONObject().put("error", error).toString()
    }

    private fun errorReply(result: BillingResult, product: String? = null): String {
        var kind =
            when (result.responseCode) {
                BillingResponseCode.SERVICE_UNAVAILABLE,
                BillingResponseCode.SERVICE_DISCONNECTED,
                BillingResponseCode.NETWORK_ERROR -> "network"
                BillingResponseCode.BILLING_UNAVAILABLE,
                BillingResponseCode.FEATURE_NOT_SUPPORTED -> "unavailable"
                BillingResponseCode.ITEM_UNAVAILABLE,
                BillingResponseCode.ITEM_NOT_OWNED -> "product_not_found"
                BillingResponseCode.ITEM_ALREADY_OWNED -> "already_owned"
                else -> "platform"
            }
        // `product_not_found` and `already_owned` carry the product id; a
        // billing result without one can only be reported as a platform
        // error.
        if (product == null && (kind == "product_not_found" || kind == "already_owned")) {
            kind = "platform"
        }
        return errorReply(kind, result.debugMessage.ifEmpty { "billing error" }, product)
    }

    /**
     * `{"products":[{"id","kind"}]}` as sent at connect time, into
     * product id → wire `store_kind` (`subscription` for SUBS queries).
     */
    private fun parseCatalog(catalogJson: String): Map<String, String> {
        val catalog = mutableMapOf<String, String>()
        val products = JSONObject(catalogJson).getJSONArray("products")
        for (index in 0 until products.length()) {
            val product = products.getJSONObject(index)
            catalog[product.getString("id")] = product.getString("kind")
        }
        return catalog
    }

    private fun runOnMain(block: () -> Unit) {
        if (Looper.myLooper() == Looper.getMainLooper()) {
            block()
        } else {
            mainHandler.post { block() }
        }
    }

    private companion object {
        private val mainHandler = Handler(Looper.getMainLooper())
    }
}

/** Static entry points of the Play Billing backend. */
object StoreHelper {
    private const val PLAY_STORE_PACKAGE = "com.android.vending"

    /**
     * Whether this device can bill at all: the Play Store app is installed and
     * enabled. The billing setup result at `connect` is what confirms it.
     */
    @JvmStatic
    fun capabilities(context: Context): String {
        val purchases = try {
            val info = context.packageManager.getPackageInfo(PLAY_STORE_PACKAGE, 0)
            info.applicationInfo?.enabled == true
        } catch (_: PackageManager.NameNotFoundException) {
            false
        }
        return """{"ok":{"purchases":$purchases}}"""
    }
}