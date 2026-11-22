use std::{
    collections::BTreeMap,
    fs::{self, File, TryLockError},
    io::{self, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::OnceLock,
};

use anyhow::{Context, Result, anyhow, bail, ensure};
use fastnbt::LongArray;
use flate2::read::{GzDecoder, ZlibDecoder};
use lz4_java_wrc::Lz4BlockInput;
use serde::Deserialize;

use crate::world::{BlockState, Chunk, ChunkPos, Section};

const JAVA_1_21_1: i32 = 3955;
const FIRST_FLAT_CHUNK_VERSION: i32 = 2860;
const SECTOR_SIZE: u64 = 4096;
const REGION_WIDTH: i32 = 32;
const LOCATION_BYTES: usize = 4096;
const MAX_NBT_BYTES: u64 = 64 * 1024 * 1024;
const WORLD_LIMIT: i32 = 30_000_000;

pub struct JavaWorld {
    pub name: String,
    pub spawn: [i32; 3],
    root: PathBuf,
    session_lock: OnceLock<File>,
}

impl JavaWorld {
    pub fn open(path: &Path) -> Result<Self> {
        let root = path
            .canonicalize()
            .with_context(|| format!("No se pudo abrir {}", path.display()))?;
        let session_lock = open_session_lock(&root)?;
        let level = File::open(root.join("level.dat"))
            .context("La carpeta debe contener el archivo level.dat de un mundo Java")?;
        let nbt = read_limited(GzDecoder::new(level))?;
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
        let level: Level<LevelData> = fastnbt::from_bytes(&nbt).context(
            "El archivo level.dat no contiene metadatos válidos de Minecraft Java 1.21.1",
        )?;

        let mut world = Self {
            name: level.data.name,
            spawn: [level.data.spawn_x, level.data.spawn_y, level.data.spawn_z],
            root,
            session_lock: session_lock.map(OnceLock::from).unwrap_or_default(),
        };
        world.find_entry_point()?;
        Ok(world)
    }

    pub fn load_chunk(&self, pos: ChunkPos) -> Result<Option<Chunk>> {
        self.check_session_lock()?;
        let result = self
            .read_chunk(pos)
            .with_context(|| format!("No se pudo leer el chunk ({}, {})", pos.x, pos.z));
        self.check_session_lock()?;
        result
    }

    fn check_session_lock(&self) -> Result<()> {
        // A copied save can omit the lock file. Acquire and retain the lock
        // before returning data if the file appears while Blocko is open.
        if self.session_lock.get().is_none()
            && let Some(file) = open_session_lock(&self.root)?
        {
            let _ = self.session_lock.set(file);
        }
        Ok(())
    }

    fn read_chunk(&self, pos: ChunkPos) -> Result<Option<Chunk>> {
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
        decode_chunk(&nbt, pos)
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

fn decompress(data: impl Read, compression: u8) -> Result<Vec<u8>> {
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

fn decode_chunk(nbt: &[u8], pos: ChunkPos) -> Result<Option<Chunk>> {
    let chunk: JavaChunk =
        fastnbt::from_bytes(nbt).context("Estructura NBT de chunk Java incompatible")?;
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
    Ok(Some(Chunk { sections }))
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
    use fastnbt::Value;
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
        let palette = Value::List(vec![compound([
            ("Name", Value::String("minecraft:oak_log".into())),
            (
                "Properties",
                compound([("axis", Value::String("x".into()))]),
            ),
        ])]);
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
