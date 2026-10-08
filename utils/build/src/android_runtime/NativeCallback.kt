package waterkit.build

import java.util.concurrent.atomic.AtomicLong

/**
 * A single result delivered from Kotlin into Rust.
 *
 * The object owns a boxed Rust peer as a `long`. `complete` delivers the
 * result; `fail` reports an error. Both are `synchronized`, call `external`
 * natives, and zero the peer — a call after that throws
 * [IllegalStateException]. If the object is collected unanswered, the
 * [java.lang.ref.Cleaner] releases the peer through
 * `PeerNatives.releaseNative` — Rust sees cancellation.
 */
class NativeCallback private constructor(peer: Long) {
    private val peer = AtomicLong(peer)

    init {
        PeerCleaner.register(this, peer)
    }

    /** Delivers the result. */
    @Synchronized
    fun complete(result: Any?) {
        val p = peer.getAndSet(0L)
        check(p != 0L) { "NativeCallback already completed or released" }
        completeNative(p, result)
    }

    /** Reports [error] as the result's failure. */
    @Synchronized
    fun fail(error: String?) {
        val p = peer.getAndSet(0L)
        check(p != 0L) { "NativeCallback already completed or released" }
        failNative(p, error)
    }

    private external fun completeNative(peer: Long, result: Any?)
    private external fun failNative(peer: Long, error: String?)
}
