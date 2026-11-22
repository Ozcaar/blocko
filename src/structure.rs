use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
};

use anyhow::{Context, Result, bail, ensure};
use bevy::{
    prelude::*,
    tasks::{IoTaskPool, Task, futures::check_ready},
};

use crate::{
    editor::View,
    java::JavaWorld,
    world::{BlockEntity, BlockEntityRecord, BlockState, Chunk, ChunkPos},
};

pub const MIN_HORIZONTAL: i32 = -29_999_984;
pub const MAX_HORIZONTAL: i32 = 29_999_983;
pub const MIN_Y: i32 = -64;
pub const MAX_Y: i32 = 319;
const MAX_BLOCKS: u64 = 1_048_576;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Volume {
    pub min: [i32; 3],
    pub max: [i32; 3],
}

impl Volume {
    pub fn between(a: [i32; 3], b: [i32; 3]) -> Self {
        Self {
            min: std::array::from_fn(|axis| a[axis].min(b[axis])),
            max: std::array::from_fn(|axis| a[axis].max(b[axis])),
        }
    }

    pub fn dimensions(self) -> [u32; 3] {
        std::array::from_fn(|axis| self.max[axis].abs_diff(self.min[axis]).saturating_add(1))
    }

    pub fn validate(self) -> Result<()> {
        let within = |axis: usize, min, max| {
            (min..=max).contains(&self.min[axis])
                && (self.min[axis]..=max).contains(&self.max[axis])
        };
        ensure!(
            within(0, MIN_HORIZONTAL, MAX_HORIZONTAL)
                && within(1, MIN_Y, MAX_Y)
                && within(2, MIN_HORIZONTAL, MAX_HORIZONTAL),
            "Las esquinas deben estar dentro de los límites del Overworld"
        );
        let blocks = self
            .dimensions()
            .into_iter()
            .map(u64::from)
            .product::<u64>();
        ensure!(
            blocks <= MAX_BLOCKS,
            "La selección contiene {blocks} bloques y el límite por copia es de {MAX_BLOCKS}"
        );
        Ok(())
    }

    fn contains(self, position: [i32; 3]) -> bool {
        (0..3).all(|axis| (self.min[axis]..=self.max[axis]).contains(&position[axis]))
    }

    fn relative(self, position: [i32; 3]) -> [u32; 3] {
        std::array::from_fn(|axis| (position[axis] - self.min[axis]) as u32)
    }
}

pub struct Structure {
    pub source_name: String,
    pub volume: Volume,
    palette: Vec<BlockState>,
    // X varies fastest, followed by Z and then Y. Air has its own palette entry.
    blocks: Vec<u32>,
    block_entities: BTreeMap<[u32; 3], BlockEntity>,
}

impl Structure {
    pub fn block_count(&self) -> usize {
        debug_assert!(self.block_at([0; 3]).is_some());
        self.blocks.len()
    }

    pub fn block_entity_count(&self) -> usize {
        self.block_entities.len()
    }

    #[cfg(test)]
    pub fn block_entity_at(&self, position: [u32; 3]) -> Option<&BlockEntity> {
        self.block_entities.get(&position)
    }

