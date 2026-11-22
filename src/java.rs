use std::{
    collections::{BTreeMap, HashMap},
    fs::{self, File, TryLockError},
    io::{self, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::{
        Mutex, RwLock,
        atomic::{AtomicBool, Ordering},
    },
};

use anyhow::{Context, Result, anyhow, bail, ensure};
use fastnbt::{LongArray, Value};
use flate2::read::{GzDecoder, ZlibDecoder};
use lz4_java_wrc::Lz4BlockInput;
use serde::Deserialize;

use crate::world::{
    BlockEntity, BlockEntityRecord, BlockState, ChestItem, Chunk, ChunkPos, Section, SignText,
};
use crate::{transaction, world_write::WorldBaseline};

pub(crate) const JAVA_1_21_1: i32 = 3955;
const FIRST_FLAT_CHUNK_VERSION: i32 = 2860;
const SECTOR_SIZE: u64 = 4096;
const REGION_WIDTH: i32 = 32;
const LOCATION_BYTES: usize = 4096;
const MAX_NBT_BYTES: u64 = 64 * 1024 * 1024;
const WORLD_LIMIT: i32 = 30_000_000;

pub struct JavaWorld {
    pub name: String,
    pub spawn: [i32; 3],
    pub(crate) root: PathBuf,
    session_lock: Mutex<Option<File>>,
    io: RwLock<()>,
    faulted: AtomicBool,
    pub(crate) baseline: Mutex<Result<WorldBaseline, String>>,
}

impl JavaWorld {
    pub fn open(path: &Path) -> Result<Self> {
        let root = path
            .canonicalize()
            .with_context(|| format!("No se pudo abrir {}", path.display()))?;
        if !transaction::pending(&root)?.is_empty() {
            read_level(&root)?;
            let _lock = exclusive_session(&root)?;
            read_level(&root)?;
            transaction::recover(&root)?;
        }
        let session_lock = open_session_lock(&root)?;
        let (level, identity) = read_level(&root)?;
        let baseline =
            WorldBaseline::capture(&root, identity).map_err(|error| format!("{error:#}"));
        let mut world = Self {
            name: level.name,
            spawn: [level.spawn_x, level.spawn_y, level.spawn_z],
            root,
            session_lock: Mutex::new(session_lock),
            io: RwLock::new(()),
            faulted: AtomicBool::new(false),
            baseline: Mutex::new(baseline),
        };
        world.find_entry_point()?;
        Ok(world)
    }

    pub fn load_chunk(&self, pos: ChunkPos) -> Result<Option<Chunk>> {
        self.load_checked(pos, false)
    }

    pub fn load_chunk_for_copy(&self, pos: ChunkPos) -> Result<Option<Chunk>> {
        self.load_checked(pos, true)
    }

    fn load_checked(&self, pos: ChunkPos, exact: bool) -> Result<Option<Chunk>> {
        self.with_read_lock(|| {
            let result = self
                .read_chunk(pos, exact)
                .with_context(|| format!("No se pudo leer el chunk ({}, {})", pos.x, pos.z));
            self.check_session_lock()?;
            result
        })
    }

    pub(crate) fn with_read_lock<T>(&self, read: impl FnOnce() -> Result<T>) -> Result<T> {
        let _access = self
            .io
            .read()
            .map_err(|_| anyhow!("Se interrumpió el acceso al mundo"))?;
        self.check_session_lock()?;
        read()
    }

    pub(crate) fn with_write_lock<T>(&self, write: impl FnOnce() -> Result<T>) -> Result<T> {
        let _access = self
            .io
            .write()
            .map_err(|_| anyhow!("Se interrumpió el acceso al mundo"))?;
        ensure!(
            !self.faulted.load(Ordering::Acquire),
            "Cierra esta ventana y vuelve a abrir el mundo para recuperarlo"
        );
        let mut slot = self
            .session_lock
            .lock()
            .map_err(|_| anyhow!("Se interrumpió el bloqueo de Minecraft"))?;
        // Release only this explorer's shared lock. Other attached readers keep
        // their locks, so the exclusive attempt fails without touching terrain.
        drop(slot.take());
        let lock = match exclusive_session(&self.root) {
            Ok(lock) => lock,
            Err(error) => {
                *slot = open_session_lock(&self.root).ok().flatten();
                return Err(error);
            }
        };
        let mut guard = ExclusiveSession {
            lock: Some(lock),
            slot: &mut slot,
            keep_exclusive: false,
            root: &self.root,
        };
        let result = write();
        if !transaction::pending(&self.root).is_ok_and(|pending| pending.is_empty()) {
            // Keep the exclusive handle if a rollback needs recovery. Existing
            // readers in this window are refused until the world is reopened.
            guard.keep_exclusive = true;
            self.faulted.store(true, Ordering::Release);
        }
        result
    }

    fn check_session_lock(&self) -> Result<()> {
        ensure!(
            !self.faulted.load(Ordering::Acquire),
            "El mundo tiene una restauración pendiente. Cierra esta ventana y vuelve a abrirlo"
        );
        // A copied save can omit the lock file. Acquire and retain the lock
        // before returning data if the file appears while Blocko is open.
        let mut slot = self
            .session_lock
            .lock()
            .map_err(|_| anyhow!("Se interrumpió el bloqueo de Minecraft"))?;
        if slot.is_none() {
            ensure!(
                transaction::pending(&self.root)?.is_empty(),
                "El mundo tiene una restauración pendiente. Reabre su carpeta en Blocko"
            );
            *slot = open_session_lock(&self.root)?;
        }
        Ok(())
    }

    fn read_chunk(&self, pos: ChunkPos, exact: bool) -> Result<Option<Chunk>> {
        let rx = pos.x.div_euclid(REGION_WIDTH);
        let rz = pos.z.div_euclid(REGION_WIDTH);
        let region_dir = self.root.join("region");
        let mut file = match File::open(region_dir.join(format!("r.{rx}.{rz}.mca"))) {
            Ok(file) => file,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(err.into()),
        };

        let index = pos.x.rem_euclid(REGION_WIDTH) + pos.z.rem_euclid(REGION_WIDTH) * REGION_WIDTH;
        let file_len = file.metadata()?.len();
        ensure!(file_len >= 2 * SECTOR_SIZE, "Encabezado Anvil incompleto");
        file.seek(SeekFrom::Start(index as u64 * 4))?;
        let mut location = [0; 4];
        file.read_exact(&mut location)?;
        let Some((start, capacity)) = chunk_span(location, file_len)? else {
            return Ok(None);
        };
        file.seek(SeekFrom::Start(start))?;
        let mut prefix = [0; 5];
        file.read_exact(&mut prefix)?;
        let len = u32::from_be_bytes([prefix[0], prefix[1], prefix[2], prefix[3]]) as u64;
        ensure!(
            (1..=capacity - 4).contains(&len),
            "Longitud de chunk Anvil inválida"
        );
        let external = prefix[4] & 0x80 != 0;
        let compression = prefix[4] & 0x7f;
        let nbt = if external {
            ensure!(len == 1, "Referencia a chunk externo Anvil inválida");
            let name = format!("c.{}.{}.mcc", pos.x, pos.z);
            let external = File::open(region_dir.join(&name))
                .with_context(|| format!("No se pudo abrir el chunk externo {name}"))?;
            decompress(external, compression)?
        } else {
            decompress(file.take(len - 1), compression)?
        };
        decode_chunk_version(&nbt, pos, exact)
    }

    fn find_entry_point(&mut self) -> Result<()> {
        let [x, spawn_y, z] = self.spawn;
        ensure!(
            (-WORLD_LIMIT..=WORLD_LIMIT).contains(&x) && (-WORLD_LIMIT..=WORLD_LIMIT).contains(&z),
            "El punto de aparición está fuera de los límites del Overworld"
        );
        let pos = ChunkPos::from_block(x, z);
        let (chunk, x, z, ground_fallback) = match self.load_chunk(pos)? {
            Some(chunk) => (chunk, x, z, spawn_y),
            None => {
                let (nearest, chunk) = self.nearest_completed_chunk(pos)?;
                (chunk, nearest.x * 16 + 8, nearest.z * 16 + 8, -63)
            }
        };
        let column = (z.rem_euclid(16) * 16 + x.rem_euclid(16)) as usize;
        let height = chunk.sections.iter().rev().find_map(|(&sy, section)| {
            (0..16).rev().find_map(|y| {
                let state = &section.palette[section.blocks[y * 256 + column] as usize];
                (!is_air(&state.name)).then_some(sy * 16 + y as i32 + 1)
            })
        });
        let ground_fallback = ground_fallback.clamp(-63, 320);
        self.spawn = [x, height.unwrap_or(ground_fallback).max(ground_fallback), z];
        Ok(())
    }

    fn nearest_completed_chunk(&self, target: ChunkPos) -> Result<(ChunkPos, Chunk)> {
        let entries = fs::read_dir(self.root.join("region"))
            .context("No se encontró la carpeta region del Overworld")?;
        let mut regions = Vec::new();
        for entry in entries {
            let entry = entry?;
            if let Some((x, z)) = parse_region_name(&entry.file_name().to_string_lossy()) {
                let nearest = ChunkPos {
                    x: target.x.clamp(x, x + REGION_WIDTH - 1),
                    z: target.z.clamp(z, z + REGION_WIDTH - 1),
                };
                regions.push((distance_squared(target, nearest), x, z, entry.path()));
            }
        }
        regions.sort_unstable();
        let mut best: Option<(u64, ChunkPos, Chunk)> = None;
        let mut first_error = None;
        for (min_distance, x, z, path) in regions {
            if best
                .as_ref()
                .is_some_and(|(distance, ..)| min_distance >= *distance)
            {
                break;
            }
            let mut locations = [0; LOCATION_BYTES];
            if let Err(error) = File::open(&path)
                .and_then(|mut file| file.read_exact(&mut locations))
                .with_context(|| format!("No se pudo leer el encabezado de {}", path.display()))
            {
                first_error.get_or_insert(error);
                continue;
            }
            let mut candidates = Vec::new();
            for (index, location) in locations.chunks_exact(4).enumerate() {
                if location == [0; 4] {
                    continue;
                }
                let index = index as i32;
                let pos = ChunkPos {
                    x: x + index % REGION_WIDTH,
                    z: z + index / REGION_WIDTH,
                };
                candidates.push((distance_squared(target, pos), pos));
            }
            candidates.sort_unstable_by_key(|(distance, pos)| (*distance, pos.x, pos.z));
            for (distance, pos) in candidates {
                if best.as_ref().is_some_and(|(best, ..)| distance >= *best) {
                    break;
                }
                match self.load_chunk(pos) {
                    Ok(Some(chunk)) => best = Some((distance, pos, chunk)),
                    Ok(None) => (),
                    Err(error) => {
                        first_error.get_or_insert(error);
                    }
                }
            }
        }
        best.map(|(_, pos, chunk)| (pos, chunk)).ok_or_else(|| {
            first_error.unwrap_or_else(|| {
                anyhow!("El Overworld no contiene chunks generados por completo")
            })
        })
    }
}

fn read_level(root: &Path) -> Result<(LevelData, blake3::Hash)> {
    let compressed = transaction::read_bounded(&root.join("level.dat"), MAX_NBT_BYTES)
        .context("La carpeta debe contener el archivo level.dat de un mundo Java")?;
    let nbt = read_limited(GzDecoder::new(compressed.as_slice()))?;
    let header: Level<LevelHeader> = fastnbt::from_bytes(&nbt)
        .context("El archivo level.dat no contiene metadatos Java válidos")?;
    if header.data.version != JAVA_1_21_1 {
        let version = header.data.version;
        let detected = header
            .data
            .release
            .and_then(|release| release.name)
            .map_or_else(
                || format!("DataVersion {version}"),
                |name| format!("Minecraft Java {name} (DataVersion {version})"),
            );
        bail!(
            "El mundo se guardó con {detected}. Blocko admite Minecraft Java 1.21.1 (DataVersion {JAVA_1_21_1})."
        );
    }
    let level: Level<LevelData> = fastnbt::from_bytes(&nbt)
        .context("El archivo level.dat no contiene metadatos válidos de Minecraft Java 1.21.1")?;
    Ok((level.data, blake3::hash(&compressed)))
}

struct ExclusiveSession<'a> {
    lock: Option<File>,
    slot: &'a mut Option<File>,
    keep_exclusive: bool,
    root: &'a Path,
}

