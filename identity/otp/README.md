# Waterkit OTP

`waterkit-otp` retrieves one SMS one-time code without requesting
`READ_SMS`.

## Addressed mode

Call `AddressedRequest::start()` and put the returned `AppToken` in the SMS
sent by the server. The server must ask the app for a token on every request:
the value differs between Android's SMS Retriever and app-specific-token
realizations, so a hash must never be hard-coded.

On Android, a device with Google Play services uses SMS Retriever. A device
without Play services uses `SmsManager::createAppSpecificSmsToken` when it
advertises telephony messaging. An SMS Retriever message must contain the
returned token and is limited to 140 bytes. An app-specific-token message must
also contain its returned token; the Retriever-specific 140-byte limit does
not apply to that realization. Neither realization requires an SMS permission.

SMS Retriever and SMS User Consent time out after Play services' five-minute
window. App-specific-token requests have no system deadline and remain pending
until a message arrives or the request is dropped. Apply a caller-side timeout
when using that realization if one is needed.

## Consent mode

Call `ConsentRequest::start(sender)` when the server cannot change its SMS
format. Android's Google Play services User Consent API shows the system
prompt for one incoming message. `sender` can restrict which sender starts the
prompt. The `ndk_context` context must be an `androidx.activity.ComponentActivity`;
WaterUI's `HydrolysisActivity` and the Android harness's `AppCompatActivity`
satisfy this requirement. The consent request times out after Play services'
five-minute window.

## Other platforms

Apple platforms cannot let an application read incoming SMS, so addressed and
consent retrieval are unavailable. iOS and macOS instead offer a code above
the keyboard for a one-time-code text field; use WaterUI's field
([water-rs/waterui#1874](https://github.com/water-rs/waterui/issues/1874)).
Windows, Linux, and wasm are unavailable.

The API uses `start()` followed by `message()` rather than the usual
`Type::new` plus `events()` convention because each request yields exactly one
message, not a stream. `start()` also makes the server token available before
that message is sent.
