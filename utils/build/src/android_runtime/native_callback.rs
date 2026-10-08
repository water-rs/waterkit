//! Java-owned callback objects shared by `WaterKit` crates on Android.
//!
//! A Kotlin helper receives a `waterkit.build.NativeCallback` (one result) or
//! `waterkit.build.NativeChannel` (a stream of values, then `close` or `fail`)
//! argument instead of a numeric id. The Java object holds the Rust peer as a
//! boxed `long`; `complete` / `send` / `close` / `fail` are `synchronized` and
//! call back through `RegisterNatives`-bound natives. The Java object's finalizer
//! releases a still-live peer when the Java object is collected, so the Rust
//! side observes cancellation or stream end instead of a dangling request.

use super::{
    AndroidError, DexHelper, android_error_with_pending_exception, decode_optional_string,
};
use futures_channel::{mpsc, oneshot};
use jni::errors::ThrowRuntimeExAndDefault;
use jni::objects::{Global, JClass, JObject, JValue};
use jni::sys::jlong;
use jni::{Env, EnvUnowned, NativeMethod, jni_sig, jni_str};
use std::marker::PhantomData;

static CALLBACK: DexHelper = DexHelper::new("waterkit.build.NativeCallback");
static CHANNEL: DexHelper = DexHelper::new("waterkit.build.NativeChannel");
static PEER_NATIVES: DexHelper = DexHelper::new("waterkit.build.NativeChannel$PeerNatives");

/// Converts a Java payload object into the Rust value a caller waits on.
///
/// Implement once per payload type in the consuming crate.
pub trait FromJava: Sized {
    /// Decodes `object` delivered through `complete` / `send`.
    ///
    /// # Errors
    ///
    /// Returns [`AndroidError`] when a JNI call or the conversion fails.
    fn from_java(env: &mut Env<'_>, object: &JObject<'_>) -> Result<Self, AndroidError>;
}

impl FromJava for String {
    fn from_java(env: &mut Env<'_>, object: &JObject<'_>) -> Result<Self, AndroidError> {
        super::decode_string(env, object)
    }
}

impl FromJava for Global<JObject<'static>> {
    fn from_java(env: &mut Env<'_>, object: &JObject<'_>) -> Result<Self, AndroidError> {
        env.new_global_ref(object).map_err(AndroidError::from)
    }
}

pub use crate::peers::PeerError;
use crate::peers::{CallbackDelivery, ChannelDelivery};

/// What a boxed peer offers its Java owner.
///
/// The JNI natives call [`deliver`](PeerTarget::deliver) for `complete` and
/// `send`, and [`terminate`](PeerTarget::terminate) for `close`, `fail`, and
/// the finalizer's `release`. `deliver` is the only entry point that needs the
/// JVM; everything else is plain Rust state and is what the unit tests cover.
trait PeerTarget: Send {
    /// A decoded-able payload arrived from Java. `terminal` is set by
    /// `complete` so the implementor stops accepting further values.
    fn deliver(&mut self, env: &mut Env<'_>, object: &JObject<'_>, terminal: bool);

    /// `close` (`None`), `fail` (`Some(message)`), or `release` (`None`).
    /// Runs at most once per peer — the Java side zeroes the peer atomically
    /// before calling, and only one of the terminal calls survives.
    fn terminate(&mut self, reason: Option<String>);
}

/// The boxed target a Java `NativeCallback` / `NativeChannel` points at.
///
/// `Box::into_raw` / `Box::from_raw` appear only here and only for this type,
/// the single sanctioned exception to the no-address-crossing rule.
// SAFETY: the peer is owned by exactly one live Java object, and is consumed
// exactly once by `complete` / `close` / `release`.
struct Peer(Box<dyn PeerTarget>);

/// One result through `waterkit.build.NativeCallback.complete` / `.fail`.
struct CallbackTarget<T> {
    delivery: CallbackDelivery<T>,
}