impl Drop for ExclusiveSession<'_> {
    fn drop(&mut self) {
        let Some(lock) = self.lock.take() else {
            return;
        };
        if self.keep_exclusive {
            *self.slot = Some(lock);
        } else {
            drop(lock);
            *self.slot = open_session_lock(self.root).ok().flatten();
        }
    }
}

fn exclusive_session(root: &Path) -> Result<File> {
    transaction::plain_dir(root)?;
    let path = root.join("session.lock");
    if path.try_exists()? {
        transaction::plain_file(&path)?;
    }
    let mut options = File::options();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ,
            FILE_SHARE_WRITE,
        };
        options
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
        let file = options
            .open(&path)
            .context("No se pudo abrir session.lock para guardar")?;
        ensure!(
            file.metadata()?.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0,
            "session.lock apunta a un enlace"
        );
        return lock_exclusive(file);
    }
    #[cfg(not(windows))]
    lock_exclusive(
        options
            .open(&path)
            .context("No se pudo abrir session.lock para guardar")?,
    )
}

fn lock_exclusive(file: File) -> Result<File> {
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(TryLockError::WouldBlock) => {
            bail!("El mundo está abierto en Minecraft o en otra ventana de Blocko")
        }
        Err(TryLockError::Error(error)) => {
            Err(error).context("No se pudo bloquear el mundo para guardar")
        }
    }
}

