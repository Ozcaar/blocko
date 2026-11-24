use std::{collections::BTreeMap, sync::Arc};

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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BlockValue {
    pub state: BlockState,
    pub entity: Option<BlockEntity>,
}

pub type ChunkPatch = BTreeMap<[i32; 3], Arc<BlockValue>>;
pub type BlockPatch = BTreeMap<ChunkPos, ChunkPatch>;

#[derive(Clone, Debug)]
pub enum BlockEntityRecord {
    Supported(BlockEntity),
    Unsupported { kind: String, reason: String },
}

#[derive(Clone, Debug)]
pub struct Section {
    pub palette: Vec<BlockState>,
    pub blocks: Box<[u16; 4096]>,
}

#[derive(Clone, Debug, Default)]
pub struct Chunk {
    pub sections: BTreeMap<i32, Section>,
    pub block_entities: BTreeMap<[i32; 3], BlockEntityRecord>,
    pub block_entity_error: Option<String>,
}

impl Chunk {
    pub fn apply_patch(&mut self, patch: &ChunkPatch) {
        use std::collections::HashMap;

        let mut indices = HashMap::new();
        for (&[x, y, z], value) in patch {
            let sy = y.div_euclid(16);
            let Some(section) = self.sections.get_mut(&sy) else {
                continue;
            };
            let palette = indices.entry(sy).or_insert_with(|| {
                section
                    .palette
                    .iter()
                    .cloned()
                    .enumerate()
                    .map(|(index, state)| (state, index as u16))
                    .collect::<HashMap<_, _>>()
            });
            let index = if let Some(&index) = palette.get(&value.state) {
                index
            } else {
                let index = section.palette.len() as u16;
                palette.insert(value.state.clone(), index);
                section.palette.push(value.state.clone());
                index
            };
            let cell = (y.rem_euclid(16) * 256 + z.rem_euclid(16) * 16 + x.rem_euclid(16)) as usize;
            section.blocks[cell] = index;
            if let Some(entity) = &value.entity {
                self.block_entities
                    .insert([x, y, z], BlockEntityRecord::Supported(entity.clone()));
            } else {
                self.block_entities.remove(&[x, y, z]);
            }
        }
    }

    pub fn block(&self, x: i32, y: i32, z: i32) -> Option<&BlockState> {
        if !(0..16).contains(&x) || !(0..16).contains(&z) {
            return None;
        }
        let section = self.sections.get(&y.div_euclid(16))?;
        let index = (y.rem_euclid(16) * 256 + z * 16 + x) as usize;
        section.palette.get(section.blocks[index] as usize)
    }
}
