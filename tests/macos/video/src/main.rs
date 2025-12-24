//! Video recording and playback test.
//!
//! 1. Record screen for 10 seconds → H.265 → MOV
//! 2. Read MOV and playback in winit window

use std::sync::Arc;
use std::time::{Duration, Instant};
use waterkit_codec::sys::{AppleDecoder, AppleEncoder, IOSurfaceFrame};
use waterkit_codec::CodecType;
use waterkit_screen::SCKCapturer;
use waterkit_video::{VideoReader, VideoWriter};
use metal::{DeviceRef, MTLPixelFormat, MTLStorageMode, MTLTexture, MTLTextureType, MTLTextureUsage, Texture, TextureDescriptor};
use objc::runtime::Object;
use objc::msg_send;
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

const RECORD_DURATION_SECS: u64 = 10;
const TARGET_FPS: u32 = 60; // Attempt 60fps for cleaner playback

fn main() {
    println!("=== Video Recording & Playback Test ===\n");

    // Step 1: Record screen to MOV
    let mov_path = "/tmp/screen_recording.mov";
    record_screen(mov_path, RECORD_DURATION_SECS);

    // Step 2: Playback in winit window
    playback_video(mov_path);
}

fn record_screen(output_path: &str, duration_secs: u64) {
    println!("Step 1: Recording screen for {} seconds...", duration_secs);

    // Initialize screen capture
    let capturer = match SCKCapturer::new() {
        Some(c) => c,
        None => {
            eprintln!("Failed to initialize ScreenCaptureKit");
            return;
        }
    };
    capturer.set_raw_frames_enabled(false);

    // Wait for capture to start
    std::thread::sleep(Duration::from_millis(500));

    // Get dimensions from first frame
    let frame = loop {
        if let Some(f) = capturer.get_frame().filter(|f| f.width > 0 && f.height > 0) {
            break f;
        }
        std::thread::sleep(Duration::from_millis(100));
    };

    let width = frame.width;
    let height = frame.height;
    println!("Capture dimensions: {}x{}", width, height);

    // Initialize H.265 encoder
    let mut encoder = match AppleEncoder::with_size(CodecType::H265, width, height) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("Failed to create encoder: {:?}", e);
            return;
        }
    };

    // Initialize video writer
    let mut writer = match VideoWriter::new(
        output_path,
        width,
        height,
        30,
        waterkit_video::CodecType::H265,
    ) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("Failed to create video writer: {:?}", e);
            return;
        }
    };

    // Record frames
    let start = Instant::now();
    let frame_interval = Duration::from_secs_f64(1.0 / TARGET_FPS as f64);
    let mut frame_count = 0u64;
    let mut last_frame = Instant::now();

    while start.elapsed() < Duration::from_secs(duration_secs) {
        let now = Instant::now();
        if now.duration_since(last_frame) < frame_interval {
             std::thread::sleep(Duration::from_millis(1)); // Sleep briefly
             continue;
        }
        last_frame = now;

        // Get IOSurface pointer for zero-copy encoding
        if let Some(iosurface_ptr) = capturer.iosurface_ptr() {
            // Zero-copy encode directly from IOSurface
            match encoder.encode_iosurface(iosurface_ptr) {
                Ok(encoded) => {
                    if !encoded.is_empty() {
                        // Capture codec config if available and not yet set
                        if let Some(config) = encoder.get_codec_config() {
                            writer.set_codec_config(config);
                        }

                        let is_keyframe = frame_count % TARGET_FPS as u64 == 0;
                        if let Err(e) = writer.write_sample(&encoded, is_keyframe) {
                            eprintln!("Failed to write sample: {:?}", e);
                        }
                        frame_count += 1;
                        
                        if frame_count % 60 == 0 {
                            let elapsed_secs = start.elapsed().as_secs_f64();
                            println!("  {} frames ({:.1}s) [zero-copy]", frame_count, elapsed_secs);
                        }
                    }
                }
                Err(e) => {
                    eprintln!("Encode error: {:?}", e);
                }
            }
        }
    }

    // Finish
    if let Err(e) = writer.finish() {
        eprintln!("Failed to finish video: {:?}", e);
    }
    println!("Saved to: {}\n", output_path);
}

struct WgpuState {
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    render_pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    config: wgpu::SurfaceConfiguration,
}

struct VideoPlayer {
    path: String,
    window: Option<Arc<Window>>,
    wgpu_state: Option<WgpuState>,
    reader: VideoReader,
    decoder: Option<AppleDecoder>,
    current_frame: Option<GpuFrame>,
    start_time: Option<Instant>,
    frame_count: usize,
    last_frame_time: Option<Instant>,
    decoded_frames_total: u64,
    render_frames_total: u64,
    stats_start: Instant,
    last_title_update: Instant,
    last_decoded_len: usize,
    loop_count: u32,
}

struct GpuFrame {
    texture: wgpu::Texture,
    bind_group: wgpu::BindGroup,
    _surface: IOSurfaceFrame,
}

