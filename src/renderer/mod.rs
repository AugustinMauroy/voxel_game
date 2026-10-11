use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use anyhow::Context;
use egui_wgpu::ScreenDescriptor;
use glam::{Mat4, Vec3, Vec4};
use wgpu::util::DeviceExt;
use winit::{
    event::*,
    event_loop::EventLoop,
    window::{Window, WindowBuilder},
};

use crate::{
    app::AppState,
    debug::{Metrics, MetricsHistory},
    gameplay::{
        falling::FallingSandSystem, InteractionAction, InteractionResult, Player, PlayerConfig,
        PlayerInput,
    },
    meshing::{block_vertices, VoxelVertex},
    persistence::{PlayerSave, WorldStore, WorldSummary},
    streaming::{ChunkCoord, StreamingConfig, StreamingWorld},
    ui::{load_worlds, Ui, UiAction},
    world::chunks::world_to_chunk_local,
    world::terrain::TerrainGenerator,
};

const SHADER: &str = include_str!("voxel.wgsl");

pub fn run() -> anyhow::Result<()> {
    let event_loop = EventLoop::new()?;
    let window = Arc::new(
        WindowBuilder::new()
            .with_title("Voxel Game")
            .build(&event_loop)?,
    );
    let mut state = pollster::block_on(Renderer::new(window.clone()))?;

    event_loop.run(move |event, target| match event {
        Event::DeviceEvent {
            event: DeviceEvent::MouseMotion { delta },
            ..
        } => state.controller.process_mouse(delta),
        Event::WindowEvent { event, window_id } if window_id == state.window.id() => {
            let ui_consumed = state
                .egui_state
                .on_window_event(&state.window, &event)
                .consumed;
            if !ui_consumed && !state.input(&event) {
                match event {
                    WindowEvent::CloseRequested => {
                        if let Err(error) = state.save_active_world() {
                            eprintln!("could not save before exit: {error:#}");
                        }
                        target.exit();
                    }
                    WindowEvent::Resized(size) => state.resize(size),
                    WindowEvent::RedrawRequested => {
                        state.update();
                        match state.render() {
                            Ok(()) => {}
                            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                                state.resize(state.size)
                            }
                            Err(wgpu::SurfaceError::OutOfMemory) => target.exit(),
                            Err(wgpu::SurfaceError::Timeout) => eprintln!("surface timeout"),
                        }
                    }
                    _ => {}
                }
            }
        }
        Event::AboutToWait => state.window.request_redraw(),
        _ => {}
    })?;
    Ok(())
}

struct Renderer {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    size: winit::dpi::PhysicalSize<u32>,
    render_pipeline: wgpu::RenderPipeline,
    chunks: HashMap<ChunkCoord, ChunkGpu>,
    streaming: StreamingWorld,
    frame_count: u64,
    depth_view: wgpu::TextureView,
    camera_buffer: wgpu::Buffer,
    camera_bind_group: wgpu::BindGroup,
    camera: Camera,
    player: Player,
    controller: CameraController,
    pending_action: Option<InteractionAction>,
    left_mouse_down: bool,
    right_mouse_down: bool,
    last_update: Instant,
    egui_context: egui::Context,
    egui_state: egui_winit::State,
    egui_renderer: egui_wgpu::Renderer,
    ui: Ui,
    store: WorldStore,
    worlds: Vec<WorldSummary>,
    active_world: Option<WorldSummary>,
    metrics: Metrics,
    metric_history: MetricsHistory,
    egui_jobs: Vec<egui::ClippedPrimitive>,
    egui_textures: egui::TexturesDelta,
    dirty_chunks: HashSet<ChunkCoord>,
    falling_sand: FallingSandSystem,
    falling_vertex_buffer: Option<wgpu::Buffer>,
    falling_vertex_count: u32,
    save_lock: Arc<Mutex<()>>,
    autosave_elapsed: Duration,
}

