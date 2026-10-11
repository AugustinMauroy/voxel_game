use noise::{NoiseFn, Perlin};

use super::{
    biomes::{Biome, BiomeGenerator},
    blocks::Block,
    chunks::{Chunk, CHUNK_SIZE},
};

const BASE_HEIGHT: i64 = 16;
const HEIGHT_VARIATION: i64 = 12;
const NOISE_FREQUENCY: f64 = 0.01;
const DIRT_DEPTH: i64 = 3;
const TREE_TRUNK_HEIGHT: i64 = 7;
const TREE_NOISE_FREQUENCY: f64 = 0.055;
const TREE_NOISE_THRESHOLD: f64 = -0.05;
const TREE_MIN_TRUNK_DISTANCE: i64 = 4;

/// Generates deterministic terrain from a world seed.
#[derive(Clone, Debug)]
pub struct TerrainGenerator {
    seed: u64,
    noise: Perlin,
    biomes: BiomeGenerator,
    tree_noise: Perlin,
}

impl TerrainGenerator {
    pub fn new(seed: u64) -> Self {
        Self {
            seed,
            noise: Perlin::new(seed as u32 ^ (seed >> 32) as u32),
            biomes: BiomeGenerator::new(seed),
            tree_noise: Perlin::new((seed as u32).rotate_left(13) ^ 0xa5a5_5a5a),
        }
    }

    pub const fn seed(&self) -> u64 {
        self.seed
    }

    /// Returns the terrain surface height at a global horizontal coordinate.
    ///
    /// The same global coordinate is used regardless of which chunk requests
    /// it, so neighboring chunks always agree at their shared border.
    pub fn height_at(&self, x: i64, z: i64) -> i64 {
        let sample = self
            .noise
            .get([x as f64 * NOISE_FREQUENCY, z as f64 * NOISE_FREQUENCY]);
        BASE_HEIGHT + (sample * HEIGHT_VARIATION as f64).round() as i64
    }

    pub fn biome_at(&self, x: i64, z: i64) -> Biome {
        self.biomes.at(x, z)
    }

    /// Generates one chunk at the supplied chunk coordinates.
    pub fn generate_chunk(&self, coordinates: [i64; 3]) -> Chunk {
        let mut chunk = Chunk::new(coordinates);
        let chunk_size = CHUNK_SIZE as i64;
        let world_origin = [
            coordinates[0] * chunk_size,
            coordinates[1] * chunk_size,
            coordinates[2] * chunk_size,
        ];

        for z in 0..CHUNK_SIZE {
            for x in 0..CHUNK_SIZE {
                let world_x = world_origin[0] + x as i64;
                let world_z = world_origin[2] + z as i64;
                let surface = self.height_at(world_x, world_z);

                for y in 0..CHUNK_SIZE {
                    let world_y = world_origin[1] + y as i64;
                    let biome = self.biome_at(world_x, world_z);
                    let block = self.block_at_feature(world_y, surface, biome);
                    chunk.set(x, y, z, block);
                }
            }
        }

        self.place_trees(&mut chunk, world_origin);
        chunk
    }

    fn block_at_height(&self, y: i64, surface: i64, biome: Biome) -> Block {
        if y > surface {
            Block::AIR
        } else if y == surface {
            biome.surface_block()
        } else if y > surface - DIRT_DEPTH {
            Block::DIRT
        } else {
            Block::STONE
        }
    }

    fn block_at_feature(&self, y: i64, surface: i64, biome: Biome) -> Block {
        if !biome.is_water() {
            return self.block_at_height(y, surface, biome);
        }

        let water_level = if matches!(biome, Biome::Lake) {
            BASE_HEIGHT
        } else {
            surface
        };
        let floor = (water_level - 2).min(surface);
        if y <= floor {
            self.block_at_height(y, floor, biome)
        } else if y <= water_level {
            Block::WATER
        } else {
            Block::AIR
        }
    }

