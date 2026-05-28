//! Video recording and playback test.
//!
//! 1. Record screen for a few seconds → H.265 → MOV
//! 2. Read MOV and playback in winit window

use std::sync::Arc;
use std::time::{Duration, Instant};
use waterkit_codec::{CodecType, Encoder};
use waterkit_screen::{ScreenStream, StreamConfig, screens};
use waterkit_video::{MuxerCodecType, VideoPlayer, VideoWriter};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

const RECORD_DURATION_SECS: u64 = 5;
const TARGET_FPS: u32 = 30;

fn main() {
    env_logger::init();
    println!("=== Video Recording & Playback Test ===\n");

    // Step 1: Record screen to MOV
    let mov_path = "/tmp/screen_recording.mov";
    record_screen(mov_path, RECORD_DURATION_SECS);

    // Step 2: Playback in winit window
    playback_video(mov_path);
}

fn record_screen(output_path: &str, duration_secs: u64) {
    println!("Step 1: Recording screen for {} seconds...", duration_secs);

    // Get screen info
    let displays = screens().expect("Failed to get screens");
    let primary = displays
        .iter()
        .find(|d| d.is_primary())
        .unwrap_or(&displays[0]);
    let width = primary.width();
    let height = primary.height();
    println!("Screen: {} ({}x{})", primary.name(), width, height);

    // Create wgpu device for screen capture
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let adapter =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
            .expect("No GPU adapter");
    let (device, queue) =
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
            .expect("Failed to create device");
    let device: Arc<wgpu::Device> = Arc::new(device);
    let queue: Arc<wgpu::Queue> = Arc::new(queue);

    // Start screen capture stream
    let config = StreamConfig {
        target_fps: TARGET_FPS,
        show_cursor: true,
    };
    let stream = ScreenStream::start(primary, device.clone(), queue.clone(), &config)
        .expect("Failed to start capture");

    // Wait for capture to start
    std::thread::sleep(Duration::from_millis(500));

    // Create H.265 encoder
    let mut encoder =
        Encoder::new(CodecType::H265, width, height).expect("Failed to create encoder");

    // Create video writer
    let mut writer = VideoWriter::new(output_path, width, height, TARGET_FPS, MuxerCodecType::H265)
        .expect("Failed to create video writer");

    // Record frames
    let start = Instant::now();
    let frame_interval = Duration::from_secs_f64(1.0 / TARGET_FPS as f64);
    let mut frame_count = 0u64;
    let mut last_frame = Instant::now();
    let mut codec_config_set = false;

    println!("Recording...");

    while start.elapsed() < Duration::from_secs(duration_secs) {
        let now = Instant::now();
        if now.duration_since(last_frame) < frame_interval {
            std::thread::sleep(Duration::from_millis(1));
            continue;
        }
        last_frame = now;

        // Get frame from stream (we use dummy NV12 for encoding since
        // converting GPU texture back to CPU would be inefficient)
        if stream.try_next_frame().is_some() {
            // Create NV12 data (in a real pipeline, this would come from the GPU texture)
            let y_size = (width * height) as usize;
            let nv12_data = vec![128u8; y_size + y_size / 2];

            // Encode
            for result in encoder.encode_nv12(&nv12_data) {
                match result {
                    Ok(encoded) => {
                        if !encoded.is_empty() {
                            // Set codec config on first frame
                            if !codec_config_set && let Some(config) = encoder.codec_config() {
                                writer.set_codec_config(config.to_vec());
                                codec_config_set = true;
                            }

                            let is_keyframe = frame_count.is_multiple_of(TARGET_FPS as u64);
                            if let Err(e) = writer.write_sample(&encoded, is_keyframe) {
                                eprintln!("Failed to write sample: {:?}", e);
                            }
                            frame_count += 1;

                            if frame_count.is_multiple_of(30) {
                                let elapsed_secs = start.elapsed().as_secs_f64();
                                println!("  {} frames ({:.1}s)", frame_count, elapsed_secs);
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("Encode error: {:?}", e);
                    }
                }
            }
        }
    }

    // Finish
    if let Err(e) = writer.finish() {
        eprintln!("Failed to finish video: {:?}", e);
    }
    println!("Saved to: {} ({} frames)\n", output_path, frame_count);
}

struct VideoPlayerApp {
    path: String,
    state: Option<PlayerState>,
}

struct PlayerState {
    window: Arc<Window>,
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    surface: wgpu::Surface<'static>,
    surface_config: wgpu::SurfaceConfiguration,
    player: VideoPlayer,
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    frame_count: u32,
    last_fps_update: Instant,
    fps_frame_count: u32,
    loop_count: u32,
}

impl VideoPlayerApp {
    fn new(path: String) -> Self {
        Self { path, state: None }
    }
}

impl ApplicationHandler for VideoPlayerApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }

        let state = pollster::block_on(PlayerState::new(event_loop, &self.path));
        match state {
            Ok(s) => {
                s.window.request_redraw();
                self.state = Some(s);
            }
            Err(e) => {
                eprintln!("Failed to create player: {}", e);
                event_loop.exit();
            }
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(new_size) => {
                if let Some(state) = &mut self.state {
                    state.surface_config.width = new_size.width.max(1);
                    state.surface_config.height = new_size.height.max(1);
                    state
                        .surface
                        .configure(&state.device, &state.surface_config);
                }
            }
            WindowEvent::RedrawRequested => {
                if let Some(state) = &mut self.state {
                    state.render_frame();
                    state.window.request_redraw();
                }
            }
            _ => {}
        }
    }
}