impl<T: FromJava + Send> PeerTarget for CallbackTarget<T> {
    fn deliver(&mut self, env: &mut Env<'_>, object: &JObject<'_>, _terminal: bool) {
        self.delivery
            .deliver(T::from_java(env, object).map_err(PeerError::from));
    }

    fn terminate(&mut self, reason: Option<String>) {
        self.delivery.terminate(reason);
    }
}

/// A stream of values through `waterkit.build.NativeChannel.send`, ending with
/// `close` (clean end), `fail` (an error item, then the end), or the finalizer's
/// `release` (a bare end — cancellation).
struct ChannelTarget<T> {
    delivery: ChannelDelivery<T>,
}

impl<T: FromJava + Send> PeerTarget for ChannelTarget<T> {
    fn deliver(&mut self, env: &mut Env<'_>, object: &JObject<'_>, _terminal: bool) {
        self.delivery
            .deliver(T::from_java(env, object).map_err(PeerError::from));
    }

    fn terminate(&mut self, reason: Option<String>) {
        self.delivery.terminate(reason);
    }
}

/// A Java `NativeCallback` object paired with the receiver its `complete` or
/// `fail` resolves.
///
/// `T` is decoded from the payload through [`FromJava`]; the receiver resolves
/// to `Err` on `fail` and to `Err(oneshot::Canceled)`-shaped cancellation when
/// the Java object is collected without being answered.
#[derive(Debug)]
pub struct NativeCallback<T> {
    object: Global<JObject<'static>>,
    _payload: PhantomData<fn() -> T>,
}

impl<T: FromJava + Send + 'static> NativeCallback<T> {
    /// Creates a `NativeCallback` Java object and its receiver.
    ///
    /// # Errors
    ///
    /// Returns [`AndroidError`] when the helper class or the object cannot be
    /// created through JNI.
    ///
    /// # Panics
    ///
    /// Panics if `ndk_context` has no `JavaVM` or Android `Context` yet.
    pub fn new(
        env: &mut Env<'_>,
    ) -> Result<(Self, oneshot::Receiver<Result<T, PeerError>>), AndroidError> {
        let (delivery, receiver) = CallbackDelivery::new();
        let target: Box<dyn PeerTarget> = Box::new(CallbackTarget::<T> { delivery });
        let object = new_peer_object(env, &CALLBACK, Peer(target), callback_natives())?;
        Ok((
            Self {
                object,
                _payload: PhantomData,
            },
            receiver,
        ))
    }

    /// The Java object, for `JValue::Object` when calling the helper.
    #[must_use]
    pub fn as_obj(&self) -> &JObject<'_> {
        self.object.as_obj()
    }

    /// The Java object as a global reference.
    #[must_use]
    pub const fn as_global(&self) -> &Global<JObject<'static>> {
        &self.object
    }
}

/// A Java `NativeChannel` object paired with the stream its `send` feeds.
///
/// The stream yields each `send` as an item, `fail` as an error item followed
/// by the end, and `close` or the finalizer's `release` as the end.
#[derive(Debug)]
pub struct NativeChannel<T> {
    object: Global<JObject<'static>>,
    _payload: PhantomData<fn() -> T>,
}

impl<T: FromJava + Send + 'static> NativeChannel<T> {
    /// Creates a `NativeChannel` Java object and its stream.
    ///
    /// # Errors
    ///
    /// Returns [`AndroidError`] when the helper class or the object cannot be
    /// created through JNI.
    ///
    /// # Panics
    ///
    /// Panics if `ndk_context` has no `JavaVM` or Android `Context` yet.
    pub fn new(
        env: &mut Env<'_>,
    ) -> Result<(Self, mpsc::UnboundedReceiver<Result<T, PeerError>>), AndroidError> {
        let (delivery, receiver) = ChannelDelivery::new();
        let target: Box<dyn PeerTarget> = Box::new(ChannelTarget::<T> { delivery });
        let object = new_peer_object(env, &CHANNEL, Peer(target), channel_natives())?;
        Ok((
            Self {
                object,
                _payload: PhantomData,
            },
            receiver,
        ))
    }

    /// The Java object, for `JValue::Object` when calling the helper.
    #[must_use]
    pub fn as_obj(&self) -> &JObject<'_> {
        self.object.as_obj()
    }

    /// The Java object as a global reference.
    #[must_use]
    pub const fn as_global(&self) -> &Global<JObject<'static>> {
        &self.object
    }
}

