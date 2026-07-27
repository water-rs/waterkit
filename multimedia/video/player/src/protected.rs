//! Android platform-CDM playback through secure `MediaCodec` output.

use std::{marker::PhantomData, num::NonZeroUsize, sync::Arc, time::Duration};

use jni::{
    Env, JavaVM,
    errors::Error as JniError,
    jni_sig, jni_str,
    objects::{Global, JByteArray, JIntArray, JObject, JString, JThrowable, JValue},
    strings::JNIStr,
};
use waterkit_audio::DecodedAudioFrame;
use waterkit_codec::NalStreamConverter;
use waterkit_video_container::{Codec, EncodedSample, TrackInfo, TrackKind};
use waterkit_video_core::{CommonEncryptionScheme, Error, ProtectionInitData, TrackProtection};
use waterkit_video_streaming::{LicenseRequest, LicenseResponse, LicenseServer, Url};

use crate::{AndroidVideoSurface, android_surface::with_attached_env};

type GlobalObjectRef = Global<JObject<'static>>;

const ANDROID_MINIMUM_API: i32 = 24;
const MEDIA_DRM_KEY_TYPE_STREAMING: i32 = 1;
const MEDIA_DRM_KEY_TYPE_OFFLINE: i32 = 2;
const MEDIA_DRM_KEY_TYPE_RELEASE: i32 = 3;
const MEDIA_CODEC_LIST_ALL_CODECS: i32 = 1;
const MEDIA_CODEC_INFO_TRY_AGAIN_LATER: i32 = -1;
const MEDIA_CODEC_INFO_OUTPUT_FORMAT_CHANGED: i32 = -2;
const MEDIA_CODEC_INFO_OUTPUT_BUFFERS_CHANGED: i32 = -3;
const MEDIA_CODEC_BUFFER_FLAG_END_OF_STREAM: i32 = 4;
const MEDIA_CODEC_CRYPTO_MODE_AES_CTR: i32 = 1;
const MEDIA_CODEC_CRYPTO_MODE_AES_CBC: i32 = 2;
const MEDIA_CODEC_PCM_16_BIT: i32 = 2;
const MEDIA_CODEC_PCM_FLOAT: i32 = 4;
const INPUT_WAIT_FOREVER_MICROS: i64 = -1;

/// Android DRM key-request reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AndroidKeyRequestType {
    /// Initial streaming license acquisition.
    Initial,
    /// Renewal before or after key expiration.
    Renewal,
    /// Offline-license release.
    Release,
    /// The platform did not provide a request reason.
    None,
    /// Existing keys require an update.
    Update,
}

/// Remaining lifetime reported by an Android DRM provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AndroidKeyDuration {
    /// The provider reports that the key does not expire.
    Unlimited,
    /// Concrete remaining lifetime.
    Remaining(Duration),
    /// The provider does not expose this lifetime.
    Unavailable,
}

/// Current streaming-key lifetime reported by `MediaDrm.queryKeyStatus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AndroidKeyStatus {
    license: AndroidKeyDuration,
    playback: AndroidKeyDuration,
}

/// Opaque, non-empty Android persistent-license identity.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AndroidOfflineKeySet(Vec<u8>);

impl AndroidOfflineKeySet {
    /// Creates a persistent key-set identity returned by `MediaDrm`.
    ///
    /// # Errors
    ///
    /// Returns a platform error when the identity is empty.
    pub fn new(bytes: impl Into<Vec<u8>>) -> Result<Self, Error> {
        let bytes = bytes.into();
        if bytes.is_empty() {
            return Err(Error::Platform(String::from(
                "Android offline key-set identity must not be empty",
            )));
        }
        Ok(Self(bytes))
    }

    /// Returns the opaque key-set bytes owned by the platform CDM.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl AndroidKeyStatus {
    /// Returns the remaining license lifetime.
    #[must_use]
    pub const fn license_duration(&self) -> AndroidKeyDuration {
        self.license
    }

    /// Returns the remaining playback lifetime.
    #[must_use]
    pub const fn playback_duration(&self) -> AndroidKeyDuration {
        self.playback
    }

    /// Returns whether either finite lifetime has reached the renewal threshold.
    #[must_use]
    pub fn requires_renewal(&self, threshold: Duration) -> bool {
        [self.license, self.playback].into_iter().any(|duration| {
            matches!(duration, AndroidKeyDuration::Remaining(remaining) if remaining <= threshold)
        })
    }
}

impl AndroidKeyRequestType {
    fn from_android(value: i32) -> Result<Self, Error> {
        match value {
            0 => Ok(Self::Initial),
            1 => Ok(Self::Renewal),
            2 => Ok(Self::Release),
            3 => Ok(Self::None),
            4 => Ok(Self::Update),
            _ => Err(Error::Platform(format!(
                "Android MediaDrm returned unknown key request type {value}"
            ))),
        }
    }
}

/// Secure Android output surface retained across a protected decode session.
pub struct AndroidProtectedSurface {
    context: AndroidDrmContext,
    surface: AndroidVideoSurface,
}

/// Instance-scoped Android JVM context used to open platform CDM sessions.
#[derive(Clone)]
pub struct AndroidDrmContext {
    vm: Arc<JavaVM>,
}

impl AndroidDrmContext {
    /// Creates a DRM context from the current Android JVM.
    ///
    /// # Safety
    ///
    /// `env` must belong to the application JVM that owns subsequent media objects.
    ///
    /// # Errors
    ///
    /// Returns an error when Android is too old or the `JavaVM` cannot be retained.
    pub unsafe fn from_jni(env: &mut Env<'_>) -> Result<Self, Error> {
        let api_level = android_api_level(env)?;
        if api_level < ANDROID_MINIMUM_API {
            return Err(Error::Unsupported(format!(
                "protected Android playback requires API {ANDROID_MINIMUM_API} or newer, got {api_level}"
            )));
        }
        let vm = env
            .get_java_vm()
            .map_err(|error| platform_jni_error(env, "get JavaVM", error))?;
        Ok(Self { vm: Arc::new(vm) })
    }

    /// Returns whether Android exposes a CDM for the specified DRM UUID.
    ///
    /// # Errors
    ///
    /// Returns a platform error when the JVM query fails.
    pub fn supports_system_id(&self, system_id: &[u8; 16]) -> Result<bool, Error> {
        with_attached_env(&self.vm, |env| {
            let uuid = create_java_uuid(env, *system_id)?;
            env.call_static_method(
                jni_str!("android/media/MediaDrm"),
                jni_str!("isCryptoSchemeSupported"),
                jni_sig!("(Ljava/util/UUID;)Z"),
                &[JValue::Object(&uuid)],
            )
            .and_then(jni::objects::JValueOwned::z)
            .map_err(|error| platform_jni_error(env, "query Android DRM system support", error))
        })
    }
}

impl std::fmt::Debug for AndroidDrmContext {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AndroidDrmContext")
            .finish_non_exhaustive()
    }
}

impl AndroidProtectedSurface {
    /// Marks a retained video surface as protected output.
    ///
    /// # Safety
    ///
    /// The surface must originate from a `SurfaceView` whose secure flag was
    /// set before attachment to its window.
    #[must_use]
    pub unsafe fn from_video_surface(surface: AndroidVideoSurface) -> Self {
        let context = AndroidDrmContext {
            vm: Arc::clone(&surface.context.vm),
        };
        Self { context, surface }
    }

    /// Retains an Android `Surface` created by a secure `SurfaceView`.
    ///
    /// `SurfaceView.setSecure(true)` must have been called before the containing
    /// window was attached. This is a caller-held platform invariant because
    /// Android exposes no API for querying it from a `Surface`.
    ///
    /// # Safety
    ///
    /// `surface` must come from a `SurfaceView` configured as secure before
    /// window attachment and must belong to the same JVM as `env`.
    ///
    /// # Errors
    ///
    /// Returns a platform error when the Android API level is below 24, the
    /// object is not an `android.view.Surface`, or a global reference fails.
    pub unsafe fn from_jni(env: &mut Env<'_>, surface: &JObject<'_>) -> Result<Self, Error> {
        let surface = unsafe { AndroidVideoSurface::from_jni(env, surface) }?;
        Ok(unsafe { Self::from_video_surface(surface) })
    }

    /// Returns the instance-scoped platform-CDM context.
    #[must_use]
    pub const fn drm_context(&self) -> &AndroidDrmContext {
        &self.context
    }

    /// Returns whether Android exposes a CDM for the specified DRM UUID.
    ///
    /// # Errors
    ///
    /// Returns a platform error when the JVM query fails.
    pub fn supports_system_id(&self, system_id: &[u8; 16]) -> Result<bool, Error> {
        self.context.supports_system_id(system_id)
    }
}

impl Clone for AndroidProtectedSurface {
    fn clone(&self) -> Self {
        Self {
            context: self.context.clone(),
            surface: self.surface.clone(),
        }
    }
}

impl std::fmt::Debug for AndroidProtectedSurface {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AndroidProtectedSurface")
            .finish_non_exhaustive()
    }
}

/// Opaque Android device-provisioning request.
#[derive(Debug, Clone)]
pub struct AndroidProvisionRequest {
    system_id: [u8; 16],
    data: Vec<u8>,
    default_url: Option<Url>,
}

impl AndroidProvisionRequest {
    /// Returns the DRM system UUID in network byte order.
    #[must_use]
    pub const fn system_id(&self) -> &[u8; 16] {
        &self.system_id
    }

    /// Returns the opaque platform provisioning message.
    #[must_use]
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// Returns the platform-recommended provisioning endpoint when supplied.
    #[must_use]
    pub const fn default_url(&self) -> Option<&Url> {
        self.default_url.as_ref()
    }

    /// Builds a bounded Zenwave provisioning exchange.
    ///
    /// `url` overrides the platform recommendation. An override is required
    /// when Android returns an empty default URL.
    ///
    /// # Errors
    ///
    /// Returns an error when neither an override nor a default URL exists.
    pub fn network_request(
        &self,
        url: Option<Url>,
        maximum_response_bytes: NonZeroUsize,
    ) -> Result<LicenseRequest, Error> {
        let url = url.or_else(|| self.default_url.clone()).ok_or_else(|| {
            Error::Streaming(String::from(
                "Android MediaDrm provisioning request has no server URL",
            ))
        })?;
        LicenseRequest::new(
            self.system_id,
            url,
            self.data.clone(),
            maximum_response_bytes,
        )
    }
}

/// Opaque Android streaming-license challenge.
#[derive(Debug, Clone)]
pub struct AndroidLicenseChallenge {
    system_id: [u8; 16],
    data: Vec<u8>,
    default_url: Option<Url>,
    request_type: AndroidKeyRequestType,
}

impl AndroidLicenseChallenge {
    /// Returns the DRM system UUID in network byte order.
    #[must_use]
    pub const fn system_id(&self) -> &[u8; 16] {
        &self.system_id
    }

    /// Returns the opaque platform key request.
    #[must_use]
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// Returns the platform-recommended license endpoint when supplied.
    #[must_use]
    pub const fn default_url(&self) -> Option<&Url> {
        self.default_url.as_ref()
    }

    /// Returns why Android requested a license exchange.
    #[must_use]
    pub const fn request_type(&self) -> AndroidKeyRequestType {
        self.request_type
    }

    /// Builds a bounded Zenwave license exchange.
    ///
    /// `url` overrides the platform recommendation. An override is required
    /// when Android returns an empty default URL.
    ///
    /// # Errors
    ///
    /// Returns an error when neither an override nor a default URL exists.
    pub fn network_request(
        &self,
        url: Option<Url>,
        maximum_response_bytes: NonZeroUsize,
    ) -> Result<LicenseRequest, Error> {
        let url = url.or_else(|| self.default_url.clone()).ok_or_else(|| {
            Error::Streaming(String::from(
                "Android MediaDrm key request has no license server URL",
            ))
        })?;
        LicenseRequest::new(
            self.system_id,
            url,
            self.data.clone(),
            maximum_response_bytes,
        )
    }
}

/// Type marker for a protected video decoder flow.
#[doc(hidden)]
#[derive(Debug)]
pub struct AndroidVideoDecoderTarget;

/// Type marker for a protected audio decoder flow.
#[doc(hidden)]
#[derive(Debug)]
pub struct AndroidAudioDecoderTarget;

/// First Android DRM state: either device provisioning or license acquisition.
#[derive(Debug)]
pub enum AndroidDrmBootstrap<T = AndroidVideoDecoderTarget> {
    /// Device provisioning must finish before a key request can be generated.
    Provisioning(AndroidProvisioningDecoder<T>),
    /// The platform CDM is ready for a license response.
    License(AndroidPendingDecoder<T>),
    /// A persistent license was restored into the open CDM session.
    Restored(AndroidReadyDecoder<T>),
}

/// Android DRM bootstrap for a protected AAC track.
pub type AndroidAudioDrmBootstrap = AndroidDrmBootstrap<AndroidAudioDecoderTarget>;

/// Type marker for acquiring a new Android persistent license.
#[derive(Debug)]
pub struct AndroidOfflineLicenseAcquisition {
    init_data: ProtectionInitData,
    container_mime: &'static str,
}

/// Type marker for renewing an existing Android persistent license.
#[derive(Debug)]
pub struct AndroidOfflineLicenseRenewal {
    init_data: ProtectionInitData,
    container_mime: &'static str,
    key_set: AndroidOfflineKeySet,
}

/// Type marker for releasing an Android persistent license.
#[derive(Debug)]
pub struct AndroidOfflineLicenseRelease {
    system_id: [u8; 16],
    key_set: AndroidOfflineKeySet,
}

/// First Android offline-license state: provisioning or a license challenge.
pub enum AndroidOfflineLicenseBootstrap<T> {
    /// Device provisioning must finish before the license operation can continue.
    Provisioning(AndroidOfflineLicenseProvisioning<T>),
    /// The platform CDM is ready for a license response.
    License(AndroidPendingOfflineLicense<T>),
}

/// Offline-license acquisition bootstrap.
pub type AndroidOfflineLicenseAcquisitionBootstrap =
    AndroidOfflineLicenseBootstrap<AndroidOfflineLicenseAcquisition>;

/// Offline-license renewal bootstrap.
pub type AndroidOfflineLicenseRenewalBootstrap =
    AndroidOfflineLicenseBootstrap<AndroidOfflineLicenseRenewal>;

/// Offline-license release bootstrap.
pub type AndroidOfflineLicenseReleaseBootstrap =
    AndroidOfflineLicenseBootstrap<AndroidOfflineLicenseRelease>;

/// Android offline-license operation waiting for device provisioning.
pub struct AndroidOfflineLicenseProvisioning<T> {
    request: AndroidProvisionRequest,
    operation: AndroidOfflineLicenseRequest<T>,
}

/// Android offline-license operation waiting for a license response.
pub struct AndroidPendingOfflineLicense<T> {
    challenge: AndroidLicenseChallenge,
    operation: AndroidOfflineLicenseRequest<T>,
}

struct AndroidOfflineLicenseRequest<T> {
    drm_session: AndroidDrmSession,
    target: T,
}

impl AndroidOfflineLicenseAcquisitionBootstrap {
    /// Starts acquisition of a new persistent license for one protected track.
    ///
    /// # Errors
    ///
    /// Returns an error for a clear or unsupported track, mismatched protection
    /// initialization data, an unsupported DRM system, or Android JNI failure.
    pub fn new(
        context: &AndroidDrmContext,
        track: &TrackInfo,
        init_data: ProtectionInitData,
    ) -> Result<Self, Error> {
        let container_mime = offline_license_container_mime(track, &init_data)?;
        begin_offline_license(
            context,
            AndroidOfflineLicenseAcquisition {
                init_data,
                container_mime,
            },
        )
    }
}