impl Renderer {
    async fn new(window: Arc<Window>) -> anyhow::Result<Self> {
        let size = window.inner_size();
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all(),
            dx12_shader_compiler: Default::default(),
            flags: wgpu::InstanceFlags::default(),
            gles_minor_version: wgpu::Gles3MinorVersion::Automatic,
        });
        let surface = instance
            .create_surface(window.clone())
            .context("creating the window surface")?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .context("no suitable GPU adapter found")?;
        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: Some("voxel device"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::default(),
                },
                None,
            )
            .await
            .context("requesting GPU device")?;

        let capabilities = surface.get_capabilities(&adapter);
        let format = capabilities
            .formats
            .iter()
            .copied()
            .find(wgpu::TextureFormat::is_srgb)
            .unwrap_or(capabilities.formats[0]);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: capabilities.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        let depth_view = create_depth_view(&device, &config);
        let controller = CameraController::default();
        let player = Player::spawn_above_terrain(
            &TerrainGenerator::new(42),
            42,
            48,
            PlayerConfig {
                walk_speed: controller.speed,
                mouse_sensitivity: controller.sensitivity,
                ..Default::default()
            },
        );
        let mut camera = Camera::new(config.width as f32 / config.height as f32);
        camera.position = player.eye_position();
        camera.yaw = player.yaw;
        camera.pitch = player.pitch;
        let camera_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("camera buffer"),
            contents: bytemuck::cast_slice(&[camera.matrix().to_cols_array_2d()]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let camera_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("camera bind group layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let camera_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("camera bind group"),
            layout: &camera_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: camera_buffer.as_entire_binding(),
            }],
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("voxel shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("voxel pipeline layout"),
            bind_group_layouts: &[&camera_layout],
            push_constant_ranges: &[],
        });
        let render_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("voxel render pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: "vs_main",
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<VoxelVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x3,
                            offset: 0,
                            shader_location: 0,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x4,
                            offset: 12,
                            shader_location: 1,
                        },
                    ],
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: "fs_main",
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth24Plus,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            multiview: None,
        });
        let store = WorldStore::discover()?;
        let settings = store.load_settings()?;
        let egui_context = egui::Context::default();
        let egui_state = egui_winit::State::new(
            egui_context.clone(),
            egui::ViewportId::ROOT,
            window.as_ref(),
            None,
            None,
        );
        let egui_renderer = egui_wgpu::Renderer::new(&device, format, None, 1);
        let (worlds, _) = load_worlds(&store);

        Ok(Self {
            window,
            surface,
            device,
            queue,
            config,
            size,
            render_pipeline,
            chunks: HashMap::new(),
            streaming: StreamingWorld::new(
                42,
                StreamingConfig {
                    view_distance: 4,
                    worker_count: 2,
                    queue_capacity: 8,
                },
            ),
            frame_count: 0,
            depth_view,
            camera_buffer,
            camera_bind_group,
            camera,
            player,
            controller,
            pending_action: None,
            left_mouse_down: false,
            right_mouse_down: false,
            last_update: Instant::now(),
            egui_context,
            egui_state,
            egui_renderer,
            ui: Ui::new(settings),
            store,
            worlds,
            active_world: None,
            metrics: Metrics::default(),
            metric_history: MetricsHistory::default(),
            egui_jobs: Vec::new(),
            egui_textures: egui::TexturesDelta::default(),
            dirty_chunks: HashSet::new(),
            falling_sand: FallingSandSystem::default(),
            falling_vertex_buffer: None,
            falling_vertex_count: 0,
            save_lock: Arc::new(Mutex::new(())),
            autosave_elapsed: Duration::ZERO,
        })
    }

    fn input(&mut self, event: &WindowEvent) -> bool {
        if self.ui.state != AppState::InGame {
            if matches!(
                event,
                WindowEvent::KeyboardInput { .. } | WindowEvent::MouseInput { .. }
            ) {
                return true;
            }
        }
        match event {
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state == ElementState::Pressed {
                    if let winit::keyboard::PhysicalKey::Code(code) = event.physical_key {
                        match code {
                            winit::keyboard::KeyCode::F3 => {
                                self.ui.show_debug = !self.ui.show_debug;
                                return true;
                            }
                            winit::keyboard::KeyCode::Escape
                                if self.ui.state == AppState::InGame =>
                            {
                                self.ui.state = AppState::Paused;
                                self.release_cursor();
                                return true;
                            }
                            code if self.ui.state == AppState::InGame
                                && matches!(
                                    code,
                                    winit::keyboard::KeyCode::Digit1
                                        | winit::keyboard::KeyCode::Digit2
                                        | winit::keyboard::KeyCode::Digit3
                                        | winit::keyboard::KeyCode::Digit4
                                        | winit::keyboard::KeyCode::Digit5
                                        | winit::keyboard::KeyCode::Digit6
                                        | winit::keyboard::KeyCode::Digit7
                                        | winit::keyboard::KeyCode::Digit8
                                        | winit::keyboard::KeyCode::Digit9
                                ) =>
                            {
                                let index = match code {
                                    winit::keyboard::KeyCode::Digit1 => 0,
                                    winit::keyboard::KeyCode::Digit2 => 1,
                                    winit::keyboard::KeyCode::Digit3 => 2,
                                    winit::keyboard::KeyCode::Digit4 => 3,
                                    winit::keyboard::KeyCode::Digit5 => 4,
                                    winit::keyboard::KeyCode::Digit6 => 5,
                                    winit::keyboard::KeyCode::Digit7 => 6,
                                    winit::keyboard::KeyCode::Digit8 => 7,
                                    _ => 8,
                                };
                                self.ui.selected_slot = index;
                                return true;
                            }
                            _ => {}
                        }
                    }
                }
                self.controller.process_keyboard(event);
                true
            }
            WindowEvent::MouseInput { state, button, .. } => {
                match (button, state) {
                    (MouseButton::Left, ElementState::Pressed) if !self.left_mouse_down => {
                        self.left_mouse_down = true;
                        self.pending_action = Some(InteractionAction::Break);
                    }
                    (MouseButton::Right, ElementState::Pressed) if !self.right_mouse_down => {
                        self.right_mouse_down = true;
                        self.pending_action = Some(InteractionAction::Place(
                            crate::world::blocks::Block::new(self.ui.hotbar[self.ui.selected_slot]),
                        ));
                    }
                    (MouseButton::Left, ElementState::Released) => self.left_mouse_down = false,
                    (MouseButton::Right, ElementState::Released) => self.right_mouse_down = false,
                    _ => {}
                }
                true
            }
            WindowEvent::ScaleFactorChanged { .. } => true,
            _ => false,
        }
    }

    fn resize(&mut self, size: winit::dpi::PhysicalSize<u32>) {
        if size.width == 0 || size.height == 0 {
            return;
        }
        self.size = size;
        self.config.width = size.width;
        self.config.height = size.height;
        self.surface.configure(&self.device, &self.config);
        self.depth_view = create_depth_view(&self.device, &self.config);
        let camera = Camera::new(size.width as f32 / size.height as f32);
        self.queue.write_buffer(
            &self.camera_buffer,
            0,
            bytemuck::cast_slice(&[camera.matrix().to_cols_array_2d()]),
        );
    }

    fn update(&mut self) {
        let elapsed = self.last_update.elapsed();
        self.last_update = Instant::now();
        self.autosave_elapsed += elapsed;
        self.run_ui();
        if self.ui.state != AppState::InGame {
            self.update_metrics(elapsed.as_secs_f32() * 1000.0);
            if self.autosave_elapsed >= Duration::from_secs(30) {
                self.autosave_elapsed = Duration::ZERO;
                self.start_autosave();
            }
            return;
        }
        let input = PlayerInput {
            forward: self.controller.forward,
            backward: self.controller.backward,
            left: self.controller.left,
            right: self.controller.right,
            jump: self.controller.up,
        };
        self.player.yaw +=
            self.controller.mouse_delta.0 as f32 * self.player.config.mouse_sensitivity;
        self.player.pitch -=
            self.controller.mouse_delta.1 as f32 * self.player.config.mouse_sensitivity;
        self.player.pitch = self.player.pitch.clamp(-1.54, 1.54);
        self.controller.mouse_delta = (0.0, 0.0);
        self.player.update(
            input,
            elapsed.min(Duration::from_millis(100)).as_secs_f32(),
            &self.streaming,
        );
        self.camera.position = self.player.eye_position();
        self.camera.yaw = self.player.yaw;
        self.camera.pitch = self.player.pitch;
        self.queue.write_buffer(
            &self.camera_buffer,
            0,
            bytemuck::cast_slice(&[self.camera.matrix().to_cols_array_2d()]),
        );
        let player_world = [
            self.player.position.x.floor() as i64,
            self.player.position.y.floor() as i64,
            self.player.position.z.floor() as i64,
        ];
        let (player_chunk, _) = world_to_chunk_local(player_world);
        for coordinates in self.streaming.update(player_chunk) {
            self.chunks.remove(&coordinates);
        }
        for coordinates in self.streaming.loaded_coordinates().collect::<Vec<_>>() {
            if !self.chunks.contains_key(&coordinates) {
                if let Some(vertices) = self.streaming.chunk_vertices(coordinates) {
                    self.upload_chunk_mesh(coordinates, vertices);
                }
            }
        }
        for completed in self.streaming.drain_completed() {
            self.upload_chunk_mesh(completed.coordinates, completed.vertices);
        }
        if let Some(action) = self.pending_action.take() {
            let result =
                self.player
                    .interact(&mut self.streaming, action, self.player.config.reach);
            if let InteractionResult::Changed { voxel, .. } = result {
                self.falling_sand.block_changed(&mut self.streaming, voxel);
                for coordinates in self.streaming.mesh_chunks_affected_by(voxel) {
                    if let Some(vertices) = self.streaming.chunk_vertices(coordinates) {
                        self.upload_chunk_mesh(coordinates, vertices);
                    }
                }
                self.persist_modified_chunk(voxel);
            }
        }

        let changed = self.falling_sand.update(
            &mut self.streaming,
            elapsed.min(Duration::from_millis(100)).as_secs_f32(),
        );
        for voxel in changed {
            for coordinates in self.streaming.mesh_chunks_affected_by(voxel) {
                if let Some(vertices) = self.streaming.chunk_vertices(coordinates) {
                    self.upload_chunk_mesh(coordinates, vertices);
                }
            }
            self.persist_modified_chunk(voxel);
        }
        let mut falling_vertices = Vec::new();
        for entity in self.falling_sand.entities() {
            falling_vertices.extend(block_vertices(entity.position.to_array(), entity.block));
        }
        self.falling_vertex_count = falling_vertices.len() as u32;
        self.falling_vertex_buffer = (!falling_vertices.is_empty()).then(|| {
            self.device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("falling sand vertex buffer"),
                    contents: bytemuck::cast_slice(&falling_vertices),
                    usage: wgpu::BufferUsages::VERTEX,
                })
        });

        self.update_metrics(elapsed.as_secs_f32() * 1000.0);
        self.frame_count += 1;
        if self.frame_count % 120 == 0 {
            let metrics = self.streaming.metrics();
            eprintln!(
                "streaming: loaded={} queued={} last_generation_ms={:?} average_generation_ms={:?}",
                metrics.loaded_chunks,
                metrics.queued_jobs,
                metrics.last_generation_ms,
                metrics.average_generation_ms
            );
        }
    }

    fn run_ui(&mut self) {
        let raw_input = self.egui_state.take_egui_input(&self.window);
        let mut action = UiAction::None;
        let full_output = self.egui_context.run(raw_input, |ctx| {
            action = self.ui.draw(ctx, &self.metrics, &self.worlds);
        });
        self.handle_ui_action(action);
        self.egui_state
            .handle_platform_output(&self.window, full_output.platform_output.clone());
        self.egui_jobs = self
            .egui_context
            .tessellate(full_output.shapes, full_output.pixels_per_point);
        self.egui_textures = full_output.textures_delta;
    }

    fn handle_ui_action(&mut self, action: UiAction) {
        match action {
            UiAction::None => {}
            UiAction::Play => self.ui.state = AppState::WorldSelection,
            UiAction::CreateWorld => {
                self.ui.status = None;
                self.ui.state = AppState::WorldCreation;
            }
            UiAction::OpenSettings => self.ui.state = AppState::Settings,
            UiAction::Back => {
                if let Err(error) = self.store.save_settings(&self.ui.settings) {
                    self.ui.status = Some(error.to_string());
                }
                self.controller.sensitivity = self.ui.settings.mouse_sensitivity;
                self.streaming
                    .set_view_distance(self.ui.settings.render_distance);
                self.ui.status = None;
                self.ui.state = if self.active_world.is_some() {
                    AppState::Paused
                } else {
                    AppState::MainMenu
                };
            }
            UiAction::Resume => {
                self.ui.state = AppState::InGame;
                self.capture_cursor();
            }
            UiAction::Quit => std::process::exit(0),
            UiAction::SaveAndMenu => {
                if let Err(error) = self.save_active_world() {
                    self.ui.status = Some(error.to_string());
                } else {
                    let (worlds, error) = load_worlds(&self.store);
                    self.worlds = worlds;
                    self.ui.status = error;
                    self.ui.state = AppState::MainMenu;
                    self.release_cursor();
                }
            }
            UiAction::CreateConfirmed => match self.create_world() {
                Ok(world) => {
                    let (worlds, error) = load_worlds(&self.store);
                    self.worlds = worlds;
                    self.ui.status = error;
                    self.start_world(world);
                }
                Err(error) => self.ui.status = Some(error.to_string()),
            },
            UiAction::SelectWorld(index) => {
                if let Some(world) = self.worlds.get(index).cloned() {
                    self.start_world(world);
                }
            }
            UiAction::DeleteWorld(index) => {
                if let Some(world) = self.worlds.get(index).cloned() {
                    match self.store.delete(&world) {
                        Ok(()) => {
                            let (worlds, error) = load_worlds(&self.store);
                            self.worlds = worlds;
                            self.ui.status = error;
                        }
                        Err(error) => self.ui.status = Some(error.to_string()),
                    }
                }
            }
        }
    }

    fn create_world(&mut self) -> anyhow::Result<WorldSummary> {
        let seed = self
            .ui
            .seed_text
            .trim()
            .parse::<u64>()
            .context("seed must be an unsigned integer")?;
        self.store.create(
            &self.ui.world_name,
            seed,
            self.ui.game_mode,
            self.ui.settings.render_distance,
        )
    }

    fn start_world(&mut self, world: WorldSummary) {
        self.chunks.clear();
        self.dirty_chunks.clear();
        self.falling_sand = FallingSandSystem::default();
        self.falling_vertex_buffer = None;
        self.falling_vertex_count = 0;
        self.streaming = StreamingWorld::new(
            world.metadata.seed,
            StreamingConfig {
                view_distance: world.metadata.render_distance,
                worker_count: 2,
                queue_capacity: 8,
            },
        );
        self.player = Player::spawn_above_terrain(
            &TerrainGenerator::new(world.metadata.seed),
            42,
            48,
            PlayerConfig {
                walk_speed: self.controller.speed,
                mouse_sensitivity: self.ui.settings.mouse_sensitivity,
                ..Default::default()
            },
        );
        if let Ok(Some(save)) = self.store.load_player(&world) {
            save.apply_to(&mut self.player);
            self.ui.selected_slot = save.selected_slot.min(8);
            self.ui.hotbar = save.hotbar;
            if self.ui.hotbar[1] == 0 {
                self.ui.hotbar[1] = crate::world::blocks::Block::DIRT.id();
            }
        }
        if let Ok(chunks) = self.store.load_modified_chunks(&world) {
            for chunk in chunks {
                self.streaming.restore_chunk(chunk);
            }
        }
        self.active_world = Some(world);
        self.ui.state = AppState::InGame;
        self.capture_cursor();
    }

    fn save_active_world(&mut self) -> anyhow::Result<()> {
        let Some(world) = &self.active_world else {
            return Ok(());
        };
        let _save_guard = self.save_lock.lock().expect("save lock poisoned");
        let dirty = self.dirty_chunks.iter().copied().collect::<Vec<_>>();
        for coordinates in dirty {
            if let Some(chunk) = self.streaming.chunk(coordinates) {
                self.store.save_chunk(world, &chunk)?;
            }
        }
        self.store.save_player(
            world,
            PlayerSave::from_player(&self.player, self.ui.selected_slot, self.ui.hotbar),
        )?;
        self.store.touch(world)?;
        self.dirty_chunks.clear();
        Ok(())
    }

    fn start_autosave(&self) {
        let Some(world) = self.active_world.clone() else {
            return;
        };
        let store = self.store.clone();
        let player = PlayerSave::from_player(&self.player, self.ui.selected_slot, self.ui.hotbar);
        let save_lock = self.save_lock.clone();
        std::thread::spawn(move || {
            let _guard = save_lock.lock().expect("save lock poisoned");
            if let Err(error) = store
                .save_player(&world, player)
                .and_then(|_| store.touch(&world))
            {
                eprintln!("autosave failed: {error:#}");
            }
        });
    }

    fn persist_modified_chunk(&mut self, voxel: [i64; 3]) {
        let Some(world) = self.active_world.clone() else {
            return;
        };
        let (coordinates, _) = world_to_chunk_local(voxel);
        let Some(chunk) = self.streaming.chunk(coordinates) else {
            return;
        };
        self.dirty_chunks.insert(coordinates);
        let store = self.store.clone();
        let save_lock = self.save_lock.clone();
        std::thread::spawn(move || {
            let _guard = save_lock.lock().expect("save lock poisoned");
            if let Err(error) = store.save_chunk(&world, &chunk) {
                eprintln!("could not persist modified chunk: {error:#}");
            }
        });
    }

    fn capture_cursor(&self) {
        let _ = self
            .window
            .set_cursor_grab(winit::window::CursorGrabMode::Locked);
        self.window.set_cursor_visible(false);
    }

    fn release_cursor(&mut self) {
        let _ = self
            .window
            .set_cursor_grab(winit::window::CursorGrabMode::None);
        self.window.set_cursor_visible(true);
        self.controller.mouse_delta = (0.0, 0.0);
    }

    fn update_metrics(&mut self, frame_ms: f32) {
        let (fps, average, low) = self.metric_history.record(frame_ms);
        let player_world = [
            self.player.position.x.floor() as i64,
            self.player.position.y.floor() as i64,
            self.player.position.z.floor() as i64,
        ];
        let (_, local) = world_to_chunk_local(player_world);
        let streaming = self.streaming.metrics();
        self.metrics = Metrics {
            frame_ms,
            fps,
            average_frame_ms: average,
            low_1_percent_fps: low,
            cpu_ms: frame_ms,
            loaded_chunks: streaming.loaded_chunks,
            queued_chunks: streaming.queued_jobs,
            rendered_chunks: self.chunks.len(),
            vertices: self
                .chunks
                .values()
                .map(|chunk| chunk.opaque_vertex_count as usize + chunk.water_vertex_count as usize)
                .sum(),
            player_position: self.player.position.to_array(),
            player_velocity: self.player.velocity.to_array(),
            player_chunk: world_to_chunk_local(player_world).0,
            local_position: local,
            seed: self
                .active_world
                .as_ref()
                .map_or(42, |world| world.metadata.seed),
            world_name: self
                .active_world
                .as_ref()
                .map_or_else(|| "No world".into(), |world| world.metadata.name.clone()),
            state: self.ui.state.to_string(),
        };
    }

    fn upload_chunk_mesh(&mut self, coordinates: ChunkCoord, vertices: Vec<VoxelVertex>) {
        if vertices.is_empty() {
            self.chunks.remove(&coordinates);
            return;
        }
        let (opaque, water): (Vec<_>, Vec<_>) = vertices
            .into_iter()
            .partition(|vertex| vertex.color[3] >= 1.0);
        let opaque_buffer = (!opaque.is_empty()).then(|| {
            self.device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("streamed chunk vertex buffer"),
                    contents: bytemuck::cast_slice(&opaque),
                    usage: wgpu::BufferUsages::VERTEX,
                })
        });
        let water_buffer = (!water.is_empty()).then(|| {
            self.device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("streamed water vertex buffer"),
                    contents: bytemuck::cast_slice(&water),
                    usage: wgpu::BufferUsages::VERTEX,
                })
        });
        self.chunks.insert(
            coordinates,
            ChunkGpu {
                opaque_buffer,
                opaque_vertex_count: (opaque.len()) as u32,
                water_buffer,
                water_vertex_count: (water.len()) as u32,
            },
        );
    }

    fn render(&mut self) -> Result<(), wgpu::SurfaceError> {
        let output = self.surface.get_current_texture()?;
        let view = output.texture.create_view(&Default::default());
        let mut encoder = self.device.create_command_encoder(&Default::default());
        let render_world = matches!(self.ui.state, AppState::InGame | AppState::Paused);
        let clear_color = if render_world {
            wgpu::Color {
                r: 0.43,
                g: 0.68,
                b: 0.92,
                a: 1.0,
            }
        } else {
            wgpu::Color {
                r: 0.055,
                g: 0.07,
                b: 0.065,
                a: 1.0,
            }
        };
        let screen_descriptor = ScreenDescriptor {
            size_in_pixels: [self.config.width, self.config.height],
            pixels_per_point: self.egui_context.pixels_per_point(),
        };
        for (id, image_delta) in &self.egui_textures.set {
            self.egui_renderer
                .update_texture(&self.device, &self.queue, *id, image_delta);
        }
        let _ = self.egui_renderer.update_buffers(
            &self.device,
            &self.queue,
            &mut encoder,
            &self.egui_jobs,
            &screen_descriptor,
        );
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("voxel render pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color { ..clear_color }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            if render_world {
                pass.set_pipeline(&self.render_pipeline);
                pass.set_bind_group(0, &self.camera_bind_group, &[]);
                for chunk in self.chunks.values() {
                    if let Some(buffer) = &chunk.opaque_buffer {
                        pass.set_vertex_buffer(0, buffer.slice(..));
                        pass.draw(0..chunk.opaque_vertex_count, 0..1);
                    }
                }
                if let Some(buffer) = &self.falling_vertex_buffer {
                    pass.set_vertex_buffer(0, buffer.slice(..));
                    pass.draw(0..self.falling_vertex_count, 0..1);
                }
                for chunk in self.chunks.values() {
                    if let Some(buffer) = &chunk.water_buffer {
                        pass.set_vertex_buffer(0, buffer.slice(..));
                        pass.draw(0..chunk.water_vertex_count, 0..1);
                    }
                }
            }
        }
        {
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("egui overlay pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
                self.egui_renderer
                    .render(&mut pass, &self.egui_jobs, &screen_descriptor);
            }
        }
        for id in &self.egui_textures.free {
            self.egui_renderer.free_texture(id);
        }
        self.egui_textures = egui::TexturesDelta::default();
        self.queue.submit(Some(encoder.finish()));
        output.present();
        Ok(())
    }
}

