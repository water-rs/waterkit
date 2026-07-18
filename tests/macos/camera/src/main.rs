//! Camera preview test using winit + wgpu.
//!
//! Lists cameras, allows selection, and renders the camera feed to a window.
//! The camera API returns GPU textures directly for zero-copy rendering.

use futures::StreamExt;
use std::sync::Arc;
use std::time::Instant;
use waterkit_camera::{Camera, CameraConfig, CameraInfo, Frame};
use shaderloom::CompiledShader;
use winit::{
    application::ApplicationHandler,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    keyboard::{Key, NamedKey},
    window::{Window, WindowId},
};

const CAMERA_TEST_SHADER: CompiledShader = include!(concat!(env!("OUT_DIR"), "/camera_test.rs"));

fn main() {
    env_logger::init();

    // Create tokio runtime for async camera operations
    let rt = tokio::runtime::Runtime::new().unwrap();

    let event_loop = EventLoop::new().unwrap();
    event_loop.set_control_flow(ControlFlow::Poll);

    let mut app = App::new(rt);
    event_loop.run_app(&mut app).unwrap();
}

struct App {
    rt: tokio::runtime::Runtime,
    state: Option<State>,
    cameras: Vec<CameraInfo>,
    selected_camera: usize,
}

struct State {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    config: wgpu::SurfaceConfiguration,
    frame_rx: async_channel::Receiver<Frame>,
    bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    pipeline: wgpu::RenderPipeline,
    current_frame: Option<Frame>,
    last_fps_update: Instant,
    frame_count: u32,
}

impl App {
    fn new(rt: tokio::runtime::Runtime) -> Self {
        let cameras = Camera::list().unwrap_or_default();

        println!("\n=== Camera Preview ===\n");
        if cameras.is_empty() {
            println!("No cameras found!");
        } else {
            println!("Available cameras:");
            for (i, cam) in cameras.iter().enumerate() {
                println!("  [{}] {} ({})", i, cam.name, cam.id);
            }
            println!("\nPress number keys to switch cameras");
            println!("Press ESC to exit\n");
        }

        Self {
            rt,
            state: None,
            cameras,
            selected_camera: 0,
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() || self.cameras.is_empty() {
            return;
        }

        let window = Arc::new(
            event_loop
                .create_window(
                    Window::default_attributes()
                        .with_title("Camera Preview")
                        .with_inner_size(winit::dpi::LogicalSize::new(1280, 720)),
                )
                .unwrap(),
        );

        let state = self.rt.block_on(State::new(
            window.clone(),
            &self.cameras[self.selected_camera].id,
        ));

        match state {
            Ok(s) => self.state = Some(s),
            Err(e) => eprintln!("Failed to initialize: {}", e),
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => {
                event_loop.exit();
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state.is_pressed() {
                    match event.logical_key {
                        Key::Named(NamedKey::Escape) => event_loop.exit(),
                        Key::Character(ref c) => {
                            if let Ok(num) = c.parse::<usize>()
                                && num < self.cameras.len()
                            {
                                println!(
                                    "Switching to camera {} not implemented in streaming mode",
                                    num
                                );
                            }
                        }
                        _ => {}
                    }
                }
            }
            WindowEvent::Resized(new_size) => {
                if let Some(state) = &mut self.state {
                    state.config.width = new_size.width.max(1);
                    state.config.height = new_size.height.max(1);
                    state.surface.configure(&state.device, &state.config);
                }
            }
            WindowEvent::RedrawRequested => {
                if let Some(state) = &mut self.state {
                    state.update_and_render();
                    state.window.request_redraw();
                }
            }
            _ => {}
        }
    }
}

impl State {
    async fn new(window: Arc<Window>, camera_id: &str) -> Result<Self, String> {
        let size = window.inner_size();

        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());

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

        let caps = surface.get_capabilities(&adapter);
        let format = caps.formats[0];

        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width,
            height: size.height,
            present_mode: wgpu::PresentMode::AutoVsync,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        // Open camera with GPU device
        let camera_config = CameraConfig::default();
        let camera = Camera::open(camera_id, camera_config, device.clone(), queue.clone())
            .await
            .map_err(|e| format!("Camera: {}", e))?;

        let res = camera.resolution();
        println!("Camera resolution: {}x{}", res.width, res.height);

        // Create channel for frames
        let (frame_tx, frame_rx) = async_channel::bounded::<Frame>(2);

        // Spawn background task to forward frames from stream to channel
        tokio::spawn(async move {
            let mut frames = std::pin::pin!(camera.frames());
            while let Some(frame) = frames.next().await {
                // Drop old frames if receiver is slow
                let _ = frame_tx.try_send(frame);
            }
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor::default());

        // Create bind group layout
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("texture_bind_group_layout"),
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

        // Create shader and pipeline
        let (vertex_shader, fragment_shader) =
            CAMERA_TEST_SHADER.create_render_stages(&device, "vs_main", "fs_main");

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("pipeline_layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("render_pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: vertex_shader.module(),
                entry_point: Some(vertex_shader.entry_point()),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: fragment_shader.module(),
                entry_point: Some(fragment_shader.entry_point()),
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
            multiview_mask: None,
            cache: None,
        });

        Ok(Self {
            window,
            surface,
            device,
            queue,
            config,
            frame_rx,
            bind_group_layout,
            sampler,
            pipeline,
            current_frame: None,
            last_fps_update: Instant::now(),
            frame_count: 0,
        })
    }

    fn update_and_render(&mut self) {
        // Try to get latest frame (non-blocking)
        while let Ok(frame) = self.frame_rx.try_recv() {
            self.current_frame = Some(frame);
        }

        // Calculate FPS
        self.frame_count += 1;
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_fps_update);
        if elapsed.as_secs_f32() >= 1.0 {
            let fps = self.frame_count as f32 / elapsed.as_secs_f32();
            self.window
                .set_title(&format!("Camera Preview - {:.1} FPS", fps));
            self.frame_count = 0;
            self.last_fps_update = now;
        }

        // Render
        let output = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(output)
            | wgpu::CurrentSurfaceTexture::Suboptimal(output) => output,
            wgpu::CurrentSurfaceTexture::Timeout
            | wgpu::CurrentSurfaceTexture::Occluded
            | wgpu::CurrentSurfaceTexture::Outdated
            | wgpu::CurrentSurfaceTexture::Lost => return,
            wgpu::CurrentSurfaceTexture::Validation => {
                panic!("camera test surface acquisition failed validation")
            }
        };

        let view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("render_pass"),
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
                multiview_mask: None,
            });

            pass.set_pipeline(&self.pipeline);
            if let Some(frame) = &self.current_frame {
                let texture_view = frame.view();
                let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("texture_bind_group"),
                    layout: &self.bind_group_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(&texture_view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::Sampler(&self.sampler),
                        },
                    ],
                });
                pass.set_bind_group(0, &bind_group, &[]);
                pass.draw(0..6, 0..1);
            }
        }

        self.queue.submit(std::iter::once(encoder.finish()));
        output.present();
    }
}