    fn capture(
        source_name: String,
        volume: Volume,
        mut load_chunk: impl FnMut(ChunkPos) -> Result<Option<Chunk>>,
    ) -> Result<Self> {
        volume.validate()?;
        let [width, height, depth] = volume.dimensions().map(|dimension| dimension as usize);
        let air = BlockState {
            name: "minecraft:air".into(),
            properties: BTreeMap::new(),
        };
        let mut palette = vec![air.clone()];
        let mut indices = HashMap::from([(air, 0)]);
        let mut blocks = vec![0; width * height * depth];
        let mut block_entities = BTreeMap::new();

        let [min_x, min_y, min_z] = volume.min;
        let [max_x, max_y, max_z] = volume.max;
        let first = ChunkPos::from_block(min_x, min_z);
        let last = ChunkPos::from_block(max_x, max_z);
        for cz in first.z..=last.z {
            for cx in first.x..=last.x {
                let chunk = load_chunk(ChunkPos { x: cx, z: cz })?.with_context(|| {
                    format!("El chunk ({cx}, {cz}) no contiene terreno guardado")
                })?;
                if let Some(error) = chunk.block_entity_error {
                    bail!("No se pueden copiar los datos del chunk ({cx}, {cz}): {error}");
                }
                for (position, entity) in chunk.block_entities {
                    if !volume.contains(position) {
                        continue;
                    }
                    let [x, y, z] = position;
                    match entity {
                        BlockEntityRecord::Supported(entity) => {
                            block_entities.insert(volume.relative(position), entity);
                        }
                        BlockEntityRecord::Unsupported { kind, reason } => {
                            bail!(
                                "Entidad de bloque no compatible en ({x}, {y}, {z}): {kind}. {reason}"
                            );
                        }
                    }
                }
                let x0 = cx * 16;
                let z0 = cz * 16;
                for (sy, section) in chunk.sections {
                    let y0 = sy * 16;
                    for y in min_y.max(y0)..=max_y.min(y0 + 15) {
                        for z in min_z.max(z0)..=max_z.min(z0 + 15) {
                            for x in min_x.max(x0)..=max_x.min(x0 + 15) {
                                let local = ((y - y0) * 256 + (z - z0) * 16 + x - x0) as usize;
                                let state = &section.palette[section.blocks[local] as usize];
                                let relative = volume.relative([x, y, z]);
                                if state.name == "minecraft:chest"
                                    && state
                                        .properties
                                        .get("type")
                                        .is_some_and(|kind| kind != "single")
                                {
                                    bail!(
                                        "Cofre doble en ({x}, {y}, {z}). La copia admite cofres simples"
                                    );
                                }
                                if (state.name == "minecraft:chest"
                                    || state.name.ends_with("_sign"))
                                    && !block_entities.contains_key(&relative)
                                {
                                    bail!("Faltan los datos de {} en ({x}, {y}, {z})", state.name);
                                }
                                let index = if let Some(index) = indices.get(state) {
                                    *index
                                } else {
                                    let index = palette.len() as u32;
                                    indices.insert(state.clone(), index);
                                    palette.push(state.clone());
                                    index
                                };
                                let [x, y, z] = relative.map(|axis| axis as usize);
                                blocks[(y * depth + z) * width + x] = index;
                            }
                        }
                    }
                }
            }
        }
        Ok(Self {
            source_name,
            volume,
            palette,
            blocks,
            block_entities,
        })
    }

    pub fn block_at(&self, [x, y, z]: [u32; 3]) -> Option<&BlockState> {
        let [width, height, depth] = self.volume.dimensions();
        if x >= width || y >= height || z >= depth {
            return None;
        }
        let index = self.blocks[((y * depth + z) * width + x) as usize];
        Some(&self.palette[index as usize])
    }
}

struct CopyTask {
    window: Entity,
    source_name: String,
    task: Task<Result<Structure>>,
}

#[derive(Default, Resource)]
pub struct Clipboard {
    pub current: Option<Structure>,
    pending: Option<CopyTask>,
}

impl Clipboard {
    pub fn copying(&self) -> bool {
        self.pending.is_some()
    }

    pub fn request(
        &mut self,
        window: Entity,
        source: Arc<JavaWorld>,
        volume: Volume,
    ) -> Result<()> {
        ensure!(!self.copying(), "Hay una copia en curso");
        volume.validate()?;
        self.pending = Some(CopyTask {
            window,
            source_name: source.name.clone(),
            task: IoTaskPool::get().spawn(async move {
                Structure::capture(source.name.clone(), volume, |pos| source.load_chunk(pos))
            }),
        });
        Ok(())
    }
}

