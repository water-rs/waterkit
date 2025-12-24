//! Camera preview test using winit + wgpu.
//!
//! Lists cameras, allows selection, and renders the camera feed to a window.

use std::sync::Arc;
use std::time::{Duration, Instant};
use std::thread;
use waterkit_camera::{Camera, CameraInfo, FrameFormat};
use winit::{
    application::ApplicationHandler,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    keyboard::{Key, NamedKey},
    window::{Window, WindowId},
};

fn main() {
    env_logger::init();
    let event_loop = EventLoop::new().unwrap();
    event_loop.set_control_flow(ControlFlow::Poll);

    let mut app = App::new();
    event_loop.run_app(&mut app).unwrap();
}

struct App {
    state: Option<State>,
    cameras: Vec<CameraInfo>,
    selected_camera: usize,
}

struct State {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    _config: wgpu::SurfaceConfiguration,
    camera: Camera,
    texture: wgpu::Texture,
    texture_width: u32,
    texture_height: u32,
    bind_group: wgpu::BindGroup,
    bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    pipeline: wgpu::RenderPipeline,
    last_dropped_frames: u64,
    last_fps_update: Instant,
    frame_count: u32,
}

impl App {
    fn new() -> Self {
        // List cameras at startup
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
            state: None,
            cameras,
            selected_camera: 0,
        }
    }

    fn open_camera(&mut self, index: usize) {
        if index >= self.cameras.len() {
            return;
        }

        let camera_id = &self.cameras[index].id;
        println!("Opening camera: {}", self.cameras[index].name);

        if let Some(state) = &mut self.state {
            // Stop existing camera
            let _ = state.camera.stop();

            // Open new camera
            match Camera::open(camera_id) {
                Ok(mut cam) => {
                    if let Err(e) = cam.start() {
                        eprintln!("Failed to start camera: {}", e);
                        return;
                    }
                    state.camera = cam;
                    self.selected_camera = index;
                    println!("Camera started!");
                }
                Err(e) => eprintln!("Failed to open camera: {}", e),
            }
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

        let state = pollster::block_on(State::new(
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
                            if let Ok(num) = c.parse::<usize>() {
                                self.open_camera(num);
                            }
                        }
                        _ => {}
                    }
                }
            }
            WindowEvent::RedrawRequested => {
                if let Some(state) = &mut self.state {
                    match state.update() {
                        true => {
                            state.render();
                        }
                        false => {
                            // No new frame, sleep briefly to avoid busy loop
                            thread::sleep(Duration::from_millis(1));
                        }
                    }
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

        // Create wgpu instance
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());

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

        let (device, queue): (wgpu::Device, wgpu::Queue) = adapter
            .request_device(&wgpu::DeviceDescriptor::default())
            .await
            .map_err(|e| format!("Device: {}", e))?;

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

        // Open camera
        let mut camera = Camera::open(camera_id).map_err(|e| format!("Camera: {}", e))?;
        camera.start().map_err(|e| format!("Start: {}", e))?;

        let res = camera.resolution();

        // Create texture for camera frames
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("camera_texture"),
            size: wgpu::Extent3d {
                width: res.width,
                height: res.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        let texture_width = res.width;
        let texture_height = res.height;
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor::default());

        // Create bind group layout and bind group
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

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("texture_bind_group"),
            layout: &bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });

        // Create shader
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("pipeline_layout"),
            bind_group_layouts: &[&bind_group_layout],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("render_pipeline"),
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
            multiview_mask: None,
            cache: None,
        });

        println!("Camera resolution: {}x{}", res.width, res.height);

        Ok(Self {
            window,
            surface,
            device,
            queue,
            _config: config,
            camera,
            texture,
            texture_width,
            texture_height,
            bind_group,
            bind_group_layout,
            sampler,
            pipeline,
            last_dropped_frames: 0,
            last_fps_update: Instant::now(),
            frame_count: 0,
        })
    }

    fn update(&mut self) -> bool {
        // Check for dropped frames
        let dropped = self.camera.dropped_frame_count();
        if dropped > self.last_dropped_frames {
            println!("WARN: Dropped {} frames (total: {})", dropped - self.last_dropped_frames, dropped);
            self.last_dropped_frames = dropped;
        }

        // Try to get a camera frame
        if let Ok(frame) = self.camera.get_frame() {
            // If frame size changed, recreate texture and bind group
            if frame.width != self.texture_width || frame.height != self.texture_height {
                println!("Frame size changed to {}x{}, recreating texture", frame.width, frame.height);
                self.texture_width = frame.width;
                self.texture_height = frame.height;
                
                self.texture = self.device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("camera_texture"),
                    size: wgpu::Extent3d {
                        width: frame.width,
                        height: frame.height,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba8UnormSrgb,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                });
                
                let view = self.texture.create_view(&wgpu::TextureViewDescriptor::default());
                self.bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("texture_bind_group"),
                    layout: &self.bind_group_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(&view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::Sampler(&self.sampler),
                        },
                    ],
                });
            }
            
            // Convert to RGBA and update texture
            let rgba = match frame.format {
                FrameFormat::Rgba => frame.data,
                FrameFormat::Bgra => {
                    let mut rgba = frame.data;
                    for chunk in rgba.chunks_exact_mut(4) {
                        chunk.swap(0, 2);
                    }
                    rgba
                }
                FrameFormat::Rgb => {
                    let mut rgba = Vec::with_capacity(frame.data.len() / 3 * 4);
                    for chunk in frame.data.chunks_exact(3) {
                        rgba.extend_from_slice(chunk);
                        rgba.push(255);
                    }
                    rgba
                }
                _ => frame.to_rgba(),
            };

            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &rgba,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(frame.width * 4),
                    rows_per_image: Some(frame.height),
                },
                wgpu::Extent3d {
                    width: frame.width,
                    height: frame.height,
                    depth_or_array_layers: 1,
                },
            );

            return true;
        }

        false
    }

    fn render(&mut self) {
        // Calculate FPS
        self.frame_count += 1;
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_fps_update);
        if elapsed.as_secs_f32() >= 1.0 {
            let fps = self.frame_count as f32 / elapsed.as_secs_f32();
            self.window.set_title(&format!("Camera Preview - {:.1} FPS", fps));
            self.frame_count = 0;
            self.last_fps_update = now;
        }

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
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.draw(0..6, 0..1);
        }

        self.queue.submit(std::iter::once(encoder.finish()));
        output.present();
    }
}

const SHADER: &str = r#"
struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) idx: u32) -> VertexOutput {
    // Full-screen triangle vertices
    var positions = array<vec2<f32>, 6>(
        vec2(-1.0, -1.0),
        vec2( 1.0, -1.0),
        vec2(-1.0,  1.0),
        vec2(-1.0,  1.0),
        vec2( 1.0, -1.0),
        vec2( 1.0,  1.0),
    );
    
    var uvs = array<vec2<f32>, 6>(
        vec2(0.0, 1.0),
        vec2(1.0, 1.0),
        vec2(0.0, 0.0),
        vec2(0.0, 0.0),
        vec2(1.0, 1.0),
        vec2(1.0, 0.0),
    );
    
    var out: VertexOutput;
    out.position = vec4(positions[idx], 0.0, 1.0);
    out.uv = uvs[idx];
    return out;
}

@group(0) @binding(0) var t_texture: texture_2d<f32>;
@group(0) @binding(1) var s_sampler: sampler;

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    return textureSample(t_texture, s_sampler, in.uv);
}
"#;