impl AndroidOfflineLicenseRenewalBootstrap {
    /// Starts renewal of an existing persistent license.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid track or key set, a DRM-system mismatch,
    /// a rejected restore, or Android JNI failure.
    pub fn renew(
        context: &AndroidDrmContext,
        track: &TrackInfo,
        init_data: ProtectionInitData,
        key_set: AndroidOfflineKeySet,
    ) -> Result<Self, Error> {
        let container_mime = offline_license_container_mime(track, &init_data)?;
        begin_offline_license(
            context,
            AndroidOfflineLicenseRenewal {
                init_data,
                container_mime,
                key_set,
            },
        )
    }
}

impl AndroidOfflineLicenseReleaseBootstrap {
    /// Starts release of an existing persistent license.
    ///
    /// # Errors
    ///
    /// Returns an error when the DRM system is unsupported or Android rejects
    /// the persistent key-set identity.
    pub fn release(
        context: &AndroidDrmContext,
        system_id: [u8; 16],
        key_set: AndroidOfflineKeySet,
    ) -> Result<Self, Error> {
        begin_offline_license(context, AndroidOfflineLicenseRelease { system_id, key_set })
    }
}

impl<T> AndroidOfflineLicenseProvisioning<T> {
    /// Returns the current device-provisioning request.
    #[must_use]
    pub const fn request(&self) -> &AndroidProvisionRequest {
        &self.request
    }
}

impl<T> AndroidPendingOfflineLicense<T> {
    /// Returns the current offline-license challenge.
    #[must_use]
    pub const fn challenge(&self) -> &AndroidLicenseChallenge {
        &self.challenge
    }
}

macro_rules! impl_offline_provisioning {
    ($target:ty) => {
        impl AndroidOfflineLicenseProvisioning<$target> {
            /// Applies the provisioning response and advances to a license challenge.
            ///
            /// # Errors
            ///
            /// Returns an error when the response is empty, rejected, or the CDM
            /// still reports that provisioning is required afterward.
            pub fn provide_response(
                self,
                response: &LicenseResponse,
            ) -> Result<AndroidOfflineLicenseBootstrap<$target>, Error> {
                complete_offline_provisioning(self, response)
            }
        }
    };
}

impl_offline_provisioning!(AndroidOfflineLicenseAcquisition);
impl_offline_provisioning!(AndroidOfflineLicenseRenewal);
impl_offline_provisioning!(AndroidOfflineLicenseRelease);

impl AndroidPendingOfflineLicense<AndroidOfflineLicenseAcquisition> {
    /// Applies the license response and returns its persistent key-set identity.
    ///
    /// # Errors
    ///
    /// Returns an error when Android rejects the response or returns no key set.
    pub fn provide_response(
        self,
        response: &LicenseResponse,
    ) -> Result<AndroidOfflineKeySet, Error> {
        finish_offline_license(self, response)
    }
}

impl AndroidPendingOfflineLicense<AndroidOfflineLicenseRenewal> {
    /// Applies the renewal response and returns the replacement key-set identity.
    ///
    /// # Errors
    ///
    /// Returns an error when Android rejects the response or returns no key set.
    pub fn provide_response(
        self,
        response: &LicenseResponse,
    ) -> Result<AndroidOfflineKeySet, Error> {
        finish_offline_license(self, response)
    }
}

impl AndroidPendingOfflineLicense<AndroidOfflineLicenseRelease> {
    /// Applies the release response and invalidates the persistent license.
    ///
    /// # Errors
    ///
    /// Returns an error when Android rejects the response or unexpectedly
    /// returns another key-set identity.
    pub fn provide_response(self, response: &LicenseResponse) -> Result<(), Error> {
        finish_offline_license(self, response)
    }
}

impl<T> std::fmt::Debug for AndroidOfflineLicenseBootstrap<T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Provisioning(state) => state.fmt(formatter),
            Self::License(state) => state.fmt(formatter),
        }
    }
}

impl<T> std::fmt::Debug for AndroidOfflineLicenseProvisioning<T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AndroidOfflineLicenseProvisioning")
            .field("request", &self.request)
            .finish_non_exhaustive()
    }
}

impl<T> std::fmt::Debug for AndroidPendingOfflineLicense<T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AndroidPendingOfflineLicense")
            .field("challenge", &self.challenge)
            .finish_non_exhaustive()
    }
}

impl AndroidDrmBootstrap<AndroidVideoDecoderTarget> {
    /// Opens a platform CDM session for one protected CMAF video track.
    ///
    /// # Errors
    ///
    /// Returns an error for clear/non-video tracks, unsupported codecs, a DRM
    /// system mismatch, malformed codec configuration, or Android JNI failure.
    pub fn new(
        surface: AndroidProtectedSurface,
        track: TrackInfo,
        init_data: ProtectionInitData,
    ) -> Result<Self, Error> {
        Self::from_inner(AndroidDecoderBootstrap::new_video(
            surface, track, init_data,
        )?)
    }

    /// Restores persistent keys and prepares one protected CMAF video track.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid tracks, key sets, DRM state, or Android JNI failure.
    pub fn restore(
        surface: AndroidProtectedSurface,
        track: TrackInfo,
        init_data: ProtectionInitData,
        key_set: AndroidOfflineKeySet,
    ) -> Result<Self, Error> {
        Self::from_inner(
            AndroidDecoderBootstrap::new_video(surface, track, init_data)?
                .with_offline_key_set(key_set),
        )
    }
}

impl AndroidDrmBootstrap<AndroidAudioDecoderTarget> {
    /// Opens a platform CDM session for one protected CMAF AAC track.
    ///
    /// The DRM context is instance-scoped and decoded audio is returned as
    /// owned PCM; no video Surface is activated for audio-only protection.
    ///
    /// # Errors
    ///
    /// Returns an error for clear/non-audio tracks, unsupported codecs,
    /// malformed configuration, or Android JNI failure.
    pub fn new(
        context: &AndroidDrmContext,
        track: TrackInfo,
        init_data: ProtectionInitData,
    ) -> Result<Self, Error> {
        Self::from_inner(AndroidDecoderBootstrap::new_audio(
            context, track, init_data,
        )?)
    }

    /// Restores persistent keys and prepares one protected CMAF AAC track.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid tracks, key sets, DRM state, or Android JNI failure.
    pub fn restore(
        context: &AndroidDrmContext,
        track: TrackInfo,
        init_data: ProtectionInitData,
        key_set: AndroidOfflineKeySet,
    ) -> Result<Self, Error> {
        Self::from_inner(
            AndroidDecoderBootstrap::new_audio(context, track, init_data)?
                .with_offline_key_set(key_set),
        )
    }
}

impl<T> AndroidDrmBootstrap<T> {
    fn from_inner(mut inner: AndroidDecoderBootstrap) -> Result<Self, Error> {
        match inner.prepare_keys()? {
            KeyPreparation::Ready(challenge) => Ok(Self::License(AndroidPendingDecoder {
                inner,
                challenge,
                target: PhantomData,
            })),
            KeyPreparation::Restored => Ok(Self::Restored(AndroidReadyDecoder {
                inner,
                target: PhantomData,
            })),
            KeyPreparation::ProvisionRequired => {
                let request = inner.prepare_provision_request()?;
                Ok(Self::Provisioning(AndroidProvisioningDecoder {
                    inner,
                    request,
                    target: PhantomData,
                }))
            }
        }
    }
}

/// Android DRM session waiting for device provisioning.
pub struct AndroidProvisioningDecoder<T> {
    inner: AndroidDecoderBootstrap,
    request: AndroidProvisionRequest,
    target: PhantomData<T>,
}

/// Android DRM session with persistent keys restored and ready for decoder creation.
pub struct AndroidReadyDecoder<T> {
    inner: AndroidDecoderBootstrap,
    target: PhantomData<T>,
}

/// Restored persistent-license state for a protected video decoder.
pub type AndroidReadyVideoDecoder = AndroidReadyDecoder<AndroidVideoDecoderTarget>;

/// Restored persistent-license state for a protected audio decoder.
pub type AndroidReadyAudioDecoder = AndroidReadyDecoder<AndroidAudioDecoderTarget>;

/// Provisioning state for a protected video decoder.
pub type AndroidProvisioningVideoDecoder = AndroidProvisioningDecoder<AndroidVideoDecoderTarget>;

/// Provisioning state for a protected audio decoder.
pub type AndroidProvisioningAudioDecoder = AndroidProvisioningDecoder<AndroidAudioDecoderTarget>;

impl<T> AndroidProvisioningDecoder<T> {
    /// Returns the current provisioning request.
    #[must_use]
    pub const fn request(&self) -> &AndroidProvisionRequest {
        &self.request
    }

    /// Provides an opaque provisioning response and advances to license state.
    ///
    /// # Errors
    ///
    /// Returns an error when Android rejects provisioning or cannot open a
    /// fresh DRM session afterward.
    pub fn provide_response(
        self,
        response: &LicenseResponse,
    ) -> Result<AndroidDrmBootstrap<T>, Error> {
        self.inner.provide_provision_response(response.bytes())?;
        AndroidDrmBootstrap::from_inner(self.inner)
    }

    /// Acquires and applies the provisioning response through a license server.
    ///
    /// # Errors
    ///
    /// Returns any network or platform provisioning failure.
    pub async fn provision(
        self,
        server: &impl LicenseServer,
        request: LicenseRequest,
    ) -> Result<AndroidDrmBootstrap<T>, Error> {
        let response = server.acquire(request).await?;
        self.provide_response(&response)
    }
}

impl AndroidReadyDecoder<AndroidVideoDecoderTarget> {
    /// Creates a secure surface decoder from a restored persistent license.
    ///
    /// # Errors
    ///
    /// Returns an error when no compatible secure hardware decoder can be configured.
    pub fn configure(self) -> Result<AndroidProtectedVideoDecoder, Error> {
        self.inner.configure_video_decoder()
    }
}

impl AndroidReadyDecoder<AndroidAudioDecoderTarget> {
    /// Creates a protected AAC-to-PCM decoder from a restored persistent license.
    ///
    /// # Errors
    ///
    /// Returns an error when no compatible protected AAC decoder can be configured.
    pub fn configure(self) -> Result<AndroidProtectedAudioDecoder, Error> {
        self.inner.configure_audio_decoder()
    }
}

impl<T> std::fmt::Debug for AndroidReadyDecoder<T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AndroidReadyDecoder")
            .finish_non_exhaustive()
    }
}

impl<T> std::fmt::Debug for AndroidProvisioningDecoder<T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AndroidProvisioningDecoder")
            .field("request", &self.request)
            .finish_non_exhaustive()
    }
}

/// Android DRM session waiting for a streaming-license response.
pub struct AndroidPendingDecoder<T> {
    inner: AndroidDecoderBootstrap,
    challenge: AndroidLicenseChallenge,
    target: PhantomData<T>,
}

/// License state for a protected video decoder.
pub type AndroidPendingVideoDecoder = AndroidPendingDecoder<AndroidVideoDecoderTarget>;

/// License state for a protected audio decoder.
pub type AndroidPendingAudioDecoder = AndroidPendingDecoder<AndroidAudioDecoderTarget>;

impl<T> AndroidPendingDecoder<T> {
    /// Returns the current platform-CDM license challenge.
    #[must_use]
    pub const fn challenge(&self) -> &AndroidLicenseChallenge {
        &self.challenge
    }
}

impl AndroidPendingDecoder<AndroidVideoDecoderTarget> {
    /// Provides license bytes and creates a secure surface decoder.
    ///
    /// # Errors
    ///
    /// Returns an error when Android rejects the keys or no compatible secure
    /// hardware decoder can be configured.
    pub fn provide_response(
        self,
        response: &LicenseResponse,
    ) -> Result<AndroidProtectedVideoDecoder, Error> {
        self.inner.provide_key_response(response.bytes())?;
        self.inner.configure_video_decoder()
    }

    /// Acquires a license and creates a secure surface decoder.
    ///
    /// # Errors
    ///
    /// Returns any network, key-response, or secure decoder failure.
    pub async fn license(
        self,
        server: &impl LicenseServer,
        request: LicenseRequest,
    ) -> Result<AndroidProtectedVideoDecoder, Error> {
        let response = server.acquire(request).await?;
        self.provide_response(&response)
    }
}

impl AndroidPendingDecoder<AndroidAudioDecoderTarget> {
    /// Provides license bytes and creates a protected AAC-to-PCM decoder.
    ///
    /// # Errors
    ///
    /// Returns an error when Android rejects the keys or cannot configure a
    /// compatible protected AAC decoder.
    pub fn provide_response(
        self,
        response: &LicenseResponse,
    ) -> Result<AndroidProtectedAudioDecoder, Error> {
        self.inner.provide_key_response(response.bytes())?;
        self.inner.configure_audio_decoder()
    }

    /// Acquires a license and creates a protected AAC-to-PCM decoder.
    ///
    /// # Errors
    ///
    /// Returns any network, key-response, or decoder failure.
    pub async fn license(
        self,
        server: &impl LicenseServer,
        request: LicenseRequest,
    ) -> Result<AndroidProtectedAudioDecoder, Error> {
        let response = server.acquire(request).await?;
        self.provide_response(&response)
    }
}

impl<T> std::fmt::Debug for AndroidPendingDecoder<T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AndroidPendingDecoder")
            .field("challenge", &self.challenge)
            .finish_non_exhaustive()
    }
}

/// One secure decoded output buffer retained by Android until presentation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AndroidProtectedVideoOutput {
    index: i32,
    sequence: u64,
    presentation_time: Duration,
    end_of_stream: bool,
}

impl AndroidProtectedVideoOutput {
    /// Returns the decoder-local generation used to reject stale releases.
    #[must_use]
    pub const fn sequence(self) -> u64 {
        self.sequence
    }

    /// Returns the presentation timestamp supplied with the encoded sample.
    #[must_use]
    pub const fn presentation_time(self) -> Duration {
        self.presentation_time
    }

    /// Returns whether Android marked this output as end-of-stream.
    #[must_use]
    pub const fn is_end_of_stream(self) -> bool {
        self.end_of_stream
    }
}

struct AndroidCodecResources {
    drm_session: AndroidDrmSession,
    media_crypto: GlobalObjectRef,
    codec: GlobalObjectRef,
    offline_key_set: Option<AndroidOfflineKeySet>,
}

impl AndroidCodecResources {
    fn with_env<T>(
        &self,
        operation: impl FnOnce(&mut Env<'_>, &JObject<'_>) -> Result<T, Error>,
    ) -> Result<T, Error> {
        with_attached_env(&self.drm_session.vm, |env| {
            operation(env, self.codec.as_obj())
        })
    }

    fn provide_key_response(
        &mut self,
        response: &LicenseResponse,
    ) -> Result<Option<AndroidOfflineKeySet>, Error> {
        let updated = self.drm_session.provide_key_response(response.bytes())?;
        match (&self.offline_key_set, updated) {
            (None, None) => Ok(None),
            (None, Some(_)) => Err(Error::Platform(String::from(
                "Android returned an offline key-set identity for a streaming renewal",
            ))),
            (Some(_), None) => Err(Error::Platform(String::from(
                "Android offline renewal returned no persistent key-set identity",
            ))),
            (Some(_), Some(updated)) => {
                self.offline_key_set = Some(updated.clone());
                Ok(Some(updated))
            }
        }
    }

