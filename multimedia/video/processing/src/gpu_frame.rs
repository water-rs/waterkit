use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};

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
/// Frames acquired from a [`GpuTexturePool`] return their allocation to that
/// pool's channel when dropped. There is no global
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

/// Channel-backed pool of recyclable processed-frame textures.
#[derive(Debug)]
pub struct GpuTexturePool {
    available: Receiver<TextureStorage>,
    recycler: Sender<TextureStorage>,
}

impl GpuTexturePool {
    /// Creates an empty pool.
    #[must_use]
    pub fn new() -> Self {
        let (recycler, available) = mpsc::channel();
        Self {
            available,
            recycler,
        }
    }

    /// Returns a recycled texture matching the request, or allocates one.
    ///
    /// # Panics
    ///
    /// Panics if the recycler channel is disconnected while the pool lives.
    #[must_use]
    pub fn acquire(
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

impl Default for GpuTexturePool {
    fn default() -> Self {
        Self::new()
    }
}
