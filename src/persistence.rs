use std::{
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use glam::Vec3;
use serde::{Deserialize, Serialize};

use crate::{
    app::{GameMode, Settings},
    world::{blocks::Block, chunks::Chunk},
};

pub const SAVE_VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorldMetadata {
    pub id: String,
    pub name: String,
    pub seed: u64,
    pub save_version: u32,
    pub game_mode: String,
    pub created_at: u64,
    pub last_played: u64,
    pub render_distance: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlayerSave {
    pub position: [f32; 3],
    pub velocity: [f32; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub selected_slot: usize,
    pub hotbar: [u8; 9],
}

#[derive(Clone, Debug)]
pub struct WorldSummary {
    pub metadata: WorldMetadata,
    pub path: PathBuf,
}

#[derive(Clone, Debug)]
pub struct WorldStore {
    root: PathBuf,
}

impl WorldStore {
    pub fn discover() -> Result<Self> {
        let base = dirs::data_dir()
            .or_else(dirs::home_dir)
            .context("cannot determine a macOS user data directory")?;
        let root = base.join("voxel_game").join("worlds");
        fs::create_dir_all(&root).context("creating world save directory")?;
        Ok(Self { root })
    }

    pub fn settings_path() -> Result<PathBuf> {
        let base = dirs::config_dir()
            .or_else(dirs::home_dir)
            .context("cannot determine a configuration directory")?;
        let directory = base.join("voxel_game");
        fs::create_dir_all(&directory).context("creating configuration directory")?;
        Ok(directory.join("settings.json"))
    }

    pub fn load_settings(&self) -> Result<Settings> {
        let path = Self::settings_path()?;
        match fs::read_to_string(path) {
            Ok(contents) => Ok(serde_json::from_str(&contents).unwrap_or_default()),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(Settings::default()),
            Err(error) => Err(error).context("reading settings"),
        }
    }

    pub fn save_settings(&self, settings: &Settings) -> Result<()> {
        let path = Self::settings_path()?;
        atomic_json_write(&path, settings).context("saving settings")
    }

    pub fn list(&self) -> Result<Vec<WorldSummary>> {
        let mut worlds = Vec::new();
        for entry in fs::read_dir(&self.root).context("reading world directory")? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let path = entry.path();
            let metadata_path = path.join("world.json");
            let contents = match fs::read_to_string(&metadata_path) {
                Ok(contents) => contents,
                Err(error) if error.kind() == ErrorKind::NotFound => continue,
                Err(error) => return Err(error).context("reading world metadata"),
            };
            let metadata: WorldMetadata =
                serde_json::from_str(&contents).context("parsing world metadata")?;
            if metadata.save_version != SAVE_VERSION {
                continue;
            }
            worlds.push(WorldSummary { metadata, path });
        }
        worlds.sort_by_key(|world| std::cmp::Reverse(world.metadata.last_played));
        Ok(worlds)
    }

    pub fn create(
        &self,
        name: &str,
        seed: u64,
        mode: GameMode,
        render_distance: i64,
    ) -> Result<WorldSummary> {
        let safe_name = sanitize_name(name)?;
        let id = format!("{:016x}-{:x}", seed, now());
        let path = self.root.join(&id);
        fs::create_dir_all(path.join("chunks")).context("creating world directory")?;
        let timestamp = now();
        let metadata = WorldMetadata {
            id,
            name: safe_name,
            seed,
            save_version: SAVE_VERSION,
            game_mode: mode.label().to_string(),
            created_at: timestamp,
            last_played: timestamp,
            render_distance,
        };
        atomic_json_write(&path.join("world.json"), &metadata)?;
        Ok(WorldSummary { metadata, path })
    }

    pub fn touch(&self, world: &WorldSummary) -> Result<()> {
        let mut metadata = world.metadata.clone();
        metadata.last_played = now();
        atomic_json_write(&world.path.join("world.json"), &metadata)
    }

    pub fn save_player(&self, world: &WorldSummary, player: PlayerSave) -> Result<()> {
        atomic_json_write(&world.path.join("player.json"), &player)
    }

    pub fn load_player(&self, world: &WorldSummary) -> Result<Option<PlayerSave>> {
        let path = world.path.join("player.json");
        match fs::read_to_string(path) {
            Ok(contents) => Ok(Some(
                serde_json::from_str(&contents).context("parsing player save")?,
            )),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error).context("reading player save"),
        }
    }

    pub fn save_chunk(&self, world: &WorldSummary, chunk: &Chunk) -> Result<()> {
        let coordinates = chunk.coordinates();
        let path = world.path.join("chunks").join(chunk_filename(coordinates));
        let mut bytes = Vec::with_capacity(12 + chunk.len());
        bytes.extend_from_slice(b"VXCH");
        bytes.extend_from_slice(&SAVE_VERSION.to_le_bytes());
        bytes.extend_from_slice(&chunk.len().to_le_bytes());
        for z in 0..crate::world::chunks::CHUNK_SIZE {
            for y in 0..crate::world::chunks::CHUNK_SIZE {
                for x in 0..crate::world::chunks::CHUNK_SIZE {
                    bytes.push(chunk.get(x, y, z).unwrap_or(Block::AIR).id());
                }
            }
        }
        atomic_bytes_write(&path, &bytes)
    }

    pub fn load_chunk(&self, world: &WorldSummary, coordinates: [i64; 3]) -> Result<Option<Chunk>> {
        let path = world.path.join("chunks").join(chunk_filename(coordinates));
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error).context("reading chunk save"),
        };
        let expected = 16 + crate::world::chunks::CHUNK_SIZE.pow(3);
        if bytes.len() != expected
            || &bytes[..4] != b"VXCH"
            || u32::from_le_bytes(bytes[4..8].try_into()?) != SAVE_VERSION
        {
            anyhow::bail!("invalid or incompatible chunk save");
        }
        let mut chunk = Chunk::new(coordinates);
        let mut index = 16;
        for z in 0..crate::world::chunks::CHUNK_SIZE {
            for y in 0..crate::world::chunks::CHUNK_SIZE {
                for x in 0..crate::world::chunks::CHUNK_SIZE {
                    chunk.set(x, y, z, Block::new(bytes[index]));
                    index += 1;
                }
            }
        }
        Ok(Some(chunk))
    }

    pub fn load_modified_chunks(&self, world: &WorldSummary) -> Result<Vec<Chunk>> {
        let directory = world.path.join("chunks");
        let mut chunks = Vec::new();
        for entry in fs::read_dir(directory).context("reading modified chunks")? {
            let path = entry?.path();
            let Some(stem) = path.file_stem().and_then(|value| value.to_str()) else {
                continue;
            };
            let coordinates = stem
                .split('x')
                .map(str::parse::<i64>)
                .collect::<std::result::Result<Vec<_>, _>>()
                .ok();
            let Some(coordinates) = coordinates else {
                continue;
            };
            if coordinates.len() == 3 {
                if let Some(chunk) =
                    self.load_chunk(world, [coordinates[0], coordinates[1], coordinates[2]])?
                {
                    chunks.push(chunk);
                }
            }
        }
        Ok(chunks)
    }

    pub fn delete(&self, world: &WorldSummary) -> Result<()> {
        if world.path.parent() != Some(self.root.as_path()) {
            anyhow::bail!("refusing to delete a world outside the save directory");
        }
        fs::remove_dir_all(&world.path).context("deleting world")
    }
}