    fn place_trees(&self, chunk: &mut Chunk, world_origin: [i64; 3]) {
        let chunk_min_x = world_origin[0];
        let chunk_min_z = world_origin[2];
        let chunk_max_x = chunk_min_x + CHUNK_SIZE as i64 - 1;
        let chunk_max_z = chunk_min_z + CHUNK_SIZE as i64 - 1;

        for base_z in (chunk_min_z - 4)..=(chunk_max_z + 4) {
            for base_x in (chunk_min_x - 4)..=(chunk_max_x + 4) {
                if !self.biome_at(base_x, base_z).has_trees() || !self.has_tree_at(base_x, base_z) {
                    continue;
                }

                let surface = self.height_at(base_x, base_z);
                for trunk_y in 1..=TREE_TRUNK_HEIGHT {
                    self.set_world_block(
                        chunk,
                        world_origin,
                        [base_x, surface + trunk_y, base_z],
                        Block::LOG,
                    );
                }

                for (y, offsets) in canopy_layers(self.tree_variant(base_x, base_z)) {
                    let canopy_y = surface + TREE_TRUNK_HEIGHT - 2 + y;
                    for [dx, dz] in offsets {
                        let [rotated_x, rotated_z] =
                            rotate_offset(*dx, *dz, self.tree_rotation(base_x, base_z));
                        self.set_world_block(
                            chunk,
                            world_origin,
                            [base_x + rotated_x, canopy_y, base_z + rotated_z],
                            Block::LEAVES,
                        );
                    }
                }
            }
        }
    }

    fn has_tree_at(&self, x: i64, z: i64) -> bool {
        if !self.is_tree_candidate(x, z) {
            return false;
        }

        for dz in -TREE_MIN_TRUNK_DISTANCE..=TREE_MIN_TRUNK_DISTANCE {
            for dx in -TREE_MIN_TRUNK_DISTANCE..=TREE_MIN_TRUNK_DISTANCE {
                if dx == 0 && dz == 0
                    || dx * dx + dz * dz >= TREE_MIN_TRUNK_DISTANCE * TREE_MIN_TRUNK_DISTANCE
                {
                    continue;
                }
                if self.is_tree_candidate(x + dx, z + dz)
                    && self.tree_hash(x + dx, z + dz) < self.tree_hash(x, z)
                {
                    return false;
                }
            }
        }
        true
    }

    fn is_tree_candidate(&self, x: i64, z: i64) -> bool {
        let density = self.tree_noise.get([
            x as f64 * TREE_NOISE_FREQUENCY,
            z as f64 * TREE_NOISE_FREQUENCY,
        ]);
        let hash = (x as u64)
            .wrapping_mul(0x9e37_79b9_7f4a_7c15)
            .wrapping_add((z as u64).wrapping_mul(0xbf58_476d_1ce4_e5b9))
            .wrapping_add(self.seed);
        density > TREE_NOISE_THRESHOLD && hash.rotate_left(17) % 100 < 42
    }

    fn tree_variant(&self, x: i64, z: i64) -> u8 {
        self.tree_hash(x, z).rotate_left(23) as u8 % 3
    }

    fn tree_rotation(&self, x: i64, z: i64) -> u8 {
        (self.tree_hash(x, z).rotate_right(11) as u8) % 4
    }

    fn tree_hash(&self, x: i64, z: i64) -> u64 {
        (x as u64)
            .wrapping_mul(0x9e37_79b9_7f4a_7c15)
            .wrapping_add((z as u64).wrapping_mul(0xbf58_476d_1ce4_e5b9))
            .wrapping_add(self.seed)
    }

    fn set_world_block(
        &self,
        chunk: &mut Chunk,
        world_origin: [i64; 3],
        world: [i64; 3],
        block: Block,
    ) {
        let local = [
            world[0] - world_origin[0],
            world[1] - world_origin[1],
            world[2] - world_origin[2],
        ];
        if local
            .iter()
            .any(|&coordinate| !(0..CHUNK_SIZE as i64).contains(&coordinate))
        {
            return;
        }

        if chunk
            .get(local[0] as usize, local[1] as usize, local[2] as usize)
            .is_some_and(|current| !current.is_air())
        {
            return;
        }
        chunk.set(
            local[0] as usize,
            local[1] as usize,
            local[2] as usize,
            block,
        );
    }
}

