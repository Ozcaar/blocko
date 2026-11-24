use std::{
    collections::{BTreeMap, HashMap},
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context, Result, anyhow, ensure};

use crate::{
    anvil_write::RegionImage,
    java::JavaWorld,
    java_paste,
    transaction::{self, RegionEdit},
    world::{BlockPatch, ChunkPatch, ChunkPos},
};

const MAX_PREPARED_BYTES: usize = 256 * 1024 * 1024;

pub struct WorldBaseline {
    level: blake3::Hash,
    regions: HashMap<String, blake3::Hash>,
}

impl WorldBaseline {
    pub fn capture(root: &Path, level: blake3::Hash) -> Result<Self> {
        let path = transaction::plain_dir(&root.join("region"))?;
        let mut regions = HashMap::new();
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let name = entry.file_name();
            if let Some(name) = name.to_str().filter(|name| transaction::region_name(name)) {
                regions.insert(name.into(), transaction::hash_file(&entry.path())?);
            }
        }
        let baseline = Self { level, regions };
        baseline.check_level(root)?;
        Ok(baseline)
    }

    fn check_level(&self, root: &Path) -> Result<()> {
        transaction::plain_dir(root)?;
        ensure!(
            transaction::hash_file(&root.join("level.dat"))? == self.level,
            "level.dat cambió desde que se abrió el mundo. Vuelve a abrirlo"
        );
        Ok(())
    }

    fn check_region(&self, name: &str, current: blake3::Hash) -> Result<()> {
        let expected = self
            .regions
            .get(name)
            .with_context(|| format!("La región {name} no existía al abrir el mundo"))?;
        ensure!(
            &current == expected,
            "La región {name} cambió desde que se abrió el mundo. Vuelve a abrirlo"
        );
        Ok(())
    }
}

pub struct WorldSave {
    pub backup: Option<PathBuf>,
    pub blocks: usize,
}

pub fn save_changes(world: Arc<JavaWorld>, patch: &BlockPatch) -> Result<WorldSave> {
    let (edits, blocks) = prepare(&world, patch)?;
    let backup = if edits.is_empty() {
        None
    } else {
        Some(commit(&world, edits)?)
    };
    Ok(WorldSave { backup, blocks })
}

fn prepare(world: &JavaWorld, patch: &BlockPatch) -> Result<(Vec<RegionEdit>, usize)> {
    let mut groups = BTreeMap::<[i32; 2], Vec<(ChunkPos, &ChunkPatch)>>::new();
    for (&pos, cells) in patch.iter().filter(|(_, cells)| !cells.is_empty()) {
        groups
            .entry([pos.x.div_euclid(32), pos.z.div_euclid(32)])
            .or_default()
            .push((pos, cells));
    }
    if groups.is_empty() {
        return Ok((Vec::new(), 0));
    }
    world.with_read_lock(|| {
        let locked = world
            .baseline
            .lock()
            .map_err(|_| anyhow!("Se interrumpió la validación de destino"))?;
        let baseline = locked
            .as_ref()
            .map_err(|error| anyhow!("Este mundo no se puede guardar: {error}"))?;
        baseline.check_level(&world.root)?;
        ensure!(
            transaction::pending(&world.root)?.is_empty(),
            "El mundo tiene una restauración pendiente"
        );
        let directory = transaction::plain_dir(&world.root.join("region"))?;
        let mut total_bytes = 0;
        let mut total_blocks = 0;
        let mut edits = Vec::new();
        for ([rx, rz], chunks) in groups {
            let name = format!("r.{rx}.{rz}.mca");
            let path = directory.join(&name);
            let original = transaction::read_bounded(&path, transaction::MAX_REGION_BYTES)?;
            let before = blake3::hash(&original);
            baseline.check_region(&name, before)?;
            let mut image =
                RegionImage::new(original, [rx, rz]).with_context(|| format!("Región {name}"))?;
            let mut region_blocks = 0;
            for (pos, cells) in chunks {
                let nbt = image
                    .read_chunk(pos)
                    .with_context(|| format!("Chunk destino ({}, {})", pos.x, pos.z))?;
                if let Some((edited, blocks)) = java_paste::edit_chunk_patch(&nbt, pos, cells)
                    .with_context(|| format!("Chunk destino ({}, {})", pos.x, pos.z))?
                {
                    image.write_chunk(pos, &edited)?;
                    region_blocks += blocks;
                }
            }
            if region_blocks > 0 {
                let bytes = image.into_bytes();
                total_bytes += bytes.len();
                total_blocks += region_blocks;
                ensure!(
                    total_bytes <= MAX_PREPARED_BYTES,
                    "El guardado requiere preparar más de 256 MiB de regiones"
                );
                edits.push(RegionEdit {
                    name,
                    before,
                    bytes,
                });
            }
        }
        baseline.check_level(&world.root)?;
        Ok((edits, total_blocks))
    })
}

fn commit(world: &JavaWorld, edits: Vec<RegionEdit>) -> Result<PathBuf> {
    world.with_write_lock(|| {
        let mut slot = world
            .baseline
            .lock()
            .map_err(|_| anyhow!("Se interrumpió la validación de destino"))?;
        let baseline = slot
            .as_mut()
            .map_err(|error| anyhow!("Este mundo no se puede guardar: {error}"))?;
        baseline.check_level(&world.root)?;
        ensure!(
            transaction::pending(&world.root)?.is_empty(),
            "El mundo tiene una restauración pendiente"
        );
        let directory = transaction::plain_dir(&world.root.join("region"))?;
        for edit in &edits {
            let path = directory.join(&edit.name);
            let current = transaction::hash_file(&path)?;
            baseline.check_region(&edit.name, current)?;
            ensure!(
                current == edit.before,
                "La región {} cambió desde que se preparó el guardado. Vuelve a abrir el mundo",
                edit.name
            );
        }
        let known: Vec<_> = edits
            .iter()
            .map(|edit| (edit.name.clone(), blake3::hash(&edit.bytes)))
            .collect();
        let backup = transaction::commit(&world.root, baseline.level, edits)?;
        for (name, after) in known {
            baseline.regions.insert(name, after);
        }
        Ok(backup)
    })
}