    fn key_status(&self) -> Result<AndroidKeyStatus, Error> {
        self.drm_session.key_status()
    }

    fn renewal_challenge_if_needed(
        &self,
        init_data: &ProtectionInitData,
        container_mime: &str,
        threshold: Duration,
    ) -> Result<Option<AndroidLicenseChallenge>, Error> {
        if !self.key_status()?.requires_renewal(threshold) {
            return Ok(None);
        }
        match self.drm_session.prepare_key_request(
            init_data,
            container_mime,
            if self.offline_key_set.is_some() {
                MEDIA_DRM_KEY_TYPE_OFFLINE
            } else {
                MEDIA_DRM_KEY_TYPE_STREAMING
            },
        )? {
            LicensePreparation::Ready(challenge) => Ok(Some(challenge)),
            LicensePreparation::ProvisionRequired => Err(Error::Platform(String::from(
                "Android MediaDrm requested provisioning during key renewal",
            ))),
        }
    }

    fn discard_output_on_drop(&self, index: i32) {
        if let Err(error) = with_attached_env(&self.drm_session.vm, |env| {
            env.call_method(
                self.codec.as_obj(),
                jni_str!("releaseOutputBuffer"),
                jni_sig!("(IZ)V"),
                &[JValue::Int(index), JValue::Bool(false)],
            )
            .map_err(|error| platform_jni_error(env, "discard protected Android output", error))?;
            Ok(())
        }) {
            tracing::error!(%error, "failed to discard outstanding protected Android output");
        }
    }
}

impl Drop for AndroidCodecResources {
    fn drop(&mut self) {
        if let Err(error) = with_attached_env(&self.drm_session.vm, |env| {
            release_java_object(
                env,
                self.codec.as_obj(),
                jni_str!("stop"),
                "stop",
                "protected MediaCodec",
            );
            release_java_object(
                env,
                self.codec.as_obj(),
                jni_str!("release"),
                "release",
                "protected MediaCodec",
            );
            release_java_object(
                env,
                self.media_crypto.as_obj(),
                jni_str!("release"),
                "release",
                "MediaCrypto",
            );
            Ok(())
        }) {
            tracing::error!(%error, "failed to attach JVM while releasing protected Android decoder");
        }
    }
}

/// Licensed Android decoder whose pixels remain inside a protected Surface.
pub struct AndroidProtectedVideoDecoder {
    resources: AndroidCodecResources,
    _surface: Arc<GlobalObjectRef>,
    converter: NalStreamConverter,
    track: TrackInfo,
    init_data: ProtectionInitData,
    container_mime: &'static str,
    outstanding_output: Option<(u64, i32)>,
    next_output_sequence: u64,
    input_finished: bool,
    secure_decoder: bool,
}

impl AndroidProtectedVideoDecoder {
    /// Returns the protected track configuration.
    #[must_use]
    pub const fn track_info(&self) -> &TrackInfo {
        &self.track
    }

    /// Returns whether the CDM required Android's secure decoder capability.
    #[must_use]
    pub const fn uses_secure_decoder(&self) -> bool {
        self.secure_decoder
    }

    /// Queries the remaining streaming-key lifetime.
    ///
    /// # Errors
    ///
    /// Returns an error when the DRM provider rejects the query or returns an
    /// invalid duration value for a standard lifetime key.
    pub fn key_status(&self) -> Result<AndroidKeyStatus, Error> {
        self.resources.key_status()
    }

    /// Returns the persistent key-set identity backing this decoder, if restored.
    #[must_use]
    pub const fn offline_key_set(&self) -> Option<&AndroidOfflineKeySet> {
        self.resources.offline_key_set.as_ref()
    }

    /// Creates a renewal challenge when a finite key lifetime reaches `threshold`.
    ///
    /// Providers that report unlimited or no duration do not generate a
    /// speculative renewal request.
    ///
    /// # Errors
    ///
    /// Returns an error when key status or renewal request generation fails.
    pub fn renewal_challenge_if_needed(
        &self,
        threshold: Duration,
    ) -> Result<Option<AndroidLicenseChallenge>, Error> {
        self.resources
            .renewal_challenge_if_needed(&self.init_data, self.container_mime, threshold)
    }

    /// Applies a renewal response to the active DRM session.
    ///
    /// # Errors
    ///
    /// Returns an error when the response is empty or rejected by the provider.
    pub fn provide_key_response(
        &mut self,
        response: &LicenseResponse,
    ) -> Result<Option<AndroidOfflineKeySet>, Error> {
        self.resources.provide_key_response(response)
    }

    /// Queues one CENC-protected sample through `queueSecureInputBuffer`.
    ///
    /// This method may wait on the dedicated decoder thread until `MediaCodec`
    /// returns a concrete input buffer; it never blocks the UI/render thread.
    ///
    /// # Errors
    ///
    /// Returns an error for a wrong/clear sample, invalid CENC ranges, timestamp
    /// overflow, or Android codec/crypto failure.
    pub fn queue(&mut self, sample: &EncodedSample) -> Result<(), Error> {
        if self.input_finished {
            return Err(Error::Codec(String::from(
                "cannot queue protected samples after end of stream",
            )));
        }
        if sample.track_id() != self.track.id() {
            return Err(Error::Codec(format!(
                "protected decoder for track {} received track {}",
                self.track.id().get(),
                sample.track_id().get()
            )));
        }
        let encryption = sample.encryption().ok_or_else(|| {
            Error::Codec(format!(
                "protected track {} received a sample without CENC metadata",
                self.track.id().get()
            ))
        })?;
        let converted = self
            .converter
            .convert_protected_sample(sample.data(), encryption.subsamples())
            .map_err(|error| Error::Codec(error.to_string()))?;
        let (data, subsamples) = converted.into_parts();
        queue_secure_sample(&self.resources, &self.track, sample, &data, &subsamples)
    }

    /// Signals end-of-stream to the secure decoder.
    ///
    /// # Errors
    ///
    /// Returns an error when Android cannot acquire or queue an EOS input.
    pub fn finish_input(&mut self) -> Result<(), Error> {
        if self.input_finished {
            return Ok(());
        }
        queue_end_of_stream(&self.resources)?;
        self.input_finished = true;
        Ok(())
    }

    /// Dequeues one secure output without releasing it to the Surface.
    ///
    /// At most one output may be outstanding. Release or discard it before
    /// calling this method again.
    ///
    /// # Errors
    ///
    /// Returns an error for an unreleased output, invalid timestamp, or Android
    /// codec failure.
    pub fn try_dequeue_output(&mut self) -> Result<Option<AndroidProtectedVideoOutput>, Error> {
        self.dequeue_output_with_timeout(0)
    }

    /// Waits for one secure output without releasing it to the Surface.
    ///
    /// This is intended for the dedicated decoder thread after end of input,
    /// when Android must eventually emit its terminal output.
    ///
    /// # Errors
    ///
    /// Returns an error for an unreleased output, invalid timestamp, or Android
    /// codec failure.
    pub fn wait_dequeue_output(&mut self) -> Result<AndroidProtectedVideoOutput, Error> {
        self.dequeue_output_with_timeout(INPUT_WAIT_FOREVER_MICROS)?
            .ok_or_else(|| {
                Error::Platform(String::from(
                    "MediaCodec returned no protected output while waiting indefinitely",
                ))
            })
    }

    fn dequeue_output_with_timeout(
        &mut self,
        timeout_micros: i64,
    ) -> Result<Option<AndroidProtectedVideoOutput>, Error> {
        if self.outstanding_output.is_some() {
            return Err(Error::Codec(String::from(
                "release the outstanding protected video output before dequeuing another",
            )));
        }
        let result = self.with_env(|env, codec| {
            let buffer_info = env.new_object(jni_str!("android/media/MediaCodec$BufferInfo"), jni_sig!("()V"), &[])
                .map_err(|error| platform_jni_error(env, "create BufferInfo", error))?;
            loop {
                let index = env.call_method(codec, jni_str!("dequeueOutputBuffer"), jni_sig!("(Landroid/media/MediaCodec$BufferInfo;J)I"), &[
                    JValue::Object(&buffer_info),
                    JValue::Long(timeout_micros),
                ])
                    .and_then(jni::objects::JValueOwned::i)
                    .map_err(|error| {
                        platform_jni_error(env, "dequeue protected output", error)
                    })?;
                match index {
                    MEDIA_CODEC_INFO_TRY_AGAIN_LATER => return Ok(None),
                    MEDIA_CODEC_INFO_OUTPUT_FORMAT_CHANGED
                    | MEDIA_CODEC_INFO_OUTPUT_BUFFERS_CHANGED => {}
                    index if index >= 0 => {
                        let presentation_time_micros = env.get_field(&buffer_info, jni_str!("presentationTimeUs"), jni_sig!("J"))
                            .and_then(jni::objects::JValueOwned::j)
                            .map_err(|error| {
                                platform_jni_error(env, "read protected output PTS", error)
                            })?;
                        let flags = env.get_field(&buffer_info, jni_str!("flags"), jni_sig!("I"))
                            .and_then(jni::objects::JValueOwned::i)
                            .map_err(|error| {
                                platform_jni_error(env, "read protected output flags", error)
                            })?;
                        let presentation_time_micros =
                            u64::try_from(presentation_time_micros).map_err(|_| {
                                Error::Codec(format!(
                                    "MediaCodec returned negative protected output PTS {presentation_time_micros}"
                                ))
                            })?;
                        return Ok(Some((
                            index,
                            Duration::from_micros(presentation_time_micros),
                            flags & MEDIA_CODEC_BUFFER_FLAG_END_OF_STREAM != 0,
                        )));
                    }
                    other => {
                        return Err(Error::Platform(format!(
                            "MediaCodec returned unknown output status {other}"
                        )));
                    }
                }
            }
        })?;
        let Some((index, presentation_time, end_of_stream)) = result else {
            return Ok(None);
        };
        let output = AndroidProtectedVideoOutput {
            index,
            sequence: self.next_output_sequence,
            presentation_time,
            end_of_stream,
        };
        self.next_output_sequence = self.next_output_sequence.saturating_add(1);
        self.outstanding_output = Some((output.sequence, output.index));
        Ok(Some(output))
    }

    /// Releases one output to the secure Surface after a monotonic delay.
    ///
    /// A zero delay presents as soon as possible. The platform schedules the
    /// buffer against `System.nanoTime`, not media PTS.
    ///
    /// # Errors
    ///
    /// Returns an error for a stale/foreign output, delay overflow, or Android
    /// presentation failure.
    pub fn render_output_after(
        &mut self,
        output: AndroidProtectedVideoOutput,
        delay: Duration,
    ) -> Result<(), Error> {
        self.validate_output(output)?;
        let delay_nanos = i64::try_from(delay.as_nanos()).map_err(|_| {
            Error::Platform(String::from(
                "protected output presentation delay exceeds Android jlong",
            ))
        })?;
        self.with_env(|env, codec| {
            let now = env
                .call_static_method(
                    jni_str!("java/lang/System"),
                    jni_str!("nanoTime"),
                    jni_sig!("()J"),
                    &[],
                )
                .and_then(jni::objects::JValueOwned::j)
                .map_err(|error| platform_jni_error(env, "read Android monotonic time", error))?;
            let render_at = now.checked_add(delay_nanos).ok_or_else(|| {
                Error::Platform(String::from(
                    "protected output render timestamp overflowed jlong",
                ))
            })?;
            env.call_method(
                codec,
                jni_str!("releaseOutputBuffer"),
                jni_sig!("(IJ)V"),
                &[JValue::Int(output.index), JValue::Long(render_at)],
            )
            .map_err(|error| platform_jni_error(env, "render protected output", error))?;
            Ok(())
        })?;
        self.outstanding_output = None;
        Ok(())
    }

    /// Discards one output without exposing or rendering its pixels.
    ///
    /// # Errors
    ///
    /// Returns an error for a stale/foreign output or Android codec failure.
    pub fn discard_output(&mut self, output: AndroidProtectedVideoOutput) -> Result<(), Error> {
        self.validate_output(output)?;
        self.with_env(|env, codec| {
            env.call_method(
                codec,
                jni_str!("releaseOutputBuffer"),
                jni_sig!("(IZ)V"),
                &[JValue::Int(output.index), JValue::Bool(false)],
            )
            .map_err(|error| platform_jni_error(env, "discard protected output", error))?;
            Ok(())
        })?;
        self.outstanding_output = None;
        Ok(())
    }

    /// Flushes decoder state after a seek or discontinuity.
    ///
    /// # Errors
    ///
    /// Returns an error when Android rejects the flush.
    pub fn flush(&mut self) -> Result<(), Error> {
        flush_codec(&self.resources)?;
        self.outstanding_output = None;
        self.input_finished = false;
        Ok(())
    }

    fn validate_output(&self, output: AndroidProtectedVideoOutput) -> Result<(), Error> {
        if self.outstanding_output != Some((output.sequence, output.index)) {
            return Err(Error::Codec(String::from(
                "protected video output is stale or belongs to another decoder state",
            )));
        }
        Ok(())
    }

    fn with_env<T>(
        &self,
        operation: impl FnOnce(&mut Env<'_>, &JObject<'_>) -> Result<T, Error>,
    ) -> Result<T, Error> {
        self.resources.with_env(operation)
    }
}

impl std::fmt::Debug for AndroidProtectedVideoDecoder {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AndroidProtectedVideoDecoder")
            .field("track", &self.track)
            .field("secure_decoder", &self.secure_decoder)
            .field("input_finished", &self.input_finished)
            .field("outstanding_output", &self.outstanding_output)
            .finish_non_exhaustive()
    }
}

impl Drop for AndroidProtectedVideoDecoder {
    fn drop(&mut self) {
        if let Some((_, index)) = self.outstanding_output.take() {
            self.resources.discard_output_on_drop(index);
        }
    }
}

/// Licensed Android decoder that returns protected AAC as owned interleaved PCM.
pub struct AndroidProtectedAudioDecoder {
    resources: AndroidCodecResources,
    track: TrackInfo,
    init_data: ProtectionInitData,
    container_mime: &'static str,
    channels: std::num::NonZeroU16,
    sample_rate: std::num::NonZeroU32,
    pcm_encoding: i32,
    input_finished: bool,
    secure_decoder: bool,
}

impl AndroidProtectedAudioDecoder {
    /// Returns the protected track configuration.
    #[must_use]
    pub const fn track_info(&self) -> &TrackInfo {
        &self.track
    }

    /// Returns whether the CDM required a secure audio decoder component.
    #[must_use]
    pub const fn uses_secure_decoder(&self) -> bool {
        self.secure_decoder
    }

    /// Queries the remaining streaming-key lifetime.
    ///
    /// # Errors
    ///
    /// Returns an error when the DRM provider rejects the query or returns an
    /// invalid standard lifetime value.
    pub fn key_status(&self) -> Result<AndroidKeyStatus, Error> {
        self.resources.key_status()
    }

    /// Returns the persistent key-set identity backing this decoder, if restored.
    #[must_use]
    pub const fn offline_key_set(&self) -> Option<&AndroidOfflineKeySet> {
        self.resources.offline_key_set.as_ref()
    }

