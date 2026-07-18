use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};

use filtrate::{
    Effect, EffectContext, EffectFrameTiming, EffectInput, EffectOutput, EffectRedrawCallback,
};
use waterkit_video_core::{Error, FrameTiming};

use crate::{FrameProcessor, TimedFrame};

struct TextureStorage {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
}

impl TextureStorage {
    fn new(texture: wgpu::Texture) -> Self {
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Self { texture, view }
    }

    fn matches(&self, width: u32, height: u32, format: wgpu::TextureFormat) -> bool {
        self.texture.width() == width
            && self.texture.height() == height
            && self.texture.format() == format
    }
}

/// One owned wgpu texture suitable for deterministic video-frame processing.
///
/// Frames created by [`GpuEffectProcessor`] return their allocation to that
/// processor's channel-backed texture pool when dropped. There is no global
/// cache, lock, CPU readback, or implicit device ownership.
pub struct GpuTextureFrame {
    storage: Option<TextureStorage>,
    recycler: Option<Sender<TextureStorage>>,
}

impl GpuTextureFrame {
    /// Wraps an application-owned texture without attaching a recycler.
    #[must_use]
    pub fn new(texture: wgpu::Texture) -> Self {
        Self {
            storage: Some(TextureStorage::new(texture)),
            recycler: None,
        }
    }

    const fn pooled(storage: TextureStorage, recycler: Sender<TextureStorage>) -> Self {
        Self {
            storage: Some(storage),
            recycler: Some(recycler),
        }
    }

    const fn storage(&self) -> &TextureStorage {
        self.storage
            .as_ref()
            .expect("a live GPU texture frame must own its storage")
    }

    /// Returns the underlying GPU texture.
    #[must_use]
    pub const fn texture(&self) -> &wgpu::Texture {
        &self.storage().texture
    }

    /// Returns the persistent default view for the texture.
    #[must_use]
    pub const fn view(&self) -> &wgpu::TextureView {
        &self.storage().view
    }

    /// Returns the texture width in pixels.
    #[must_use]
    pub fn width(&self) -> u32 {
        self.texture().width()
    }

    /// Returns the texture height in pixels.
    #[must_use]
    pub fn height(&self) -> u32 {
        self.texture().height()
    }

    /// Returns the texture format.
    #[must_use]
    pub fn format(&self) -> wgpu::TextureFormat {
        self.texture().format()
    }
}

impl std::fmt::Debug for GpuTextureFrame {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GpuTextureFrame")
            .field("width", &self.width())
            .field("height", &self.height())
            .field("format", &self.format())
            .finish_non_exhaustive()
    }
}

impl Drop for GpuTextureFrame {
    fn drop(&mut self) {
        let Some(storage) = self.storage.take() else {
            return;
        };
        if let Some(recycler) = self.recycler.take() {
            let _ = recycler.send(storage);
        }
    }
}

struct TexturePool {
    available: Receiver<TextureStorage>,
    recycler: Sender<TextureStorage>,
}

impl TexturePool {
    fn new() -> Self {
        let (recycler, available) = mpsc::channel();
        Self {
            available,
            recycler,
        }
    }

    fn acquire(
        &self,
        device: &wgpu::Device,
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
    ) -> GpuTextureFrame {
        loop {
            match self.available.try_recv() {
                Ok(storage) if storage.matches(width, height, format) => {
                    return GpuTextureFrame::pooled(storage, self.recycler.clone());
                }
                Ok(_) => {}
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    panic!("GPU texture recycler disconnected while its owner is alive")
                }
            }
        }

        let storage_usage = if matches!(
            format,
            wgpu::TextureFormat::Rgba8Unorm
                | wgpu::TextureFormat::Rgba16Float
                | wgpu::TextureFormat::Rgba32Float
        ) {
            wgpu::TextureUsages::STORAGE_BINDING
        } else {
            wgpu::TextureUsages::empty()
        };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("waterkit video processed frame"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | storage_usage
                | wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        GpuTextureFrame::pooled(TextureStorage::new(texture), self.recycler.clone())
    }
}

/// A Filtrate effect adapted to `WaterKit`'s typed video processing pipeline.
///
/// The processor submits GPU commands without waiting for completion; wgpu
/// queue ordering keeps the returned texture usable by subsequent GPU stages.
/// Its output allocations are recycled after the corresponding frame drops.
pub struct GpuEffectProcessor<E: Effect> {
    effect: E,
    device: wgpu::Device,
    queue: wgpu::Queue,
    input_format: wgpu::TextureFormat,
    output_format: wgpu::TextureFormat,
    texture_pool: TexturePool,
    needs_redraw: bool,
}

impl<E: Effect> std::fmt::Debug for GpuEffectProcessor<E> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GpuEffectProcessor")
            .field("input_format", &self.input_format)
            .field("output_format", &self.output_format)
            .field("needs_redraw", &self.needs_redraw())
            .finish_non_exhaustive()
    }
}