pub fn finish_copying(mut clipboard: ResMut<Clipboard>, mut views: Query<&mut View>) {
    let Some(pending) = &mut clipboard.pending else {
        return;
    };
    let Some(result) = check_ready(&mut pending.task) else {
        return;
    };
    let window = pending.window;
    let notice = match result {
        Ok(structure) => {
            clipboard.current = Some(structure);
            "Construcción copiada al portapapeles de Blocko".into()
        }
        Err(error) => format!("No se pudo copiar desde {}. {error:#}", pending.source_name),
    };
    clipboard.pending = None;
    if let Ok(mut view) = views.get_mut(window)
        && view.copying
    {
        view.copying = false;
        view.copy_notice = Some(notice);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::{ChestItem, Section, SignText};

    fn test_chunk(pos: ChunkPos) -> Chunk {
        let mut chunk = Chunk::default();
        if pos.x == -1 {
            chunk.sections.insert(
                -4,
                Section {
                    palette: vec![BlockState {
                        name: "minecraft:oak_stairs".into(),
                        properties: BTreeMap::from([("facing".into(), "east".into())]),
                    }],
                    blocks: Box::new([0; 4096]),
                },
            );
        } else {
            let mut blocks = Box::new([0; 4096]);
            blocks[0] = 1;
            blocks[16] = 2;
            chunk.sections.insert(
                -4,
                Section {
                    palette: ["minecraft:air", "minecraft:chest", "minecraft:oak_sign"]
                        .map(|name| BlockState {
                            name: name.into(),
                            properties: BTreeMap::new(),
                        })
                        .into(),
                    blocks,
                },
            );
            chunk.block_entities.insert(
                [0, -64, 0],
                BlockEntityRecord::Supported(BlockEntity::Chest {
                    items: vec![ChestItem {
                        slot: 2,
                        id: "minecraft:torch".into(),
                        count: 12,
                    }],
                    custom_name: None,
                    lock: None,
                }),
            );
            let face = |line: &str| SignText {
                messages: [line.into(), r#""""#.into(), r#""""#.into(), r#""""#.into()],
                filtered_messages: None,
                color: "black".into(),
                glowing: false,
            };
            chunk.block_entities.insert(
                [0, -64, 1],
                BlockEntityRecord::Supported(BlockEntity::Sign {
                    front: face(r#""Entrada""#),
                    back: face(r#""Salida""#),
                    waxed: true,
                }),
            );
            chunk.block_entities.insert(
                [1, -64, 0],
                BlockEntityRecord::Unsupported {
                    kind: "minecraft:furnace".into(),
                    reason: "Datos fuera del alcance".into(),
                },
            );
        }
        chunk
    }

    #[test]
    fn copies_reversed_bounds_across_zero_with_air_properties_and_relative_entity() -> Result<()> {
        let volume = Volume::between([0, -63, 1], [-1, -64, 0]);
        assert_eq!(volume.dimensions(), [2, 2, 2]);
        let copy = Structure::capture("Bosque".into(), volume, |pos| Ok(Some(test_chunk(pos))))?;
        assert_eq!(copy.block_count(), 8);
        assert_eq!(
            copy.block_at([0, 1, 0]).unwrap().properties["facing"],
            "east"
        );
        assert!(copy.block_at([1, 1, 0]).unwrap().is_air());
        assert!(matches!(
            &copy.block_entities[&[1, 0, 0]],
            BlockEntity::Chest { items, .. } if items[0].count == 12 && items[0].slot == 2
        ));
        assert!(matches!(
            &copy.block_entities[&[1, 0, 1]],
            BlockEntity::Sign { front, back, waxed: true }
                if front.messages[0] == r#""Entrada""# && back.messages[0] == r#""Salida""#
        ));
        Ok(())
    }

    #[test]
    fn refuses_unsupported_entities_and_missing_chunks() {
        let volume = Volume::between([1, -64, 0], [1, -64, 0]);
        let error = Structure::capture("Bosque".into(), volume, |pos| Ok(Some(test_chunk(pos))))
            .err()
            .unwrap()
            .to_string();
        assert!(error.contains("(1, -64, 0): minecraft:furnace"));

        let error = Structure::capture("Bosque".into(), volume, |_| Ok(None))
            .err()
            .unwrap()
            .to_string();
        assert!(error.contains("chunk (0, 0) no contiene terreno guardado"));
    }

    #[test]
    fn rejects_volumes_outside_the_overworld_or_copy_limit() {
        assert!(Volume::between([0, -65, 0], [0, 0, 0]).validate().is_err());
        assert!(
            Volume::between([0, 0, 0], [1_048_576, 0, 0])
                .validate()
                .is_err()
        );
        assert!(
            Volume::between([0, 0, 0], [1023, 1023, 0])
                .validate()
                .is_err()
        );
    }
}