    /// Creates a renewal challenge when a finite key lifetime reaches `threshold`.
    ///
    /// # Errors
    ///
    /// Returns an error when status or request generation fails.
    pub fn renewal_challenge_if_needed(
        &self,
        threshold: Duration,
    ) -> Result<Option<AndroidLicenseChallenge>, Error> {
        self.resources
            .renewal_challenge_if_needed(&self.init_data, self.container_mime, threshold)
    }

    /// Applies a renewal response to the active DRM session.
    ///
    /// # Errors
    ///
    /// Returns an error when Android rejects the response.
    pub fn provide_key_response(
        &mut self,
        response: &LicenseResponse,
    ) -> Result<Option<AndroidOfflineKeySet>, Error> {
        self.resources.provide_key_response(response)
    }

    /// Queues one CENC-protected AAC access unit and returns available PCM.
    ///
    /// # Errors
    ///
    /// Returns an error for a wrong/clear sample, invalid CENC metadata, an
    /// unexpected PCM format change, or Android codec failure.
    pub fn decode(&mut self, sample: &EncodedSample) -> Result<Vec<DecodedAudioFrame>, Error> {
        if self.input_finished {
            return Err(Error::Codec(String::from(
                "cannot queue protected audio after end of stream",
            )));
        }
        queue_secure_sample(
            &self.resources,
            &self.track,
            sample,
            sample.data(),
            sample
                .encryption()
                .ok_or_else(|| {
                    Error::Codec(format!(
                        "protected audio track {} received a sample without CENC metadata",
                        self.track.id().get()
                    ))
                })?
                .subsamples(),
        )?;
        self.collect_output(0, false)
    }

    /// Signals end of input and drains every delayed PCM frame.
    ///
    /// # Errors
    ///
    /// Returns an error when Android rejects EOS or does not emit its terminal output.
    pub fn finish(&mut self) -> Result<Vec<DecodedAudioFrame>, Error> {
        if !self.input_finished {
            queue_end_of_stream(&self.resources)?;
            self.input_finished = true;
        }
        self.collect_output(INPUT_WAIT_FOREVER_MICROS, true)
    }

    /// Flushes protected audio state after a seek or discontinuity.
    ///
    /// # Errors
    ///
    /// Returns an error when Android rejects the flush.
    pub fn flush(&mut self) -> Result<(), Error> {
        flush_codec(&self.resources)?;
        self.input_finished = false;
        Ok(())
    }

    fn collect_output(
        &mut self,
        timeout_micros: i64,
        require_end_of_stream: bool,
    ) -> Result<Vec<DecodedAudioFrame>, Error> {
        let mut frames = Vec::new();
        loop {
            match dequeue_audio_output(&self.resources, timeout_micros)? {
                AndroidAudioOutput::TryAgainLater if require_end_of_stream => {
                    return Err(Error::Platform(String::from(
                        "MediaCodec returned no protected audio output while draining EOS",
                    )));
                }
                AndroidAudioOutput::TryAgainLater => return Ok(frames),
                AndroidAudioOutput::Format {
                    channels,
                    sample_rate,
                    pcm_encoding,
                } => {
                    if channels != self.channels || sample_rate != self.sample_rate {
                        return Err(Error::Codec(format!(
                            "protected audio format changed from {}ch/{}Hz to {}ch/{}Hz without reconfiguration",
                            self.channels, self.sample_rate, channels, sample_rate
                        )));
                    }
                    self.pcm_encoding = pcm_encoding;
                }
                AndroidAudioOutput::Buffer {
                    presentation_time,
                    bytes,
                    end_of_stream,
                } => {
                    if !bytes.is_empty() {
                        frames.push(
                            DecodedAudioFrame::from_interleaved_pcm(
                                presentation_time,
                                self.channels,
                                self.sample_rate,
                                decode_android_pcm(&bytes, self.pcm_encoding)?,
                            )
                            .map_err(|error| Error::Codec(error.to_string()))?,
                        );
                    }
                    if end_of_stream {
                        return Ok(frames);
                    }
                }
            }
        }
    }
}

impl std::fmt::Debug for AndroidProtectedAudioDecoder {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AndroidProtectedAudioDecoder")
            .field("track", &self.track)
            .field("channels", &self.channels)
            .field("sample_rate", &self.sample_rate)
            .field("secure_decoder", &self.secure_decoder)
            .field("input_finished", &self.input_finished)
            .finish_non_exhaustive()
    }
}

enum AndroidAudioOutput {
    TryAgainLater,
    Format {
        channels: std::num::NonZeroU16,
        sample_rate: std::num::NonZeroU32,
        pcm_encoding: i32,
    },
    Buffer {
        presentation_time: Duration,
        bytes: Vec<u8>,
        end_of_stream: bool,
    },
}

struct AndroidDrmSession {
    vm: Arc<JavaVM>,
    drm: GlobalObjectRef,
    system_id: [u8; 16],
    session_id: Option<GlobalObjectRef>,
}

impl AndroidDrmSession {
    fn new(vm: Arc<JavaVM>, system_id: [u8; 16]) -> Result<(Self, GlobalObjectRef), Error> {
        let (drm, uuid) = with_attached_env(&vm, |env| {
            let uuid = create_java_uuid(env, system_id)?;
            let drm = env
                .new_object(
                    jni_str!("android/media/MediaDrm"),
                    jni_sig!("(Ljava/util/UUID;)V"),
                    &[JValue::Object(&uuid)],
                )
                .map_err(|error| platform_jni_error(env, "create MediaDrm", error))?;
            let drm = env
                .new_global_ref(drm)
                .map_err(|error| platform_jni_error(env, "retain MediaDrm", error))?;
            let uuid = env.new_global_ref(uuid).map_err(|error| {
                release_java_object(
                    env,
                    drm.as_obj(),
                    jni_str!("release"),
                    "release",
                    "MediaDrm",
                );
                platform_jni_error(env, "retain DRM UUID", error)
            })?;
            Ok((drm, uuid))
        })?;
        Ok((
            Self {
                vm,
                drm,
                system_id,
                session_id: None,
            },
            uuid,
        ))
    }

    fn open(&mut self) -> Result<bool, Error> {
        if self.session_id.is_some() {
            return Ok(false);
        }
        let session = with_attached_env(&self.vm, |env| {
            let session = match env.call_method(
                self.drm.as_obj(),
                jni_str!("openSession"),
                jni_sig!("()[B"),
                &[],
            ) {
                Ok(value) => value
                    .l()
                    .map_err(|error| platform_jni_error(env, "read MediaDrm session", error))?,
                Err(error) => {
                    if take_not_provisioned(env, error, "open MediaDrm session")? {
                        return Ok(None);
                    }
                    unreachable!("non-provisioning Java errors return from take_not_provisioned")
                }
            };
            env.new_global_ref(session)
                .map(Some)
                .map_err(|error| platform_jni_error(env, "retain MediaDrm session", error))
        })?;
        self.session_id = session;
        Ok(self.session_id.is_none())
    }

    fn close(&mut self) {
        let vm = Arc::clone(&self.vm);
        if let Err(error) = with_attached_env(&vm, |env| {
            close_session(env, &self.drm, &mut self.session_id);
            Ok(())
        }) {
            tracing::error!(%error, "failed to attach JVM while closing Android MediaDrm session");
        }
    }

    const fn session_id(&self) -> &GlobalObjectRef {
        self.session_id
            .as_ref()
            .expect("prepared MediaDrm operation must retain an open session")
    }

    fn prepare_key_request(
        &self,
        init_data: &ProtectionInitData,
        container_mime: &str,
        key_type: i32,
    ) -> Result<LicensePreparation, Error> {
        with_attached_env(&self.vm, |env| {
            prepare_key_request(
                env,
                self.drm.as_obj(),
                self.session_id().as_obj(),
                init_data,
                container_mime,
                key_type,
            )
        })
    }

    fn prepare_release_request(
        &self,
        key_set: &AndroidOfflineKeySet,
    ) -> Result<LicensePreparation, Error> {
        with_attached_env(&self.vm, |env| {
            let scope = env
                .byte_array_from_slice(key_set.as_bytes())
                .map_err(|error| platform_jni_error(env, "create release key-set scope", error))?;
            let request = match env.call_method(self.drm.as_obj(), jni_str!("getKeyRequest"), jni_sig!("([B[BLjava/lang/String;ILjava/util/HashMap;)Landroid/media/MediaDrm$KeyRequest;"), &[
                JValue::Object(&scope),
                JValue::Object(&JObject::null()),
                JValue::Object(&JObject::null()),
                JValue::Int(MEDIA_DRM_KEY_TYPE_RELEASE),
                JValue::Object(&JObject::null()),
            ]) {
                Ok(value) => value.l().map_err(|error| {
                    platform_jni_error(env, "read MediaDrm release request", error)
                })?,
                Err(error) => {
                    if take_not_provisioned(env, error, "create MediaDrm release request")? {
                        return Ok(LicensePreparation::ProvisionRequired);
                    }
                    unreachable!("non-provisioning Java errors return from take_not_provisioned")
                }
            };
            read_license_challenge(env, &request, self.system_id)
        })
    }

    fn restore_keys(&self, key_set: &AndroidOfflineKeySet) -> Result<bool, Error> {
        with_attached_env(&self.vm, |env| {
            restore_keys(env, self.drm.as_obj(), self.session_id().as_obj(), key_set)
        })
    }

    fn prepare_provision_request(&self) -> Result<AndroidProvisionRequest, Error> {
        with_attached_env(&self.vm, |env| {
            let request = env
                .call_method(
                    self.drm.as_obj(),
                    jni_str!("getProvisionRequest"),
                    jni_sig!("()Landroid/media/MediaDrm$ProvisionRequest;"),
                    &[],
                )
                .and_then(jni::objects::JValueOwned::l)
                .map_err(|error| {
                    platform_jni_error(env, "create MediaDrm provision request", error)
                })?;
            Ok(AndroidProvisionRequest {
                system_id: self.system_id,
                data: byte_array_method(env, &request, jni_str!("getData"), "provision request")?,
                default_url: url_method(env, &request, "provision request")?,
            })
        })
    }

    fn provide_provision_response(&self, response: &[u8]) -> Result<(), Error> {
        if response.is_empty() {
            return Err(Error::Streaming(String::from(
                "Android MediaDrm provisioning response must not be empty",
            )));
        }
        with_attached_env(&self.vm, |env| {
            let response = env.byte_array_from_slice(response).map_err(|error| {
                platform_jni_error(env, "create MediaDrm provision response", error)
            })?;
            env.call_method(
                self.drm.as_obj(),
                jni_str!("provideProvisionResponse"),
                jni_sig!("([B)V"),
                &[JValue::Object(&response)],
            )
            .map_err(|error| {
                platform_jni_error(env, "provide MediaDrm provision response", error)
            })?;
            Ok(())
        })
    }

    fn provide_key_response(&self, response: &[u8]) -> Result<Option<AndroidOfflineKeySet>, Error> {
        provide_key_response(&self.vm, &self.drm, self.session_id(), response)
    }

    fn provide_release_response(
        &self,
        key_set: &AndroidOfflineKeySet,
        response: &[u8],
    ) -> Result<(), Error> {
        with_attached_env(&self.vm, |env| {
            let scope = env
                .byte_array_from_slice(key_set.as_bytes())
                .map_err(|error| platform_jni_error(env, "create release key-set scope", error))?;
            if provide_key_response_in_env(env, self.drm.as_obj(), &scope, response)?.is_some() {
                return Err(Error::Platform(String::from(
                    "Android offline release unexpectedly returned a key-set identity",
                )));
            }
            Ok(())
        })
    }

    fn key_status(&self) -> Result<AndroidKeyStatus, Error> {
        with_attached_env(&self.vm, |env| {
            query_key_status(env, self.drm.as_obj(), self.session_id().as_obj())
        })
    }
}

impl Drop for AndroidDrmSession {
    fn drop(&mut self) {
        let vm = Arc::clone(&self.vm);
        if let Err(error) = with_attached_env(&vm, |env| {
            close_session(env, &self.drm, &mut self.session_id);
            release_java_object(
                env,
                self.drm.as_obj(),
                jni_str!("release"),
                "release",
                "MediaDrm",
            );
            Ok(())
        }) {
            tracing::error!(%error, "failed to attach JVM while releasing Android MediaDrm session");
        }
    }
}

trait OfflineLicenseTarget: Sized {
    type Output;

    fn system_id(&self) -> [u8; 16];

    fn prepare(&self, drm_session: &mut AndroidDrmSession) -> Result<LicensePreparation, Error>;

    fn finish(
        self,
        drm_session: &AndroidDrmSession,
        response: &LicenseResponse,
    ) -> Result<Self::Output, Error>;
}

impl OfflineLicenseTarget for AndroidOfflineLicenseAcquisition {
    type Output = AndroidOfflineKeySet;

    fn system_id(&self) -> [u8; 16] {
        *self.init_data.system_id()
    }

    fn prepare(&self, drm_session: &mut AndroidDrmSession) -> Result<LicensePreparation, Error> {
        if drm_session.open()? {
            return Ok(LicensePreparation::ProvisionRequired);
        }
        drm_session.prepare_key_request(
            &self.init_data,
            self.container_mime,
            MEDIA_DRM_KEY_TYPE_OFFLINE,
        )
    }

    fn finish(
        self,
        drm_session: &AndroidDrmSession,
        response: &LicenseResponse,
    ) -> Result<Self::Output, Error> {
        drm_session
            .provide_key_response(response.bytes())?
            .ok_or_else(|| {
                Error::Platform(String::from(
                    "Android offline acquisition returned no persistent key-set identity",
                ))
            })
    }
}

impl OfflineLicenseTarget for AndroidOfflineLicenseRenewal {
    type Output = AndroidOfflineKeySet;

    fn system_id(&self) -> [u8; 16] {
        *self.init_data.system_id()
    }

    fn prepare(&self, drm_session: &mut AndroidDrmSession) -> Result<LicensePreparation, Error> {
        if drm_session.open()? {
            return Ok(LicensePreparation::ProvisionRequired);
        }
        if drm_session.restore_keys(&self.key_set)? {
            drm_session.close();
            return Ok(LicensePreparation::ProvisionRequired);
        }
        drm_session.prepare_key_request(
            &self.init_data,
            self.container_mime,
            MEDIA_DRM_KEY_TYPE_OFFLINE,
        )
    }

    fn finish(
        self,
        drm_session: &AndroidDrmSession,
        response: &LicenseResponse,
    ) -> Result<Self::Output, Error> {
        drm_session
            .provide_key_response(response.bytes())?
            .ok_or_else(|| {
                Error::Platform(String::from(
                    "Android offline renewal returned no replacement key-set identity",
                ))
            })
    }
}

impl OfflineLicenseTarget for AndroidOfflineLicenseRelease {
    type Output = ();

    fn system_id(&self) -> [u8; 16] {
        self.system_id
    }

    fn prepare(&self, drm_session: &mut AndroidDrmSession) -> Result<LicensePreparation, Error> {
        drm_session.prepare_release_request(&self.key_set)
    }

    fn finish(
        self,
        drm_session: &AndroidDrmSession,
        response: &LicenseResponse,
    ) -> Result<Self::Output, Error> {
        drm_session.provide_release_response(&self.key_set, response.bytes())
    }
}

