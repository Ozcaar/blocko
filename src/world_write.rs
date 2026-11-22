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
    placement::{AirMode, Placement},
    structure::Volume,
    transaction::{self, RegionEdit},
    world::ChunkPos,
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

pub struct PasteSummary {
    pub world_name: String,
    pub world_path: PathBuf,
    pub volume: Volume,
    pub mode: AirMode,
    pub chunks: usize,
    pub regions: usize,
}

pub struct PastePlan {
    pub summary: PasteSummary,
    pub(crate) world: Arc<JavaWorld>,
    pub(crate) preview: Placement,
    edits: Vec<RegionEdit>,
}

pub struct PasteResult {
    pub summary: PasteSummary,
    pub backup: PathBuf,
}

pub fn prepare(world: Arc<JavaWorld>, preview: Placement) -> Result<PastePlan> {
    let volume = preview.volume();
    volume.validate()?;
    let (summary, edits) = world.with_read_lock(|| {
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
        let start = ChunkPos::from_block(volume.min[0], volume.min[2]);
        let end = ChunkPos::from_block(volume.max[0], volume.max[2]);
        let mut groups = BTreeMap::<[i32; 2], Vec<ChunkPos>>::new();
        for z in start.z..=end.z {
            for x in start.x..=end.x {
                groups
                    .entry([x.div_euclid(32), z.div_euclid(32)])
                    .or_default()
                    .push(ChunkPos { x, z });
            }
        }
        let mut total_bytes = 0;
        let mut changed_chunks = 0;
        let mut edits = Vec::new();
        for ([rx, rz], chunks) in groups {
            let name = format!("r.{rx}.{rz}.mca");
            let path = directory.join(&name);
            let original = transaction::read_bounded(&path, transaction::MAX_REGION_BYTES)?;
            let before = blake3::hash(&original);
            baseline.check_region(&name, before)?;
            let mut image =
                RegionImage::new(original, [rx, rz]).with_context(|| format!("Región {name}"))?;
            let mut changed = false;
            for pos in chunks {
                let nbt = image
                    .read_chunk(pos)
                    .with_context(|| format!("Chunk destino ({}, {})", pos.x, pos.z))?;
                if let Some(edited) = java_paste::edit_chunk(&nbt, pos, &preview)
                    .with_context(|| format!("Chunk destino ({}, {})", pos.x, pos.z))?
                {
                    image.write_chunk(pos, &edited)?;
                    changed = true;
                    changed_chunks += 1;
                }
            }
            if changed {
                let bytes = image.into_bytes();
                total_bytes += bytes.len();
                ensure!(
                    total_bytes <= MAX_PREPARED_BYTES,
                    "El pegado requiere preparar más de 256 MiB de regiones"
                );
                edits.push(RegionEdit {
                    name,
                    before,
                    bytes,
                });
            }
        }
        ensure!(
            !edits.is_empty(),
            "La vista previa no cambiaría ningún bloque ni entidad"
        );
        baseline.check_level(&world.root)?;
        Ok((
            PasteSummary {
                world_name: world.name.clone(),
                world_path: world.root.clone(),
                volume,
                mode: preview.mode,
                chunks: changed_chunks,
                regions: edits.len(),
            },
            edits,
        ))
    })?;
    Ok(PastePlan {
        summary,
        world,
        preview,
        edits,
    })
}

pub fn save(plan: PastePlan) -> Result<PasteResult> {
    let PastePlan {
        summary,
        world,
        edits,
        ..
    } = plan;
    ensure!(
        summary.world_path == world.root,
        "El plan pertenece a otro mundo"
    );
    let backup = world.with_write_lock(|| {
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
                "La región {} cambió desde que se preparó el pegado. Vuelve a abrir el mundo",
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
    })?;
    Ok(PasteResult { summary, backup })
}