impl VideoPlayer {
    fn new(path: String) -> Self {
        let reader = VideoReader::open(&path).expect("Failed to open video");
        println!("Opened video with {} samples", reader.sample_count());
        Self {
            path,
            window: None,
            wgpu_state: None,
            reader,
            decoder: None,
            current_frame: None,
            start_time: None,
            frame_count: 0,
            last_frame_time: None,
            decoded_frames_total: 0,
            render_frames_total: 0,
            stats_start: Instant::now(),
            last_title_update: Instant::now(),
            last_decoded_len: 0,
            loop_count: 0,
        }
    }

    fn metal_device(state: &WgpuState) -> metal::Device {
        let mut device_out: Option<metal::Device> = None;
        unsafe {
            state.device.as_hal::<wgpu::hal::api::Metal, _, _>(|hal_device| {
                if let Some(hal_device) = hal_device {
                    device_out = Some(hal_device.raw_device().lock().clone());
                }
            });
        }
        device_out.expect("Metal device unavailable")
    }

    fn metal_texture_from_iosurface(device: &metal::Device, frame: &IOSurfaceFrame) -> Texture {
        let desc = TextureDescriptor::new();
        desc.set_texture_type(MTLTextureType::D2);
        desc.set_pixel_format(MTLPixelFormat::BGRA8Unorm);
        desc.set_width(frame.width as u64);
        desc.set_height(frame.height as u64);
        desc.set_mipmap_level_count(1);
        desc.set_usage(MTLTextureUsage::ShaderRead);
        desc.set_storage_mode(MTLStorageMode::Shared);

        let surface_ptr = frame.iosurface_ptr() as *mut Object;
        let device_ref: &metal::DeviceRef = device.as_ref();
        let raw: *mut MTLTexture = unsafe {
            msg_send![device_ref, newTextureWithDescriptor: desc iosurface: surface_ptr plane: 0]
        };
        if raw.is_null() {
            panic!("Failed to create Metal texture from IOSurface");
        }
        unsafe { Texture::from_ptr(raw) }
    }

    fn create_gpu_frame(state: &WgpuState, frame: IOSurfaceFrame) -> GpuFrame {
        let metal_device = Self::metal_device(state);
        let metal_texture = Self::metal_texture_from_iosurface(&metal_device, &frame);

        let size = wgpu::Extent3d {
            width: frame.width,
            height: frame.height,
            depth_or_array_layers: 1,
        };
        let desc = wgpu::TextureDescriptor {
            label: Some("Video IOSurface"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Bgra8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        };

        let hal_texture = unsafe {
            wgpu::hal::metal::Device::texture_from_raw(
                metal_texture,
                desc.format,
                MTLTextureType::D2,
                1,
                1,
                wgpu::hal::CopyExtent {
                    width: frame.width,
                    height: frame.height,
                    depth: 1,
                },
            )
        };

        let texture = unsafe {
            state.device.create_texture_from_hal::<wgpu::hal::api::Metal>(hal_texture, &desc)
        };
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = state.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Video Bind Group"),
            layout: &state.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&state.sampler),
                },
            ],
        });

        GpuFrame {
            texture,
            bind_group,
            _surface: frame,
        }
    }

    async fn init_wgpu(&mut self, window: Arc<Window>) {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let surface = instance.create_surface(window.clone()).unwrap();
        let adapter = instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
        }).await.unwrap();

        let (device, queue) = adapter.request_device(
            &wgpu::DeviceDescriptor::default(), None,
        ).await.unwrap();

        let size = window.inner_size();
        let mut config = surface.get_default_config(&adapter, size.width, size.height).unwrap();
        // Use sRGB format for correct color display
        config.format = wgpu::TextureFormat::Bgra8UnormSrgb;
        surface.configure(&device, &config);

        // Simple Shader (Vertex + Fragment)
        // Draw a full-screen quad (triange strip logic in VS)
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Shader"),
            source: wgpu::ShaderSource::Wgsl(std::borrow::Cow::Borrowed(r#"
                struct VertexOutput {
                    @builtin(position) position: vec4<f32>,
                    @location(0) uv: vec2<f32>,
                };

                @vertex
                fn vs_main(@builtin(vertex_index) in_vertex_index: u32) -> VertexOutput {
                    var out: VertexOutput;
                    // Draw full screen quad using a large triangle
                    let uv = vec2<f32>(
                        f32((in_vertex_index << 1u) & 2u),
                        f32(in_vertex_index & 2u)
                    );
                    out.position = vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
                    // Invert Y for texture sampling if needed (wgpu matches metal/vulkan)
                    out.uv = vec2<f32>(uv.x, 1.0 - uv.y); 
                    return out;
                }

                @group(0) @binding(0) var t_diffuse: texture_2d<f32>;
                @group(0) @binding(1) var s_diffuse: sampler;

                @fragment
                fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
                    return textureSample(t_diffuse, s_diffuse, in.uv);
                }
            "#)),
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Bind Group Layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Pipeline Layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        let render_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Render Pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: config.format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
             cache: None,
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        self.wgpu_state = Some(WgpuState {
            device,
            queue,
            surface,
            render_pipeline,
            bind_group_layout,
            sampler,
            config,
        });
        self.window = Some(window);
        
        // Initialize decoder now that we have config from reader (opened in new)
        let config = self.reader.codec_config();
        if let Some(config_bytes) = config {
             let (width, height) = self.reader.dimensions();
             println!("Initializing AppleDecoder with {} bytes config ({}x{}): {:02X?}", config_bytes.len(), width, height, config_bytes);
             self.decoder = Some(AppleDecoder::new_zero_copy(CodecType::H265, Some(config_bytes), width, height).expect("Failed to init decoder"));
        } else {
             panic!("No config in MOV file!");
        }
        
        self.start_time = Some(Instant::now());
    }
}