fn begin_offline_license<T: OfflineLicenseTarget>(
    context: &AndroidDrmContext,
    target: T,
) -> Result<AndroidOfflineLicenseBootstrap<T>, Error> {
    let system_id = target.system_id();
    if !context.supports_system_id(&system_id)? {
        return Err(Error::Unsupported(format!(
            "Android does not expose a CDM for DRM system {system_id:02x?}"
        )));
    }
    let (drm_session, uuid) = AndroidDrmSession::new(Arc::clone(&context.vm), system_id)?;
    drop(uuid);
    advance_offline_license(
        AndroidOfflineLicenseRequest {
            drm_session,
            target,
        },
        false,
    )
}

fn complete_offline_provisioning<T: OfflineLicenseTarget>(
    state: AndroidOfflineLicenseProvisioning<T>,
    response: &LicenseResponse,
) -> Result<AndroidOfflineLicenseBootstrap<T>, Error> {
    state
        .operation
        .drm_session
        .provide_provision_response(response.bytes())?;
    advance_offline_license(state.operation, true)
}

fn advance_offline_license<T: OfflineLicenseTarget>(
    mut operation: AndroidOfflineLicenseRequest<T>,
    provisioned: bool,
) -> Result<AndroidOfflineLicenseBootstrap<T>, Error> {
    match operation.target.prepare(&mut operation.drm_session)? {
        LicensePreparation::Ready(challenge) => Ok(AndroidOfflineLicenseBootstrap::License(
            AndroidPendingOfflineLicense {
                challenge,
                operation,
            },
        )),
        LicensePreparation::ProvisionRequired if provisioned => Err(Error::Platform(String::from(
            "Android MediaDrm still requires provisioning after accepting its response",
        ))),
        LicensePreparation::ProvisionRequired => {
            operation.drm_session.close();
            let request = operation.drm_session.prepare_provision_request()?;
            Ok(AndroidOfflineLicenseBootstrap::Provisioning(
                AndroidOfflineLicenseProvisioning { request, operation },
            ))
        }
    }
}

fn finish_offline_license<T: OfflineLicenseTarget>(
    state: AndroidPendingOfflineLicense<T>,
    response: &LicenseResponse,
) -> Result<T::Output, Error> {
    let AndroidOfflineLicenseRequest {
        drm_session,
        target,
    } = state.operation;
    target.finish(&drm_session, response)
}

fn offline_license_container_mime(
    track: &TrackInfo,
    init_data: &ProtectionInitData,
) -> Result<&'static str, Error> {
    let protection = track.protection().ok_or_else(|| {
        Error::Codec(format!(
            "clear track {} cannot acquire an offline DRM license",
            track.id().get()
        ))
    })?;
    if !init_data.key_ids().is_empty() && !init_data.key_ids().contains(protection.default_key_id())
    {
        return Err(Error::Container(format!(
            "DRM initialization data does not cover protected track {} key {:02x?}",
            track.id().get(),
            protection.default_key_id()
        )));
    }
    match track.kind() {
        TrackKind::Video => Ok("video/mp4"),
        TrackKind::Audio => Ok("audio/mp4"),
        TrackKind::Subtitle | TrackKind::Metadata => Err(Error::Unsupported(format!(
            "Android offline DRM does not support {:?} track {}",
            track.kind(),
            track.id().get()
        ))),
    }
}

struct AndroidDecoderBootstrap {
    surface: Option<Arc<GlobalObjectRef>>,
    drm_session: AndroidDrmSession,
    uuid: GlobalObjectRef,
    init_data: ProtectionInitData,
    track: TrackInfo,
    converter: Option<NalStreamConverter>,
    decoder_mime: &'static str,
    container_mime: &'static str,
    primary_csd: Vec<u8>,
    secondary_csd: Vec<u8>,
    offline_key_set: Option<AndroidOfflineKeySet>,
}

struct AndroidDecoderDescription {
    surface: Option<Arc<GlobalObjectRef>>,
    converter: Option<NalStreamConverter>,
    decoder_mime: &'static str,
    container_mime: &'static str,
    primary_csd: Vec<u8>,
    secondary_csd: Vec<u8>,
}

impl AndroidDecoderBootstrap {
    fn new_video(
        surface: AndroidProtectedSurface,
        track: TrackInfo,
        init_data: ProtectionInitData,
    ) -> Result<Self, Error> {
        if track.kind() != TrackKind::Video {
            return Err(Error::Codec(format!(
                "track {} is not video and cannot use a protected video surface",
                track.id().get()
            )));
        }
        if track.protection().is_none() {
            return Err(Error::Codec(format!(
                "track {} is clear and must use the ordinary frame decoder",
                track.id().get()
            )));
        }
        let (decoder_mime, is_hevc) = match track.codec() {
            Codec::H264 => ("video/avc", false),
            Codec::H265 => ("video/hevc", true),
            codec => {
                return Err(Error::Unsupported(format!(
                    "Android protected surface decoder does not yet support {codec:?}"
                )));
            }
        };
        let converter = NalStreamConverter::new(is_hevc, Some(track.decoder_configuration()))
            .map_err(|error| Error::Codec(error.to_string()))?;
        let (primary_csd, secondary_csd) = converter.codec_specific_data();
        let primary_csd = primary_csd.map_or_else(Vec::new, <[u8]>::to_vec);
        let secondary_csd = secondary_csd.map_or_else(Vec::new, <[u8]>::to_vec);
        Self::new_configured(
            Arc::clone(&surface.context.vm),
            track,
            init_data,
            AndroidDecoderDescription {
                surface: Some(surface.surface.surface),
                converter: Some(converter),
                decoder_mime,
                container_mime: "video/mp4",
                primary_csd,
                secondary_csd,
            },
        )
    }

    fn new_audio(
        context: &AndroidDrmContext,
        track: TrackInfo,
        init_data: ProtectionInitData,
    ) -> Result<Self, Error> {
        if track.kind() != TrackKind::Audio {
            return Err(Error::Codec(format!(
                "track {} is not audio and cannot use a protected audio decoder",
                track.id().get()
            )));
        }
        if track.protection().is_none() {
            return Err(Error::Codec(format!(
                "track {} is clear and must use the ordinary audio decoder",
                track.id().get()
            )));
        }
        if track.codec() != Codec::Aac {
            return Err(Error::Unsupported(format!(
                "Android protected audio decoder does not yet support {:?}",
                track.codec()
            )));
        }
        if track.audio_layout().is_none() {
            return Err(Error::Container(format!(
                "protected audio track {} has no channel or sample-rate layout",
                track.id().get()
            )));
        }
        if track.decoder_configuration().is_empty() {
            return Err(Error::Container(format!(
                "protected AAC track {} has no AudioSpecificConfig",
                track.id().get()
            )));
        }
        let primary_csd = track.decoder_configuration().to_vec();
        Self::new_configured(
            Arc::clone(&context.vm),
            track,
            init_data,
            AndroidDecoderDescription {
                surface: None,
                converter: None,
                decoder_mime: "audio/mp4a-latm",
                container_mime: "audio/mp4",
                primary_csd,
                secondary_csd: Vec::new(),
            },
        )
    }

    fn new_configured(
        vm: Arc<JavaVM>,
        track: TrackInfo,
        init_data: ProtectionInitData,
        description: AndroidDecoderDescription,
    ) -> Result<Self, Error> {
        let (drm_session, uuid) = AndroidDrmSession::new(vm, *init_data.system_id())?;
        let AndroidDecoderDescription {
            surface,
            converter,
            decoder_mime,
            container_mime,
            primary_csd,
            secondary_csd,
        } = description;
        Ok(Self {
            surface,
            drm_session,
            uuid,
            init_data,
            track,
            converter,
            decoder_mime,
            container_mime,
            primary_csd,
            secondary_csd,
            offline_key_set: None,
        })
    }

    fn with_offline_key_set(mut self, key_set: AndroidOfflineKeySet) -> Self {
        self.offline_key_set = Some(key_set);
        self
    }

    fn prepare_keys(&mut self) -> Result<KeyPreparation, Error> {
        if self.drm_session.open()? {
            return Ok(KeyPreparation::ProvisionRequired);
        }
        if let Some(key_set) = &self.offline_key_set {
            if self.drm_session.restore_keys(key_set)? {
                self.drm_session.close();
                return Ok(KeyPreparation::ProvisionRequired);
            }
            return Ok(KeyPreparation::Restored);
        }
        let preparation = match self.drm_session.prepare_key_request(
            &self.init_data,
            self.container_mime,
            MEDIA_DRM_KEY_TYPE_STREAMING,
        )? {
            LicensePreparation::Ready(challenge) => KeyPreparation::Ready(challenge),
            LicensePreparation::ProvisionRequired => KeyPreparation::ProvisionRequired,
        };
        if matches!(preparation, KeyPreparation::ProvisionRequired) {
            self.drm_session.close();
        }
        Ok(preparation)
    }

    fn prepare_provision_request(&self) -> Result<AndroidProvisionRequest, Error> {
        self.drm_session.prepare_provision_request()
    }

    fn provide_provision_response(&self, response: &[u8]) -> Result<(), Error> {
        self.drm_session.provide_provision_response(response)
    }

    fn provide_key_response(&self, response: &[u8]) -> Result<(), Error> {
        let key_set = self.drm_session.provide_key_response(response)?;
        if key_set.is_some() {
            return Err(Error::Platform(String::from(
                "Android returned persistent keys for a streaming license request",
            )));
        }
        Ok(())
    }

    fn configure_video_decoder(mut self) -> Result<AndroidProtectedVideoDecoder, Error> {
        let dimensions = self.track.video_dimensions().ok_or_else(|| {
            Error::Container(format!(
                "protected video track {} has no coded dimensions",
                self.track.id().get()
            ))
        })?;
        let width = i32::try_from(dimensions.width.get()).map_err(|_| {
            Error::Codec(String::from("protected video width exceeds Android jint"))
        })?;
        let height = i32::try_from(dimensions.height.get()).map_err(|_| {
            Error::Codec(String::from("protected video height exceeds Android jint"))
        })?;
        let (codec, media_crypto, secure_decoder) =
            with_attached_env(&self.drm_session.vm, |env| {
                let config = SecureDecoderConfig {
                    uuid: self.uuid.as_obj(),
                    session: self.drm_session.session_id().as_obj(),
                    surface: self
                        .surface
                        .as_ref()
                        .expect("video decoder bootstrap must retain its secure Surface")
                        .as_obj(),
                    mime: self.decoder_mime,
                    width,
                    height,
                    primary_csd: &self.primary_csd,
                    secondary_csd: &self.secondary_csd,
                };
                let media_crypto = create_media_crypto(env, &config)?;
                let decoder = create_secure_media_codec(env, &config, &media_crypto);
                let (codec, secure_decoder) = match decoder {
                    Ok(decoder) => decoder,
                    Err(error) => {
                        release_java_object(
                            env,
                            &media_crypto,
                            jni_str!("release"),
                            "release",
                            "MediaCrypto",
                        );
                        return Err(error);
                    }
                };
                let (codec, media_crypto) = retain_decoder_objects(env, &codec, &media_crypto)?;
                Ok((codec, media_crypto, secure_decoder))
            })?;
        Ok(AndroidProtectedVideoDecoder {
            resources: AndroidCodecResources {
                drm_session: self.drm_session,
                media_crypto,
                codec,
                offline_key_set: self.offline_key_set.clone(),
            },
            _surface: self
                .surface
                .take()
                .expect("video decoder bootstrap must retain its secure Surface"),
            converter: self.converter.take().ok_or_else(|| {
                Error::Codec(String::from(
                    "configured protected decoder lost its NAL converter",
                ))
            })?,
            track: self.track.clone(),
            init_data: self.init_data.clone(),
            container_mime: self.container_mime,
            outstanding_output: None,
            next_output_sequence: 0,
            input_finished: false,
            secure_decoder,
        })
    }

    fn configure_audio_decoder(self) -> Result<AndroidProtectedAudioDecoder, Error> {
        let layout = self.track.audio_layout().ok_or_else(|| {
            Error::Container(format!(
                "protected audio track {} has no channel or sample-rate layout",
                self.track.id().get()
            ))
        })?;
        let channels = u16::try_from(layout.channels.get()).map_err(|_| {
            Error::Unsupported(format!(
                "protected audio track {} declares {} channels, exceeding u16",
                self.track.id().get(),
                layout.channels
            ))
        })?;
        let channels = std::num::NonZeroU16::new(channels).expect("audio layout is non-zero");
        let android_channels = i32::from(channels.get());
        let android_sample_rate = i32::try_from(layout.sample_rate.get()).map_err(|_| {
            Error::Unsupported(format!(
                "protected audio sample rate {} exceeds Android jint",
                layout.sample_rate
            ))
        })?;
        let (codec, media_crypto, secure_decoder) =
            with_attached_env(&self.drm_session.vm, |env| {
                let config = SecureAudioDecoderConfig {
                    uuid: self.uuid.as_obj(),
                    session: self.drm_session.session_id().as_obj(),
                    mime: self.decoder_mime,
                    channels: android_channels,
                    sample_rate: android_sample_rate,
                    primary_csd: &self.primary_csd,
                };
                let media_crypto = create_audio_media_crypto(env, &config)?;
                let decoder = create_secure_audio_media_codec(env, &config, &media_crypto);
                let (codec, secure_decoder) = match decoder {
                    Ok(decoder) => decoder,
                    Err(error) => {
                        release_java_object(
                            env,
                            &media_crypto,
                            jni_str!("release"),
                            "release",
                            "MediaCrypto",
                        );
                        return Err(error);
                    }
                };
                let (codec, media_crypto) = retain_decoder_objects(env, &codec, &media_crypto)?;
                Ok((codec, media_crypto, secure_decoder))
            })?;
        Ok(AndroidProtectedAudioDecoder {
            resources: AndroidCodecResources {
                drm_session: self.drm_session,
                media_crypto,
                codec,
                offline_key_set: self.offline_key_set.clone(),
            },
            track: self.track.clone(),
            init_data: self.init_data.clone(),
            container_mime: self.container_mime,
            channels,
            sample_rate: layout.sample_rate,
            pcm_encoding: MEDIA_CODEC_PCM_16_BIT,
            input_finished: false,
            secure_decoder,
        })
    }
}

enum LicensePreparation {
    Ready(AndroidLicenseChallenge),
    ProvisionRequired,
}

enum KeyPreparation {
    Ready(AndroidLicenseChallenge),
    Restored,
    ProvisionRequired,
}

fn prepare_key_request(
    env: &mut Env<'_>,
    drm: &JObject<'_>,
    session: &JObject<'_>,
    init_data: &ProtectionInitData,
    container_mime: &str,
    key_type: i32,
) -> Result<LicensePreparation, Error> {
    let init_data_bytes = env
        .byte_array_from_slice(init_data.init_data())
        .map_err(|error| platform_jni_error(env, "create DRM init data", error))?;
    let container_mime = env
        .new_string(container_mime)
        .map_err(|error| platform_jni_error(env, "create DRM MIME", error))?;
    let request = match env.call_method(
        drm,
        jni_str!("getKeyRequest"),
        jni_sig!("([B[BLjava/lang/String;ILjava/util/HashMap;)Landroid/media/MediaDrm$KeyRequest;"),
        &[
            JValue::Object(session),
            JValue::Object(&init_data_bytes),
            JValue::Object(&container_mime),
            JValue::Int(key_type),
            JValue::Object(&JObject::null()),
        ],
    ) {
        Ok(value) => value
            .l()
            .map_err(|error| platform_jni_error(env, "read MediaDrm key request", error))?,
        Err(error) => {
            if take_not_provisioned(env, error, "create MediaDrm key request")? {
                return Ok(LicensePreparation::ProvisionRequired);
            }
            unreachable!("non-provisioning Java errors return from take_not_provisioned")
        }
    };
    read_license_challenge(env, &request, *init_data.system_id())
}

