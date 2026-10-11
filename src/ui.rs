use egui::{
    Align2, Color32, Context, Direction, Layout, RichText, Sense, Slider, TopBottomPanel, Vec2,
    Window,
};

use crate::{
    app::{AppState, GameMode, Settings},
    debug::{recommendations, Metrics},
    persistence::{WorldStore, WorldSummary},
    world::blocks::Block,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UiAction {
    None,
    Play,
    CreateWorld,
    OpenSettings,
    Back,
    Resume,
    SaveAndMenu,
    Quit,
    CreateConfirmed,
    SelectWorld(usize),
    DeleteWorld(usize),
}

pub struct Ui {
    pub state: AppState,
    pub settings: Settings,
    pub world_name: String,
    pub seed_text: String,
    pub game_mode: GameMode,
    pub selected_world: Option<usize>,
    pub show_debug: bool,
    pub hotbar: [u8; 9],
    pub selected_slot: usize,
    pub status: Option<String>,
    pub delete_confirmation: Option<usize>,
}

impl Ui {
    pub fn new(settings: Settings) -> Self {
        Self {
            show_debug: settings.debug_overlay,
            settings,
            state: AppState::MainMenu,
            world_name: "My World".into(),
            seed_text: "42".into(),
            game_mode: GameMode::Survival,
            selected_world: None,
            hotbar: [1, 2, 0, 0, 0, 0, 0, 0, 0],
            selected_slot: 0,
            status: None,
            delete_confirmation: None,
        }
    }

    pub fn draw(&mut self, ctx: &Context, metrics: &Metrics, worlds: &[WorldSummary]) -> UiAction {
        if self.state == AppState::InGame && self.show_debug {
            self.debug_overlay(ctx, metrics);
        }
        match self.state {
            AppState::MainMenu => self.main_menu(ctx, worlds),
            AppState::WorldSelection => self.world_selection(ctx, worlds),
            AppState::WorldCreation => self.world_creation(ctx),
            AppState::Paused => self.pause_menu(ctx),
            AppState::Settings => self.settings(ctx),
            AppState::InGame => {
                self.hud(ctx, metrics);
                UiAction::None
            }
        }
    }

    fn panel(ctx: &Context, title: &str, content: impl FnOnce(&mut egui::Ui)) {
        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(Color32::from_rgba_unmultiplied(24, 30, 29, 235)))
            .show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(70.0);
                    ui.heading(
                        RichText::new(title)
                            .color(Color32::from_rgb(212, 183, 116))
                            .size(34.0),
                    );
                    ui.add_space(24.0);
                    content(ui);
                });
            });
    }

    fn main_menu(&mut self, ctx: &Context, worlds: &[WorldSummary]) -> UiAction {
        let mut action = UiAction::None;
        Self::panel(ctx, "VOXEL GROVE", |ui| {
            ui.label(RichText::new("A small, procedural world").color(Color32::LIGHT_GRAY));
            ui.add_space(25.0);
            if !worlds.is_empty() && ui.button("Play").clicked() {
                action = UiAction::Play;
            }
            if ui.button("Create World").clicked() {
                action = UiAction::CreateWorld;
            }
            if ui.button("Settings").clicked() {
                action = UiAction::OpenSettings;
            }
            if ui.button("Quit").clicked() {
                action = UiAction::Quit;
            }
            if let Some(status) = &self.status {
                ui.colored_label(Color32::LIGHT_RED, status);
            }
        });
        action
    }

    fn world_selection(&mut self, ctx: &Context, worlds: &[WorldSummary]) -> UiAction {
        let mut action = UiAction::None;
        Self::panel(ctx, "SELECT WORLD", |ui| {
            if worlds.is_empty() {
                ui.label("No worlds found. Create your first world.");
            }
            for (index, world) in worlds.iter().enumerate() {
                let selected = self.selected_world == Some(index);
                if ui
                    .selectable_label(
                        selected,
                        format!("{}  ·  seed {}", world.metadata.name, world.metadata.seed),
                    )
                    .clicked()
                {
                    self.selected_world = Some(index);
                }
            }
            ui.horizontal(|ui| {
                if ui.button("Play Selected").clicked() && self.selected_world.is_some() {
                    action = UiAction::SelectWorld(self.selected_world.unwrap());
                }
                if ui.button("Create New").clicked() {
                    action = UiAction::CreateWorld;
                }
                if ui.button("Delete").clicked() {
                    self.delete_confirmation = self.selected_world;
                }
                if ui.button("Back").clicked() {
                    action = UiAction::Back;
                }
            });
            if let Some(index) = self.delete_confirmation {
                ui.colored_label(
                    Color32::YELLOW,
                    format!("Delete '{}' permanently?", worlds[index].metadata.name),
                );
                ui.horizontal(|ui| {
                    if ui.button("Confirm Delete").clicked() {
                        action = UiAction::DeleteWorld(index);
                        self.delete_confirmation = None;
                    }
                    if ui.button("Cancel").clicked() {
                        self.delete_confirmation = None;
                    }
                });
            }
            if let Some(status) = &self.status {
                ui.colored_label(Color32::LIGHT_RED, status);
            }
        });
        action
    }

    fn world_creation(&mut self, ctx: &Context) -> UiAction {
        let mut action = UiAction::None;
        Self::panel(ctx, "CREATE WORLD", |ui| {
            ui.horizontal(|ui| {
                ui.label("Name");
                ui.text_edit_singleline(&mut self.world_name);
            });
            ui.horizontal(|ui| {
                ui.label("Seed");
                ui.text_edit_singleline(&mut self.seed_text);
                if ui.button("Randomize").clicked() {
                    self.seed_text = format!("{}", fastrand_seed());
                }
            });
            ui.horizontal(|ui| {
                ui.label("Mode");
                ui.selectable_value(
                    &mut self.game_mode,
                    GameMode::Survival,
                    GameMode::Survival.label(),
                );
                ui.selectable_value(
                    &mut self.game_mode,
                    GameMode::Creative,
                    GameMode::Creative.label(),
                );
            });
            ui.add(Slider::new(&mut self.settings.render_distance, 2..=8).text("Render distance"));
            ui.horizontal(|ui| {
                if ui.button("Create").clicked() {
                    action = UiAction::CreateConfirmed;
                }
                if ui.button("Back").clicked() {
                    action = UiAction::Back;
                }
            });
            if let Some(status) = &self.status {
                ui.colored_label(Color32::LIGHT_RED, status);
            }
        });
        action
    }

    fn pause_menu(&mut self, ctx: &Context) -> UiAction {
        let mut action = UiAction::None;
        Window::new("GAME PAUSED")
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .collapsible(false)
            .show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    if ui.button("Resume").clicked() {
                        action = UiAction::Resume;
                    }
                    if ui.button("Settings").clicked() {
                        action = UiAction::OpenSettings;
                    }
                    if ui.button("Save and Return to Main Menu").clicked() {
                        action = UiAction::SaveAndMenu;
                    }
                    if ui.button("Quit to Desktop").clicked() {
                        action = UiAction::Quit;
                    }
                });
            });
        action
    }

    fn settings(&mut self, ctx: &Context) -> UiAction {
        let mut action = UiAction::None;
        Self::panel(ctx, "SETTINGS", |ui| {
            ui.add(
                Slider::new(&mut self.settings.mouse_sensitivity, 0.0005..=0.01)
                    .text("Mouse sensitivity"),
            );
            ui.add(Slider::new(&mut self.settings.render_distance, 2..=8).text("Render distance"));
            ui.checkbox(&mut self.show_debug, "Show Smart F3 overlay");
            ui.label("VSync and GPU timing are controlled by the current wgpu backend.");
            if ui.button("Back").clicked() {
                action = UiAction::Back;
            }
        });
        action
    }

    fn hud(&mut self, ctx: &Context, metrics: &Metrics) -> UiAction {
        TopBottomPanel::bottom("hotbar").show(ctx, |ui| {
            ui.set_height(76.0);
            ui.with_layout(
                Layout::centered_and_justified(Direction::LeftToRight),
                |ui| {
                    for slot in 0..9 {
                        let selected = slot == self.selected_slot;
                        let (rect, response) =
                            ui.allocate_exact_size(Vec2::new(62.0, 62.0), Sense::click());
                        let block = Block::new(self.hotbar[slot]);
                        let painter = ui.painter();
                        painter.rect_filled(rect, 2.0, Color32::from_rgb(18, 18, 18));
                        painter.rect_stroke(
                            rect,
                            2.0,
                            egui::Stroke::new(
                                if selected { 3.0_f32 } else { 1.0_f32 },
                                if selected {
                                    Color32::from_rgb(238, 238, 238)
                                } else {
                                    Color32::from_rgb(92, 92, 92)
                                },
                            ),
                        );
                        if !block.is_air() {
                            draw_block_icon(painter, rect.shrink(9.0), block);
                        }
                        painter.text(
                            rect.left_top() + Vec2::new(5.0, 3.0),
                            egui::Align2::LEFT_TOP,
                            slot + 1,
                            egui::FontId::monospace(11.0),
                            Color32::WHITE,
                        );
                        if response.clicked() {
                            self.selected_slot = slot;
                        }
                    }
                },
            );
        });
        egui::Area::new("crosshair".into())
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(RichText::new("+").size(20.0).color(Color32::WHITE));
            });
        egui::Area::new("selected_block".into())
            .anchor(Align2::CENTER_BOTTOM, [0.0, -50.0])
            .show(ctx, |ui| {
                ui.label(
                    RichText::new(block_name(Block::new(self.hotbar[self.selected_slot])))
                        .strong()
                        .color(Color32::WHITE),
                );
            });
        let _ = metrics;
        UiAction::None
    }

    fn debug_overlay(&self, ctx: &Context, metrics: &Metrics) {
        Window::new("VOXEL ENGINE DEBUG")
            .anchor(Align2::LEFT_TOP, [8.0, 8.0])
            .resizable(false)
            .show(ctx, |ui| {
                ui.monospace(format!(
                    "FPS {:>6.1} | Frame {:>6.2} ms | Avg {:>6.2} ms",
                    metrics.fps, metrics.frame_ms, metrics.average_frame_ms
                ));
                ui.monospace(format!(
                    "1% low {:>6.1} FPS | CPU {:>6.2} ms | GPU Unavailable",
                    metrics.low_1_percent_fps, metrics.cpu_ms
                ));
                ui.monospace(format!(
                    "State {} | Window metrics supplied by renderer",
                    metrics.state
                ));
                ui.separator();
                ui.label(RichText::new("PLAYER").strong());
                ui.monospace(format!(
                    "Position {:.2} {:.2} {:.2}",
                    metrics.player_position[0],
                    metrics.player_position[1],
                    metrics.player_position[2]
                ));
                ui.monospace(format!(
                    "Velocity {:.2} {:.2} {:.2}",
                    metrics.player_velocity[0],
                    metrics.player_velocity[1],
                    metrics.player_velocity[2]
                ));
                ui.monospace(format!(
                    "Chunk {:?} | Local {:?}",
                    metrics.player_chunk, metrics.local_position
                ));
                ui.separator();
                ui.label(RichText::new("WORLD / MESHING").strong());
                ui.monospace(format!(
                    "World {} | Seed {}",
                    metrics.world_name, metrics.seed
                ));
                ui.monospace(format!(
                    "Loaded {} | Queued {} | Rendered {}",
                    metrics.loaded_chunks, metrics.queued_chunks, metrics.rendered_chunks
                ));
                ui.monospace(format!(
                    "Vertices {} | Algorithm visible-face",
                    metrics.vertices
                ));
                for recommendation in recommendations(metrics) {
                    ui.colored_label(Color32::YELLOW, format!("WARNING: {}", recommendation));
                }
            });
    }
}