impl<E: Effect> GpuEffectProcessor<E> {
    /// Initializes an effect for a device and explicit input/output formats.
    ///
    /// `redraw_callback` lets an interactive host wake when a reactive filter
    /// parameter changes. Offline and continuous video pipelines can pass
    /// `None` because the next input frame already drives rendering.
    ///
    /// # Errors
    ///
    /// Returns a processing error when effect setup fails.
    pub async fn new(
        device: wgpu::Device,
        queue: wgpu::Queue,
        mut effect: E,
        input_format: wgpu::TextureFormat,
        output_format: wgpu::TextureFormat,
        redraw_callback: Option<EffectRedrawCallback>,
    ) -> Result<Self, Error> {
        if let Some(callback) = redraw_callback {
            effect.set_redraw_callback(callback);
        }
        effect
            .setup(&EffectContext {
                device: &device,
                queue: &queue,
                input_format,
                output_format,
            })
            .await
            .map_err(|message| Error::Processing(message.to_owned()))?;

        Ok(Self {
            effect,
            device,
            queue,
            input_format,
            output_format,
            texture_pool: TexturePool::new(),
            needs_redraw: false,
        })
    }

    /// Borrows the configured effect.
    #[must_use]
    pub const fn effect(&self) -> &E {
        &self.effect
    }

    /// Mutably borrows the configured effect.
    pub const fn effect_mut(&mut self) -> &mut E {
        &mut self.effect
    }

    /// Returns whether animation or a parameter update requires another frame.
    #[must_use]
    pub fn needs_redraw(&self) -> bool {
        self.needs_redraw || self.effect.redraw_hint()
    }
}

impl<E: Effect> FrameProcessor<GpuTextureFrame> for GpuEffectProcessor<E> {
    type Output = GpuTextureFrame;

    async fn process(
        &mut self,
        input: TimedFrame<GpuTextureFrame>,
    ) -> Result<TimedFrame<Self::Output>, Error> {
        let (input, timing) = input.into_parts();
        if input.format() != self.input_format {
            return Err(Error::Processing(format!(
                "Filtrate processor expected {:?} input but received {:?}",
                self.input_format,
                input.format()
            )));
        }

        let (output_width, output_height) = self.effect.output_size(input.width(), input.height());
        if output_width == 0 || output_height == 0 {
            return Err(Error::Processing(format!(
                "Filtrate effect produced invalid {output_width}x{output_height} output dimensions"
            )));
        }
        let output = self.texture_pool.acquire(
            &self.device,
            output_width,
            output_height,
            self.output_format,
        );
        let effect_timing = effect_timing(timing);
        let effect_input = EffectInput {
            device: &self.device,
            queue: &self.queue,
            texture: input.texture(),
            view: input.view().clone(),
            format: input.format(),
            width: input.width(),
            height: input.height(),
            timing: effect_timing,
        };
        let effect_output = EffectOutput {
            device: &self.device,
            queue: &self.queue,
            texture: output.texture(),
            view: output.view().clone(),
            format: output.format(),
            width: output.width(),
            height: output.height(),
        };
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("waterkit video Filtrate frame"),
            });
        self.needs_redraw = self
            .effect
            .encode_render(&effect_input, &effect_output, &mut encoder)
            .map_err(|message| Error::Processing(message.to_owned()))?;
        self.queue.submit([encoder.finish()]);
        Ok(TimedFrame::new(output, timing))
    }
}

const fn effect_timing(timing: FrameTiming) -> EffectFrameTiming {
    EffectFrameTiming::new(
        timing.presentation_time(),
        timing.duration(),
        timing.sequence(),
    )
    .with_discontinuity(timing.is_discontinuity())
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use filtrate::{FilterAdapter, filters::Brightness};
    use waterkit_video_core::FrameTiming;

    use crate::{FrameProcessor, TimedFrame};

    use super::{GpuEffectProcessor, GpuTextureFrame, effect_timing};

    #[test]
    fn filtrate_timing_uses_media_time_without_wall_clock_sampling() {
        let media = FrameTiming::new(Duration::from_secs(7), Duration::from_millis(16), 420)
            .with_discontinuity(true);
        let effect = effect_timing(media);

        assert_eq!(effect.presentation_time(), Duration::from_secs(7));
        assert_eq!(effect.delta(), Duration::from_millis(16));
        assert_eq!(effect.sequence(), 420);
        assert!(effect.is_discontinuity());
    }

    #[test]
    fn gpu_effect_processor_preserves_frame_dimensions_and_timing() {
        futures_lite::future::block_on(async {
            let instance =
                wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
            let adapter = instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::HighPerformance,
                    compatible_surface: None,
                    force_fallback_adapter: false,
                })
                .await
                .expect("WaterKit Filtrate test requires a GPU adapter");
            let (device, queue) = adapter
                .request_device(&wgpu::DeviceDescriptor::default())
                .await
                .expect("WaterKit Filtrate test requires a working GPU device");
            let format = wgpu::TextureFormat::Rgba8Unorm;
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("WaterKit Filtrate test input"),
                size: wgpu::Extent3d {
                    width: 8,
                    height: 6,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let timing = FrameTiming::new(Duration::from_secs(2), Duration::from_millis(16), 120);
            let mut processor = GpuEffectProcessor::new(
                device.clone(),
                queue,
                FilterAdapter::new(Brightness(0.25_f32)),
                format,
                format,
                None,
            )
            .await
            .expect("Filtrate processor setup must succeed");
            let output = processor
                .process(TimedFrame::new(GpuTextureFrame::new(texture), timing))
                .await
                .expect("Filtrate frame processing must succeed");

            assert_eq!(output.frame().width(), 8);
            assert_eq!(output.frame().height(), 6);
            assert_eq!(output.frame().format(), format);
            assert_eq!(output.timing(), timing);
            let _ = device.poll(wgpu::PollType::wait_indefinitely());
        });
    }
}
