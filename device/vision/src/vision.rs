use std::{future::Future, sync::Arc};

use crate::{
    Image, Request, VisionCapabilities, VisionError,
    capability::{ENABLED, uncarried},
    sealed::{Context, Pass, Plan},
};

/// Policy for choosing between native and portable realizations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Policy {
    /// Use native when it serves the request, otherwise use portable.
    #[default]
    PreferNative,
    /// Always use portable.
    PortableOnly,
}

/// A vision request planner and runner using the application's GPU device.
#[derive(Debug)]
pub struct Vision {
    pub(crate) device: Arc<wgpu::Device>,
    pub(crate) queue: Arc<wgpu::Queue>,
    pub(crate) policy: Policy,
    capabilities: VisionCapabilities,
}

impl Vision {
    /// Creates a vision engine that prefers native realizations.
    #[must_use]
    pub fn new(device: Arc<wgpu::Device>, queue: Arc<wgpu::Queue>) -> Self {
        Self::with_policy(device, queue, Policy::PreferNative)
    }

    /// Creates a vision engine with an explicit realization policy.
    ///
    /// # Panics
    ///
    /// Under [`Policy::PortableOnly`], panics when the application carries no
    /// portable realization for an enabled capability. This packaging error
    /// is fixed in `Water.toml`; the message names each capability.
    #[must_use]
    pub fn with_policy(device: Arc<wgpu::Device>, queue: Arc<wgpu::Queue>, policy: Policy) -> Self {
        let missing = uncarried(policy, ENABLED);
        assert!(
            missing.is_empty(),
            "PortableOnly requires portable realizations for: {}",
            missing.join(", ")
        );
        tracing::debug!(?policy, "vision policy configured");
        Self {
            device,
            queue,
            policy,
            capabilities: VisionCapabilities::new(),
        }
    }

    /// Capabilities compiled into this build and served on this device, as
    /// observed at construction.
    ///
    /// `barcodes.native` is `DetectBarcodesRequest.supportedSymbologies` and
    /// `text.native` the per-level `supportedRecognitionLanguages`
    /// intersection on Apple; `text.native` is
    /// `OcrEngine::AvailableRecognizerLanguages` on Windows. What they lack
    /// is served by the portable realization when the application carries
    /// one.
    #[must_use]
    pub fn capabilities(&self) -> VisionCapabilities {
        self.capabilities.clone()
    }