fn read_license_challenge(
    env: &mut Env<'_>,
    request: &JObject<'_>,
    system_id: [u8; 16],
) -> Result<LicensePreparation, Error> {
    let data = byte_array_method(env, request, jni_str!("getData"), "key request")?;
    let default_url = url_method(env, request, "key request")?;
    let request_type = env
        .call_method(request, jni_str!("getRequestType"), jni_sig!("()I"), &[])
        .and_then(jni::objects::JValueOwned::i)
        .map_err(|error| platform_jni_error(env, "read MediaDrm key request type", error))?;
    Ok(LicensePreparation::Ready(AndroidLicenseChallenge {
        system_id,
        data,
        default_url,
        request_type: AndroidKeyRequestType::from_android(request_type)?,
    }))
}

fn provide_key_response(
    vm: &JavaVM,
    drm: &GlobalObjectRef,
    session: &GlobalObjectRef,
    response: &[u8],
) -> Result<Option<AndroidOfflineKeySet>, Error> {
    with_attached_env(vm, |env| {
        provide_key_response_in_env(env, drm.as_obj(), session.as_obj(), response)
    })
}

fn provide_key_response_in_env(
    env: &mut Env<'_>,
    drm: &JObject<'_>,
    scope: &JObject<'_>,
    response: &[u8],
) -> Result<Option<AndroidOfflineKeySet>, Error> {
    if response.is_empty() {
        return Err(Error::Streaming(String::from(
            "Android MediaDrm key response must not be empty",
        )));
    }
    let response = env
        .byte_array_from_slice(response)
        .map_err(|error| platform_jni_error(env, "create MediaDrm key response", error))?;
    let key_set = env
        .call_method(
            drm,
            jni_str!("provideKeyResponse"),
            jni_sig!("([B[B)[B"),
            &[JValue::Object(scope), JValue::Object(&response)],
        )
        .and_then(jni::objects::JValueOwned::l)
        .map_err(|error| platform_jni_error(env, "provide MediaDrm key response", error))?;
    if key_set.is_null() {
        return Ok(None);
    }
    let key_set = env
        .cast_local::<JByteArray>(key_set)
        .map_err(|error| platform_jni_error(env, "cast MediaDrm key-set identity", error))?;
    let key_set = env
        .convert_byte_array(&key_set)
        .map_err(|error| platform_jni_error(env, "copy MediaDrm key-set identity", error))?;
    if key_set.is_empty() {
        Ok(None)
    } else {
        AndroidOfflineKeySet::new(key_set).map(Some)
    }
}

fn restore_keys(
    env: &mut Env<'_>,
    drm: &JObject<'_>,
    session: &JObject<'_>,
    key_set: &AndroidOfflineKeySet,
) -> Result<bool, Error> {
    let key_set = env
        .byte_array_from_slice(key_set.as_bytes())
        .map_err(|error| platform_jni_error(env, "create offline key-set identity", error))?;
    match env.call_method(
        drm,
        jni_str!("restoreKeys"),
        jni_sig!("([B[B)V"),
        &[JValue::Object(session), JValue::Object(&key_set)],
    ) {
        Ok(_) => Ok(false),
        Err(error) => take_not_provisioned(env, error, "restore MediaDrm offline keys"),
    }
}

fn query_key_status(
    env: &mut Env<'_>,
    drm: &JObject<'_>,
    session: &JObject<'_>,
) -> Result<AndroidKeyStatus, Error> {
    let status = env
        .call_method(
            drm,
            jni_str!("queryKeyStatus"),
            jni_sig!("([B)Ljava/util/HashMap;"),
            &[JValue::Object(session)],
        )
        .and_then(jni::objects::JValueOwned::l)
        .map_err(|error| platform_jni_error(env, "query MediaDrm key status", error))?;
    Ok(AndroidKeyStatus {
        license: key_duration_value(env, &status, "LicenseDurationRemaining")?,
        playback: key_duration_value(env, &status, "PlaybackDurationRemaining")?,
    })
}

fn key_duration_value(
    env: &mut Env<'_>,
    status: &JObject<'_>,
    key: &str,
) -> Result<AndroidKeyDuration, Error> {
    let key = env
        .new_string(key)
        .map_err(|error| platform_jni_error(env, "create MediaDrm status key", error))?;
    let value = env
        .call_method(
            status,
            jni_str!("get"),
            jni_sig!("(Ljava/lang/Object;)Ljava/lang/Object;"),
            &[JValue::Object(&key)],
        )
        .and_then(jni::objects::JValueOwned::l)
        .map_err(|error| platform_jni_error(env, "read MediaDrm key duration", error))?;
    if value.is_null() {
        return Ok(AndroidKeyDuration::Unavailable);
    }
    let value = env
        .as_cast::<JString>(&value)
        .and_then(|value| value.try_to_string(env))
        .map_err(|error| platform_jni_error(env, "copy MediaDrm key duration", error))?;
    if value.eq_ignore_ascii_case("unlimited") {
        return Ok(AndroidKeyDuration::Unlimited);
    }
    let seconds = value.parse::<u64>().map_err(|error| {
        Error::Platform(format!(
            "MediaDrm returned invalid key duration {value:?}: {error}"
        ))
    })?;
    Ok(AndroidKeyDuration::Remaining(Duration::from_secs(seconds)))
}

fn android_api_level(env: &mut Env<'_>) -> Result<i32, Error> {
    env.get_static_field(
        jni_str!("android/os/Build$VERSION"),
        jni_str!("SDK_INT"),
        jni_sig!("I"),
    )
    .and_then(jni::objects::JValueOwned::i)
    .map_err(|error| platform_jni_error(env, "read Android API level", error))
}

fn create_java_uuid<'local>(
    env: &mut Env<'local>,
    system_id: [u8; 16],
) -> Result<JObject<'local>, Error> {
    let most = i64::from_be_bytes(
        system_id[..8]
            .try_into()
            .expect("DRM system ID prefix must be eight bytes"),
    );
    let least = i64::from_be_bytes(
        system_id[8..]
            .try_into()
            .expect("DRM system ID suffix must be eight bytes"),
    );
    env.new_object(
        jni_str!("java/util/UUID"),
        jni_sig!("(JJ)V"),
        &[JValue::Long(most), JValue::Long(least)],
    )
    .map_err(|error| platform_jni_error(env, "create DRM UUID", error))
}

fn close_session(
    env: &mut Env<'_>,
    drm: &GlobalObjectRef,
    session_id: &mut Option<GlobalObjectRef>,
) {
    let Some(session) = session_id.take() else {
        return;
    };
    if let Err(error) = env.call_method(
        drm.as_obj(),
        jni_str!("closeSession"),
        jni_sig!("([B)V"),
        &[JValue::Object(session.as_obj())],
    ) {
        tracing::error!(%error, "failed to close unprovisioned MediaDrm session");
        clear_pending_exception(env);
    }
}

fn byte_array_method(
    env: &mut Env<'_>,
    object: &JObject<'_>,
    method: &JNIStr,
    label: &str,
) -> Result<Vec<u8>, Error> {
    let array = env
        .call_method(object, method, jni_sig!("()[B"), &[])
        .and_then(jni::objects::JValueOwned::l)
        .map_err(|error| platform_jni_error(env, &format!("read {label} data"), error))?;
    let array = env
        .cast_local::<JByteArray>(array)
        .map_err(|error| platform_jni_error(env, &format!("cast {label} data"), error))?;
    env.convert_byte_array(&array)
        .map_err(|error| platform_jni_error(env, &format!("copy {label} data"), error))
}

fn url_method(env: &mut Env<'_>, object: &JObject<'_>, label: &str) -> Result<Option<Url>, Error> {
    let url = env
        .call_method(
            object,
            jni_str!("getDefaultUrl"),
            jni_sig!("()Ljava/lang/String;"),
            &[],
        )
        .and_then(jni::objects::JValueOwned::l)
        .map_err(|error| platform_jni_error(env, &format!("read {label} URL"), error))?;
    let url = env
        .as_cast::<JString>(&url)
        .and_then(|url| url.try_to_string(env))
        .map_err(|error| platform_jni_error(env, &format!("copy {label} URL"), error))?;
    if url.is_empty() {
        Ok(None)
    } else {
        Url::parse(&url)
            .map(Some)
            .map_err(|error| Error::Streaming(format!("invalid Android {label} URL: {error}")))
    }
}

struct SecureDecoderConfig<'a> {
    uuid: &'a JObject<'static>,
    session: &'a JObject<'static>,
    surface: &'a JObject<'static>,
    mime: &'static str,
    width: i32,
    height: i32,
    primary_csd: &'a [u8],
    secondary_csd: &'a [u8],
}

struct SecureAudioDecoderConfig<'a> {
    uuid: &'a JObject<'static>,
    session: &'a JObject<'static>,
    mime: &'static str,
    channels: i32,
    sample_rate: i32,
    primary_csd: &'a [u8],
}

fn create_media_crypto<'local>(
    env: &mut Env<'local>,
    config: &SecureDecoderConfig<'_>,
) -> Result<JObject<'local>, Error> {
    create_media_crypto_for(env, config.uuid, config.session)
}

fn create_audio_media_crypto<'local>(
    env: &mut Env<'local>,
    config: &SecureAudioDecoderConfig<'_>,
) -> Result<JObject<'local>, Error> {
    create_media_crypto_for(env, config.uuid, config.session)
}

fn create_media_crypto_for<'local>(
    env: &mut Env<'local>,
    uuid: &JObject<'_>,
    session: &JObject<'_>,
) -> Result<JObject<'local>, Error> {
    env.new_object(
        jni_str!("android/media/MediaCrypto"),
        jni_sig!("(Ljava/util/UUID;[B)V"),
        &[JValue::Object(uuid), JValue::Object(session)],
    )
    .map_err(|error| platform_jni_error(env, "create MediaCrypto", error))
}

fn create_secure_media_codec<'local>(
    env: &mut Env<'local>,
    config: &SecureDecoderConfig<'_>,
    media_crypto: &JObject<'local>,
) -> Result<(JObject<'local>, bool), Error> {
    let decoder_mime = env
        .new_string(config.mime)
        .map_err(|error| platform_jni_error(env, "create secure decoder MIME", error))?;
    let secure_decoder = requires_secure_decoder(env, media_crypto, &decoder_mime)?;
    let format = create_secure_media_format(env, config, &decoder_mime, secure_decoder)?;
    let codec = create_media_codec(
        env,
        config.mime,
        Some(config.surface),
        &format,
        media_crypto,
        secure_decoder,
    )?;
    Ok((codec, secure_decoder))
}

fn create_secure_audio_media_codec<'local>(
    env: &mut Env<'local>,
    config: &SecureAudioDecoderConfig<'_>,
    media_crypto: &JObject<'local>,
) -> Result<(JObject<'local>, bool), Error> {
    let decoder_mime = env
        .new_string(config.mime)
        .map_err(|error| platform_jni_error(env, "create protected audio MIME", error))?;
    let secure_decoder = requires_secure_decoder(env, media_crypto, &decoder_mime)?;
    let format = create_secure_audio_media_format(env, config, &decoder_mime, secure_decoder)?;
    let codec = create_media_codec(
        env,
        config.mime,
        None,
        &format,
        media_crypto,
        secure_decoder,
    )?;
    Ok((codec, secure_decoder))
}

fn requires_secure_decoder(
    env: &mut Env<'_>,
    media_crypto: &JObject<'_>,
    decoder_mime: &JString<'_>,
) -> Result<bool, Error> {
    env.call_method(
        media_crypto,
        jni_str!("requiresSecureDecoderComponent"),
        jni_sig!("(Ljava/lang/String;)Z"),
        &[JValue::Object(decoder_mime)],
    )
    .and_then(jni::objects::JValueOwned::z)
    .map_err(|error| platform_jni_error(env, "query secure decoder requirement", error))
}

fn create_secure_media_format<'local>(
    env: &mut Env<'local>,
    config: &SecureDecoderConfig<'_>,
    decoder_mime: &JString<'local>,
    secure_decoder: bool,
) -> Result<JObject<'local>, Error> {
    let format = env
        .call_static_method(
            jni_str!("android/media/MediaFormat"),
            jni_str!("createVideoFormat"),
            jni_sig!("(Ljava/lang/String;II)Landroid/media/MediaFormat;"),
            &[
                JValue::Object(decoder_mime),
                JValue::Int(config.width),
                JValue::Int(config.height),
            ],
        )
        .and_then(jni::objects::JValueOwned::l)
        .map_err(|error| platform_jni_error(env, "create secure decoder format", error))?;
    set_codec_specific_data(env, &format, "csd-0", config.primary_csd)?;
    set_codec_specific_data(env, &format, "csd-1", config.secondary_csd)?;
    enable_secure_playback_if_required(env, &format, secure_decoder)?;
    Ok(format)
}

fn create_secure_audio_media_format<'local>(
    env: &mut Env<'local>,
    config: &SecureAudioDecoderConfig<'_>,
    decoder_mime: &JString<'local>,
    secure_decoder: bool,
) -> Result<JObject<'local>, Error> {
    let format = env
        .call_static_method(
            jni_str!("android/media/MediaFormat"),
            jni_str!("createAudioFormat"),
            jni_sig!("(Ljava/lang/String;II)Landroid/media/MediaFormat;"),
            &[
                JValue::Object(decoder_mime),
                JValue::Int(config.sample_rate),
                JValue::Int(config.channels),
            ],
        )
        .and_then(jni::objects::JValueOwned::l)
        .map_err(|error| platform_jni_error(env, "create protected audio format", error))?;
    set_media_format_integer(env, &format, "is-adts", 0)?;
    set_media_format_integer(env, &format, "pcm-encoding", MEDIA_CODEC_PCM_FLOAT)?;
    set_codec_specific_data(env, &format, "csd-0", config.primary_csd)?;
    enable_secure_playback_if_required(env, &format, secure_decoder)?;
    Ok(format)
}

fn enable_secure_playback_if_required(
    env: &mut Env<'_>,
    format: &JObject<'_>,
    secure_decoder: bool,
) -> Result<(), Error> {
    if !secure_decoder {
        return Ok(());
    }
    let secure_feature = env
        .new_string("secure-playback")
        .map_err(|error| platform_jni_error(env, "create secure feature", error))?;
    env.call_method(
        format,
        jni_str!("setFeatureEnabled"),
        jni_sig!("(Ljava/lang/String;Z)V"),
        &[JValue::Object(&secure_feature), JValue::Bool(true)],
    )
    .map_err(|error| platform_jni_error(env, "enable secure decoder feature", error))?;
    Ok(())
}

fn set_media_format_integer(
    env: &mut Env<'_>,
    format: &JObject<'_>,
    key: &str,
    value: i32,
) -> Result<(), Error> {
    let key = env
        .new_string(key)
        .map_err(|error| platform_jni_error(env, "create MediaFormat integer key", error))?;
    env.call_method(
        format,
        jni_str!("setInteger"),
        jni_sig!("(Ljava/lang/String;I)V"),
        &[JValue::Object(&key), JValue::Int(value)],
    )
    .map_err(|error| platform_jni_error(env, "set MediaFormat integer", error))?;
    Ok(())
}