/// Constructs the Java peer object with the boxed [`Peer`] and binds its
/// natives. `RegisterNatives` is idempotent, so every construction registers
/// without needing a once-guard.
fn new_peer_object(
    env: &mut Env<'_>,
    helper: &DexHelper,
    peer: Peer,
    natives: &[NativeMethod<'static>],
) -> Result<Global<JObject<'static>>, AndroidError> {
    let (_, context) = super::jvm_and_context()?;
    let class = helper.class(env, context.as_obj())?;
    // SAFETY: every descriptor pairs a `private external` method on the class
    // with the matching `extern "system"` fn below; instance methods take
    // `this: JObject` as their second parameter.
    unsafe { env.register_native_methods(class, natives) }
        .map_err(|error| android_error_with_pending_exception(env, error))?;
    register_peer_natives(env, context.as_obj())?;

    // SAFETY: the Java object owns this box until `complete` / `close` /
    // `release` consumes it exactly once (see `Peer`).
    let raw = Box::into_raw(Box::new(peer)) as jlong;
    let object = env
        .new_object(class, jni_sig!("(J)V"), &[JValue::Long(raw)])
        .map_err(|error| android_error_with_pending_exception(env, error))?;
    env.new_global_ref(object)
        .map_err(|error| android_error_with_pending_exception(env, error))
}

fn register_peer_natives(env: &mut Env<'_>, context: &JObject<'_>) -> Result<(), AndroidError> {
    let class = PEER_NATIVES.class(env, context)?;
    // SAFETY: `release` on `NativeChannel$PeerNatives` matches the static
    // `extern "system"` fn below (`class: JClass` second parameter).
    unsafe {
        env.register_native_methods(
            class,
            &[NativeMethod::from_raw_parts(
                jni_str!("releaseNative"),
                jni_str!("(J)V"),
                peer_release as *mut std::ffi::c_void,
            )],
        )
    }
    .map_err(|error| android_error_with_pending_exception(env, error))
}

const fn callback_natives() -> &'static [NativeMethod<'static>] {
    // SAFETY: each fn pointer matches the declared instance signature.
    const NATIVES: [NativeMethod<'static>; 2] = [
        unsafe {
            NativeMethod::from_raw_parts(
                jni_str!("completeNative"),
                jni_str!("(JLjava/lang/Object;)V"),
                callback_complete as *mut std::ffi::c_void,
            )
        },
        unsafe {
            NativeMethod::from_raw_parts(
                jni_str!("failNative"),
                jni_str!("(JLjava/lang/String;)V"),
                peer_fail as *mut std::ffi::c_void,
            )
        },
    ];
    &NATIVES
}

const fn channel_natives() -> &'static [NativeMethod<'static>] {
    // SAFETY: each fn pointer matches the declared instance signature.
    const NATIVES: [NativeMethod<'static>; 3] = [
        unsafe {
            NativeMethod::from_raw_parts(
                jni_str!("sendNative"),
                jni_str!("(JLjava/lang/Object;)V"),
                channel_send as *mut std::ffi::c_void,
            )
        },
        unsafe {
            NativeMethod::from_raw_parts(
                jni_str!("closeNative"),
                jni_str!("(J)V"),
                peer_close as *mut std::ffi::c_void,
            )
        },
        unsafe {
            NativeMethod::from_raw_parts(
                jni_str!("failNative"),
                jni_str!("(JLjava/lang/String;)V"),
                peer_fail as *mut std::ffi::c_void,
            )
        },
    ];
    &NATIVES
}

