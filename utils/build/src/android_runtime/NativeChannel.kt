package waterkit.build

import java.lang.ref.Cleaner
import java.util.concurrent.atomic.AtomicLong

/**
 * A stream of values delivered from Kotlin into Rust.
 *
 * The object owns a boxed Rust peer as a `long`. `send` may be called any
 * number of times; `close` ends the stream cleanly and `fail` reports an
 * error. All three are `synchronized` and call `external` natives; the
 * terminal calls zero the peer and a call after that throws
 * [IllegalStateException]. If the object is collected with the peer still
 * live, the [Cleaner] releases it through `PeerNatives.releaseNative` — Rust
 * sees the stream end.
 */
class NativeChannel private constructor(peer: Long) {
    private val peer = AtomicLong(peer)

    init {
        PeerCleaner.register(this, peer)
    }

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

    private external fun sendNative(peer: Long, value: Any?)
    private external fun closeNative(peer: Long)
    private external fun failNative(peer: Long, error: String?)
}

/** Shared `java.lang.ref.Cleaner` release for `NativeCallback`/`NativeChannel` peers. */
internal object PeerCleaner {
    private val cleaner = Cleaner.create()

    fun register(obj: Any, peer: AtomicLong) {
        cleaner.register(obj, Releaser(peer))
    }

    private class Releaser(private val peer: AtomicLong) : Runnable {
        override fun run() {
            val p = peer.getAndSet(0L)
            if (p != 0L) PeerNatives.releaseNative(p)
        }
    }
}

internal object PeerNatives {
    @JvmStatic external fun releaseNative(peer: Long)
}