fn create_media_codec<'local>(
    env: &mut Env<'local>,
    mime: &str,
    surface: Option<&JObject<'_>>,
    format: &JObject<'local>,
    media_crypto: &JObject<'local>,
    secure_decoder: bool,
) -> Result<JObject<'local>, Error> {
    let codec_list = env
        .new_object(
            jni_str!("android/media/MediaCodecList"),
            jni_sig!("(I)V"),
            &[JValue::Int(MEDIA_CODEC_LIST_ALL_CODECS)],
        )
        .map_err(|error| platform_jni_error(env, "create MediaCodecList", error))?;
    let decoder_name = env
        .call_method(
            &codec_list,
            jni_str!("findDecoderForFormat"),
            jni_sig!("(Landroid/media/MediaFormat;)Ljava/lang/String;"),
            &[JValue::Object(format)],
        )
        .and_then(jni::objects::JValueOwned::l)
        .map_err(|error| platform_jni_error(env, "select secure decoder", error))?;
    if decoder_name.is_null() {
        return Err(Error::Unsupported(format!(
            "Android has no {} decoder for {}",
            if secure_decoder { "secure" } else { "DRM" },
            mime
        )));
    }
    let codec = env
        .call_static_method(
            jni_str!("android/media/MediaCodec"),
            jni_str!("createByCodecName"),
            jni_sig!("(Ljava/lang/String;)Landroid/media/MediaCodec;"),
            &[JValue::Object(&decoder_name)],
        )
        .and_then(jni::objects::JValueOwned::l)
        .map_err(|error| platform_jni_error(env, "create selected secure decoder", error))?;
    if let Err(error) = configure_and_start_codec(env, surface, format, media_crypto, &codec) {
        release_java_object(
            env,
            &codec,
            jni_str!("release"),
            "release",
            "unconfigured MediaCodec",
        );
        return Err(error);
    }
    Ok(codec)
}

fn configure_and_start_codec(
    env: &mut Env<'_>,
    surface: Option<&JObject<'_>>,
    format: &JObject<'_>,
    media_crypto: &JObject<'_>,
    codec: &JObject<'_>,
) -> Result<(), Error> {
    let null_surface = JObject::null();
    let surface = surface.unwrap_or(&null_surface);
    env.call_method(
        codec,
        jni_str!("configure"),
        jni_sig!(
            "(Landroid/media/MediaFormat;Landroid/view/Surface;Landroid/media/MediaCrypto;I)V"
        ),
        &[
            JValue::Object(format),
            JValue::Object(surface),
            JValue::Object(media_crypto),
            JValue::Int(0),
        ],
    )
    .map_err(|error| platform_jni_error(env, "configure secure decoder", error))?;
    env.call_method(codec, jni_str!("start"), jni_sig!("()V"), &[])
        .map_err(|error| platform_jni_error(env, "start secure decoder", error))?;
    Ok(())
}

fn retain_decoder_objects(
    env: &mut Env<'_>,
    codec: &JObject<'_>,
    media_crypto: &JObject<'_>,
) -> Result<(GlobalObjectRef, GlobalObjectRef), Error> {
    let retained_crypto = match env.new_global_ref(media_crypto) {
        Ok(reference) => reference,
        Err(error) => {
            release_java_object(env, codec, jni_str!("release"), "release", "MediaCodec");
            release_java_object(
                env,
                media_crypto,
                jni_str!("release"),
                "release",
                "MediaCrypto",
            );
            return Err(platform_jni_error(env, "retain MediaCrypto", error));
        }
    };
    let retained_codec = match env.new_global_ref(codec) {
        Ok(reference) => reference,
        Err(error) => {
            release_java_object(env, codec, jni_str!("release"), "release", "MediaCodec");
            release_java_object(
                env,
                media_crypto,
                jni_str!("release"),
                "release",
                "MediaCrypto",
            );
            return Err(platform_jni_error(env, "retain secure MediaCodec", error));
        }
    };
    Ok((retained_codec, retained_crypto))
}

fn set_codec_specific_data(
    env: &mut Env<'_>,
    format: &JObject<'_>,
    key: &str,
    data: &[u8],
) -> Result<(), Error> {
    if data.is_empty() {
        return Ok(());
    }
    let key = env
        .new_string(key)
        .map_err(|error| platform_jni_error(env, "create codec data key", error))?;
    let data = env
        .byte_array_from_slice(data)
        .map_err(|error| platform_jni_error(env, "create codec data", error))?;
    let buffer = env
        .call_static_method(
            jni_str!("java/nio/ByteBuffer"),
            jni_str!("wrap"),
            jni_sig!("([B)Ljava/nio/ByteBuffer;"),
            &[JValue::Object(&data)],
        )
        .and_then(jni::objects::JValueOwned::l)
        .map_err(|error| platform_jni_error(env, "wrap codec data", error))?;
    env.call_method(
        format,
        jni_str!("setByteBuffer"),
        jni_sig!("(Ljava/lang/String;Ljava/nio/ByteBuffer;)V"),
        &[JValue::Object(&key), JValue::Object(&buffer)],
    )
    .map_err(|error| platform_jni_error(env, "install codec data", error))?;
    Ok(())
}

fn write_codec_input(
    env: &mut Env<'_>,
    codec: &JObject<'_>,
    index: i32,
    data: &[u8],
) -> Result<(), Error> {
    let buffer = env
        .call_method(
            codec,
            jni_str!("getInputBuffer"),
            jni_sig!("(I)Ljava/nio/ByteBuffer;"),
            &[JValue::Int(index)],
        )
        .and_then(jni::objects::JValueOwned::l)
        .map_err(|error| platform_jni_error(env, "get secure input buffer", error))?;
    if buffer.is_null() {
        return Err(Error::Platform(format!(
            "MediaCodec secure input buffer {index} is null"
        )));
    }
    env.call_method(
        &buffer,
        jni_str!("clear"),
        jni_sig!("()Ljava/nio/Buffer;"),
        &[],
    )
    .map_err(|error| platform_jni_error(env, "clear secure input buffer", error))?;
    let capacity = env
        .call_method(&buffer, jni_str!("remaining"), jni_sig!("()I"), &[])
        .and_then(jni::objects::JValueOwned::i)
        .map_err(|error| platform_jni_error(env, "read secure input capacity", error))?;
    let data_len = i32::try_from(data.len()).map_err(|_| {
        Error::Codec(String::from(
            "protected access unit exceeds Android jint length",
        ))
    })?;
    if capacity < data_len {
        return Err(Error::Codec(format!(
            "protected access unit has {data_len} bytes but MediaCodec input capacity is {capacity}"
        )));
    }
    let data = env
        .byte_array_from_slice(data)
        .map_err(|error| platform_jni_error(env, "create secure input bytes", error))?;
    env.call_method(
        &buffer,
        jni_str!("put"),
        jni_sig!("([B)Ljava/nio/ByteBuffer;"),
        &[JValue::Object(&data)],
    )
    .map_err(|error| platform_jni_error(env, "write secure input buffer", error))?;
    Ok(())
}

fn queue_secure_sample(
    resources: &AndroidCodecResources,
    track: &TrackInfo,
    sample: &EncodedSample,
    data: &[u8],
    subsamples: &[waterkit_video_core::EncryptionSubsample],
) -> Result<(), Error> {
    if sample.track_id() != track.id() {
        return Err(Error::Codec(format!(
            "protected decoder for track {} received track {}",
            track.id().get(),
            sample.track_id().get()
        )));
    }
    let encryption = sample.encryption().ok_or_else(|| {
        Error::Codec(format!(
            "protected track {} received a sample without CENC metadata",
            track.id().get()
        ))
    })?;
    let protection = track.protection().ok_or_else(|| {
        Error::Codec(String::from(
            "licensed protected decoder lost its track protection metadata",
        ))
    })?;
    let (clear_bytes, encrypted_bytes) = android_subsamples(subsamples, data.len())?;
    let initialization_vector = normalized_initialization_vector(
        encryption.initialization_vector(),
        protection.constant_iv(),
    )?;
    let presentation_time_micros =
        i64::try_from(sample.presentation_time().to_duration()?.as_micros()).map_err(|_| {
            Error::Codec(String::from("protected sample PTS exceeds Android jlong"))
        })?;
    resources.with_env(|env, codec| {
        let input_index = env
            .call_method(
                codec,
                jni_str!("dequeueInputBuffer"),
                jni_sig!("(J)I"),
                &[JValue::Long(INPUT_WAIT_FOREVER_MICROS)],
            )
            .and_then(jni::objects::JValueOwned::i)
            .map_err(|error| platform_jni_error(env, "dequeue secure input", error))?;
        if input_index < 0 {
            return Err(Error::Platform(format!(
                "MediaCodec returned {input_index} while waiting indefinitely for secure input"
            )));
        }
        write_codec_input(env, codec, input_index, data)?;
        let crypto_info = create_crypto_info(
            env,
            &clear_bytes,
            &encrypted_bytes,
            protection.default_key_id(),
            &initialization_vector,
            protection,
        )?;
        env.call_method(
            codec,
            jni_str!("queueSecureInputBuffer"),
            jni_sig!("(IILandroid/media/MediaCodec$CryptoInfo;JI)V"),
            &[
                JValue::Int(input_index),
                JValue::Int(0),
                JValue::Object(&crypto_info),
                JValue::Long(presentation_time_micros),
                JValue::Int(0),
            ],
        )
        .map_err(|error| platform_jni_error(env, "queue secure input", error))?;
        Ok(())
    })
}

fn queue_end_of_stream(resources: &AndroidCodecResources) -> Result<(), Error> {
    resources.with_env(|env, codec| {
        let input_index = env
            .call_method(
                codec,
                jni_str!("dequeueInputBuffer"),
                jni_sig!("(J)I"),
                &[JValue::Long(INPUT_WAIT_FOREVER_MICROS)],
            )
            .and_then(jni::objects::JValueOwned::i)
            .map_err(|error| platform_jni_error(env, "dequeue EOS input", error))?;
        if input_index < 0 {
            return Err(Error::Platform(format!(
                "MediaCodec returned {input_index} while waiting indefinitely for EOS input"
            )));
        }
        env.call_method(
            codec,
            jni_str!("queueInputBuffer"),
            jni_sig!("(IIIJI)V"),
            &[
                JValue::Int(input_index),
                JValue::Int(0),
                JValue::Int(0),
                JValue::Long(0),
                JValue::Int(MEDIA_CODEC_BUFFER_FLAG_END_OF_STREAM),
            ],
        )
        .map_err(|error| platform_jni_error(env, "queue secure EOS", error))?;
        Ok(())
    })
}

fn flush_codec(resources: &AndroidCodecResources) -> Result<(), Error> {
    resources.with_env(|env, codec| {
        env.call_method(codec, jni_str!("flush"), jni_sig!("()V"), &[])
            .map_err(|error| platform_jni_error(env, "flush protected decoder", error))?;
        Ok(())
    })
}

fn dequeue_audio_output(
    resources: &AndroidCodecResources,
    timeout_micros: i64,
) -> Result<AndroidAudioOutput, Error> {
    resources.with_env(|env, codec| {
        let buffer_info = env
            .new_object(
                jni_str!("android/media/MediaCodec$BufferInfo"),
                jni_sig!("()V"),
                &[],
            )
            .map_err(|error| platform_jni_error(env, "create audio BufferInfo", error))?;
        loop {
            let index = env
                .call_method(
                    codec,
                    jni_str!("dequeueOutputBuffer"),
                    jni_sig!("(Landroid/media/MediaCodec$BufferInfo;J)I"),
                    &[JValue::Object(&buffer_info), JValue::Long(timeout_micros)],
                )
                .and_then(jni::objects::JValueOwned::i)
                .map_err(|error| {
                    platform_jni_error(env, "dequeue protected audio output", error)
                })?;
            match index {
                MEDIA_CODEC_INFO_TRY_AGAIN_LATER => return Ok(AndroidAudioOutput::TryAgainLater),
                MEDIA_CODEC_INFO_OUTPUT_BUFFERS_CHANGED => {}
                MEDIA_CODEC_INFO_OUTPUT_FORMAT_CHANGED => {
                    return read_audio_output_format(env, codec);
                }
                index if index >= 0 => {
                    return read_audio_output_buffer(env, codec, &buffer_info, index);
                }
                other => {
                    return Err(Error::Platform(format!(
                        "MediaCodec returned unknown protected audio output status {other}"
                    )));
                }
            }
        }
    })
}

fn read_audio_output_format(
    env: &mut Env<'_>,
    codec: &JObject<'_>,
) -> Result<AndroidAudioOutput, Error> {
    let format = env
        .call_method(
            codec,
            jni_str!("getOutputFormat"),
            jni_sig!("()Landroid/media/MediaFormat;"),
            &[],
        )
        .and_then(jni::objects::JValueOwned::l)
        .map_err(|error| platform_jni_error(env, "read protected audio format", error))?;
    let channels = media_format_integer(env, &format, "channel-count")?;
    let sample_rate = media_format_integer(env, &format, "sample-rate")?;
    let channels = u16::try_from(channels)
        .ok()
        .and_then(std::num::NonZeroU16::new)
        .ok_or_else(|| Error::Codec(format!("invalid protected audio channel count {channels}")))?;
    let sample_rate = u32::try_from(sample_rate)
        .ok()
        .and_then(std::num::NonZeroU32::new)
        .ok_or_else(|| {
            Error::Codec(format!("invalid protected audio sample rate {sample_rate}"))
        })?;
    let pcm_encoding = media_format_integer_optional(env, &format, "pcm-encoding")?
        .unwrap_or(MEDIA_CODEC_PCM_16_BIT);
    if !matches!(pcm_encoding, MEDIA_CODEC_PCM_16_BIT | MEDIA_CODEC_PCM_FLOAT) {
        return Err(Error::Unsupported(format!(
            "Android protected audio decoder returned unsupported PCM encoding {pcm_encoding}"
        )));
    }
    Ok(AndroidAudioOutput::Format {
        channels,
        sample_rate,
        pcm_encoding,
    })
}

fn read_audio_output_buffer(
    env: &mut Env<'_>,
    codec: &JObject<'_>,
    info: &JObject<'_>,
    index: i32,
) -> Result<AndroidAudioOutput, Error> {
    let offset = env
        .get_field(info, jni_str!("offset"), jni_sig!("I"))
        .and_then(jni::objects::JValueOwned::i)
        .map_err(|error| platform_jni_error(env, "read protected audio output offset", error))?;
    let size = env
        .get_field(info, jni_str!("size"), jni_sig!("I"))
        .and_then(jni::objects::JValueOwned::i)
        .map_err(|error| platform_jni_error(env, "read protected audio output size", error))?;
    let presentation_time_micros = env
        .get_field(info, jni_str!("presentationTimeUs"), jni_sig!("J"))
        .and_then(jni::objects::JValueOwned::j)
        .map_err(|error| platform_jni_error(env, "read protected audio output PTS", error))?;
    let flags = env
        .get_field(info, jni_str!("flags"), jni_sig!("I"))
        .and_then(jni::objects::JValueOwned::i)
        .map_err(|error| platform_jni_error(env, "read protected audio output flags", error))?;
    let bytes = read_audio_output_bytes(env, codec, index, offset, size);
    let released = env
        .call_method(
            codec,
            jni_str!("releaseOutputBuffer"),
            jni_sig!("(IZ)V"),
            &[JValue::Int(index), JValue::Bool(false)],
        )
        .map_err(|error| platform_jni_error(env, "release protected audio output", error));
    released?;
    let presentation_time_micros = u64::try_from(presentation_time_micros).map_err(|_| {
        Error::Codec(format!(
            "MediaCodec returned negative protected audio PTS {presentation_time_micros}"
        ))
    })?;
    Ok(AndroidAudioOutput::Buffer {
        presentation_time: Duration::from_micros(presentation_time_micros),
        bytes: bytes?,
        end_of_stream: flags & MEDIA_CODEC_BUFFER_FLAG_END_OF_STREAM != 0,
    })
}

