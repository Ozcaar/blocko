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

#[derive(Debug)]
pub struct BlockState {
    pub name: String,
    pub properties: BTreeMap<String, String>,
}

#[derive(Debug)]
pub struct Section {
    pub palette: Vec<BlockState>,
    pub blocks: Box<[u16; 4096]>,
}

#[derive(Debug)]
pub struct Chunk {
    pub sections: BTreeMap<i32, Section>,
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