fn open_session_lock(root: &Path) -> Result<Option<File>> {
    let mut options = File::options();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const SHARE_READ_WRITE: u32 = 0x01 | 0x02;
        options.share_mode(SHARE_READ_WRITE);
    }
    let file = match options.open(root.join("session.lock")) {
        Ok(file) => file,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err).context("No se pudo comprobar session.lock"),
    };
    match file.try_lock_shared() {
        Ok(()) => Ok(Some(file)),
        Err(TryLockError::WouldBlock) => {
            bail!("Minecraft tiene abierto este mundo. Cierra el mundo o usa una copia")
        }
        Err(TryLockError::Error(err)) => {
            Err(err).context("No se pudo comprobar el bloqueo de Minecraft")
        }
    }
}

fn parse_region_name(name: &str) -> Option<(i32, i32)> {
    let (x, z) = name
        .strip_prefix("r.")?
        .strip_suffix(".mca")?
        .split_once('.')?;
    let region_start = |n: &str| {
        let n = n.parse::<i32>().ok()?.checked_mul(REGION_WIDTH)?;
        let bound = WORLD_LIMIT.div_euclid(16) + REGION_WIDTH;
        (-bound..=bound).contains(&n).then_some(n)
    };
    Some((region_start(x)?, region_start(z)?))
}

fn distance_squared(a: ChunkPos, b: ChunkPos) -> u64 {
    let x = i64::from(a.x) - i64::from(b.x);
    let z = i64::from(a.z) - i64::from(b.z);
    (x * x + z * z) as u64
}

fn chunk_span(location: [u8; 4], file_len: u64) -> Result<Option<(u64, u64)>> {
    if location == [0; 4] {
        return Ok(None);
    }
    let offset = u32::from_be_bytes([0, location[0], location[1], location[2]]);
    let start = u64::from(offset) * SECTOR_SIZE;
    let capacity = u64::from(location[3]) * SECTOR_SIZE;
    ensure!(
        offset >= 2 && capacity > 0 && start + capacity <= file_len,
        "Sectores del chunk fuera de la región Anvil"
    );
    Ok(Some((start, capacity)))
}

pub(crate) fn decompress(data: impl Read, compression: u8) -> Result<Vec<u8>> {
    match compression {
        1 => read_limited(GzDecoder::new(data)),
        2 => read_limited(ZlibDecoder::new(data)),
        3 => read_limited(data),
        4 => read_limited(Lz4BlockInput::new(data)),
        _ => bail!("Compresión Anvil {compression} incompatible"),
    }
}

fn read_limited(data: impl Read) -> Result<Vec<u8>> {
    let mut nbt = Vec::new();
    data.take(MAX_NBT_BYTES + 1)
        .read_to_end(&mut nbt)
        .context("No se pudieron descomprimir los datos NBT")?;
    ensure!(
        (nbt.len() as u64) <= MAX_NBT_BYTES,
        "Los datos NBT superan el límite de 64 MiB"
    );
    Ok(nbt)
}

#[derive(Deserialize)]
struct Level<T> {
    #[serde(rename = "Data")]
    data: T,
}

#[derive(Deserialize)]
struct LevelHeader {
    #[serde(rename = "DataVersion")]
    version: i32,
    #[serde(rename = "Version")]
    release: Option<GameRelease>,
}