fn canopy_layers(variant: u8) -> [(i64, &'static [[i64; 2]]); 5] {
    const ROUND_BOTTOM: [[i64; 2]; 9] = [
        [-1, -1],
        [0, -1],
        [1, -1],
        [-1, 0],
        [0, 0],
        [1, 0],
        [-1, 1],
        [0, 1],
        [1, 1],
    ];
    const ROUND_MIDDLE: [[i64; 2]; 5] = [[0, -1], [-1, 0], [0, 0], [1, 0], [0, 1]];
    const ROUND_TOP: [[i64; 2]; 1] = [[0, 0]];
    const CROSS_BOTTOM: [[i64; 2]; 5] = [[0, -2], [-2, 0], [0, 0], [2, 0], [0, 2]];
    const CROSS_MIDDLE: [[i64; 2]; 5] = [[-1, 0], [0, -1], [0, 0], [0, 1], [1, 0]];
    const CROSS_TOP: [[i64; 2]; 1] = [[0, 0]];
    const CONE_BOTTOM: [[i64; 2]; 9] = [
        [-1, -1],
        [0, -1],
        [1, -1],
        [-1, 0],
        [0, 0],
        [1, 0],
        [-1, 1],
        [0, 1],
        [1, 1],
    ];
    const CONE_MIDDLE: [[i64; 2]; 5] = [[0, -1], [-1, 0], [0, 0], [1, 0], [0, 1]];
    const CONE_TOP: [[i64; 2]; 1] = [[0, 0]];

    match variant {
        0 => [
            (0, &ROUND_BOTTOM),
            (1, &ROUND_BOTTOM),
            (2, &ROUND_MIDDLE),
            (3, &ROUND_MIDDLE),
            (4, &ROUND_TOP),
        ],
        1 => [
            (0, &CROSS_BOTTOM),
            (1, &CROSS_BOTTOM),
            (2, &CROSS_MIDDLE),
            (3, &CROSS_MIDDLE),
            (4, &CROSS_TOP),
        ],
        _ => [
            (0, &CONE_BOTTOM),
            (1, &CONE_BOTTOM),
            (2, &CONE_MIDDLE),
            (3, &CONE_MIDDLE),
            (4, &CONE_TOP),
        ],
    }
}

fn rotate_offset(x: i64, z: i64, rotation: u8) -> [i64; 2] {
    match rotation {
        1 => [-z, x],
        2 => [-x, -z],
        3 => [z, -x],
        _ => [x, z],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_generates_identical_chunks() {
        let generator = TerrainGenerator::new(0x0123_4567_89ab_cdef);
        let first = generator.generate_chunk([2, 0, -3]);
        let second = TerrainGenerator::new(generator.seed()).generate_chunk([2, 0, -3]);

        assert_eq!(first, second);
    }

    #[test]
    fn neighboring_chunks_have_continuous_borders() {
        let generator = TerrainGenerator::new(42);
        let left = generator.generate_chunk([0, 0, 0]);
        let right = generator.generate_chunk([1, 0, 0]);

        assert_column_matches_global_height(&generator, &left, 31, 0);
        assert_column_matches_global_height(&generator, &right, 32, 0);
    }

    #[test]
    fn negative_neighboring_chunks_have_continuous_borders() {
        let generator = TerrainGenerator::new(42);
        let left = generator.generate_chunk([-1, 0, 0]);
        let right = generator.generate_chunk([0, 0, 0]);

        assert_column_matches_global_height(&generator, &left, -1, 0);
        assert_column_matches_global_height(&generator, &right, 0, 0);
    }

    #[test]
    fn terrain_contains_expected_layers() {
        let generator = TerrainGenerator::new(7);
        let surface = generator.height_at(0, 0);
        let chunk = generator.generate_chunk([0, 0, 0]);
        let biome = generator.biome_at(0, 0);
        assert_eq!(
            chunk.get(0, surface as usize, 0),
            Some(generator.block_at_feature(surface, surface, biome))
        );
        assert_eq!(
            chunk.get(0, (surface - 1) as usize, 0),
            Some(generator.block_at_feature(surface - 1, surface, biome))
        );
        assert_eq!(
            chunk.get(0, (surface - 4) as usize, 0),
            Some(generator.block_at_feature(surface - 4, surface, biome))
        );
    }

    #[test]
    fn forest_biome_generates_trees() {
        let generator = TerrainGenerator::new(7);
        let (x, z) = (-500..500)
            .flat_map(|x| (-500..500).map(move |z| (x, z)))
            .find(|&(x, z)| {
                generator.biome_at(x, z) == Biome::Forest
                    && generator.has_tree_at(x, z)
                    && generator.height_at(x, z) >= 0
            })
            .expect("seed should contain a forest tree location");
        let surface = generator.height_at(x, z);
        let chunk = generator.generate_chunk([
            x.div_euclid(CHUNK_SIZE as i64),
            (surface + 1).div_euclid(CHUNK_SIZE as i64),
            z.div_euclid(CHUNK_SIZE as i64),
        ]);
        let local_x = x.rem_euclid(CHUNK_SIZE as i64) as usize;
        let local_z = z.rem_euclid(CHUNK_SIZE as i64) as usize;
        let local_y = (surface + 1).rem_euclid(CHUNK_SIZE as i64) as usize;

        assert_eq!(chunk.get(local_x, local_y, local_z), Some(Block::LOG));
    }

    #[test]
    fn forest_trees_have_tall_trunks_and_full_canopies() {
        let generator = TerrainGenerator::new(7);
        let (x, z) = (-500..500)
            .flat_map(|x| (-500..500).map(move |z| (x, z)))
            .find(|&(x, z)| {
                generator.biome_at(x, z) == Biome::Forest && generator.has_tree_at(x, z)
            })
            .expect("seed should contain a forest tree location");
        let surface = generator.height_at(x, z);
        let block_at = |world: [i64; 3]| {
            let coordinates = world.map(|coordinate| coordinate.div_euclid(CHUNK_SIZE as i64));
            let chunk = generator.generate_chunk(coordinates);
            let local = world.map(|coordinate| coordinate.rem_euclid(CHUNK_SIZE as i64) as usize);
            chunk.get(local[0], local[1], local[2])
        };

        for y in 1..=TREE_TRUNK_HEIGHT {
            assert_eq!(block_at([x, surface + y, z]), Some(Block::LOG));
        }

        let foliage_blocks = ((surface + TREE_TRUNK_HEIGHT - 2)
            ..=(surface + TREE_TRUNK_HEIGHT + 2))
            .flat_map(|y| (-4..=4).flat_map(move |dx| (-4..=4).map(move |dz| [x + dx, y, z + dz])))
            .filter(|&world| block_at(world) == Some(Block::LEAVES))
            .count();
        assert!(
            foliage_blocks >= 15,
            "large tree should have a substantial canopy, got {foliage_blocks} leaves"
        );
    }

    #[test]
    fn forest_tree_trunks_keep_a_clear_distance() {
        let generator = TerrainGenerator::new(7);
        let trees: Vec<_> = (-200..200)
            .flat_map(|x| (-200..200).map(move |z| (x, z)))
            .filter(|&(x, z)| {
                generator.biome_at(x, z) == Biome::Forest && generator.has_tree_at(x, z)
            })
            .collect();

        for (index, &(x, z)) in trees.iter().enumerate() {
            for &(other_x, other_z) in &trees[index + 1..] {
                let distance_squared = (x - other_x).pow(2) + (z - other_z).pow(2);
                assert!(
                    distance_squared >= TREE_MIN_TRUNK_DISTANCE.pow(2),
                    "tree trunks are too close: ({x}, {z}) and ({other_x}, {other_z})"
                );
            }
        }
    }

    #[test]
    fn forest_has_multiple_rotatable_canopy_shapes() {
        let generator = TerrainGenerator::new(7);
        let mut variants = [false; 3];
        let mut rotations = [false; 4];

        for x in -200..200 {
            for z in -200..200 {
                if generator.biome_at(x, z) == Biome::Forest && generator.has_tree_at(x, z) {
                    variants[generator.tree_variant(x, z) as usize] = true;
                    rotations[generator.tree_rotation(x, z) as usize] = true;
                }
            }
        }

        assert!(variants.iter().filter(|&&present| present).count() >= 2);
        assert!(rotations.iter().filter(|&&present| present).count() >= 2);
    }

    #[test]
    fn forest_has_woodland_density() {
        let generator = TerrainGenerator::new(7);
        let forest_positions: Vec<_> = (-200..200)
            .flat_map(|x| (-200..200).map(move |z| (x, z)))
            .filter(|&(x, z)| generator.biome_at(x, z) == Biome::Forest)
            .collect();
        let tree_count = forest_positions
            .iter()
            .filter(|&&(x, z)| generator.has_tree_at(x, z))
            .count();

        assert!(forest_positions.len() > 10_000);
        assert!(
            tree_count >= 100,
            "forest should contain many trees, got {tree_count} in {} forest positions",
            forest_positions.len()
        );
    }

    fn assert_column_matches_global_height(
        generator: &TerrainGenerator,
        chunk: &Chunk,
        world_x: i64,
        world_z: i64,
    ) {
        let surface = generator.height_at(world_x, world_z);
        for y in 0..CHUNK_SIZE {
            let world_y = y as i64;
            let expected =
                generator.block_at_feature(world_y, surface, generator.biome_at(world_x, world_z));
            let local_x = world_x.rem_euclid(CHUNK_SIZE as i64) as usize;
            let local_z = world_z.rem_euclid(CHUNK_SIZE as i64) as usize;
            assert_eq!(chunk.get(local_x, y, local_z), Some(expected));
        }
    }
}
