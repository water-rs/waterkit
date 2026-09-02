//! Typed, UI-independent video frame processing.
//!
//! Processing stages receive explicit media timing. Animated effects therefore
//! follow presentation time and remain deterministic during pause, seek,
//! offline export, frame stepping, and playback-rate changes.

#![warn(missing_docs)]

#[cfg(feature = "filtrate")]
mod gpu_effect;

#[cfg(feature = "filtrate")]
pub use gpu_effect::{GpuEffectProcessor, GpuTextureFrame};

use std::future::Future;

use waterkit_video_core::{Error, FrameTiming};

/// A frame payload paired with deterministic media timing.
#[derive(Debug)]
pub struct TimedFrame<Frame> {
    frame: Frame,
    timing: FrameTiming,
}

impl<Frame> TimedFrame<Frame> {
    /// Creates a timed frame.
    #[must_use]
    pub const fn new(frame: Frame, timing: FrameTiming) -> Self {
        Self { frame, timing }
    }

    /// Returns the frame payload.
    #[must_use]
    pub const fn frame(&self) -> &Frame {
        &self.frame
    }

    /// Returns the media timing.
    #[must_use]
    pub const fn timing(&self) -> FrameTiming {
        self.timing
    }

    /// Consumes the wrapper and returns its payload and timing.
    #[must_use]
    pub fn into_parts(self) -> (Frame, FrameTiming) {
        (self.frame, self.timing)
    }

    /// Transforms only the payload while preserving media timing.
    #[must_use]
    pub fn map<Output>(self, map: impl FnOnce(Frame) -> Output) -> TimedFrame<Output> {
        TimedFrame::new(map(self.frame), self.timing)
    }
}

/// One typed asynchronous frame-processing stage.
pub trait FrameProcessor<Input> {
    /// Output frame payload.
    type Output;

    /// Processes one frame without changing its timeline identity.
    fn process(
        &mut self,
        input: TimedFrame<Input>,
    ) -> impl Future<Output = Result<TimedFrame<Self::Output>, Error>>;

    /// Composes this stage with a following typed stage.
    fn then<Next>(self, next: Next) -> Then<Self, Next>
    where
        Self: Sized,
        Next: FrameProcessor<Self::Output>,
    {
        Then { first: self, next }
    }
}

/// Two statically composed processing stages.
#[derive(Debug, Clone, Copy)]
pub struct Then<First, Next> {
    first: First,
    next: Next,
}

impl<Input, First, Next> FrameProcessor<Input> for Then<First, Next>
where
    First: FrameProcessor<Input>,
    Next: FrameProcessor<First::Output>,
{
    type Output = Next::Output;

    async fn process(
        &mut self,
        input: TimedFrame<Input>,
    ) -> Result<TimedFrame<Self::Output>, Error> {
        let intermediate = self.first.process(input).await?;
        self.next.process(intermediate).await
    }
}

/// Zero-cost stage that preserves a frame unchanged.
#[derive(Debug, Clone, Copy, Default)]
pub struct Identity;

impl<Frame> FrameProcessor<Frame> for Identity {
    type Output = Frame;

    fn process(
        &mut self,
        input: TimedFrame<Frame>,
    ) -> impl Future<Output = Result<TimedFrame<Frame>, Error>> {
        core::future::ready(Ok(input))
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use waterkit_video_core::{Error, FrameTiming};

    use super::{FrameProcessor, Identity, TimedFrame};

    #[derive(Debug, Clone, Copy)]
    struct Add(u32);

    impl FrameProcessor<u32> for Add {
        type Output = u32;

        async fn process(&mut self, input: TimedFrame<u32>) -> Result<TimedFrame<u32>, Error> {
            Ok(input.map(|value| value + self.0))
        }
    }

    #[test]
    fn typed_composition_preserves_media_timing() {
        let timing = FrameTiming::new(Duration::from_secs(5), Duration::from_millis(20), 250);
        let mut processor = Identity.then(Add(2)).then(Add(3));
        let output = futures_lite::future::block_on(processor.process(TimedFrame::new(10, timing)))
            .expect("processing chain must succeed");

        assert_eq!(*output.frame(), 15);
        assert_eq!(output.timing(), timing);
    }
}
