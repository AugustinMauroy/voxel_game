use std::collections::HashSet;

use glam::Vec3;

use crate::world::blocks::Block;

use super::EditableVoxelWorld;

pub const FALLING_GRAVITY: f32 = 10.0;
pub const MAX_FALLING_SPEED: f32 = 8.0;
pub const FIXED_STEP: f32 = 1.0 / 60.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FallingBlock {
    pub id: u64,
    pub block: Block,
    pub position: Vec3,
    pub velocity: Vec3,
    pub active: bool,
}

#[derive(Debug, Default)]
pub struct FallingSandSystem {
    next_id: u64,
    accumulator: f32,
    entities: Vec<FallingBlock>,
    positions: HashSet<[i64; 3]>,
}

impl FallingSandSystem {
    pub fn entities(&self) -> &[FallingBlock] {
        &self.entities
    }

    pub fn len(&self) -> usize {
        self.entities.len()
    }

    /// Rechecks sand at a changed voxel and immediately above it. This keeps
    /// updates local while covering both support removal and sand placement.
    pub fn block_changed<W: EditableVoxelWorld>(&mut self, world: &mut W, voxel: [i64; 3]) {
        for y in [voxel[1], voxel[1] + 1] {
            self.spawn_if_unsupported(world, [voxel[0], y, voxel[2]]);
        }
    }

    pub fn spawn_if_unsupported<W: EditableVoxelWorld>(
        &mut self,
        world: &mut W,
        voxel: [i64; 3],
    ) -> bool {
        if self.positions.contains(&voxel)
            || world.block_at(voxel) != Some(Block::SAND)
            || !matches!(world.block_at([voxel[0], voxel[1] - 1, voxel[2]]), Some(block) if !block.is_solid())
        {
            return false;
        }

        if !world.set_block_at(voxel, Block::AIR) {
            return false;
        }
        self.next_id += 1;
        self.positions.insert(voxel);
        self.entities.push(FallingBlock {
            id: self.next_id,
            block: Block::SAND,
            position: Vec3::new(voxel[0] as f32, voxel[1] as f32, voxel[2] as f32),
            velocity: Vec3::ZERO,
            active: true,
        });
        true
    }

    /// Advances with a fixed timestep. Entities in unloaded space are kept
    /// suspended until the relevant chunk becomes available.
    pub fn update<W: EditableVoxelWorld>(&mut self, world: &mut W, dt: f32) -> Vec<[i64; 3]> {
        self.accumulator = (self.accumulator + dt.clamp(0.0, 0.25)).min(0.25);
        let mut changed = Vec::new();
        while self.accumulator >= FIXED_STEP {
            self.accumulator -= FIXED_STEP;
            for entity in &mut self.entities {
                if !entity.active {
                    continue;
                }
                entity.velocity.y =
                    (entity.velocity.y - FALLING_GRAVITY * FIXED_STEP).max(-MAX_FALLING_SPEED);
                let current = [
                    entity.position.x.floor() as i64,
                    entity.position.y.floor() as i64,
                    entity.position.z.floor() as i64,
                ];
                let below = [current[0], current[1] - 1, current[2]];
                let Some(below_block) = world.block_at(below) else {
                    continue;
                };
                if below_block.is_solid() {
                    if world
                        .block_at(current)
                        .is_some_and(|block| block.is_solid())
                    {
                        continue;
                    }
                    let _ = world.set_block_at(current, entity.block);
                    entity.active = false;
                    self.positions.remove(&current);
                    changed.push(current);
                    continue;
                }
                entity.position.y += entity.velocity.y * FIXED_STEP;
            }
            self.entities.retain(|entity| entity.active);
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::gameplay::VoxelWorld;

    #[derive(Default)]
    struct TestWorld(HashMap<[i64; 3], Block>);

    impl VoxelWorld for TestWorld {
        fn block_at(&self, position: [i64; 3]) -> Option<Block> {
            Some(*self.0.get(&position).unwrap_or(&Block::AIR))
        }
    }

    impl EditableVoxelWorld for TestWorld {
        fn set_block_at(&mut self, position: [i64; 3], block: Block) -> bool {
            self.0.insert(position, block);
            true
        }
    }

    #[test]
    fn unsupported_sand_is_removed_and_becomes_one_entity() {
        let mut world = TestWorld::default();
        world.0.insert([0, 2, 0], Block::SAND);
        let mut system = FallingSandSystem::default();
        assert!(system.spawn_if_unsupported(&mut world, [0, 2, 0]));
        assert!(!system.spawn_if_unsupported(&mut world, [0, 2, 0]));
        assert_eq!(world.block_at([0, 2, 0]), Some(Block::AIR));
        assert_eq!(system.len(), 1);
    }

    #[test]
    fn sand_settles_on_solid_ground() {
        let mut world = TestWorld::default();
        world.0.insert([0, 0, 0], Block::STONE);
        world.0.insert([0, 3, 0], Block::SAND);
        let mut system = FallingSandSystem::default();
        system.spawn_if_unsupported(&mut world, [0, 3, 0]);
        for _ in 0..120 {
            system.update(&mut world, FIXED_STEP);
        }
        assert_eq!(world.block_at([0, 1, 0]), Some(Block::SAND));
        assert_eq!(system.len(), 0);
    }
}
