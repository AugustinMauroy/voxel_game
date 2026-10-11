use glam::Vec3;

pub mod falling;

use crate::world::{
    blocks::Block,
    chunks::{world_to_chunk_local, Chunk},
    terrain::TerrainGenerator,
};

pub trait VoxelWorld {
    fn block_at(&self, world: [i64; 3]) -> Option<Block>;
}

pub trait EditableVoxelWorld: VoxelWorld {
    fn set_block_at(&mut self, world: [i64; 3], block: Block) -> bool;
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RayHit {
    pub voxel: [i64; 3],
    pub previous_voxel: [i64; 3],
    pub face_normal: [i64; 3],
    pub distance: f32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InteractionAction {
    Break,
    Place(Block),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InteractionResult {
    Changed { voxel: [i64; 3], mesh_chunks: usize },
    NoTarget,
    Unloaded,
    Blocked,
}

pub fn cast_voxel_ray<W: VoxelWorld>(
    world: &W,
    origin: Vec3,
    direction: Vec3,
    max_reach: f32,
) -> Option<RayHit> {
    cast_voxel_ray_status(world, origin, direction, max_reach)
        .ok()
        .flatten()
}

fn cast_voxel_ray_status<W: VoxelWorld>(
    world: &W,
    origin: Vec3,
    direction: Vec3,
    max_reach: f32,
) -> Result<Option<RayHit>, ()> {
    if max_reach <= 0.0 || direction.length_squared() < f32::EPSILON {
        return Ok(None);
    }
    let direction = direction.normalize();
    let mut voxel = [
        origin.x.floor() as i64,
        origin.y.floor() as i64,
        origin.z.floor() as i64,
    ];
    let mut previous_voxel = voxel;
    let mut distance = 0.0;
    let step = [
        direction.x.signum() as i64,
        direction.y.signum() as i64,
        direction.z.signum() as i64,
    ];
    let mut t_max = [
        axis_boundary(origin.x, voxel[0], direction.x),
        axis_boundary(origin.y, voxel[1], direction.y),
        axis_boundary(origin.z, voxel[2], direction.z),
    ];
    let t_delta = [
        if direction.x == 0.0 {
            f32::INFINITY
        } else {
            1.0 / direction.x.abs()
        },
        if direction.y == 0.0 {
            f32::INFINITY
        } else {
            1.0 / direction.y.abs()
        },
        if direction.z == 0.0 {
            f32::INFINITY
        } else {
            1.0 / direction.z.abs()
        },
    ];

    loop {
        match world.block_at(voxel) {
            Some(block) if !block.is_air() => {
                return Ok(Some(RayHit {
                    voxel,
                    previous_voxel,
                    face_normal: [
                        previous_voxel[0] - voxel[0],
                        previous_voxel[1] - voxel[1],
                        previous_voxel[2] - voxel[2],
                    ],
                    distance,
                }));
            }
            None => return Err(()),
            _ => {}
        }
        let axis = if t_max[0] <= t_max[1] && t_max[0] <= t_max[2] {
            0
        } else if t_max[1] <= t_max[2] {
            1
        } else {
            2
        };
        distance = t_max[axis];
        if distance > max_reach {
            return Ok(None);
        }
        previous_voxel = voxel;
        voxel[axis] += step[axis];
        t_max[axis] += t_delta[axis];
    }
}

fn axis_boundary(origin: f32, voxel: i64, direction: f32) -> f32 {
    if direction > 0.0 {
        (voxel as f32 + 1.0 - origin) / direction
    } else if direction < 0.0 {
        (voxel as f32 - origin) / direction
    } else {
        f32::INFINITY
    }
}

#[derive(Clone, Copy, Debug)]
pub struct PlayerConfig {
    pub walk_speed: f32,
    pub mouse_sensitivity: f32,
    pub jump_speed: f32,
    pub water_jump_speed: f32,
    pub water_sink_speed: f32,
    pub gravity: f32,
    pub step_height: f32,
    pub half_width: f32,
    pub height: f32,
    pub reach: f32,
}

impl Default for PlayerConfig {
    fn default() -> Self {
        Self {
            walk_speed: 12.0,
            mouse_sensitivity: 0.0025,
            jump_speed: 8.5,
            water_jump_speed: 4.0,
            water_sink_speed: 1.5,
            gravity: 24.0,
            step_height: 1.0,
            half_width: 0.3,
            height: 1.8,
            reach: 6.0,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct PlayerInput {
    pub forward: bool,
    pub backward: bool,
    pub left: bool,
    pub right: bool,
    pub jump: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct Player {
    pub position: Vec3,
    pub velocity: Vec3,
    pub grounded: bool,
    pub yaw: f32,
    pub pitch: f32,
    pub config: PlayerConfig,
}

impl Player {
    pub fn spawn_above_terrain(
        generator: &TerrainGenerator,
        x: i64,
        z: i64,
        config: PlayerConfig,
    ) -> Self {
        Self::new(
            Vec3::new(
                x as f32 + 0.5,
                generator.height_at(x, z) as f32 + 1.0,
                z as f32 + 0.5,
            ),
            config,
        )
    }

    pub const fn new(position: Vec3, config: PlayerConfig) -> Self {
        Self {
            position,
            velocity: Vec3::ZERO,
            grounded: false,
            yaw: -2.25,
            pitch: -0.49,
            config,
        }
    }

    pub fn eye_position(self) -> Vec3 {
        self.position + Vec3::Y * (self.config.height - 0.1)
    }

    pub fn forward(self) -> Vec3 {
        Vec3::new(
            self.pitch.cos() * self.yaw.cos(),
            self.pitch.sin(),
            self.pitch.cos() * self.yaw.sin(),
        )
        .normalize()
    }

    pub fn intersects_block(self, voxel: [i64; 3]) -> bool {
        let min = self.position - Vec3::new(self.config.half_width, 0.0, self.config.half_width);
        let max = self.position
            + Vec3::new(
                self.config.half_width,
                self.config.height,
                self.config.half_width,
            );
        min.x < voxel[0] as f32 + 1.0
            && max.x > voxel[0] as f32
            && min.y < voxel[1] as f32 + 1.0
            && max.y > voxel[1] as f32
            && min.z < voxel[2] as f32 + 1.0
            && max.z > voxel[2] as f32
    }

    pub fn interact<W: EditableVoxelWorld>(
        &self,
        world: &mut W,
        action: InteractionAction,
        max_reach: f32,
    ) -> InteractionResult {
        let hit = match cast_voxel_ray_status(world, self.eye_position(), self.forward(), max_reach)
        {
            Ok(Some(hit)) => hit,
            Ok(None) => return InteractionResult::NoTarget,
            Err(()) => return InteractionResult::Unloaded,
        };
        let target = match action {
            InteractionAction::Break => hit.voxel,
            InteractionAction::Place(_) => hit.previous_voxel,
        };
        if matches!(action, InteractionAction::Place(_)) && self.intersects_block(target) {
            return InteractionResult::Blocked;
        }
        let block = match action {
            InteractionAction::Break => Block::AIR,
            InteractionAction::Place(block) => block,
        };
        if world.set_block_at(target, block) {
            InteractionResult::Changed {
                voxel: target,
                mesh_chunks: 1,
            }
        } else {
            InteractionResult::Unloaded
        }
    }

    pub fn update<W: VoxelWorld>(&mut self, input: PlayerInput, dt: f32, world: &W) {
        let dt = dt.clamp(0.0, 0.1);
        let forward = Vec3::new(self.yaw.cos(), 0.0, self.yaw.sin());
        let right = forward.cross(Vec3::Y);
        let mut direction = Vec3::ZERO;
        if input.forward {
            direction += forward;
        }
        if input.backward {
            direction -= forward;
        }
        if input.right {
            direction += right;
        }
        if input.left {
            direction -= right;
        }
        direction = direction.normalize_or_zero();
        self.velocity.x = direction.x * self.config.walk_speed;
        self.velocity.z = direction.z * self.config.walk_speed;

        let in_water = self.is_in_water(world);
        if input.jump && (self.grounded || in_water) {
            self.velocity.y = if in_water {
                self.config.water_jump_speed
            } else {
                self.config.jump_speed
            };
            self.grounded = false;
        }
        if in_water {
            if !input.jump {
                self.velocity.y = (self.velocity.y - self.config.gravity * 0.2 * dt)
                    .max(-self.config.water_sink_speed);
            }
        } else {
            self.velocity.y -= self.config.gravity * dt;
        }

        self.move_axis(0, self.velocity.x * dt, world);
        self.move_axis(2, self.velocity.z * dt, world);
        let vertical_delta = self.velocity.y * dt;
        let was_falling = self.velocity.y <= 0.0;
        self.move_axis(1, vertical_delta, world);
        if was_falling && self.collides_at(self.position - Vec3::Y * 0.001, world) {
            self.grounded = true;
            self.velocity.y = 0.0;
        }
    }

    fn move_axis<W: VoxelWorld>(&mut self, axis: usize, delta: f32, world: &W) {
        let steps = (delta.abs() / 0.25).ceil() as usize;
        let step_delta = delta / steps.max(1) as f32;
        for _ in 0..steps.max(1) {
            self.move_axis_step(axis, step_delta, world);
        }
    }

    fn move_axis_step<W: VoxelWorld>(&mut self, axis: usize, delta: f32, world: &W) {
        if delta == 0.0 {
            return;
        }
        let mut candidate = self.position;
        candidate[axis] += delta;
        if !self.collides_at(candidate, world) {
            self.position = candidate;
            return;
        }

        if (axis == 0 || axis == 2) && self.grounded && self.try_step_up(candidate, world) {
            self.velocity.y = self.config.jump_speed;
            self.grounded = false;
            return;
        }

        let mut resolved = candidate[axis];
        let ranges = self.voxel_ranges(candidate);
        for y in ranges[1].0..=ranges[1].1 {
            for z in ranges[2].0..=ranges[2].1 {
                for x in ranges[0].0..=ranges[0].1 {
                    let coordinates = [x, y, z];
                    if !self.is_solid(world, coordinates) {
                        continue;
                    }
                    let boundary = match axis {
                        0 if delta > 0.0 => x as f32 - self.config.half_width,
                        0 => x as f32 + 1.0 + self.config.half_width,
                        1 if delta > 0.0 => y as f32 - self.config.height,
                        1 => y as f32 + 1.0,
                        2 if delta > 0.0 => z as f32 - self.config.half_width,
                        _ => z as f32 + 1.0 + self.config.half_width,
                    };
                    if (delta > 0.0 && boundary < resolved) || (delta < 0.0 && boundary > resolved)
                    {
                        resolved = boundary;
                    }
                }
            }
        }
        self.position[axis] = resolved;
        if axis == 1 && delta < 0.0 {
            self.grounded = true;
        }
        if axis == 1 {
            self.velocity.y = 0.0;
        }
    }

    fn try_step_up<W: VoxelWorld>(&mut self, candidate: Vec3, world: &W) -> bool {
        let stepped = candidate + Vec3::Y * self.config.step_height;
        if self.collides_at(stepped, world) {
            return false;
        }

        let support_y = (stepped.y - f32::EPSILON).floor() as i64;
        let ranges = self.voxel_ranges(stepped);
        let has_support = (ranges[0].0..=ranges[0].1)
            .any(|x| (ranges[2].0..=ranges[2].1).any(|z| self.is_solid(world, [x, support_y, z])));
        if !has_support {
            return false;
        }

        self.position = stepped;
        true
    }

    fn collides_at<W: VoxelWorld>(&self, position: Vec3, world: &W) -> bool {
        let ranges = self.voxel_ranges(position);
        for y in ranges[1].0..=ranges[1].1 {
            for z in ranges[2].0..=ranges[2].1 {
                for x in ranges[0].0..=ranges[0].1 {
                    if self.is_solid(world, [x, y, z]) {
                        return true;
                    }
                }
            }
        }
        false
    }

    fn is_in_water<W: VoxelWorld>(&self, world: &W) -> bool {
        let ranges = self.voxel_ranges(self.position);
        for y in ranges[1].0..=ranges[1].1 {
            for z in ranges[2].0..=ranges[2].1 {
                for x in ranges[0].0..=ranges[0].1 {
                    if world.block_at([x, y, z]).is_some_and(Block::is_water) {
                        return true;
                    }
                }
            }
        }
        false
    }

    fn voxel_ranges(&self, position: Vec3) -> [(i64, i64); 3] {
        [
            range(
                position.x - self.config.half_width,
                position.x + self.config.half_width,
            ),
            range(position.y, position.y + self.config.height),
            range(
                position.z - self.config.half_width,
                position.z + self.config.half_width,
            ),
        ]
    }

    fn is_solid<W: VoxelWorld>(&self, world: &W, coordinates: [i64; 3]) -> bool {
        world.block_at(coordinates).is_some_and(Block::is_solid)
    }
}

fn range(min: f32, max: f32) -> (i64, i64) {
    (min.floor() as i64, (max - f32::EPSILON).floor() as i64)
}

impl VoxelWorld for Chunk {
    fn block_at(&self, world: [i64; 3]) -> Option<Block> {
        let (chunk, local) = world_to_chunk_local(world);
        (chunk == self.coordinates()).then(|| self.get(local[0], local[1], local[2]))?
    }
}

impl EditableVoxelWorld for Chunk {
    fn set_block_at(&mut self, world: [i64; 3], block: Block) -> bool {
        let (chunk, local) = world_to_chunk_local(world);
        chunk == self.coordinates() && self.set(local[0], local[1], local[2], block)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    struct TestWorld {
        blocks: HashMap<[i64; 3], Block>,
    }

    impl VoxelWorld for TestWorld {
        fn block_at(&self, coordinates: [i64; 3]) -> Option<Block> {
            Some(*self.blocks.get(&coordinates).unwrap_or(&Block::AIR))
        }
    }

    impl EditableVoxelWorld for TestWorld {
        fn set_block_at(&mut self, coordinates: [i64; 3], block: Block) -> bool {
            self.blocks.insert(coordinates, block);
            true
        }
    }

    fn world(blocks: &[[i64; 3]]) -> TestWorld {
        TestWorld {
            blocks: blocks.iter().copied().map(|p| (p, Block::STONE)).collect(),
        }
    }

    fn player(position: Vec3) -> Player {
        Player::new(
            position,
            PlayerConfig {
                walk_speed: 4.0,
                gravity: 10.0,
                jump_speed: 5.0,
                ..Default::default()
            },
        )
    }

    #[test]
    fn floor_collision_sets_grounded() {
        let world = world(&[[0, 0, 0]]);
        let mut player = player(Vec3::new(0.5, 2.0, 0.5));
        for _ in 0..20 {
            player.update(PlayerInput::default(), 0.1, &world);
        }
        assert_eq!(player.position.y, 1.0);
        assert!(player.grounded);
    }

    #[test]
    fn wall_collision_stops_horizontal_motion() {
        let world = world(&[[1, 1, 0]]);
        let mut player = player(Vec3::new(0.5, 1.0, 0.5));
        player.update(
            PlayerInput {
                right: true,
                ..Default::default()
            },
            0.5,
            &world,
        );
        assert!(player.position.x <= 0.7);
    }

    #[test]
    fn auto_jump_steps_over_one_block_obstacle() {
        let world = world(&[[0, 0, 0], [1, 1, 0]]);
        let mut player = player(Vec3::new(0.5, 1.0, 0.5));
        player.yaw = 0.0;
        player.grounded = true;

        for _ in 0..3 {
            player.update(
                PlayerInput {
                    forward: true,
                    ..Default::default()
                },
                0.1,
                &world,
            );
        }

        assert!(player.position.x > 1.0, "{player:?}");
        assert!(player.position.y > 2.0, "{player:?}");
    }

    #[test]
    fn jumping_in_water_provides_upward_float() {
        let mut blocks = HashMap::new();
        blocks.insert([0, 0, 0], Block::STONE);
        blocks.insert([0, 1, 0], Block::WATER);
        let world = TestWorld { blocks };
        let mut player = player(Vec3::new(0.5, 1.2, 0.5));
        let start_y = player.position.y;

        player.update(
            PlayerInput {
                jump: true,
                ..Default::default()
            },
            0.1,
            &world,
        );

        assert!(player.position.y > start_y);
        assert!(!player.grounded);
    }

    #[test]
    fn jumping_only_starts_when_grounded() {
        let world = world(&[[0, 0, 0]]);
        let mut player = player(Vec3::new(0.5, 1.0, 0.5));
        player.grounded = true;
        player.update(
            PlayerInput {
                jump: true,
                ..Default::default()
            },
            0.1,
            &world,
        );
        assert!(player.velocity.y > 0.0);
        let velocity = player.velocity.y;
        player.update(
            PlayerInput {
                jump: true,
                ..Default::default()
            },
            0.1,
            &world,
        );
        assert!(player.velocity.y < velocity);
    }

    #[test]
    fn falling_stops_on_floor() {
        let world = world(&[[0, 0, 0]]);
        let mut player = player(Vec3::new(0.5, 4.0, 0.5));
        for _ in 0..20 {
            player.update(PlayerInput::default(), 0.1, &world);
        }
        assert_eq!(player.position.y, 1.0);
        assert!(player.grounded);
    }

    #[test]
    fn collision_crosses_negative_chunk_boundary() {
        let mut blocks = Vec::new();
        for x in -10..=10 {
            for z in -10..=10 {
                blocks.push([x, 0, z]);
            }
        }
        for z in -10..=10 {
            blocks.push([-1, 1, z]);
            blocks.push([-1, 2, z]);
        }
        let world = world(&blocks);
        let mut player = player(Vec3::new(0.5, 1.0, 0.5));
        for _ in 0..20 {
            player.update(
                PlayerInput {
                    forward: true,
                    ..Default::default()
                },
                0.1,
                &world,
            );
        }
        assert!(player.position.x >= 0.3);
    }

    #[test]
    fn dda_selects_hit_face_and_adjacent_voxel() {
        let world = world(&[[2, 0, 0]]);
        let hit =
            cast_voxel_ray(&world, Vec3::new(0.5, 0.5, 0.5), Vec3::X, 6.0).expect("ray should hit");
        assert_eq!(hit.voxel, [2, 0, 0]);
        assert_eq!(hit.previous_voxel, [1, 0, 0]);
        assert_eq!(hit.face_normal, [-1, 0, 0]);
    }

    #[test]
    fn dda_supports_negative_coordinates() {
        let world = world(&[[-2, 0, 0]]);
        let hit = cast_voxel_ray(&world, Vec3::new(-0.5, 0.5, 0.5), -Vec3::X, 6.0)
            .expect("ray should hit");
        assert_eq!(hit.voxel, [-2, 0, 0]);
        assert_eq!(hit.previous_voxel, [-1, 0, 0]);
        assert_eq!(hit.face_normal, [1, 0, 0]);
    }

    #[test]
    fn interaction_breaks_and_places_once_at_selected_face() {
        let mut world = world(&[[2, 0, 0]]);
        let mut player = player(Vec3::new(0.5, -1.3, 0.5));
        player.yaw = 0.0;
        player.pitch = 0.0;
        assert!(matches!(
            player.interact(&mut world, InteractionAction::Break, 6.0),
            InteractionResult::Changed {
                voxel: [2, 0, 0],
                ..
            }
        ));
        assert_eq!(world.block_at([2, 0, 0]), Some(Block::AIR));

        world.blocks.insert([2, 0, 0], Block::STONE);
        assert!(matches!(
            player.interact(&mut world, InteractionAction::Place(Block::DIRT), 6.0),
            InteractionResult::Changed {
                voxel: [1, 0, 0],
                ..
            }
        ));
        assert_eq!(world.block_at([1, 0, 0]), Some(Block::DIRT));
    }

    #[test]
    fn placement_is_rejected_inside_player_aabb() {
        let mut world = world(&[[2, 0, 0]]);
        let mut player = player(Vec3::new(0.9, -0.8, 0.5));
        player.yaw = 0.0;
        player.pitch = 0.0;
        assert_eq!(
            player.interact(&mut world, InteractionAction::Place(Block::DIRT), 6.0),
            InteractionResult::Blocked
        );
    }

    #[test]
    fn interaction_edits_across_a_negative_chunk_boundary() {
        let mut world = world(&[[-32, 0, 0], [-31, 0, 0]]);
        let mut player = player(Vec3::new(-33.5, -1.3, 0.5));
        player.yaw = 0.0;
        player.pitch = 0.0;
        assert!(matches!(
            player.interact(&mut world, InteractionAction::Break, 6.0),
            InteractionResult::Changed {
                voxel: [-32, 0, 0],
                ..
            }
        ));
        assert_eq!(world.block_at([-32, 0, 0]), Some(Block::AIR));
    }

    #[test]
    fn interaction_stops_at_unloaded_voxels() {
        let mut chunk = Chunk::new([0, 0, 0]);
        let mut player = player(Vec3::new(29.5, -1.3, 0.5));
        player.yaw = 0.0;
        player.pitch = 0.0;
        assert_eq!(
            player.interact(&mut chunk, InteractionAction::Break, 6.0),
            InteractionResult::Unloaded
        );
    }
}