impl ApplicationHandler for VideoPlayer {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let (width, height) = self.reader.dimensions();
        let window = Arc::new(event_loop.create_window(
            Window::default_attributes()
                .with_title(format!("Video Playback - {}x{}", width, height))
                .with_inner_size(winit::dpi::PhysicalSize::new(width, height))
        ).unwrap());
        
        // Block on async init
        pollster::block_on(self.init_wgpu(window));
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _window_id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::RedrawRequested => {
                if let Some(state) = &mut self.wgpu_state {
                    // Frame rate control: only process next frame every ~33ms (30fps)
                    let should_decode = match self.last_frame_time {
                        None => true, // First frame
                        Some(last) => last.elapsed() >= std::time::Duration::from_millis(33),
                    };
                    
                    // Read and Decode (only if enough time has passed)
                    if should_decode {
                        if let Some(decoder) = &mut self.decoder {
                            // Read sample
                            if let Some((sample_data, pts, _key)) = self.reader.read_sample() {
                                self.last_frame_time = Some(Instant::now());
                                self.frame_count += 1;
                                if self.frame_count % 30 == 0 {
                                    println!("Playing frame {}", self.frame_count);
                                }
                                
                                // Decode - frames returned from previous callback (IOSurface zero-copy)
                                let timescale = self.reader.timescale();
                                match decoder.decode_surface(&sample_data, pts, timescale) {
                                    Ok(mut frames) => {
                                        if self.frame_count % 30 == 0 {
                                            println!("Frame {}: decoded, got {} frames", self.frame_count, frames.len());
                                        }
                                        self.last_decoded_len = frames.len();
                                        if let Some(frame) = frames.pop() {
                                            self.decoded_frames_total += 1;
                                            self.current_frame = Some(Self::create_gpu_frame(state, frame));
                                        }
                                    }
                                    Err(e) => {
                                        eprintln!("Decode error frame {}: {:?}", self.frame_count, e);
                                    }
                                }
                            } else {
                                // End of stream - loop back to start
                                println!("End of stream - looping");
                                self.reader.reset();
                                self.last_frame_time = None;
                                self.frame_count = 0;
                                self.loop_count = self.loop_count.saturating_add(1);
                            }
                            // Note: no sleep - event loop with ControlFlow::Poll handles timing
                        }
                    } // end if should_decode
                    
                    // Render
                    let output = state.surface.get_current_texture().unwrap();
                    let view = output.texture.create_view(&wgpu::TextureViewDescriptor::default());
                    let mut encoder = state.device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
                    
                    {
                        let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                            label: Some("Render Pass"),
                            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                                view: &view,
                                resolve_target: None,
                                ops: wgpu::Operations {
                                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                                    store: wgpu::StoreOp::Store,
                                },
                            })],
                            depth_stencil_attachment: None,
                             timestamp_writes: None,
                             occlusion_query_set: None,
                        });
                        
                        rpass.set_pipeline(&state.render_pipeline);
                        if let Some(frame) = &self.current_frame {
                             rpass.set_bind_group(0, &frame.bind_group, &[]);
                             rpass.draw(0..3, 0..1); // Full screen triangle
                        }
                    }
                    
                    state.queue.submit(Some(encoder.finish()));
                    output.present();
                    self.render_frames_total += 1;

                    if self.last_title_update.elapsed() >= Duration::from_millis(500) {
                        let elapsed = self.stats_start.elapsed().as_secs_f64().max(0.001);
                        let decode_fps = self.decoded_frames_total as f64 / elapsed;
                        let render_fps = self.render_frames_total as f64 / elapsed;
                        let (width, height) = self.reader.dimensions();
                        let title = format!(
                            "Video Playback - {}x{} | frame {} | decoded {} | fps d/r {:.1}/{:.1} | loops {}",
                            width,
                            height,
                            self.frame_count,
                            self.last_decoded_len,
                            decode_fps,
                            render_fps,
                            self.loop_count
                        );
                        if let Some(window) = &self.window {
                            window.set_title(&title);
                        }
                        self.last_title_update = Instant::now();
                    }
                    
                    // Request next frame immediately for benchmark
                    self.window.as_ref().unwrap().request_redraw();
                }
            }
            _ => {}
        }
    }
}

fn playback_video(path: &str) {
    let event_loop = EventLoop::new().unwrap();
    event_loop.set_control_flow(ControlFlow::Poll);
    
    let mut app = VideoPlayer::new(path.to_string());
    event_loop.run_app(&mut app).unwrap();
}
