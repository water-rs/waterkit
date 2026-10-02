package waterkit.clipboard

import android.content.ClipboardManager
import android.content.Context

/**
 * Per-watcher bridge between [ClipboardManager.OnPrimaryClipChangedListener]
 * and the Rust clipboard event channel.
 *
 * `Clipboard::watch` creates one instance of this class and registers it with
 * `ClipboardHelper.startWatching`, so watchers are fully independent.
 *
 * `waterkit_watch_state` carries the Rust-side state pointer. It is written
 * once before registration and zeroed by [releaseNativeState]. Both
 * [onPrimaryClipChanged] and [releaseNativeState] are serialized on this
 * object's monitor, so once release returns no queued clip notification can
 * dereference the released state.
 *
 * `OnPrimaryClipChangedListener` exists since API 11 and fires on every
 * primary clip change — including same-type updates — which is what the old
 * polling watcher could not observe. Callbacks arrive on the main thread.
 */
class ClipboardWatchCallback(context: Context) : ClipboardManager.OnPrimaryClipChangedListener {

    private val appContext = context.applicationContext

    @JvmField
    var waterkit_watch_state: Long = 0

    private external fun onPrimaryClipChangedNative(
        hasText: Boolean,
        hasHtml: Boolean,
        hasFiles: Boolean,
        hasImage: Boolean
    )

    @Synchronized
    override fun onPrimaryClipChanged() {
        if (waterkit_watch_state == 0L) {
            return
        }
        onPrimaryClipChangedNative(
            ClipboardHelper.hasText(appContext),
            ClipboardHelper.hasHtml(appContext),
            ClipboardHelper.hasFiles(appContext),
            ClipboardHelper.hasImage(appContext),
        )
    }

    @Synchronized
    fun releaseNativeState() {
        waterkit_watch_state = 0
    }
}