#[derive(Deserialize)]
struct GameRelease {
    #[serde(rename = "Name")]
    name: Option<String>,
}

#[derive(Deserialize)]
struct LevelData {
    #[serde(rename = "LevelName")]
    name: String,
    #[serde(rename = "SpawnX")]
    spawn_x: i32,
    #[serde(rename = "SpawnY")]
    spawn_y: i32,
    #[serde(rename = "SpawnZ")]
    spawn_z: i32,
}

#[derive(Deserialize)]
struct JavaChunk {
    #[serde(rename = "DataVersion")]
    version: i32,
    #[serde(rename = "xPos")]
    x: i32,
    #[serde(rename = "zPos")]
    z: i32,
    #[serde(rename = "Status")]
    status: String,
    sections: Option<Vec<JavaSection>>,
    block_entities: Option<Value>,
}

#[derive(Deserialize)]
struct JavaSection {
    #[serde(rename = "Y")]
    y: i8,
    block_states: Option<JavaBlockStates>,
}

#[derive(Deserialize)]
struct JavaBlockStates {
    palette: Vec<JavaBlockState>,
    data: Option<LongArray>,
}

#[derive(Deserialize)]
struct JavaBlockState {
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "Properties", default)]
    properties: BTreeMap<String, String>,
}

pub(crate) fn decode_chunk(nbt: &[u8], pos: ChunkPos) -> Result<Option<Chunk>> {
    decode_chunk_version(nbt, pos, false)
}

fn decode_chunk_version(nbt: &[u8], pos: ChunkPos, exact: bool) -> Result<Option<Chunk>> {
    let chunk: JavaChunk =
        fastnbt::from_bytes(nbt).context("Estructura NBT de chunk Java incompatible")?;
    ensure!(
        !exact || chunk.version == JAVA_1_21_1,
        "Para copiar, los chunks deben haberse guardado en Minecraft Java 1.21.1. Este chunk tiene DataVersion {}",
        chunk.version
    );
    ensure!(
        (FIRST_FLAT_CHUNK_VERSION..=JAVA_1_21_1).contains(&chunk.version),
        "DataVersion {} de chunk incompatible",
        chunk.version
    );
    ensure!(
        (chunk.x, chunk.z) == (pos.x, pos.z),
        "Las coordenadas del chunk no coinciden con su posición Anvil"
    );
    if !matches!(chunk.status.as_str(), "full" | "minecraft:full") {
        return Ok(None);
    }
    let mut sections = BTreeMap::new();
    let entries = chunk
        .sections
        .context("El chunk completo no incluye secciones")?;
    for entry in entries {
        let Some(states) = entry.block_states else {
            continue;
        };
        ensure!(
            (-4..=19).contains(&entry.y),
            "Sección Y={} fuera de los límites del Overworld",
            entry.y
        );
        let blocks = unpack_indices(states.palette.len(), states.data.as_deref())
            .with_context(|| format!("Paleta inválida en la sección Y={}", entry.y))?;
        let palette = states
            .palette
            .into_iter()
            .map(|state| BlockState {
                name: state.name,
                properties: state.properties,
            })
            .collect();
        let section = Section { palette, blocks };
        ensure!(
            sections.insert(i32::from(entry.y), section).is_none(),
            "Sección Y={} duplicada",
            entry.y
        );
    }
    let mut decoded = Chunk {
        sections,
        ..Chunk::default()
    };
    read_block_entities(&mut decoded, pos, chunk.block_entities);
    Ok(Some(decoded))
}

type Compound = HashMap<String, Value>;

fn read_block_entities(chunk: &mut Chunk, owner: ChunkPos, data: Option<Value>) {
    let Some(data) = data else {
        return;
    };
    let Value::List(entities) = data else {
        chunk.block_entity_error = Some("La lista de entidades de bloque es inválida".into());
        return;
    };

    for entity in entities {
        let Some((coordinates, tags)) = coordinates_and_tags(&entity, owner) else {
            chunk.block_entity_error.get_or_insert_with(|| {
                "Hay una entidad de bloque sin coordenadas válidas dentro del chunk".into()
            });
            continue;
        };
        let kind = match tags.get("id") {
            Some(Value::String(id)) if !id.is_empty() => id.as_str(),
            _ => "entidad sin identificador",
        };
        let [x, y, z] = coordinates;
        let block = chunk.block(x.rem_euclid(16), y, z.rem_euclid(16));
        let record = match decode_block_entity(kind, tags, block) {
            Ok(entity) => BlockEntityRecord::Supported(entity),
            Err(error) => BlockEntityRecord::Unsupported {
                kind: kind.into(),
                reason: format!("{error:#}"),
            },
        };
        match chunk.block_entities.entry(coordinates) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(record);
            }
            std::collections::btree_map::Entry::Occupied(mut entry) => {
                entry.insert(BlockEntityRecord::Unsupported {
                    kind: kind.into(),
                    reason: "Hay varias entidades de bloque en la misma posición".into(),
                });
            }
        }
    }
}

fn coordinates_and_tags(data: &Value, owner: ChunkPos) -> Option<([i32; 3], &Compound)> {
    let Value::Compound(tags) = data else {
        return None;
    };
    let (Some(&Value::Int(x)), Some(&Value::Int(y)), Some(&Value::Int(z))) =
        (tags.get("x"), tags.get("y"), tags.get("z"))
    else {
        return None;
    };
    (ChunkPos::from_block(x, z) == owner && (-64..=319).contains(&y)).then_some(([x, y, z], tags))
}