fn block_name(block: Block) -> &'static str {
    match block.id() {
        1 => "Grass",
        2 => "Dirt",
        3 => "Stone",
        4 => "Log",
        5 => "Leaves",
        6 => "Sand",
        7 => "Water",
        _ => "Empty",
    }
}

fn block_color(block: Block) -> Color32 {
    match block.id() {
        1 => Color32::from_rgb(93, 158, 67),
        2 => Color32::from_rgb(126, 83, 48),
        3 => Color32::from_rgb(123, 128, 128),
        4 => Color32::from_rgb(127, 82, 45),
        5 => Color32::from_rgb(56, 132, 64),
        6 => Color32::from_rgb(218, 194, 123),
        7 => Color32::from_rgb(66, 137, 196),
        _ => Color32::from_rgb(31, 35, 33),
    }
}

fn draw_block_icon(painter: &egui::Painter, rect: egui::Rect, block: Block) {
    let face = block_color(block);
    painter.rect_filled(rect, 0.0, face);
    if block == Block::GRASS {
        painter.rect_filled(
            egui::Rect::from_min_max(
                rect.left_top(),
                egui::pos2(rect.right(), rect.top() + rect.height() * 0.22),
            ),
            0.0,
            Color32::from_rgb(103, 178, 72),
        );
        painter.rect_filled(
            egui::Rect::from_min_max(
                egui::pos2(rect.left(), rect.top() + rect.height() * 0.22),
                egui::pos2(rect.right(), rect.top() + rect.height() * 0.34),
            ),
            0.0,
            Color32::from_rgb(82, 112, 48),
        );
    }
}

fn fastrand_seed() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64
}

pub fn load_worlds(store: &WorldStore) -> (Vec<WorldSummary>, Option<String>) {
    match store.list() {
        Ok(worlds) => (worlds, None),
        Err(error) => (Vec::new(), Some(error.to_string())),
    }
}
