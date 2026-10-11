use noise::{NoiseFn, Perlin};

/// Biomes currently available to the terrain generator.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Biome {
    Plains,
    Forest,
    Lake,
    River,
}

impl Biome {
    pub const fn surface_block(self) -> super::blocks::Block {
        match self {
            Self::Lake | Self::River => super::blocks::Block::SAND,
            Self::Plains | Self::Forest => super::blocks::Block::GRASS,
        }
    }

    pub const fn has_trees(self) -> bool {
        matches!(self, Self::Forest)
    }

    pub const fn is_water(self) -> bool {
        matches!(self, Self::Lake | Self::River)
    }
}

/// Selects a biome from a stable world-space noise field.
#[derive(Clone, Debug)]
pub struct BiomeGenerator {
    noise: Perlin,
    river_noise: Perlin,
    seed: u64,
}

impl BiomeGenerator {
    pub fn new(seed: u64) -> Self {
        let mixed_seed = (seed as u32)
            .wrapping_mul(0x9e37_79b9)
            .wrapping_add((seed >> 32) as u32);
        Self {
            noise: Perlin::new(mixed_seed),
            river_noise: Perlin::new(mixed_seed ^ 0x6d2b_79f5),
            seed,
        }
    }

    pub fn at(&self, x: i64, z: i64) -> Biome {
        if self.is_lake(x, z) {
            return Biome::Lake;
        }
        if self.is_river(x, z) {
            return Biome::River;
        }
        let sample = self.noise.get([x as f64 * 0.004, z as f64 * 0.004]);
        if sample > 0.1 {
            Biome::Forest
        } else {
            Biome::Plains
        }
    }

    /// Circular basins are repeated on a coarse grid so generation remains
    /// deterministic and chunk-order independent.
    pub fn lake_at(&self, x: i64, z: i64) -> bool {
        self.is_lake(x, z)
    }

    pub fn river_at(&self, x: i64, z: i64) -> bool {
        self.is_river(x, z)
    }

    fn is_lake(&self, x: i64, z: i64) -> bool {
        const CELL: i64 = 192;
        const RADIUS: f64 = 15.0;
        let cell_x = x.div_euclid(CELL);
        let cell_z = z.div_euclid(CELL);
        for dz in -1..=1 {
            for dx in -1..=1 {
                let cx = cell_x + dx;
                let cz = cell_z + dz;
                let hash = self.feature_hash(cx, cz);
                let center_x = cx * CELL + 48 + (hash as i64 & 63);
                let center_z = cz * CELL + 48 + ((hash >> 8) as i64 & 63);
                let radius = RADIUS + ((hash >> 16) & 7) as f64;
                let distance_squared = (x - center_x) as f64 * (x - center_x) as f64
                    + (z - center_z) as f64 * (z - center_z) as f64;
                if distance_squared <= radius.max(RADIUS).powi(2) {
                    return true;
                }
            }
        }
        false
    }

    fn is_river(&self, x: i64, z: i64) -> bool {
        let center = self.river_noise.get([x as f64 * 0.006, 0.0]) * 34.0;
        let distance = (z as f64 - center).abs();
        distance
            < 2.0
                + self
                    .river_noise
                    .get([x as f64 * 0.018, z as f64 * 0.002])
                    .abs()
    }

    fn feature_hash(&self, x: i64, z: i64) -> u64 {
        (x as u64)
            .wrapping_mul(0x9e37_79b9_7f4a_7c15)
            .wrapping_add((z as u64).wrapping_mul(0xbf58_476d_1ce4_e5b9))
            .wrapping_add(self.seed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lake_feature_is_circular_and_deterministic() {
        let generator = BiomeGenerator::new(42);
        let mut lake = None;
        for z in -256..256 {
            for x in -256..256 {
                if generator.lake_at(x, z) {
                    lake = Some((x, z));
                    break;
                }
            }
            if lake.is_some() {
                break;
            }
        }
        let (x, z) = lake.expect("seed should produce a lake in the search area");
        assert!(generator.lake_at(x, z));
        assert_eq!(generator.at(x, z), Biome::Lake);
    }

    #[test]
    fn river_feature_is_available() {
        let generator = BiomeGenerator::new(42);
        assert!((-512..512).any(|x| (-64..64).any(|z| generator.river_at(x, z))));
    }
}