fn decode_block_entity(
    kind: &str,
    tags: &Compound,
    block: Option<&BlockState>,
) -> Result<BlockEntity> {
    let payload = match kind {
        "minecraft:chest" => ["Items", "CustomName", "Lock"].as_slice(),
        "minecraft:sign" | "minecraft:hanging_sign" => {
            ["front_text", "back_text", "is_waxed"].as_slice()
        }
        _ => bail!("Este tipo de entidad de bloque no se puede copiar"),
    };
    if kind == "minecraft:chest" {
        ensure!(
            !tags.contains_key("LootTable") && !tags.contains_key("LootTableSeed"),
            "El cofre contiene una tabla de botín pendiente"
        );
    }
    ensure!(
        empty_components(tags),
        "La entidad de bloque tiene componentes adicionales"
    );
    // Anvil uses this flag to defer instantiation. The persisted entity still
    // has its entire payload, which is validated below before it is copied.
    optional_boolean(tags, "keepPacked")?;
    check_fields(
        tags,
        payload,
        &["id", "x", "y", "z", "keepPacked", "components"],
    )?;

    let block = block.context("La entidad no corresponde a un bloque en el chunk")?;
    if kind == "minecraft:chest" {
        ensure!(
            block.name == kind
                && matches!(
                    block.properties.get("type").map(String::as_str),
                    None | Some("single")
                ),
            "Solo se pueden copiar cofres simples"
        );
        return Ok(BlockEntity::Chest {
            items: chest_items(tags.get("Items"))?,
            custom_name: optional_string(tags, "CustomName")?,
            lock: optional_string(tags, "Lock")?,
        });
    }

    let is_hanging = block.name.ends_with("_hanging_sign");
    ensure!(
        block.name.starts_with("minecraft:")
            && block.name.ends_with("_sign")
            && (kind == "minecraft:hanging_sign") == is_hanging,
        "La entidad del cartel no corresponde al bloque"
    );
    Ok(BlockEntity::Sign {
        front: sign_face(tags.get("front_text")).context("Texto del frente del cartel")?,
        back: sign_face(tags.get("back_text")).context("Texto del reverso del cartel")?,
        waxed: optional_boolean(tags, "is_waxed")?,
    })
}

fn chest_items(data: Option<&Value>) -> Result<Vec<ChestItem>> {
    let Some(data) = data else {
        return Ok(Vec::new());
    };
    let Value::List(entries) = data else {
        bail!("El inventario del cofre no es una lista");
    };
    let mut used_slots = [false; 27];
    let mut items = Vec::with_capacity(entries.len().min(27));
    for entry in entries {
        let Value::Compound(tags) = entry else {
            bail!("El inventario contiene un objeto inválido");
        };
        let Some(&Value::Byte(slot)) = tags.get("Slot") else {
            bail!("Un objeto del cofre no tiene una ranura válida");
        };
        let slot = u8::try_from(slot).context("Ranura de cofre fuera de rango")?;
        let used = used_slots
            .get_mut(usize::from(slot))
            .context("Ranura de cofre fuera de rango")?;
        ensure!(!*used, "Hay varios objetos en la ranura {slot}");
        *used = true;

        ensure!(
            empty_components(tags),
            "El objeto en la ranura {slot} tiene componentes adicionales"
        );
        check_fields(tags, &["Slot", "id", "count", "components"], &[])
            .with_context(|| format!("Objeto en la ranura {slot}"))?;
        let id = optional_string(tags, "id")?
            .context("Falta el identificador de un objeto del cofre")?;
        ensure!(
            is_vanilla_item_id(&id),
            "Identificador de objeto no compatible: {id}"
        );
        let count = match tags.get("count") {
            None => 1,
            Some(&Value::Byte(n)) => i32::from(n),
            Some(&Value::Short(n)) => i32::from(n),
            Some(&Value::Int(n)) => n,
            _ => bail!("Cantidad inválida en la ranura {slot}"),
        };
        ensure!(
            (1..=99).contains(&count),
            "Cantidad fuera de rango en la ranura {slot}"
        );
        items.push(ChestItem { slot, id, count });
    }
    Ok(items)
}

fn is_vanilla_item_id(id: &str) -> bool {
    id.strip_prefix("minecraft:").is_some_and(|path| {
        !path.is_empty()
            && path != "air"
            && path
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"_-./".contains(&c))
    })
}

fn sign_face(data: Option<&Value>) -> Result<SignText> {
    let Some(data) = data else {
        return Ok(SignText {
            messages: std::array::from_fn(|_| "\"\"".into()),
            filtered_messages: None,
            color: "black".into(),
            glowing: false,
        });
    };
    let Value::Compound(tags) = data else {
        bail!("La cara del cartel tiene un formato inválido");
    };
    check_fields(
        tags,
        &["messages", "filtered_messages", "color", "has_glowing_text"],
        &[],
    )?;
    let color = optional_string(tags, "color")?.unwrap_or_else(|| "black".into());
    ensure!(
        matches!(
            color.as_str(),
            "white"
                | "orange"
                | "magenta"
                | "light_blue"
                | "yellow"
                | "lime"
                | "pink"
                | "gray"
                | "light_gray"
                | "cyan"
                | "purple"
                | "blue"
                | "brown"
                | "green"
                | "red"
                | "black"
        ),
        "Color de cartel no compatible: {color}"
    );
    Ok(SignText {
        messages: sign_lines(tags.get("messages")).context("Mensajes del cartel")?,
        filtered_messages: tags
            .get("filtered_messages")
            .map(|list| sign_lines(Some(list)))
            .transpose()
            .context("Mensajes filtrados del cartel")?,
        color,
        glowing: optional_boolean(tags, "has_glowing_text")?,
    })
}