fn sanitize_name(name: &str) -> Result<String> {
    let name = name.trim();
    if name.is_empty()
        || name.len() > 64
        || name.contains('/')
        || name.contains('\\')
        || name == "."
        || name == ".."
    {
        anyhow::bail!("world name must be 1-64 characters and contain no path separators");
    }
    Ok(name.to_string())
}

fn chunk_filename(coordinates: [i64; 3]) -> String {
    format!(
        "{}x{}x{}.bin",
        coordinates[0], coordinates[1], coordinates[2]
    )
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn atomic_json_write<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(value)?;
    atomic_bytes_write(path, &bytes)
}

fn atomic_bytes_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let temporary = path.with_extension("tmp");
    fs::write(&temporary, bytes).with_context(|| format!("writing {}", temporary.display()))?;
    fs::rename(&temporary, path).with_context(|| format!("replacing {}", path.display()))?;
    Ok(())
}

impl PlayerSave {
    pub fn from_player(
        player: &crate::gameplay::Player,
        selected_slot: usize,
        hotbar: [u8; 9],
    ) -> Self {
        Self {
            position: player.position.to_array(),
            velocity: player.velocity.to_array(),
            yaw: player.yaw,
            pitch: player.pitch,
            selected_slot,
            hotbar,
        }
    }

    pub fn apply_to(&self, player: &mut crate::gameplay::Player) {
        player.position = Vec3::from_array(self.position);
        player.velocity = Vec3::from_array(self.velocity);
        player.yaw = self.yaw;
        player.pitch = self.pitch;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn player_round_trip_preserves_state() {
        let save = PlayerSave {
            position: [1.0, 2.0, -3.0],
            velocity: [0.1, 0.2, 0.3],
            yaw: 1.2,
            pitch: -0.4,
            selected_slot: 2,
            hotbar: [1; 9],
        };
        let json = serde_json::to_string(&save).unwrap();
        assert_eq!(
            serde_json::from_str::<PlayerSave>(&json).unwrap().position,
            save.position
        );
    }

    #[test]
    fn invalid_world_names_are_rejected() {
        assert!(sanitize_name("../world").is_err());
        assert!(sanitize_name(" ").is_err());
    }
}
