use super::blocks::{Block, BlockId};

pub const CHUNK_SIZE: usize = 32;
const CHUNK_VOLUME: usize = CHUNK_SIZE * CHUNK_SIZE * CHUNK_SIZE;

/// Converts a world-space coordinate to its chunk coordinate and local offset.
///
/// `div_euclid` and `rem_euclid` are important here: unlike `/` and `%`, they
/// keep the local coordinate in `0..CHUNK_SIZE` for negative world positions.
pub fn world_to_chunk_local(world: [i64; 3]) -> ([i64; 3], [usize; 3]) {
    let size = CHUNK_SIZE as i64;
    let chunk = world.map(|coordinate| coordinate.div_euclid(size));
    let local = world.map(|coordinate| coordinate.rem_euclid(size) as usize);
    (chunk, local)
}

/// A 32³ voxel region stored as one compact block ID per voxel.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Chunk {
    coordinates: [i64; 3],
    blocks: Vec<BlockId>,
}

impl Chunk {
    pub fn new(coordinates: [i64; 3]) -> Self {
        Self {
            coordinates,
            blocks: vec![Block::AIR.id(); CHUNK_VOLUME],
        }
    }

    pub const fn coordinates(&self) -> [i64; 3] {
        self.coordinates
    }

    pub const fn len(&self) -> usize {
        CHUNK_VOLUME
    }

    pub fn get(&self, x: usize, y: usize, z: usize) -> Option<Block> {
        self.index(x, y, z)
            .map(|index| Block::new(self.blocks[index]))
    }

    pub fn set(&mut self, x: usize, y: usize, z: usize, block: Block) -> bool {
        let Some(index) = self.index(x, y, z) else {
            return false;
        };
        self.blocks[index] = block.id();
        true
    }

    fn index(&self, x: usize, y: usize, z: usize) -> Option<usize> {
        if x >= CHUNK_SIZE || y >= CHUNK_SIZE || z >= CHUNK_SIZE {
            return None;
        }

        Some(x + CHUNK_SIZE * (y + CHUNK_SIZE * z))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_chunk_is_air_and_has_expected_storage_size() {
        let chunk = Chunk::new([4, -2, 9]);

        assert_eq!(chunk.coordinates(), [4, -2, 9]);
        assert_eq!(chunk.len(), 32 * 32 * 32);
        assert_eq!(chunk.get(0, 0, 0), Some(Block::AIR));
        assert_eq!(chunk.get(31, 31, 31), Some(Block::AIR));
    }

    #[test]
    fn set_and_get_store_the_requested_block() {
        let mut chunk = Chunk::new([0, 0, 0]);
        let stone = Block::new(3);

        assert!(chunk.set(31, 7, 12, stone));
        assert_eq!(chunk.get(31, 7, 12), Some(stone));
        assert_eq!(chunk.get(30, 7, 12), Some(Block::AIR));
    }

    #[test]
    fn local_bounds_are_rejected_without_mutating_the_chunk() {
        let mut chunk = Chunk::new([0, 0, 0]);
        let stone = Block::new(3);

        assert!(!chunk.set(32, 0, 0, stone));
        assert!(!chunk.set(0, 32, 0, stone));
        assert!(!chunk.set(0, 0, 32, stone));
        assert_eq!(chunk.get(32, 0, 0), None);
        assert_eq!(chunk.get(0, 32, 0), None);
        assert_eq!(chunk.get(0, 0, 32), None);
        assert_eq!(chunk.get(0, 0, 0), Some(Block::AIR));
    }

    #[test]
    fn negative_world_coordinates_use_euclidean_chunking() {
        assert_eq!(
            world_to_chunk_local([-1, -32, -33]),
            ([-1, -1, -2], [31, 0, 31])
        );
        assert_eq!(world_to_chunk_local([-64, 63, 0]), ([-2, 1, 0], [0, 31, 0]));
    }
}