struct ChunkGpu {
    opaque_buffer: Option<wgpu::Buffer>,
    opaque_vertex_count: u32,
    water_buffer: Option<wgpu::Buffer>,
    water_vertex_count: u32,
}

fn create_depth_view(
    device: &wgpu::Device,
    config: &wgpu::SurfaceConfiguration,
) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("depth texture"),
            size: wgpu::Extent3d {
                width: config.width,
                height: config.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth24Plus,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
        .create_view(&Default::default())
}

struct Camera {
    position: Vec3,
    aspect: f32,
    yaw: f32,
    pitch: f32,
}

impl Camera {
    fn new(aspect: f32) -> Self {
        Self {
            position: Vec3::new(42.0, 34.0, 48.0),
            aspect,
            yaw: -2.25,
            pitch: -0.49,
        }
    }

    fn matrix(&self) -> Mat4 {
        let view = Mat4::look_at_rh(self.position, self.position + self.forward(), Vec3::Y);
        let projection = Mat4::perspective_rh(45_f32.to_radians(), self.aspect, 0.1, 200.0);
        OPENGL_TO_WGPU * projection * view
    }

    fn forward(&self) -> Vec3 {
        Vec3::new(
            self.pitch.cos() * self.yaw.cos(),
            self.pitch.sin(),
            self.pitch.cos() * self.yaw.sin(),
        )
        .normalize()
    }
}

