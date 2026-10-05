//! GPU helpers shared by the crate's tests.

use std::sync::Arc;
use std::time::Duration;

use crate::FrameConverter;

/// A device with the frame converter's features plus `extra_features`.
///
/// # Panics
///
/// Panics when the host has no GPU adapter offering them.
pub fn gpu(extra_features: wgpu::Features) -> (Arc<wgpu::Device>, Arc<wgpu::Queue>) {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
        apply_limit_buckets: false,
    }))
    .expect("the camera GPU tests need a GPU adapter");
    assert!(
        adapter.features().contains(extra_features),
        "the adapter lacks {extra_features:?}"
    );
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        required_features: FrameConverter::required_features(adapter.features()) | extra_features,
        ..Default::default()
    }))
    .expect("the camera GPU tests need a GPU device");
    (Arc::new(device), Arc::new(queue))
}

/// Waits until the GPU has finished every submission so far, which also runs
/// the queue's work-done callbacks.
pub fn wait_idle(device: &wgpu::Device) {
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(Duration::from_secs(30)),
        })
        .expect("the GPU finishes the submitted work");
}

/// Copies `texture` back to the CPU as tightly packed rows.
pub fn read_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
) -> Vec<u8> {
    let size = texture.size();
    let texel_bytes = texture
        .format()
        .block_copy_size(None)
        .expect("readable textures have a single aspect");
    let row_bytes = size.width * texel_bytes;
    let padded_row = row_bytes.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("camera test readback"),
        size: u64::from(padded_row * size.height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded_row),
                rows_per_image: Some(size.height),
            },
        },
        size,
    );
    queue.submit([encoder.finish()]);
    buffer.slice(..).map_async(wgpu::MapMode::Read, |result| {
        result.expect("mapping the readback buffer");
    });
    wait_idle(device);
    let mapped = buffer
        .slice(..)
        .get_mapped_range()
        .expect("the readback buffer is mapped");
    mapped
        .chunks(padded_row as usize)
        .flat_map(|row| &row[..row_bytes as usize])
        .copied()
        .collect()
}
