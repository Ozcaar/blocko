use crate::{
    actions::Action, editor::View, paste, selection::Hit, structure::Volume, world::BlockState,
};

pub fn apply(view: &mut View, action: Action, hit: Hit, material: &BlockState) {
    if let Some((target, state)) = block_change(action, hit.position, hit.adjacent(), material) {
        paste::set_blocks(view, Volume::between(target, target), state, None, false);
    }
}

fn block_change(
    action: Action,
    pointed: [i32; 3],
    adjacent: [i32; 3],
    material: &BlockState,
) -> Option<([i32; 3], BlockState)> {
    match action {
        Action::PaintBlock => Some((adjacent, material.clone())),
        Action::EraseBlock => Some((
            pointed,
            BlockState {
                name: "minecraft:air".into(),
                properties: Default::default(),
            },
        )),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::{path::PathBuf, sync::Arc};

    use anyhow::{Context, Result};

    use super::*;
    use crate::{
        edit::{EditRequest, EditSession},
        java::JavaWorld,
        transaction,
        world::{BlockEntityRecord, BlockValue, ChunkPos},
    };

    #[test]
    fn places_at_the_face_and_erases_the_pointed_block() {
        let material = BlockState {
            name: "minecraft:oak_log".into(),
            properties: [("axis".into(), "x".into())].into(),
        };
        let pointed = [15, 64, -1];
        let adjacent = [16, 64, -1];
        assert_eq!(
            block_change(Action::PaintBlock, pointed, adjacent, &material),
            Some((adjacent, material.clone()))
        );
        let (target, air) = block_change(Action::EraseBlock, pointed, adjacent, &material).unwrap();
        assert_eq!(target, pointed);
        assert!(air.is_air());
        assert!(air.properties.is_empty());
        assert!(block_change(Action::Save, pointed, adjacent, &material).is_none());
    }

    #[test]
    #[ignore = "requires BLOCKO_TEST_WORLDS with a copy of Bosque in Java 1.21.1"]
    fn manual_changes_are_undoable_and_keep_the_chest_on_disk() -> Result<()> {
        let path =
            PathBuf::from(std::env::var_os("BLOCKO_TEST_WORLDS").context("Missing test worlds")?)
                .join("Bosque");
        assert!(transaction::pending(&path)?.is_empty());
        let world = Arc::new(JavaWorld::open(&path)?);
        let region = path.join("region/r.0.0.mca");
        let hash = transaction::hash_file(&region)?;
        let chest = [86, -23, 119];
        let above = [86, -22, 119];
        let pos = ChunkPos::from_block(chest[0], chest[2]);
        let original = world.load_chunk_for_copy(pos)?.unwrap();
        let Some(BlockEntityRecord::Supported(entity)) = original.block_entities.get(&chest) else {
            anyhow::bail!("Missing supported chest in the fixture");
        };
        let mut edits = EditSession::default();
        let material = BlockState {
            name: "minecraft:oak_log".into(),
            properties: [("axis".into(), "x".into())].into(),
        };
        for action in [Action::PaintBlock, Action::EraseBlock] {
            let (target, state) = block_change(action, chest, above, &material).unwrap();
            let volume = Volume::between(target, target);
            let request = EditRequest::Set {
                volume,
                value: Arc::new(BlockValue {
                    state,
                    entity: None,
                }),
                matching: None,
            };
            let operation = request.prepare(world.clone(), &edits.patch_in(volume))?;
            assert_eq!(edits.apply(operation)?, 1);
        }
        let visible = |edits: &EditSession| {
            let mut chunk = original.clone();
            if let Some(patch) = edits.patch_for(pos) {
                chunk.apply_patch(&patch);
            }
            chunk
        };
        let painted = visible(&edits);
        assert_eq!(painted.block(6, above[1], 7), Some(&material));
        assert!(painted.block(6, chest[1], 7).unwrap().is_air());
        assert!(!painted.block_entities.contains_key(&chest));
        assert_eq!(edits.undo()?, Some(1));
        assert!(matches!(
            visible(&edits).block_entities.get(&chest),
            Some(BlockEntityRecord::Supported(restored)) if restored == entity
        ));
        assert_eq!(edits.redo()?, Some(1));
        assert!(!visible(&edits).block_entities.contains_key(&chest));
        assert_eq!(edits.undo()?, Some(1));
        assert_eq!(edits.undo()?, Some(1));
        assert!(!edits.dirty());
        assert_eq!(transaction::hash_file(&region)?, hash);
        Ok(())
    }
}
