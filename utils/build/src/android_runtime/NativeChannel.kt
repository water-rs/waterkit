package waterkit.build

import java.util.concurrent.atomic.AtomicLong

/**
 * A stream of values delivered from Kotlin into Rust.
 *
 * The object owns a boxed Rust peer as a `long`. `send` may be called any
 * number of times; `close` ends the stream cleanly and `fail` reports an
 * error. All three are `synchronized` and call `external` natives; the
 * terminal calls zero the peer and a call after that throws
 * [IllegalStateException]. If the object is collected with the peer still
 * live, its finalizer releases it through `PeerNatives.releaseNative` — Rust
 * sees the stream end.
 */
class NativeChannel private constructor(peer: Long) {
    private val peer = AtomicLong(peer)

    /** Delivers one stream item. */
    @Synchronized
    fun send(value: Any?) {
        val p = peer.get()
        check(p != 0L) { "send on a closed NativeChannel" }
        sendNative(p, value)
    }

    /** Ends the stream cleanly. */
    @Synchronized
    fun close() {
        val p = peer.getAndSet(0L)
        check(p != 0L) { "close on a closed NativeChannel" }
        closeNative(p)
    }

    /** Ends the stream after reporting [error]. */
    @Synchronized
    fun fail(error: String?) {
        val p = peer.getAndSet(0L)
        check(p != 0L) { "fail on a closed NativeChannel" }
        failNative(p, error)
    }

    /**
     * Releases a peer that was never completed, so Rust observes the end.
     * `java.lang.ref.Cleaner` needs API 33, above the minSdk of 26; ART runs
     * finalizers on its own daemon, so this adds no thread.
     */
    protected fun finalize() {
        PeerNatives.release(peer)
    }

    private external fun sendNative(peer: Long, value: Any?)
    private external fun closeNative(peer: Long)
    private external fun failNative(peer: Long, error: String?)
}

internal object PeerNatives {
    /** Zeroes [peer] and releases it if it was still live. */
    fun release(peer: AtomicLong) {
        val p = peer.getAndSet(0L)
        if (p != 0L) releaseNative(p)
    }

    @JvmStatic external fun releaseNative(peer: Long)
}
