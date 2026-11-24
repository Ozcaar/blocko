use std::{
    collections::{BTreeMap, BTreeSet, HashMap, VecDeque},
    sync::Arc,
};

use anyhow::{Context, Result, ensure};

use crate::{
    java::JavaWorld,
    java_paste::validate_cell_change,
    placement::Placement,
    structure::Volume,
    world::{
        BlockEntity, BlockEntityRecord, BlockPatch, BlockState, BlockValue, Chunk, ChunkPatch,
        ChunkPos,
    },
};

pub const MAX_PENDING: usize = 1_048_576;
const HISTORY_OPERATIONS: usize = 64;
const HISTORY_CELLS: usize = MAX_PENDING;

#[derive(Default)]
pub struct EditSession {
    pending: BTreeMap<ChunkPos, BTreeMap<[i32; 3], Change>>,
    pending_cells: usize,
    invalidated: BTreeSet<ChunkPos>,
    undo: VecDeque<Operation>,
    redo: Vec<Operation>,
    history_cells: usize,
}

#[derive(Clone)]
struct Change {
    before: Arc<BlockValue>,
    after: Arc<BlockValue>,
}

#[derive(Default)]
pub struct Operation {
    cells: Vec<([i32; 3], Change)>,
}

pub enum EditRequest {
    Paste(Placement),
    Set {
        volume: Volume,
        value: Arc<BlockValue>,
        matching: Option<String>,
    },
}

impl EditRequest {
    pub fn volume(&self) -> Volume {
        match self {
            Self::Paste(placement) => placement.volume(),
            Self::Set { volume, .. } => *volume,
        }
    }

    pub fn prepare(self, source: Arc<JavaWorld>, snapshot: &BlockPatch) -> Result<Operation> {
        self.prepare_with(snapshot, |pos| source.load_chunk_for_copy(pos))
    }

    fn prepare_with(
        self,
        snapshot: &BlockPatch,
        mut load: impl FnMut(ChunkPos) -> Result<Option<Chunk>>,
    ) -> Result<Operation> {
        let volume = self.volume();
        volume.validate()?;
        let first = ChunkPos::from_block(volume.min[0], volume.min[2]);
        let last = ChunkPos::from_block(volume.max[0], volume.max[2]);
        let mut states = HashMap::new();
        let mut operation = Operation::default();
        for cz in first.z..=last.z {
            for cx in first.x..=last.x {
                let pos = ChunkPos { x: cx, z: cz };
                let mut chunk = load(pos)?.with_context(|| {
                    format!("El chunk ({cx}, {cz}) no contiene terreno guardado")
                })?;
                if let Some(patch) = snapshot.get(&pos) {
                    chunk.apply_patch(patch);
                }
                for z in volume.min[2].max(cz * 16)..=volume.max[2].min(cz * 16 + 15) {
                    for x in volume.min[0].max(cx * 16)..=volume.max[0].min(cx * 16 + 15) {
                        for y in volume.min[1]..=volume.max[1] {
                            let coordinate = [x, y, z];
                            let old = chunk.block(x.rem_euclid(16), y, z.rem_euclid(16));
                            let Some(value) = self.replacement(coordinate, old, &mut states) else {
                                continue;
                            };
                            if !validate_cell_change(&chunk, coordinate, &value)? {
                                continue;
                            }
                            let entity = match chunk.block_entities.get(&coordinate) {
                                Some(BlockEntityRecord::Supported(entity)) => Some(entity),
                                _ => None,
                            };
                            let before = shared_value(
                                &mut states,
                                old.context("Falta el bloque de destino")?,
                                entity,
                            );
                            operation.cells.push((
                                coordinate,
                                Change {
                                    before,
                                    after: value,
                                },
                            ));
                        }
                    }
                }
            }
        }
        ensure!(
            operation.len() != 0,
            "La operación no cambiaría ningún bloque ni entidad"
        );
        Ok(operation)
    }

    fn replacement(
        &self,
        coordinate: [i32; 3],
        old: Option<&BlockState>,
        states: &mut HashMap<BlockState, Arc<BlockValue>>,
    ) -> Option<Arc<BlockValue>> {
        match self {
            Self::Paste(placement) => {
                let state = placement.writes_block(coordinate)?;
                let relative = std::array::from_fn(|axis| {
                    (coordinate[axis] - placement.origin()[axis]) as u32
                });
                let entity = placement.source.block_entity_at(relative);
                Some(shared_value(states, state, entity))
            }
            Self::Set {
                value, matching, ..
            } => matching
                .as_ref()
                .is_none_or(|id| old.is_some_and(|block| block.name == *id))
                .then(|| value.clone()),
        }
    }
}

