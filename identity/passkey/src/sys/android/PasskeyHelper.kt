package waterkit.passkey

import android.content.Context
import android.os.Build
import android.os.CancellationSignal
import android.os.Handler
import android.os.Looper
import org.json.JSONArray
import org.json.JSONObject
import java.lang.reflect.Proxy
import java.util.concurrent.Executor

class PasskeyHelper {
    companion object {
        @JvmStatic
        fun isAvailable(context: Context): Boolean {
            if (Build.VERSION.SDK_INT < Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
                return false
            }

            return try {
                val managerClass = Class.forName("android.credentials.CredentialManager")
                val getSystemService = Context::class.java.getMethod("getSystemService", Class::class.java)
                val manager = getSystemService.invoke(context, managerClass)
                manager != null
            } catch (_: Throwable) {
                false
            }
        }

        @JvmStatic
        fun register(context: Context, requestJson: String, callbackPtr: Long) {
            if (!isAvailable(context)) {
                onRegisterResult(
                    callbackPtr,
                    false,
                    "passkey registration requires Android 14+ credential APIs",
                    null,
                )
                return
            }

            runOnMain {
                try {
                    val credentialManager = credentialManager(context)
                    val createRequest = createPublicKeyCredentialRequest(buildRegistrationRequestJson(requestJson))
                    val createRequestBaseClass = Class.forName("android.credentials.CreateCredentialRequest")
                    val outcomeReceiverClass = Class.forName("android.os.OutcomeReceiver")
                    val cancellationSignal = CancellationSignal()
                    val executor = mainThreadExecutor()

                    val callbackProxy = Proxy.newProxyInstance(
                        outcomeReceiverClass.classLoader,
                        arrayOf(outcomeReceiverClass),
                    ) { _, method, args ->
                        when (method.name) {
                            "onResult" -> {
                                val response = args?.getOrNull(0)
                                    ?: throw IllegalStateException("createCredential result payload missing")
                                val responseJson = extractRegistrationResponseJson(response)
                                onRegisterResult(callbackPtr, true, null, responseJson)
                            }
                            "onError" -> {
                                val throwable = args?.getOrNull(0) as? Throwable
                                onRegisterResult(
                                    callbackPtr,
                                    false,
                                    throwable?.message ?: "registration failed",
                                    null,
                                )
                            }
                            else -> Unit
                        }
                        null
                    }

                    val createMethod = credentialManager.javaClass.getMethod(
                        "createCredential",
                        Context::class.java,
                        createRequestBaseClass,
                        CancellationSignal::class.java,
                        Executor::class.java,
                        outcomeReceiverClass,
                    )

                    createMethod.invoke(
                        credentialManager,
                        context,
                        createRequest,
                        cancellationSignal,
                        executor,
                        callbackProxy,
                    )
                } catch (error: Throwable) {
                    onRegisterResult(callbackPtr, false, error.message ?: "registration failed", null)
                }
            }
        }

        @JvmStatic
        fun authenticate(context: Context, requestJson: String, callbackPtr: Long) {
            if (!isAvailable(context)) {
                onAuthenticateResult(
                    callbackPtr,
                    false,
                    "passkey authentication requires Android 14+ credential APIs",
                    null,
                )
                return
            }

            runOnMain {
                try {
                    val credentialManager = credentialManager(context)
                    val getRequest = createGetCredentialRequest(buildAuthenticationRequestJson(requestJson))
                    val getRequestClass = Class.forName("android.credentials.GetCredentialRequest")
                    val outcomeReceiverClass = Class.forName("android.os.OutcomeReceiver")
                    val cancellationSignal = CancellationSignal()
                    val executor = mainThreadExecutor()

                    val callbackProxy = Proxy.newProxyInstance(
                        outcomeReceiverClass.classLoader,
                        arrayOf(outcomeReceiverClass),
                    ) { _, method, args ->
                        when (method.name) {
                            "onResult" -> {
                                val response = args?.getOrNull(0)
                                    ?: throw IllegalStateException("getCredential result payload missing")
                                val responseJson = extractAuthenticationResponseJson(response)
                                onAuthenticateResult(callbackPtr, true, null, responseJson)
                            }
                            "onError" -> {
                                val throwable = args?.getOrNull(0) as? Throwable
                                onAuthenticateResult(
                                    callbackPtr,
                                    false,
                                    throwable?.message ?: "authentication failed",
                                    null,
                                )
                            }
                            else -> Unit
                        }
                        null
                    }

                    val getMethod = credentialManager.javaClass.getMethod(
                        "getCredential",
                        Context::class.java,
                        getRequestClass,
                        CancellationSignal::class.java,
                        Executor::class.java,
                        outcomeReceiverClass,
                    )

                    getMethod.invoke(
                        credentialManager,
                        context,
                        getRequest,
                        cancellationSignal,
                        executor,
                        callbackProxy,
                    )
                } catch (error: Throwable) {
                    onAuthenticateResult(
                        callbackPtr,
                        false,
                        error.message ?: "authentication failed",
                        null,
                    )
                }
            }
        }

        private fun credentialManager(context: Context): Any {
            val managerClass = Class.forName("android.credentials.CredentialManager")
            val getSystemService = Context::class.java.getMethod("getSystemService", Class::class.java)
            return getSystemService.invoke(context, managerClass)
                ?: throw IllegalStateException("CredentialManager service unavailable")
        }

        private fun createPublicKeyCredentialRequest(publicKeyJson: String): Any {
            val requestClass = Class.forName("android.credentials.CreatePublicKeyCredentialRequest")
            val constructor = requestClass.getConstructor(String::class.java)
            return constructor.newInstance(publicKeyJson)
        }

        private fun createGetCredentialRequest(publicKeyJson: String): Any {
            val optionClass = Class.forName("android.credentials.GetPublicKeyCredentialOption")
            val optionConstructor = optionClass.getConstructor(String::class.java)
            val option = optionConstructor.newInstance(publicKeyJson)

            val credentialOptionClass = Class.forName("android.credentials.CredentialOption")
            val builderClass = Class.forName("android.credentials.GetCredentialRequest\$Builder")
            val builder = builderClass.getConstructor().newInstance()

            val addOption = builderClass.getMethod("addCredentialOption", credentialOptionClass)
            addOption.invoke(builder, option)

            return builderClass.getMethod("build").invoke(builder)
        }

        private fun buildRegistrationRequestJson(requestJson: String): String {
            val request = JSONObject(requestJson)
            val publicKey = JSONObject()
                .put("challenge", request.getString("challenge_b64u"))
                .put(
                    "rp",
                    JSONObject()
                        .put("id", request.getString("rp_id"))
                        .put("name", request.getString("rp_name")),
                )
                .put(
                    "user",
                    JSONObject()
                        .put("id", request.getString("user_id_b64u"))
                        .put("name", request.getString("user_name"))
                        .put("displayName", request.getString("user_display_name")),
                )
                .put(
                    "authenticatorSelection",
                    JSONObject()
                        .put(
                            "residentKey",
                            if (request.optBoolean("discoverable", true)) "required" else "discouraged",
                        )
                        .put("userVerification", request.optString("user_verification", "required")),
                )
                .put("attestation", request.optString("attestation", "none"))

            if (!request.isNull("timeout_ms")) {
                publicKey.put("timeout", request.getLong("timeout_ms"))
            }

            val algorithms = request.optJSONArray("algorithms") ?: JSONArray()
            val params = JSONArray()
            for (index in 0 until algorithms.length()) {
                params.put(
                    JSONObject()
                        .put("type", "public-key")
                        .put("alg", algorithms.getInt(index)),
                )
            }
            publicKey.put("pubKeyCredParams", params)

            val excludes = request.optJSONArray("exclude_credentials") ?: JSONArray()
            if (excludes.length() > 0) {
                val excludeCredentials = JSONArray()
                for (index in 0 until excludes.length()) {
                    excludeCredentials.put(
                        JSONObject()
                            .put("type", "public-key")
                            .put("id", excludes.getString(index)),
                    )
                }
                publicKey.put("excludeCredentials", excludeCredentials)
            }

            return JSONObject().put("publicKey", publicKey).toString()
        }

        private fun buildAuthenticationRequestJson(requestJson: String): String {
            val request = JSONObject(requestJson)
            val publicKey = JSONObject()
                .put("challenge", request.getString("challenge_b64u"))
                .put("rpId", request.getString("rp_id"))
                .put("userVerification", request.optString("user_verification", "required"))

            if (!request.isNull("timeout_ms")) {
                publicKey.put("timeout", request.getLong("timeout_ms"))
            }

            val allows = request.optJSONArray("allow_credentials") ?: JSONArray()
            if (allows.length() > 0) {
                val allowCredentials = JSONArray()
                for (index in 0 until allows.length()) {
                    allowCredentials.put(
                        JSONObject()
                            .put("type", "public-key")
                            .put("id", allows.getString(index)),
                    )
                }
                publicKey.put("allowCredentials", allowCredentials)
            }

            return JSONObject().put("publicKey", publicKey).toString()
        }

        private fun extractRegistrationResponseJson(response: Any): String {
            val jsonString = response.javaClass
                .methods
                .firstOrNull { it.name == "getRegistrationResponseJson" }
                ?.invoke(response) as? String
                ?: throw IllegalStateException("registration response JSON not available")

            val credential = JSONObject(jsonString)
            val responsePayload = credential.optJSONObject("response")
                ?: throw IllegalStateException("registration response missing `response`")
            val rawId = credential.optString("rawId", credential.optString("id"))

            if (rawId.isEmpty()) {
                throw IllegalStateException("registration response missing credential id")
            }

            val payload = JSONObject()
                .put("credential_id_b64u", rawId)
                .put("attestation_object_b64u", responsePayload.getString("attestationObject"))
                .put("client_data_json_b64u", responsePayload.getString("clientDataJSON"))

            if (responsePayload.has("authenticatorData")) {
                payload.put("authenticator_data_b64u", responsePayload.getString("authenticatorData"))
            }

            if (responsePayload.has("publicKey")) {
                payload.put("public_key_cose_b64u", responsePayload.getString("publicKey"))
            }

            return payload.toString()
        }

        private fun extractAuthenticationResponseJson(response: Any): String {
            val credential = response.javaClass
                .methods
                .firstOrNull { it.name == "getCredential" }
                ?.invoke(response)
                ?: throw IllegalStateException("authentication result missing credential")

            val jsonString = credential.javaClass
                .methods
                .firstOrNull { it.name == "getAuthenticationResponseJson" }
                ?.invoke(credential) as? String
                ?: throw IllegalStateException("authentication response JSON not available")

            val payload = JSONObject(jsonString)
            val responsePayload = payload.optJSONObject("response")
                ?: throw IllegalStateException("authentication response missing `response`")
            val rawId = payload.optString("rawId", payload.optString("id"))

            if (rawId.isEmpty()) {
                throw IllegalStateException("authentication response missing credential id")
            }

            val normalized = JSONObject()
                .put("credential_id_b64u", rawId)
                .put("authenticator_data_b64u", responsePayload.getString("authenticatorData"))
                .put("client_data_json_b64u", responsePayload.getString("clientDataJSON"))
                .put("signature_b64u", responsePayload.getString("signature"))

            if (responsePayload.has("userHandle")) {
                normalized.put("user_handle_b64u", responsePayload.getString("userHandle"))
            }

            return normalized.toString()
        }

        private fun runOnMain(block: () -> Unit) {
            if (Looper.myLooper() == Looper.getMainLooper()) {
                block()
            } else {
                Handler(Looper.getMainLooper()).post(block)
            }
        }

        private fun mainThreadExecutor(): Executor {
            val handler = Handler(Looper.getMainLooper())
            return Executor { command -> handler.post(command) }
        }

        @JvmStatic
        external fun onRegisterResult(
            callbackPtr: Long,
            success: Boolean,
            errorMessage: String?,
            responseJson: String?,
        )

        @JvmStatic
        external fun onAuthenticateResult(
            callbackPtr: Long,
            success: Boolean,
            errorMessage: String?,
            responseJson: String?,
        )
    }
}
