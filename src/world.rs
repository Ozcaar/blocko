use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ChunkPos {
    pub x: i32,
    pub z: i32,
}

impl ChunkPos {
    pub fn from_block(x: i32, z: i32) -> Self {
        Self {
            x: x.div_euclid(16),
            z: z.div_euclid(16),
        }
    }

    pub fn neighbors(self) -> [Self; 4] {
        [
            Self {
                x: self.x - 1,
                ..self
            },
            Self {
                x: self.x + 1,
                ..self
            },
            Self {
                z: self.z - 1,
                ..self
            },
            Self {
                z: self.z + 1,
                ..self
            },
        ]
    }

    pub fn distance(self, other: Self) -> u32 {
        self.x.abs_diff(other.x).max(self.z.abs_diff(other.z))
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct BlockState {
    pub name: String,
    pub properties: BTreeMap<String, String>,
}

impl BlockState {
    pub fn is_air(&self) -> bool {
        matches!(
            self.name.as_str(),
            "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChestItem {
    pub slot: u8,
    pub id: String,
    pub count: i32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SignText {
    pub messages: [String; 4],
    pub filtered_messages: Option<[String; 4]>,
    pub color: String,
    pub glowing: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BlockEntity {
    Chest {
        items: Vec<ChestItem>,
        custom_name: Option<String>,
        lock: Option<String>,
    },
    Sign {
        front: SignText,
        back: SignText,
        waxed: bool,
    },
}

#[derive(Debug)]
pub enum BlockEntityRecord {
    Supported(BlockEntity),
    Unsupported { kind: String, reason: String },
}

#[derive(Debug)]
pub struct Section {
    pub palette: Vec<BlockState>,
    pub blocks: Box<[u16; 4096]>,
}

#[derive(Debug, Default)]
pub struct Chunk {
    pub sections: BTreeMap<i32, Section>,
    pub block_entities: BTreeMap<[i32; 3], BlockEntityRecord>,
    pub block_entity_error: Option<String>,
}

impl Chunk {
    pub fn block(&self, x: i32, y: i32, z: i32) -> Option<&BlockState> {
        if !(0..16).contains(&x) || !(0..16).contains(&z) {
            return None;
        }
        let section = self.sections.get(&y.div_euclid(16))?;
        let index = (y.rem_euclid(16) * 256 + z * 16 + x) as usize;
        section.palette.get(section.blocks[index] as usize)
    }
}