    /// Prepares the selected realization's requirements without processing an
    /// image.
    ///
    /// # Errors
    ///
    /// Returns [`VisionError::Unsupported`] if no realization serves the
    /// request, or a preparation error from its selected realization.
    #[cfg_attr(
        target_arch = "wasm32",
        expect(
            clippy::future_not_send,
            reason = "on wasm32 wgpu devices, queues and textures are not `Send`, so neither is a future holding them"
        )
    )]
    pub fn prepare<R: Request>(
        &self,
        request: &R,
    ) -> impl Future<Output = Result<(), VisionError>> + wgpu::WasmNotSend + '_ {
        let plan = request.plan(Context::new(self));
        async move { plan?.prepare(Context::new(self)).await }
    }

    /// Selects and runs a request on `image`.
    ///
    /// Planning and image-handle sharing happen synchronously; the returned
    /// future borrows only this vision engine.
    ///
    /// # Errors
    ///
    /// Returns [`VisionError::Unsupported`] if no realization serves the
    /// request, or the selected realization's execution error.
    #[cfg_attr(
        target_arch = "wasm32",
        expect(
            clippy::future_not_send,
            reason = "on wasm32 wgpu devices, queues and textures are not `Send`, so neither is a future holding them"
        )
    )]
    pub fn perform<R: Request>(
        &self,
        image: &Image,
        request: &R,
    ) -> impl Future<Output = Result<R::Output, VisionError>> + wgpu::WasmNotSend + '_ {
        let plan = request.plan(Context::new(self));
        let image = image.share();
        async move {
            let plan = plan?;
            let mut pass = Pass::new(Context::new(self), &image);
            plan.run(&mut pass).await
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        future::Future,
        sync::atomic::{AtomicUsize, Ordering},
        task::{Context as TaskContext, Poll, Waker},
    };

    use crate::{
        Image, Orientation, Policy, Request, Vision, VisionError,
        sealed::{Context, Offer, Plan, Preparation, Realization, Sealed},
        test_support::gpu,
    };

    static PREPARATIONS: [AtomicUsize; 16] = [const { AtomicUsize::new(0) }; 16];
    static PLAN_PREPARATIONS: [AtomicUsize; 16] = [const { AtomicUsize::new(0) }; 16];
    static PORTABLE_RUNS: [AtomicUsize; 16] = [const { AtomicUsize::new(0) }; 16];
    static RENDEZVOUS_PREPARATIONS: [AtomicUsize; 16] = [const { AtomicUsize::new(0) }; 16];

    #[derive(Debug)]
    struct Shared<const ID: usize>;

    impl<const ID: usize> Preparation for Shared<ID> {
        #[cfg_attr(
            target_arch = "wasm32",
            expect(
                clippy::future_not_send,
                reason = "on wasm32 wgpu devices, queues and textures are not `Send`, so neither is a future holding them"
            )
        )]
        async fn prepare(
            _context: Context<'_>,
            _pixels: &crate::image::Pixels,
        ) -> Result<Self, VisionError> {
            PREPARATIONS[ID].fetch_add(1, Ordering::SeqCst);
            Ok(Self)
        }
    }

    #[derive(Debug)]
    struct Echo<const ID: usize> {
        name: &'static str,
        native: Offer,
        portable: Offer,
        fail_native: bool,
        rendezvous: bool,
    }

    #[derive(Debug)]
    struct EchoPlan<const ID: usize> {
        name: &'static str,
        realization: Realization,
        fail_native: bool,
        rendezvous: bool,
    }

    impl<const ID: usize> Request for Echo<ID> {
        type Output = (&'static str, Realization);
    }

    impl<const ID: usize> Sealed for Echo<ID> {
        type Plan = EchoPlan<ID>;

        fn plan(&self, context: Context<'_>) -> Result<Self::Plan, VisionError> {
            Ok(EchoPlan {
                name: self.name,
                realization: context.select(self.name, &self.native, &self.portable)?,
                fail_native: self.fail_native,
                rendezvous: self.rendezvous,
            })
        }
    }

    impl<const ID: usize> Plan<Echo<ID>> for EchoPlan<ID> {
        #[cfg_attr(
            target_arch = "wasm32",
            expect(
                clippy::future_not_send,
                reason = "on wasm32 wgpu devices, queues and textures are not `Send`, so neither is a future holding them"
            )
        )]
        async fn prepare(&self, _context: Context<'_>) -> Result<(), VisionError> {
            PLAN_PREPARATIONS[ID].fetch_add(1, Ordering::SeqCst);
            if self.rendezvous {
                RENDEZVOUS_PREPARATIONS[ID].fetch_add(1, Ordering::SeqCst);
                futures::future::poll_fn(|context| {
                    if RENDEZVOUS_PREPARATIONS[ID].load(Ordering::SeqCst) >= 2 {
                        Poll::Ready(())
                    } else {
                        context.waker().wake_by_ref();
                        Poll::Pending
                    }
                })
                .await;
            }
            Ok(())
        }

        #[cfg_attr(
            target_arch = "wasm32",
            expect(
                clippy::future_not_send,
                reason = "on wasm32 wgpu devices, queues and textures are not `Send`, so neither is a future holding them"
            )
        )]
        async fn run(
            self,
            pass: &mut crate::sealed::Pass<'_>,
        ) -> Result<(&'static str, Realization), VisionError> {
            let _shared = pass.prepared::<Shared<ID>>().await?;
            if self.fail_native && self.realization == Realization::Native {
                return Err(VisionError::Platform("native request failed".to_owned()));
            }
            if self.realization == Realization::Portable {
                PORTABLE_RUNS[ID].fetch_add(1, Ordering::SeqCst);
            }
            Ok((self.name, self.realization))
        }
    }

    fn echo<const ID: usize>(name: &'static str, native: Offer, portable: Offer) -> Echo<ID> {
        Echo {
            name,
            native,
            portable,
            fail_native: false,
            rendezvous: false,
        }
    }

    fn test_vision() -> Vision {
        let (device, queue) = gpu();
        Vision::new(device, queue)
    }

    fn test_image(device: &wgpu::Device) -> Image {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("vision request test image"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        Image::from_texture(texture, Orientation::Up)
    }

    #[test]
    fn tuples_preserve_order_and_share_one_preparation_per_pass() {
        let vision = test_vision();
        let image = test_image(&vision.device);
        let pair = (
            echo::<0>("native", Offer::Serves, Offer::Serves),
            echo::<0>("portable", Offer::Absent, Offer::Serves),
        );
        let pair_result = pollster::block_on(vision.perform(&image, &pair)).unwrap();
        assert_eq!(
            pair_result,
            (
                ("native", Realization::Native),
                ("portable", Realization::Portable)
            )
        );
        assert_eq!(PREPARATIONS[0].load(Ordering::SeqCst), 1);

        let nested = (
            echo::<1>("first", Offer::Serves, Offer::Absent),
            (
                echo::<1>("second", Offer::Absent, Offer::Serves),
                echo::<1>("third", Offer::Serves, Offer::Serves),
            ),
        );
        let nested_result = pollster::block_on(vision.perform(&image, &nested)).unwrap();
        assert_eq!(
            nested_result,
            (
                ("first", Realization::Native),
                (
                    ("second", Realization::Portable),
                    ("third", Realization::Native)
                )
            )
        );
        assert_eq!(PREPARATIONS[1].load(Ordering::SeqCst), 1);
    }

    #[test]
    fn an_unserved_tuple_element_prevents_every_run() {
        let vision = test_vision();
        let image = test_image(&vision.device);
        let request = (
            echo::<2>("served", Offer::Serves, Offer::Absent),
            echo::<2>("unserved", Offer::Absent, Offer::Lacks("x".to_owned())),
        );
        let error = pollster::block_on(vision.perform(&image, &request)).unwrap_err();
        assert!(
            matches!(error, VisionError::Unsupported(message) if message.contains("unserved") && message.contains('x'))
        );
        assert_eq!(PREPARATIONS[2].load(Ordering::SeqCst), 0);
        assert_eq!(PORTABLE_RUNS[2].load(Ordering::SeqCst), 0);
    }

    #[cfg(all(feature = "text", not(feature = "barcode")))]
    #[test]
    #[should_panic(expected = "PortableOnly requires portable realizations for: text")]
    fn portable_only_requires_a_carried_portable_text() {
        let (device, queue) = gpu();
        let _ = Vision::with_policy(device, queue, Policy::PortableOnly);
    }

    #[cfg(not(feature = "text"))]
    #[test]
    #[cfg(not(any(feature = "barcode", feature = "text")))]
    fn portable_only_selects_portable_and_can_be_constructed_without_enabled_capabilities() {
        let (device, queue) = gpu();
        let vision = Vision::with_policy(device, queue, Policy::PortableOnly);
        let image = test_image(&vision.device);
        let request = (
            echo::<3>("native-first", Offer::Serves, Offer::Serves),
            echo::<4>("native-only-if-preferred", Offer::Serves, Offer::Serves),
        );
        let result = pollster::block_on(vision.perform(&image, &request)).unwrap();
        assert_eq!(
            result,
            (
                ("native-first", Realization::Portable),
                ("native-only-if-preferred", Realization::Portable)
            )
        );
        assert_eq!(PORTABLE_RUNS[3].load(Ordering::SeqCst), 1);
        assert_eq!(PORTABLE_RUNS[4].load(Ordering::SeqCst), 1);
    }

    #[test]
    #[cfg(any(feature = "barcode", feature = "text"))]
    #[should_panic(expected = "PortableOnly requires portable realizations")]
    fn portable_only_panics_while_an_enabled_capability_is_not_carried() {
        let (device, queue) = gpu();
        let _vision = Vision::with_policy(device, queue, Policy::PortableOnly);
    }

    #[test]
    fn prepare_visits_each_tuple_plan_and_rejects_unserved_requests() {
        let vision = test_vision();
        let request = (
            echo::<5>("first", Offer::Serves, Offer::Absent),
            echo::<6>("second", Offer::Serves, Offer::Absent),
        );
        pollster::block_on(vision.prepare(&request)).unwrap();
        assert_eq!(PLAN_PREPARATIONS[5].load(Ordering::SeqCst), 1);
        assert_eq!(PLAN_PREPARATIONS[6].load(Ordering::SeqCst), 1);

        let unserved = echo::<7>("unserved", Offer::Absent, Offer::Absent);
        let error = pollster::block_on(vision.prepare(&unserved)).unwrap_err();
        assert!(matches!(error, VisionError::Unsupported(_)));
        assert_eq!(PLAN_PREPARATIONS[7].load(Ordering::SeqCst), 0);
    }

    #[test]
    fn tuple_prepares_plans_concurrently() {
        let vision = test_vision();
        let mut first = echo::<10>("first", Offer::Serves, Offer::Absent);
        let mut second = echo::<10>("second", Offer::Serves, Offer::Absent);
        first.rendezvous = true;
        second.rendezvous = true;
        let request = (first, second);

        let mut future = Box::pin(vision.prepare(&request));
        let waker = Waker::noop();
        let mut context = TaskContext::from_waker(waker);
        let result = (0..16).find_map(|_| match future.as_mut().poll(&mut context) {
            Poll::Ready(result) => Some(result),
            Poll::Pending => None,
        });

        assert!(matches!(result, Some(Ok(()))));
        assert_eq!(RENDEZVOUS_PREPARATIONS[10].load(Ordering::SeqCst), 2);
    }

    #[test]
    fn native_execution_failure_does_not_retry_portable() {
        let vision = test_vision();
        let image = test_image(&vision.device);
        let mut request = echo::<8>("native-error", Offer::Serves, Offer::Serves);
        request.fail_native = true;
        let error = pollster::block_on(vision.perform(&image, &request)).unwrap_err();
        assert!(
            matches!(error, VisionError::Platform(message) if message == "native request failed")
        );
        assert_eq!(PORTABLE_RUNS[8].load(Ordering::SeqCst), 0);
    }

    #[test]
    fn public_handles_and_tuple_perform_future_meet_wgpu_send_bounds() {
        fn assert_send_sync<T: wgpu::WasmNotSendSync>() {}
        fn assert_send<T: wgpu::WasmNotSend>(_: &T) {}

        assert_send_sync::<Vision>();
        assert_send_sync::<Image>();
        let vision = test_vision();
        let image = test_image(&vision.device);
        let request = (
            echo::<9>("first", Offer::Serves, Offer::Serves),
            echo::<9>("second", Offer::Serves, Offer::Serves),
        );
        let future = vision.perform(&image, &request);
        assert_send(&future);
    }
}