fn sign_lines(data: Option<&Value>) -> Result<[String; 4]> {
    let Some(Value::List(lines)) = data else {
        bail!("Los mensajes deben ser una lista");
    };
    let [
        Value::String(a),
        Value::String(b),
        Value::String(c),
        Value::String(d),
    ] = lines.as_slice()
    else {
        bail!("El cartel debe contener cuatro líneas de texto JSON");
    };
    Ok([a.clone(), b.clone(), c.clone(), d.clone()])
}

fn check_fields(tags: &Compound, payload: &[&str], metadata: &[&str]) -> Result<()> {
    if let Some(field) = tags
        .keys()
        .map(String::as_str)
        .filter(|field| !payload.contains(field) && !metadata.contains(field))
        .min()
    {
        bail!("Campo no compatible: {field}");
    }
    Ok(())
}

fn empty_components(tags: &Compound) -> bool {
    match tags.get("components") {
        None => true,
        Some(Value::Compound(components)) => components.is_empty(),
        Some(_) => false,
    }
}

fn optional_string(tags: &Compound, key: &str) -> Result<Option<String>> {
    match tags.get(key) {
        None => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => bail!("El campo {key} debe ser texto"),
    }
}

fn optional_boolean(tags: &Compound, key: &str) -> Result<bool> {
    match tags.get(key) {
        None | Some(Value::Byte(0)) => Ok(false),
        Some(Value::Byte(1)) => Ok(true),
        Some(_) => bail!("El campo {key} debe ser verdadero o falso"),
    }
}

fn unpack_indices(palette_len: usize, data: Option<&[i64]>) -> Result<Box<[u16; 4096]>> {
    ensure!(
        (1..=4096).contains(&palette_len),
        "Tamaño de paleta inválido: {palette_len}"
    );
    let mut blocks = Box::new([0; 4096]);
    let data = match data {
        None if palette_len == 1 => return Ok(blocks),
        None => bail!("Bloques empaquetados ausentes"),
        Some(data) => data,
    };

    // Anvil fills each long separately. For example, a five-bit palette
    // stores twelve indices and leaves four high bits unused in each long.
    let bits = (usize::BITS - (palette_len - 1).leading_zeros()).max(4);
    let indices_per_long = 64 / bits as usize;
    ensure!(
        data.len() == 4096_usize.div_ceil(indices_per_long),
        "Longitud de bloques empaquetados inválida"
    );
    let mask = (1_u64 << bits) - 1;
    for (out, packed) in blocks.chunks_mut(indices_per_long).zip(data) {
        let mut packed = *packed as u64;
        for block in out {
            let index = (packed & mask) as u16;
            ensure!(
                (index as usize) < palette_len,
                "Índice de paleta fuera de rango: {index}"
            );
            *block = index;
            packed >>= bits;
        }
    }
    Ok(blocks)
}