impl PlayerState {
    async fn new(event_loop: &ActiveEventLoop, path: &str) -> Result<Self, String> {
        // Create wgpu
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());

        // Create window first to get surface
        let window = Arc::new(
            event_loop
                .create_window(
                    Window::default_attributes()
                        .with_title("Video Playback")
                        .with_inner_size(winit::dpi::LogicalSize::new(1280, 720)),
                )
                .map_err(|e| format!("Window: {}", e))?,
        );

        let surface = instance
            .create_surface(window.clone())
            .map_err(|e| format!("Surface: {}", e))?;

        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                compatible_surface: Some(&surface),
                ..Default::default()
            })
            .await
            .map_err(|_| "No adapter")?;

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor::default())
            .await
            .map_err(|e| format!("Device: {}", e))?;

        let device = Arc::new(device);
        let queue = Arc::new(queue);

        let size = window.inner_size();
        let caps = surface.get_capabilities(&adapter);
        let format = caps.formats[0];

        let surface_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width,
            height: size.height,
            present_mode: wgpu::PresentMode::AutoVsync,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &surface_config);

        // Open video player
        let player = VideoPlayer::open(path, device.clone(), queue.clone())
            .map_err(|e| format!("Player: {}", e))?;

        let (width, height) = player.dimensions();
        println!(
            "Playing video: {}x{}, {} samples",
            width,
            height,
            player.sample_count()
        );

        // Update window title
        window.set_title(&format!("Video Playback - {}x{}", width, height));

        // Create render pipeline for NV12 display
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor::default());

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("video_bind_group_layout"),
            entries: &[
                // Y texture
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
                // UV texture
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                // Sampler
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("nv12_shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("nv12_shader.wgsl").into()),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("video_pipeline_layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("video_pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        Ok(Self {
            window,
            device,
            queue,
            surface,
            surface_config,
            player,
            pipeline,
            bind_group_layout,
            sampler,
            frame_count: 0,
            last_fps_update: Instant::now(),
            fps_frame_count: 0,
            loop_count: 0,
        })
    }

    fn render_frame(&mut self) {
        // Decode next frame
        let frame = match self.player.next_frame() {
            Ok(Some(f)) => {
                self.frame_count += 1;
                Some(f)
            }
            Ok(None) => {
                // Loop playback
                self.player.reset();
                self.loop_count += 1;
                self.frame_count = 0;
                println!("Looping... (loop {})", self.loop_count);
                return;
            }
            Err(e) => {
                eprintln!("Decode error: {:?}", e);
                return;
            }
        };

        // Update FPS counter
        self.fps_frame_count += 1;
        let now = Instant::now();
        if now.duration_since(self.last_fps_update) >= Duration::from_secs(1) {
            let fps = self.fps_frame_count as f32
                / now.duration_since(self.last_fps_update).as_secs_f32();
            let (width, height) = self.player.dimensions();
            self.window.set_title(&format!(
                "Video Playback - {}x{} | frame {} | {:.1} FPS | loop {}",
                width, height, self.frame_count, fps, self.loop_count
            ));
            self.fps_frame_count = 0;
            self.last_fps_update = now;
        }

        // Render
        let output = match self.surface.get_current_texture() {
            Ok(t) => t,
            Err(_) => return,
        };

        let view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());

        if let Some(frame) = frame {
            // Create bind group from GPU frame
            let y_view = frame.gpu_frame.y_texture().create_view(&Default::default());
            let uv_view = frame
                .gpu_frame
                .uv_texture()
                .create_view(&Default::default());

            let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("video_bind_group"),
                layout: &self.bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&y_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&uv_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                ],
            });

            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("video_render_pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                        depth_slice: None,
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });

                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &bind_group, &[]);
                pass.draw(0..6, 0..1);
            }
        }

        self.queue.submit(std::iter::once(encoder.finish()));
        output.present();
    }
}

fn playback_video(path: &str) {
    println!("Step 2: Playing back video...\n");

    let event_loop = EventLoop::new().unwrap();
    event_loop.set_control_flow(ControlFlow::Poll);

    let mut app = VideoPlayerApp::new(path.to_string());
    event_loop.run_app(&mut app).unwrap();
}
