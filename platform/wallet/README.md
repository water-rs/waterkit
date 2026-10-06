# waterkit-wallet

Native pass-adding flows for Google Wallet on Android and Apple Wallet on iOS.

The crate validates payload shape before handing it to the operating system; it
does not create payloads or verify their signatures. Android accepts a
server-signed Google Wallet JWT, which can contain multiple objects. iOS accepts
one or more signed `.pkpass` archives.

On Android, the published `ndk-context` context must be an `Activity`. The host
activity must forward the result from `onActivityResult` to
`WalletHelper.onActivityResult`; the helper uses request code `0x5741`.
Applications without Google Play services report wallet as unavailable.

macOS, Windows, Linux, and other targets report the wallet flow as unavailable
and do not expose `add`: PassKit's add-pass review controller is iOS-only.
