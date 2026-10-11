/// The compact value stored for a block in a chunk.
pub type BlockId = u8;

/// A voxel block identified by its compact numeric ID.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(transparent)]
pub struct Block {
    id: BlockId,
}

impl Block {
    /// The empty block ID.
    pub const AIR: Self = Self { id: 0 };
    pub const GRASS: Self = Self { id: 1 };
    pub const DIRT: Self = Self { id: 2 };
    pub const STONE: Self = Self { id: 3 };
    pub const LOG: Self = Self { id: 4 };
    pub const LEAVES: Self = Self { id: 5 };
    pub const SAND: Self = Self { id: 6 };
    pub const WATER: Self = Self { id: 7 };

    pub const fn new(id: BlockId) -> Self {
        Self { id }
    }

    pub const fn id(self) -> BlockId {
        self.id
    }

    pub const fn is_air(self) -> bool {
        self.id == Self::AIR.id
    }

    pub const fn is_water(self) -> bool {
        self.id == Self::WATER.id
    }

    pub const fn is_solid(self) -> bool {
        !self.is_air() && !self.is_water()
    }

    pub const fn is_transparent(self) -> bool {
        self.is_air() || self.is_water()
    }
}

impl From<BlockId> for Block {
    fn from(id: BlockId) -> Self {
        Self::new(id)
    }
}

impl From<Block> for BlockId {
    fn from(block: Block) -> Self {
        block.id
    }
}