fn shared_value(
    states: &mut HashMap<BlockState, Arc<BlockValue>>,
    state: &BlockState,
    entity: Option<&BlockEntity>,
) -> Arc<BlockValue> {
    if let Some(entity) = entity {
        return Arc::new(BlockValue {
            state: state.clone(),
            entity: Some(entity.clone()),
        });
    }
    if let Some(value) = states.get(state) {
        return value.clone();
    }
    let value = Arc::new(BlockValue {
        state: state.clone(),
        entity: None,
    });
    states.insert(state.clone(), value.clone());
    value
}

impl Operation {
    pub fn len(&self) -> usize {
        self.cells.len()
    }
}

impl EditSession {
    pub fn dirty(&self) -> bool {
        self.pending_cells != 0
    }
    pub fn pending_count(&self) -> usize {
        self.pending_cells
    }
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn patch(&self) -> BlockPatch {
        self.pending
            .iter()
            .map(|(&pos, changes)| (pos, chunk_patch(changes)))
            .collect()
    }

    pub fn patch_in(&self, volume: Volume) -> BlockPatch {
        let first = ChunkPos::from_block(volume.min[0], volume.min[2]);
        let last = ChunkPos::from_block(volume.max[0], volume.max[2]);
        self.pending
            .range(first..=last)
            .filter(|(pos, _)| (first.z..=last.z).contains(&pos.z))
            .map(|(&pos, changes)| {
                let patch: ChunkPatch = changes
                    .iter()
                    .filter(|(coordinate, _)| {
                        (0..3).all(|axis| {
                            (volume.min[axis]..=volume.max[axis]).contains(&coordinate[axis])
                        })
                    })
                    .map(|(&coordinate, change)| (coordinate, change.after.clone()))
                    .collect();
                (pos, patch)
            })
            .filter(|(_, patch)| !patch.is_empty())
            .collect()
    }

    pub fn patch_for(&self, pos: ChunkPos) -> Option<ChunkPatch> {
        self.pending.get(&pos).map(chunk_patch)
    }

    pub fn contains(&self, coordinate: [i32; 3]) -> bool {
        self.pending
            .get(&ChunkPos::from_block(coordinate[0], coordinate[2]))
            .is_some_and(|chunk| chunk.contains_key(&coordinate))
    }

    pub fn take_invalidated(&mut self) -> BTreeSet<ChunkPos> {
        std::mem::take(&mut self.invalidated)
    }

    pub fn apply(&mut self, operation: Operation) -> Result<usize> {
        let count = operation.len();
        self.apply_changes(&operation, false)?;
        self.history_cells -= self.redo.drain(..).map(|item| item.len()).sum::<usize>();
        while self.undo.len() >= HISTORY_OPERATIONS || self.history_cells + count > HISTORY_CELLS {
            let Some(old) = self.undo.pop_front() else {
                break;
            };
            self.history_cells -= old.len();
        }
        self.history_cells += count;
        self.undo.push_back(operation);
        Ok(count)
    }

    pub fn undo(&mut self) -> Result<Option<usize>> {
        let Some(operation) = self.undo.pop_back() else {
            return Ok(None);
        };
        if let Err(error) = self.apply_changes(&operation, true) {
            self.undo.push_back(operation);
            return Err(error);
        }
        let count = operation.len();
        self.redo.push(operation);
        Ok(Some(count))
    }

    pub fn redo(&mut self) -> Result<Option<usize>> {
        let Some(operation) = self.redo.pop() else {
            return Ok(None);
        };
        if let Err(error) = self.apply_changes(&operation, false) {
            self.redo.push(operation);
            return Err(error);
        }
        let count = operation.len();
        self.undo.push_back(operation);
        Ok(Some(count))
    }

    pub fn mark_saved(&mut self) {
        self.invalidated.extend(self.pending.keys().copied());
        self.pending.clear();
        self.pending_cells = 0;
    }