fn is_air(name: &str) -> bool {
    matches!(
        name,
        "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{
        Compression,
        write::{GzEncoder, ZlibEncoder},
    };
    use std::{
        io::Write,
        time::{SystemTime, UNIX_EPOCH},
    };

    fn compound<const N: usize>(entries: [(&str, Value); N]) -> Value {
        Value::Compound(
            entries
                .into_iter()
                .map(|(key, value)| (key.to_owned(), value))
                .collect(),
        )
    }

    #[test]
    fn five_bit_palette_indices_are_padded_per_long() -> Result<()> {
        let mut data = vec![0_i64; 4096_usize.div_ceil(12)];
        data[0] = 16 << 55;
        data[1] = 15;
        *data.last_mut().unwrap() = 14 << 15;
        let blocks = unpack_indices(17, Some(&data))?;
        assert_eq!(&blocks[10..14], &[0, 16, 15, 0]);
        assert_eq!(blocks[4095], 14);

        data[0] = 17;
        assert!(unpack_indices(17, Some(&data)).is_err());
        assert!(unpack_indices(17, Some(&data[1..])).is_err());
        Ok(())
    }

    fn chunk_nbt(pos: ChunkPos) -> Result<Vec<u8>> {
        let block = compound([
            ("Name", string("minecraft:oak_log")),
            ("Properties", compound([("axis", string("x"))])),
        ]);
        chunk_nbt_with_entities(pos, block, Vec::new())
    }

    fn string(value: &str) -> Value {
        Value::String(value.into())
    }

    fn chunk_nbt_with_entities(
        pos: ChunkPos,
        block: Value,
        block_entities: Vec<Value>,
    ) -> Result<Vec<u8>> {
        let palette = Value::List(vec![block]);
        let sections = Value::List(vec![compound([
            ("Y", Value::Byte(-4)),
            ("block_states", compound([("palette", palette)])),
        ])]);
        let nbt = compound([
            ("DataVersion", Value::Int(JAVA_1_21_1)),
            ("xPos", Value::Int(pos.x)),
            ("zPos", Value::Int(pos.z)),
            ("Status", Value::String("minecraft:full".into())),
            ("sections", sections),
            ("block_entities", Value::List(block_entities)),
        ]);
        Ok(fastnbt::to_bytes(&nbt)?)
    }

    #[test]
    fn reads_negative_sections_and_keeps_block_properties() -> Result<()> {
        let pos = ChunkPos { x: -2, z: -3 };
        let bytes = chunk_nbt(pos)?;
        let chunk = decode_chunk(&bytes, pos)?.context("Chunk ausente en la prueba")?;
        let block = chunk
            .block(15, -49, 15)
            .context("Bloque ausente en la prueba")?;
        assert_eq!(block.name, "minecraft:oak_log");
        assert_eq!(block.properties["axis"], "x");
        assert!(chunk.block(15, -48, 15).is_none());
        assert!(decode_chunk(&bytes, ChunkPos { x: -1, ..pos }).is_err());
        Ok(())
    }

    fn entity(
        kind: &str,
        x: i32,
        payload: impl IntoIterator<Item = (&'static str, Value)>,
    ) -> Value {
        let entries = [
            ("id", string(kind)),
            ("x", Value::Int(x)),
            ("y", Value::Int(-49)),
            ("z", Value::Int(-48)),
        ];
        Value::Compound(
            entries
                .into_iter()
                .chain(payload)
                .map(|(key, value)| (key.into(), value))
                .collect(),
        )
    }

    #[test]
    fn preserves_simple_chest_metadata_and_inventory() -> Result<()> {
        let pos = ChunkPos { x: -2, z: -3 };
        let block = compound([
            ("Name", string("minecraft:chest")),
            ("Properties", compound([("type", string("single"))])),
        ]);
        let items = Value::List(vec![
            compound([
                ("Slot", Value::Byte(0)),
                ("id", string("minecraft:stone")),
                ("count", Value::Int(17)),
            ]),
            compound([
                ("Slot", Value::Byte(26)),
                ("id", string("minecraft:diamond")),
            ]),
        ]);
        let name = r#"{"text": "Materiales", "color":"gold"}"#;
        let tags = [
            ("Items", items),
            ("CustomName", string(name)),
            ("Lock", string("Llave")),
            ("components", Value::Compound(HashMap::new())),
            ("keepPacked", Value::Byte(1)),
        ];
        let bytes =
            chunk_nbt_with_entities(pos, block, vec![entity("minecraft:chest", -32, tags)])?;
        let chunk = decode_chunk(&bytes, pos)?.context("Chunk ausente en la prueba")?;
        let Some(BlockEntityRecord::Supported(BlockEntity::Chest {
            items,
            custom_name,
            lock,
        })) = chunk.block_entities.get(&[-32, -49, -48])
        else {
            bail!("No se leyó el cofre de prueba: {:?}", chunk.block_entities);
        };
        assert_eq!(
            items,
            &[
                ChestItem {
                    slot: 0,
                    id: "minecraft:stone".into(),
                    count: 17
                },
                ChestItem {
                    slot: 26,
                    id: "minecraft:diamond".into(),
                    count: 1
                },
            ]
        );
        assert_eq!(custom_name.as_deref(), Some(name));
        assert_eq!(lock.as_deref(), Some("Llave"));
        assert!(chunk.block_entity_error.is_none());
        Ok(())
    }

    #[test]
    fn preserves_exact_json_on_both_faces_of_a_sign() -> Result<()> {
        let pos = ChunkPos { x: -2, z: -3 };
        let block = compound([("Name", string("minecraft:oak_wall_sign"))]);
        let front_line = r#"{"text": "Entrada", "bold":true}"#;
        let back_line = r#""Salida""#;
        let filtered_line = r#""******""#;
        let lines = |line| {
            Value::List(vec![
                string(line),
                string(r#""""#),
                string(r#""""#),
                string(r#""""#),
            ])
        };
        let tags = [
            (
                "front_text",
                compound([
                    ("messages", lines(front_line)),
                    ("filtered_messages", lines(filtered_line)),
                    ("color", string("red")),
                    ("has_glowing_text", Value::Byte(1)),
                ]),
            ),
            ("back_text", compound([("messages", lines(back_line))])),
            ("is_waxed", Value::Byte(1)),
        ];
        let bytes = chunk_nbt_with_entities(pos, block, vec![entity("minecraft:sign", -32, tags)])?;
        let chunk = decode_chunk(&bytes, pos)?.context("Chunk ausente en la prueba")?;
        let Some(BlockEntityRecord::Supported(BlockEntity::Sign { front, back, waxed })) =
            chunk.block_entities.get(&[-32, -49, -48])
        else {
            bail!("No se leyó el cartel de prueba: {:?}", chunk.block_entities);
        };
        assert_eq!(front.messages, [front_line, "\"\"", "\"\"", "\"\""]);
        assert_eq!(front.filtered_messages.as_ref().unwrap()[0], filtered_line);
        assert_eq!(front.color, "red");
        assert!(front.glowing);
        assert_eq!(back.messages[0], back_line);
        assert_eq!(back.color, "black");
        assert!(!back.glowing);
        assert!(*waxed);
        Ok(())
    }

    #[test]
    fn retains_unsupported_records_and_terrain_for_safe_selection() -> Result<()> {
        let pos = ChunkPos { x: -2, z: -3 };
        let block = compound([("Name", string("minecraft:chest"))]);
        let decorated_item = compound([
            ("Slot", Value::Byte(4)),
            ("id", string("minecraft:stone")),
            (
                "components",
                compound([("minecraft:custom_name", string("Nombre"))]),
            ),
        ]);
        let entities = vec![
            entity("minecraft:lectern", -32, []),
            entity(
                "minecraft:chest",
                -31,
                [("LootTable", string("minecraft:test"))],
            ),
            entity("minecraft:chest", -30, [("Uncatalogued", Value::Int(1))]),
            entity(
                "minecraft:chest",
                -29,
                [("Items", Value::List(vec![decorated_item]))],
            ),
            entity("minecraft:chest", -28, []),
            entity("minecraft:chest", -28, []),
            entity("minecraft:chest", 16, []),
        ];
        let bytes = chunk_nbt_with_entities(pos, block, entities)?;
        let chunk = decode_chunk(&bytes, pos)?.context("Chunk ausente en la prueba")?;
        for (x, expected) in [
            (-32, "no se puede copiar"),
            (-31, "botín"),
            (-30, "Uncatalogued"),
            (-29, "componentes adicionales"),
            (-28, "misma posición"),
        ] {
            let Some(BlockEntityRecord::Unsupported { reason, .. }) =
                chunk.block_entities.get(&[x, -49, -48])
            else {
                bail!("No se registró como incompatible la entidad en X={x}");
            };
            assert!(reason.contains(expected), "{reason}");
        }
        assert_eq!(chunk.block_entities.len(), 5);
        assert!(chunk.block_entity_error.is_some());
        assert_eq!(chunk.block(0, -49, 0).unwrap().name, "minecraft:chest");
        Ok(())
    }

    fn level_dat(version: i32) -> Result<Vec<u8>> {
        encode_level(compound([
            ("DataVersion", Value::Int(version)),
            ("LevelName", Value::String("Mundo de prueba".into())),
            ("SpawnX", Value::Int(-17)),
            ("SpawnY", Value::Int(-64)),
            ("SpawnZ", Value::Int(-33)),
        ]))
    }

    fn encode_level(data: Value) -> Result<Vec<u8>> {
        let nbt = compound([("Data", data)]);
        let mut gzip = GzEncoder::new(Vec::new(), Compression::default());
        gzip.write_all(&fastnbt::to_bytes(&nbt)?)?;
        Ok(gzip.finish()?)
    }

    struct WorldFixture(PathBuf);

    impl WorldFixture {
        fn new() -> Result<Self> {
            let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
            let path =
                std::env::temp_dir().join(format!("blocko-java-{}-{nonce}", std::process::id()));
            fs::create_dir(&path)?;
            let fixture = Self(path);
            fs::create_dir(fixture.0.join("region"))?;
            Ok(fixture)
        }
    }

    impl Drop for WorldFixture {
        fn drop(&mut self) {
            for file in ["level.dat", "session.lock", "region/r.-1.-1.mca"] {
                let _ = fs::remove_file(self.0.join(file));
            }
            let _ = fs::remove_dir(self.0.join("region"));
            let _ = fs::remove_dir(&self.0);
        }
    }

    #[test]
    fn reports_incompatible_version_before_reading_legacy_spawn() -> Result<()> {
        let fixture = WorldFixture::new()?;
        let level = encode_level(compound([
            ("DataVersion", Value::Int(4556)),
            (
                "Version",
                compound([("Name", Value::String("1.21.10".into()))]),
            ),
            ("LevelName", Value::String("Mundo de otra versión".into())),
            (
                "spawn",
                compound([("dimension", Value::String("minecraft:overworld".into()))]),
            ),
        ]))?;
        fs::write(fixture.0.join("level.dat"), level)?;
        let error = JavaWorld::open(&fixture.0)
            .err()
            .context("Se admitió una versión incompatible")?
            .to_string();
        assert!(error.contains("Minecraft Java 1.21.10 (DataVersion 4556)"));
        assert!(error.contains("Blocko admite Minecraft Java 1.21.1"));
        Ok(())
    }

    #[test]
    fn opens_anvil_world_with_shared_lock_without_changing_bytes() -> Result<()> {
        let fixture = WorldFixture::new()?;
        let path = &fixture.0;
        let level_path = path.join("level.dat");
        let region_path = path.join("region/r.-1.-1.mca");
        let lock_path = path.join("session.lock");
        let level = level_dat(JAVA_1_21_1)?;
        let lock_bytes = "☃".as_bytes();
        let pos = ChunkPos { x: -2, z: -3 };
        let mut zlib = ZlibEncoder::new(Vec::new(), Compression::default());
        zlib.write_all(&chunk_nbt(pos)?)?;
        let chunk = zlib.finish()?;
        let mut region = vec![0; 3 * SECTOR_SIZE as usize];
        let region_index = (29 * 32 + 30) * 4;
        region[region_index..region_index + 4].copy_from_slice(&[0, 0, 2, 1]);
        let start = 2 * SECTOR_SIZE as usize;
        region[start..start + 4].copy_from_slice(&(u32::try_from(chunk.len())? + 1).to_be_bytes());
        region[start + 4] = 2;
        region[start + 5..start + 5 + chunk.len()].copy_from_slice(&chunk);
        fs::write(&level_path, &level)?;
        fs::write(&region_path, &region)?;
        fs::write(&lock_path, lock_bytes)?;

        let exclusive = File::options().write(true).open(&lock_path)?;
        exclusive.try_lock()?;
        assert!(
            JavaWorld::open(path)
                .err()
                .is_some_and(|error| error.to_string().contains("Minecraft tiene abierto"))
        );
        drop(exclusive);

        let world = JavaWorld::open(path)?;
        let second_view = JavaWorld::open(path)?;
        assert_eq!(world.name, "Mundo de prueba");
        assert_eq!(world.spawn, [-17, -48, -33]);
        assert!(world.load_chunk(pos)?.is_some());
        assert!(world.load_chunk(ChunkPos { x: -1, ..pos })?.is_none());
        drop(world);
        let exclusive = File::options().write(true).open(&lock_path)?;
        assert!(matches!(
            exclusive.try_lock(),
            Err(TryLockError::WouldBlock)
        ));
        drop(second_view);
        exclusive.try_lock()?;
        drop(exclusive);

        assert_eq!(fs::read(&level_path)?, level);
        assert_eq!(fs::read(&region_path)?, region);
        assert_eq!(fs::read(&lock_path)?, lock_bytes);
        fs::write(&level_path, level_dat(JAVA_1_21_1 + 1)?)?;
        assert!(
            JavaWorld::open(path)
                .err()
                .is_some_and(|error| error.to_string().contains("DataVersion 3956"))
        );

        fs::write(&level_path, level)?;
        fs::remove_file(&lock_path)?;
        let copy_without_lock = JavaWorld::open(path)?;
        assert!(copy_without_lock.load_chunk(pos)?.is_some());
        assert!(!lock_path.try_exists()?);
        Ok(())
    }
}