fn read_audio_output_bytes(
    env: &mut Env<'_>,
    codec: &JObject<'_>,
    index: i32,
    offset: i32,
    size: i32,
) -> Result<Vec<u8>, Error> {
    if offset < 0 || size < 0 {
        return Err(Error::Codec(format!(
            "MediaCodec returned invalid protected audio range offset={offset} size={size}"
        )));
    }
    if size == 0 {
        return Ok(Vec::new());
    }
    let end = offset
        .checked_add(size)
        .ok_or_else(|| Error::Codec(String::from("protected audio output range overflow")))?;
    let byte_count =
        usize::try_from(size).expect("validated protected audio output size must fit usize");
    let buffer = env
        .call_method(
            codec,
            jni_str!("getOutputBuffer"),
            jni_sig!("(I)Ljava/nio/ByteBuffer;"),
            &[JValue::Int(index)],
        )
        .and_then(jni::objects::JValueOwned::l)
        .map_err(|error| platform_jni_error(env, "get protected audio output buffer", error))?;
    if buffer.is_null() {
        return Err(Error::Platform(format!(
            "MediaCodec protected audio output buffer {index} is null"
        )));
    }
    env.call_method(
        &buffer,
        jni_str!("position"),
        jni_sig!("(I)Ljava/nio/Buffer;"),
        &[JValue::Int(offset)],
    )
    .map_err(|error| platform_jni_error(env, "position protected audio output", error))?;
    env.call_method(
        &buffer,
        jni_str!("limit"),
        jni_sig!("(I)Ljava/nio/Buffer;"),
        &[JValue::Int(end)],
    )
    .map_err(|error| platform_jni_error(env, "limit protected audio output", error))?;
    let bytes = env
        .new_byte_array(byte_count)
        .map_err(|error| platform_jni_error(env, "allocate protected audio output", error))?;
    env.call_method(
        &buffer,
        jni_str!("get"),
        jni_sig!("([B)Ljava/nio/ByteBuffer;"),
        &[JValue::Object(&bytes)],
    )
    .map_err(|error| platform_jni_error(env, "copy protected audio output", error))?;
    env.convert_byte_array(bytes)
        .map_err(|error| platform_jni_error(env, "retain protected audio output", error))
}

fn media_format_integer(env: &mut Env<'_>, format: &JObject<'_>, key: &str) -> Result<i32, Error> {
    media_format_integer_optional(env, format, key)?.ok_or_else(|| {
        Error::Codec(format!(
            "protected audio output format omitted required key {key}"
        ))
    })
}

fn media_format_integer_optional(
    env: &mut Env<'_>,
    format: &JObject<'_>,
    key: &str,
) -> Result<Option<i32>, Error> {
    let key = env
        .new_string(key)
        .map_err(|error| platform_jni_error(env, "create MediaFormat query key", error))?;
    let contains = env
        .call_method(
            format,
            jni_str!("containsKey"),
            jni_sig!("(Ljava/lang/String;)Z"),
            &[JValue::Object(&key)],
        )
        .and_then(jni::objects::JValueOwned::z)
        .map_err(|error| platform_jni_error(env, "query MediaFormat key", error))?;
    if !contains {
        return Ok(None);
    }
    env.call_method(
        format,
        jni_str!("getInteger"),
        jni_sig!("(Ljava/lang/String;)I"),
        &[JValue::Object(&key)],
    )
    .and_then(jni::objects::JValueOwned::i)
    .map(Some)
    .map_err(|error| platform_jni_error(env, "read MediaFormat integer", error))
}

fn decode_android_pcm(bytes: &[u8], encoding: i32) -> Result<Vec<f32>, Error> {
    match encoding {
        MEDIA_CODEC_PCM_16_BIT => {
            let mut chunks = bytes.chunks_exact(2);
            let samples = chunks
                .by_ref()
                .map(|sample| f32::from(i16::from_le_bytes([sample[0], sample[1]])) / 32_768.0)
                .collect::<Vec<_>>();
            if !chunks.remainder().is_empty() {
                return Err(Error::Codec(format!(
                    "16-bit protected PCM contains {} trailing byte(s)",
                    chunks.remainder().len()
                )));
            }
            Ok(samples)
        }
        MEDIA_CODEC_PCM_FLOAT => {
            let mut chunks = bytes.chunks_exact(4);
            let samples = chunks
                .by_ref()
                .map(|sample| f32::from_le_bytes([sample[0], sample[1], sample[2], sample[3]]))
                .collect::<Vec<_>>();
            if !chunks.remainder().is_empty() {
                return Err(Error::Codec(format!(
                    "float protected PCM contains {} trailing byte(s)",
                    chunks.remainder().len()
                )));
            }
            Ok(samples)
        }
        other => Err(Error::Unsupported(format!(
            "unsupported Android protected PCM encoding {other}"
        ))),
    }
}

fn create_crypto_info<'a>(
    env: &mut Env<'a>,
    clear_bytes: &[i32],
    encrypted_bytes: &[i32],
    key_id: &[u8; 16],
    initialization_vector: &[u8; 16],
    protection: &TrackProtection,
) -> Result<JObject<'a>, Error> {
    let clear_array = int_array(env, clear_bytes, "clear CENC byte counts")?;
    let encrypted_array = int_array(env, encrypted_bytes, "encrypted CENC byte counts")?;
    let key_id = env
        .byte_array_from_slice(key_id)
        .map_err(|error| platform_jni_error(env, "create CENC key ID", error))?;
    let initialization_vector = env
        .byte_array_from_slice(initialization_vector)
        .map_err(|error| platform_jni_error(env, "create CENC IV", error))?;
    let crypto_info = env
        .new_object(
            jni_str!("android/media/MediaCodec$CryptoInfo"),
            jni_sig!("()V"),
            &[],
        )
        .map_err(|error| platform_jni_error(env, "create MediaCodec CryptoInfo", error))?;
    let mode = match protection.scheme() {
        CommonEncryptionScheme::Cenc => MEDIA_CODEC_CRYPTO_MODE_AES_CTR,
        CommonEncryptionScheme::Cbcs => MEDIA_CODEC_CRYPTO_MODE_AES_CBC,
    };
    let subsample_count = i32::try_from(clear_bytes.len())
        .expect("CENC subsample arrays originate from a Java-sized allocation");
    env.call_method(
        &crypto_info,
        jni_str!("set"),
        jni_sig!("(I[I[I[B[BI)V"),
        &[
            JValue::Int(subsample_count),
            JValue::Object(&clear_array),
            JValue::Object(&encrypted_array),
            JValue::Object(&key_id),
            JValue::Object(&initialization_vector),
            JValue::Int(mode),
        ],
    )
    .map_err(|error| platform_jni_error(env, "populate MediaCodec CryptoInfo", error))?;
    if protection.scheme() == CommonEncryptionScheme::Cbcs {
        let encrypted_blocks = i32::from(protection.crypt_byte_block());
        let clear_blocks = i32::from(protection.skip_byte_block());
        let pattern = env
            .new_object(
                jni_str!("android/media/MediaCodec$CryptoInfo$Pattern"),
                jni_sig!("(II)V"),
                &[JValue::Int(encrypted_blocks), JValue::Int(clear_blocks)],
            )
            .map_err(|error| platform_jni_error(env, "create cbcs pattern", error))?;
        env.call_method(
            &crypto_info,
            jni_str!("setPattern"),
            jni_sig!("(Landroid/media/MediaCodec$CryptoInfo$Pattern;)V"),
            &[JValue::Object(&pattern)],
        )
        .map_err(|error| platform_jni_error(env, "install cbcs pattern", error))?;
    }
    Ok(crypto_info)
}

fn int_array<'a>(env: &mut Env<'a>, values: &[i32], label: &str) -> Result<JIntArray<'a>, Error> {
    let array = env
        .new_int_array(values.len())
        .map_err(|error| platform_jni_error(env, &format!("create {label}"), error))?;
    array
        .set_region(env, 0, values)
        .map_err(|error| platform_jni_error(env, &format!("write {label}"), error))?;
    Ok(array)
}

fn android_subsamples(
    subsamples: &[waterkit_video_core::EncryptionSubsample],
    sample_len: usize,
) -> Result<(Vec<i32>, Vec<i32>), Error> {
    if subsamples.is_empty() {
        let encrypted = i32::try_from(sample_len).map_err(|_| {
            Error::Codec(String::from(
                "protected sample exceeds Android CryptoInfo jint length",
            ))
        })?;
        return Ok((vec![0], vec![encrypted]));
    }
    let described_len = subsamples.iter().try_fold(0_u64, |total, subsample| {
        total
            .checked_add(u64::from(subsample.clear_bytes()))
            .and_then(|total| total.checked_add(u64::from(subsample.encrypted_bytes())))
            .ok_or_else(|| Error::Codec(String::from("CENC subsample lengths overflow u64")))
    })?;
    if described_len != sample_len as u64 {
        return Err(Error::Codec(format!(
            "CENC subsamples describe {described_len} bytes but the protected access unit has {sample_len} bytes"
        )));
    }
    let clear = subsamples
        .iter()
        .map(|subsample| Ok(i32::from(subsample.clear_bytes())))
        .collect::<Result<Vec<_>, Error>>()?;
    let encrypted = subsamples
        .iter()
        .map(|subsample| {
            i32::try_from(subsample.encrypted_bytes()).map_err(|_| {
                Error::Codec(format!(
                    "CENC encrypted range {} exceeds Android jint",
                    subsample.encrypted_bytes()
                ))
            })
        })
        .collect::<Result<Vec<_>, Error>>()?;
    Ok((clear, encrypted))
}

fn normalized_initialization_vector(
    sample_iv: &[u8],
    constant_iv: Option<&[u8]>,
) -> Result<[u8; 16], Error> {
    let source = if sample_iv.is_empty() {
        constant_iv.ok_or_else(|| {
            Error::Container(String::from(
                "protected sample has no IV and its track has no constant IV",
            ))
        })?
    } else {
        sample_iv
    };
    if !matches!(source.len(), 8 | 16) {
        return Err(Error::Container(format!(
            "CENC IV must contain 8 or 16 bytes, got {}",
            source.len()
        )));
    }
    let mut normalized = [0_u8; 16];
    normalized[..source.len()].copy_from_slice(source);
    Ok(normalized)
}

fn take_not_provisioned(
    env: &mut Env<'_>,
    error: JniError,
    operation: &str,
) -> Result<bool, Error> {
    match error {
        JniError::JavaException => {}
        other => return Err(Error::Platform(format!("{operation}: {other}"))),
    }
    let throwable = env.exception_occurred().ok_or_else(|| {
        Error::Platform(format!(
            "{operation}: JNI reported JavaException without a pending throwable"
        ))
    })?;
    env.exception_clear();
    if env
        .is_instance_of(
            &throwable,
            jni_str!("android/media/NotProvisionedException"),
        )
        .map_err(|nested| {
            Error::Platform(format!(
                "{operation}: inspect Java exception failed: {nested}"
            ))
        })?
    {
        return Ok(true);
    }
    Err(Error::Platform(format!(
        "{operation}: {}",
        throwable_text(env, &throwable)
    )))
}

fn platform_jni_error(env: &mut Env<'_>, operation: &str, error: JniError) -> Error {
    match error {
        JniError::JavaException => {}
        other => return Error::Platform(format!("{operation}: {other}")),
    }
    let Some(throwable) = env.exception_occurred() else {
        return Error::Platform(format!(
            "{operation}: JNI reported JavaException without a pending throwable"
        ));
    };
    env.exception_clear();
    Error::Platform(format!("{operation}: {}", throwable_text(env, &throwable)))
}

fn throwable_text(env: &mut Env<'_>, throwable: &JThrowable<'_>) -> String {
    let text = env
        .call_method(
            throwable,
            jni_str!("toString"),
            jni_sig!("()Ljava/lang/String;"),
            &[],
        )
        .and_then(jni::objects::JValueOwned::l);
    let Ok(text) = text else {
        return String::from("Java exception with unavailable description");
    };
    env.as_cast::<JString>(&text)
        .and_then(|text| text.try_to_string(env))
        .map_or_else(
            |_| String::from("Java exception with invalid description"),
            |text| text,
        )
}

fn clear_pending_exception(env: &Env<'_>) {
    if env.exception_check() {
        env.exception_clear();
    }
}

fn release_java_object(
    env: &mut Env<'_>,
    object: &JObject<'_>,
    method: &JNIStr,
    method_label: &str,
    object_label: &str,
) {
    if let Err(error) = env.call_method(object, method, jni_sig!("()V"), &[]) {
        tracing::error!(
            %error,
            method = method_label,
            object = object_label,
            "failed to release Android protected playback object"
        );
        clear_pending_exception(env);
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{
        AndroidKeyDuration, AndroidKeyStatus, MEDIA_CODEC_PCM_16_BIT, MEDIA_CODEC_PCM_FLOAT,
        decode_android_pcm,
    };

    #[test]
    fn finite_key_lifetime_triggers_at_threshold() {
        let status = AndroidKeyStatus {
            license: AndroidKeyDuration::Remaining(Duration::from_secs(60)),
            playback: AndroidKeyDuration::Unlimited,
        };

        assert!(status.requires_renewal(Duration::from_secs(60)));
        assert!(!status.requires_renewal(Duration::from_secs(59)));
    }

    #[test]
    fn unavailable_and_unlimited_keys_do_not_speculate() {
        let status = AndroidKeyStatus {
            license: AndroidKeyDuration::Unavailable,
            playback: AndroidKeyDuration::Unlimited,
        };

        assert!(!status.requires_renewal(Duration::MAX));
    }

    #[test]
    fn decodes_little_endian_pcm_encodings() {
        let pcm16 = decode_android_pcm(&[0, 128, 0, 0, 255, 127], MEDIA_CODEC_PCM_16_BIT)
            .expect("aligned PCM16 must decode");
        assert_eq!(pcm16, vec![-1.0, 0.0, 32_767.0 / 32_768.0]);

        let expected = [-0.5_f32, 0.25_f32];
        let bytes = expected
            .into_iter()
            .flat_map(f32::to_le_bytes)
            .collect::<Vec<_>>();
        assert_eq!(
            decode_android_pcm(&bytes, MEDIA_CODEC_PCM_FLOAT)
                .expect("aligned float PCM must decode"),
            expected
        );
    }

    #[test]
    fn rejects_partial_pcm_samples() {
        assert!(decode_android_pcm(&[0], MEDIA_CODEC_PCM_16_BIT).is_err());
        assert!(decode_android_pcm(&[0, 0, 0], MEDIA_CODEC_PCM_FLOAT).is_err());
    }
}