    fn apply_changes(&mut self, operation: &Operation, undo: bool) -> Result<()> {
        let mut next_count = self.pending_cells as isize;
        for (coordinate, change) in &operation.cells {
            let (from, to) = transition(change, undo);
            let pos = ChunkPos::from_block(coordinate[0], coordinate[2]);
            let previous = self
                .pending
                .get(&pos)
                .and_then(|chunk| chunk.get(coordinate));
            ensure!(
                previous.is_none_or(|change| &change.after == from),
                "La edición cambió durante la operación"
            );
            let baseline = previous.map_or(from, |change| &change.before);
            next_count += isize::from(to != baseline) - isize::from(previous.is_some());
        }
        ensure!(
            next_count <= MAX_PENDING as isize,
            "Hay demasiados cambios pendientes. Guarda el mundo antes de seguir"
        );
        for (coordinate, change) in &operation.cells {
            let coordinate = *coordinate;
            let (from, to) = transition(change, undo);
            let pos = ChunkPos::from_block(coordinate[0], coordinate[2]);
            let chunk = self.pending.entry(pos).or_default();
            let baseline = chunk.get(&coordinate).map_or(from, |change| &change.before);
            if to == baseline {
                chunk.remove(&coordinate);
                if chunk.is_empty() {
                    self.pending.remove(&pos);
                }
            } else {
                chunk.insert(
                    coordinate,
                    Change {
                        before: baseline.clone(),
                        after: to.clone(),
                    },
                );
            }
            self.invalidated.insert(pos);
        }
        self.pending_cells = next_count as usize;
        Ok(())
    }
}

fn transition(change: &Change, undo: bool) -> (&Arc<BlockValue>, &Arc<BlockValue>) {
    if undo {
        (&change.after, &change.before)
    } else {
        (&change.before, &change.after)
    }
}

fn chunk_patch(changes: &BTreeMap<[i32; 3], Change>) -> ChunkPatch {
    changes
        .iter()
        .map(|(&coordinate, change)| (coordinate, change.after.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value(name: &str) -> Arc<BlockValue> {
        Arc::new(BlockValue {
            state: BlockState {
                name: format!("minecraft:{name}"),
                properties: BTreeMap::new(),
            },
            entity: None,
        })
    }

    #[test]
    fn region_replace_uses_effective_types_and_records_a_single_operation() -> Result<()> {
        let pos = ChunkPos { x: 0, z: 0 };
        let stone = value("stone");
        let dirt = value("dirt");
        let gold = value("gold_block");
        let mut blocks = Box::new([0; 4_096]);
        blocks[1] = 1;
        let chunk = Chunk {
            sections: BTreeMap::from([(
                4,
                crate::world::Section {
                    palette: vec![stone.state.clone(), dirt.state.clone()],
                    blocks,
                },
            )]),
            block_entities: BTreeMap::new(),
            block_entity_error: None,
        };
        let mut edit = EditSession::default();
        let third = [2, 64, 0];
        let volume = Volume::between([0, 64, 0], third);
        edit.apply(Operation {
            cells: vec![(
                third,
                Change {
                    before: stone.clone(),
                    after: dirt.clone(),
                },
            )],
        })?;
        let replace = EditRequest::Set {
            volume,
            value: gold.clone(),
            matching: Some(stone.state.name.clone()),
        };
        let operation =
            replace.prepare_with(&edit.patch_in(volume), |_| Ok(Some(chunk.clone())))?;
        assert_eq!(operation.len(), 1);
        edit.apply(operation)?;
        let patch = edit.patch();
        assert_eq!(patch[&pos][&[0, 64, 0]], gold);
        assert!(!patch[&pos].contains_key(&[1, 64, 0]));
        assert_eq!(patch[&pos][&third], dirt);

        let fill = EditRequest::Set {
            volume,
            value: gold,
            matching: None,
        };
        let operation = fill.prepare_with(&edit.patch_in(volume), |_| Ok(Some(chunk.clone())))?;
        assert_eq!(operation.len(), 2);
        edit.apply(operation)?;
        assert_eq!(edit.pending_count(), 3);
        assert_eq!(edit.undo()?, Some(2));
        assert_eq!(edit.patch(), patch);
        assert_eq!(edit.redo()?, Some(2));
        assert_eq!(edit.pending_count(), 3);
        Ok(())
    }

    #[test]
    fn undo_rebases_against_the_last_successful_save() -> Result<()> {
        let before = value("stone");
        let after = value("dirt");
        let mut edit = EditSession::default();
        let coordinate = [0, 64, 0];
        let pos = ChunkPos { x: 0, z: 0 };
        edit.apply(Operation {
            cells: vec![(
                coordinate,
                Change {
                    before: before.clone(),
                    after: after.clone(),
                },
            )],
        })?;
        assert!(edit.dirty());
        edit.undo()?;
        assert!(!edit.dirty());
        edit.redo()?;
        assert_eq!(edit.patch()[&pos][&coordinate], after);
        edit.mark_saved();
        assert!(!edit.dirty());
        edit.undo()?;
        assert_eq!(edit.patch()[&pos][&coordinate], before);
        edit.redo()?;
        assert!(!edit.dirty());
        Ok(())
    }
}