struct CameraController {
    forward: bool,
    backward: bool,
    left: bool,
    right: bool,
    up: bool,
    down: bool,
    mouse_delta: (f64, f64),
    speed: f32,
    sensitivity: f32,
}

impl Default for CameraController {
    fn default() -> Self {
        Self {
            forward: false,
            backward: false,
            left: false,
            right: false,
            up: false,
            down: false,
            mouse_delta: (0.0, 0.0),
            speed: 12.0,
            sensitivity: 0.0025,
        }
    }
}

impl CameraController {
    fn process_keyboard(&mut self, event: &KeyEvent) {
        let pressed = event.state == ElementState::Pressed;
        let value = match event.physical_key {
            winit::keyboard::PhysicalKey::Code(winit::keyboard::KeyCode::KeyW) => &mut self.forward,
            winit::keyboard::PhysicalKey::Code(winit::keyboard::KeyCode::KeyS) => {
                &mut self.backward
            }
            winit::keyboard::PhysicalKey::Code(winit::keyboard::KeyCode::KeyA) => &mut self.left,
            winit::keyboard::PhysicalKey::Code(winit::keyboard::KeyCode::KeyD) => &mut self.right,
            winit::keyboard::PhysicalKey::Code(winit::keyboard::KeyCode::Space) => &mut self.up,
            winit::keyboard::PhysicalKey::Code(winit::keyboard::KeyCode::ShiftLeft) => {
                &mut self.down
            }
            _ => return,
        };
        *value = pressed;
    }

    fn process_mouse(&mut self, delta: (f64, f64)) {
        self.mouse_delta.0 += delta.0;
        self.mouse_delta.1 += delta.1;
    }
}

const OPENGL_TO_WGPU: Mat4 = Mat4::from_cols(
    Vec4::new(1.0, 0.0, 0.0, 0.0),
    Vec4::new(0.0, 1.0, 0.0, 0.0),
    Vec4::new(0.0, 0.0, 0.5, 0.0),
    Vec4::new(0.0, 0.0, 0.5, 1.0),
);