/// Borrows the peer without consuming it, for the repeatable `send`.
///
/// # Safety
///
/// `peer` must be a live peer pointer owned by a live Java object.
unsafe fn borrow_peer<'a>(peer: jlong) -> &'a mut Peer {
    // SAFETY: guaranteed by the caller — `send` only runs while the Java
    // object (the peer's owner) is alive, and `@Synchronized` plus the atomic
    // claim serialize it against the terminal calls.
    unsafe { &mut *(peer as *mut Peer) }
}

/// Consumes the peer for a terminal call.
///
/// # Safety
///
/// `peer` must be a live peer pointer, claimed by exactly one terminal call —
/// the Java side zeroes the peer atomically before dispatching.
unsafe fn take_peer(peer: jlong) -> Peer {
    // SAFETY: guaranteed by the caller; the peer is reconstructed exactly once.
    *unsafe { Box::from_raw(peer as *mut Peer) }
}

fn decode_fail_reason(env: &Env<'_>, error: &JObject<'_>) -> String {
    if error.is_null() {
        return "the Kotlin helper reported failure".into();
    }
    match decode_optional_string(env, error) {
        Ok(Some(message)) => message,
        Ok(None) => "the Kotlin helper reported failure".into(),
        Err(error) => format!("(failure message could not be decoded: {error})"),
    }
}

extern "system" fn callback_complete<'caller>(
    mut env: EnvUnowned<'caller>,
    _this: JObject<'caller>,
    peer: jlong,
    result: JObject<'caller>,
) {
    env.with_env(|env| -> jni::errors::Result<()> {
        let mut peer = unsafe { take_peer(peer) };
        (peer.0).deliver(env, &result, true);
        drop(peer);
        Ok(())
    })
    .resolve::<ThrowRuntimeExAndDefault>();
}

extern "system" fn channel_send<'caller>(
    mut env: EnvUnowned<'caller>,
    _this: JObject<'caller>,
    peer: jlong,
    value: JObject<'caller>,
) {
    env.with_env(|env| -> jni::errors::Result<()> {
        let peer = unsafe { borrow_peer(peer) };
        (peer.0).deliver(env, &value, false);
        Ok(())
    })
    .resolve::<ThrowRuntimeExAndDefault>();
}

extern "system" fn peer_close<'caller>(
    mut env: EnvUnowned<'caller>,
    _this: JObject<'caller>,
    peer: jlong,
) {
    env.with_env(|_env| -> jni::errors::Result<()> {
        let mut peer = unsafe { take_peer(peer) };
        (peer.0).terminate(None);
        drop(peer);
        Ok(())
    })
    .resolve::<ThrowRuntimeExAndDefault>();
}

extern "system" fn peer_fail<'caller>(
    mut env: EnvUnowned<'caller>,
    _this: JObject<'caller>,
    peer: jlong,
    error: JObject<'caller>,
) {
    env.with_env(|env| -> jni::errors::Result<()> {
        let reason = decode_fail_reason(&*env, &error);
        let mut peer = unsafe { take_peer(peer) };
        (peer.0).terminate(Some(reason));
        drop(peer);
        Ok(())
    })
    .resolve::<ThrowRuntimeExAndDefault>();
}

/// The finalizer's `PeerNatives.releaseNative` — a still-live peer means the
/// Java object was collected unanswered: Rust observes cancellation (callback)
/// or stream end (channel).
extern "system" fn peer_release<'caller>(
    mut env: EnvUnowned<'caller>,
    _class: JClass<'caller>,
    peer: jlong,
) {
    env.with_env(|_env| -> jni::errors::Result<()> {
        drop(unsafe { take_peer(peer) });
        Ok(())
    })
    .resolve::<ThrowRuntimeExAndDefault>();
}
