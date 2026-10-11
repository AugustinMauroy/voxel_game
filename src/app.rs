use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AppState {
    MainMenu,
    WorldSelection,
    WorldCreation,
    InGame,
    Paused,
    Settings,
}

impl fmt::Display for AppState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::MainMenu => "Main Menu",
            Self::WorldSelection => "World Selection",
            Self::WorldCreation => "World Creation",
            Self::InGame => "In Game",
            Self::Paused => "Paused",
            Self::Settings => "Settings",
        };
        f.write_str(name)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum GameMode {
    Survival,
    Creative,
}

impl GameMode {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Survival => "Survival",
            Self::Creative => "Creative (experimental)",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Settings {
    pub mouse_sensitivity: f32,
    pub render_distance: i64,
    pub debug_overlay: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            mouse_sensitivity: 0.0025,
            render_distance: 4,
            debug_overlay: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn states_have_human_readable_names() {
        assert_eq!(AppState::WorldCreation.to_string(), "World Creation");
    }
}
