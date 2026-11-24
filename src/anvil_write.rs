use std::{
    io::Write,
    ops::Range,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail, ensure};
use flate2::{Compression, write::ZlibEncoder};

use crate::{java::decompress, world::ChunkPos};

const SECTOR_SIZE: usize = 4096;
const HEADER_SECTORS: usize = 2;
const REGION_WIDTH: i32 = 32;
const ENTRIES: usize = REGION_WIDTH as usize * REGION_WIDTH as usize;
const MAX_SECTOR_OFFSET: usize = 0x00ff_ffff;
const MAX_CHUNK_SECTORS: usize = u8::MAX as usize;
const EXTERNAL: u8 = 0x80;
const ZLIB: u8 = 2;

#[derive(Clone, Copy)]
struct Span {
    start: usize,
    sectors: usize,
}

impl Span {
    fn end(self) -> usize {
        self.start + self.sectors
    }

    fn byte_range(self) -> Range<usize> {
        self.start * SECTOR_SIZE..self.end() * SECTOR_SIZE
    }

    fn location(self) -> [u8; 4] {
        ((self.start as u32) << 8 | self.sectors as u32).to_be_bytes()
    }
}

pub struct RegionImage {
    bytes: Vec<u8>,
    region: [i32; 2],
    spans: [Option<Span>; ENTRIES],
}

impl RegionImage {
    pub fn new(bytes: Vec<u8>, region: [i32; 2]) -> Result<Self> {
        ensure!(
            bytes.len() >= HEADER_SECTORS * SECTOR_SIZE,
            "Encabezado Anvil incompleto"
        );
        ensure!(
            bytes.len().is_multiple_of(SECTOR_SIZE),
            "La región Anvil tiene un sector incompleto"
        );
        let sectors = bytes.len() / SECTOR_SIZE;
        ensure!(
            sectors <= MAX_SECTOR_OFFSET + MAX_CHUNK_SECTORS,
            "La región Anvil supera el espacio direccionable"
        );
        let mut spans = [None; ENTRIES];
        for (index, entry) in bytes[..SECTOR_SIZE].chunks_exact(4).enumerate() {
            let [a, b, c, count] = [entry[0], entry[1], entry[2], entry[3]];
            let start = u32::from_be_bytes([0, a, b, c]) as usize;
            if start == 0 && count == 0 {
                continue;
            }
            let span = Span {
                start,
                sectors: usize::from(count),
            };
            ensure!(
                span.start >= HEADER_SECTORS && span.sectors > 0 && span.end() <= sectors,
                "Ubicación Anvil fuera de la región en la entrada {index}"
            );
            spans[index] = Some(span);
        }
        let image = Self {
            bytes,
            region,
            spans,
        };
        let mut next = HEADER_SECTORS;
        for (index, span) in image.ordered_spans(None) {
            ensure!(
                span.start >= next,
                "Sectores Anvil superpuestos en la entrada {index}"
            );
            chunk_data(&image.bytes[span.byte_range()])
                .with_context(|| format!("Datos Anvil inválidos en la entrada {index}"))?;
            next = span.end();
        }
        Ok(image)
    }

    pub fn read_chunk(&self, pos: ChunkPos) -> Result<Vec<u8>> {
        let (_, span) = self.chunk_span(pos)?;
        let (codec, payload) = chunk_data(&self.bytes[span.byte_range()])?;
        if codec & EXTERNAL != 0 {
            bail!(
                "El chunk ({}, {}) usa un archivo externo .mcc, fuera del alcance de esta edición",
                pos.x,
                pos.z
            );
        }
        decompress(payload, codec)
            .with_context(|| format!("No se pudo leer el NBT del chunk ({}, {})", pos.x, pos.z))
    }

    pub fn write_chunk(&mut self, pos: ChunkPos, nbt: &[u8]) -> Result<()> {
        let (index, previous) = self.chunk_span(pos)?;
        ensure!(
            !nbt.is_empty(),
            "Se requiere un contenido NBT para editar el chunk"
        );
        let original = self.read_chunk(pos)?;
        if original == nbt {
            return Ok(());
        }
        let mut encoder = ZlibEncoder::new(vec![0, 0, 0, 0, ZLIB], Compression::default());
        encoder
            .write_all(nbt)
            .context("No se pudo comprimir el chunk")?;
        let mut record = encoder
            .finish()
            .context("No se pudo terminar la compresión del chunk")?;
        let required = record.len().div_ceil(SECTOR_SIZE);
        ensure!(
            required <= MAX_CHUNK_SECTORS,
            "El chunk ({}, {}) requeriría un archivo externo .mcc después de la edición",
            pos.x,
            pos.z
        );
        let length = (record.len() - 4) as u32;
        record[..4].copy_from_slice(&length.to_be_bytes());
        let (codec, payload) = chunk_data(&record)?;
        ensure!(
            decompress(payload, codec)? == nbt,
            "El chunk comprimido difiere de su NBT preparado"
        );

        let span = if required <= previous.sectors {
            previous
        } else {
            self.allocate(index, previous.start, required)?
        };
        let timestamp = u32::try_from(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs())
            .context("La fecha del sistema está fuera del intervalo de marcas de tiempo Anvil")?;
        let range = span.byte_range();
        if range.end > self.bytes.len() {
            self.bytes
                .try_reserve_exact(range.end - self.bytes.len())
                .context("No hay memoria para ampliar la región Anvil")?;
            self.bytes.resize(range.end, 0);
        }

        let contents = &mut self.bytes[range];
        contents[..record.len()].copy_from_slice(&record);
        contents[record.len()..].fill(0);
        let location = index * 4;
        self.bytes[location..location + 4].copy_from_slice(&span.location());
        let time = SECTOR_SIZE + location;
        self.bytes[time..time + 4].copy_from_slice(&timestamp.to_be_bytes());
        self.spans[index] = Some(span);
        Ok(())
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }

    fn chunk_span(&self, pos: ChunkPos) -> Result<(usize, Span)> {
        ensure!(
            self.region
                == [
                    pos.x.div_euclid(REGION_WIDTH),
                    pos.z.div_euclid(REGION_WIDTH)
                ],
            "El chunk ({}, {}) pertenece a otra región Anvil",
            pos.x,
            pos.z
        );
        let index = (pos.x.rem_euclid(REGION_WIDTH) + REGION_WIDTH * pos.z.rem_euclid(REGION_WIDTH))
            as usize;
        let span = self.spans[index].with_context(|| {
            format!(
                "El chunk ({}, {}) no tiene sectores generados en la región Anvil",
                pos.x, pos.z
            )
        })?;
        Ok((index, span))
    }

    fn ordered_spans(&self, except: Option<usize>) -> Vec<(usize, Span)> {
        let mut ordered: Vec<_> = self
            .spans
            .iter()
            .enumerate()
            .filter_map(|(index, span)| (Some(index) != except).then_some((index, (*span)?)))
            .collect();
        ordered.sort_unstable_by_key(|(_, span)| span.start);
        ordered
    }

    fn allocate(&self, index: usize, preferred: usize, sectors: usize) -> Result<Span> {
        let mut next = HEADER_SECTORS;
        let mut first_fit = None;
        let mut preferred_fits = false;
        for (_, used) in self.ordered_spans(Some(index)) {
            if used.start - next >= sectors {
                first_fit.get_or_insert(next);
                preferred_fits |= preferred >= next && preferred + sectors <= used.start;
            }
            next = used.end();
        }
        let start = if preferred_fits || preferred >= next {
            preferred
        } else {
            first_fit.unwrap_or(next)
        };
        ensure!(
            start <= MAX_SECTOR_OFFSET,
            "La región Anvil no tiene sectores direccionables libres"
        );
        ensure!(
            (start + sectors).checked_mul(SECTOR_SIZE).is_some(),
            "El tamaño de la región Anvil no cabe en esta plataforma"
        );
        Ok(Span { start, sectors })
    }
}

fn chunk_data(bytes: &[u8]) -> Result<(u8, &[u8])> {
    let Some(&[a, b, c, d, codec, ..]) = bytes.get(..5) else {
        bail!("Encabezado de chunk Anvil incompleto");
    };
    let length = u32::from_be_bytes([a, b, c, d]) as usize;
    ensure!(
        (1..=bytes.len() - 4).contains(&length),
        "Longitud del chunk Anvil fuera de sus sectores"
    );
    if codec & EXTERNAL != 0 {
        ensure!(length == 1, "Referencia externa .mcc con longitud inválida");
    } else {
        ensure!(length > 1, "El chunk Anvil no contiene datos");
    }
    Ok((codec, &bytes[5..4 + length]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::write::GzEncoder;
    use lz4_java_wrc::Lz4BlockOutput;

    const REGION: [i32; 2] = [-1, 2];
    const FIRST: ChunkPos = ChunkPos { x: -32, z: 64 };
    const SECOND: ChunkPos = ChunkPos { x: -31, z: 64 };
    const THIRD: ChunkPos = ChunkPos { x: -30, z: 64 };

    fn empty_region(sectors: usize) -> Vec<u8> {
        let mut bytes = vec![0x9a; SECTOR_SIZE * sectors];
        bytes[..2 * SECTOR_SIZE].fill(0);
        bytes
    }

    fn add_record(
        bytes: &mut [u8],
        index: usize,
        start: usize,
        sectors: usize,
        codec: u8,
        body: &[u8],
    ) {
        let span = Span { start, sectors };
        bytes[index * 4..index * 4 + 4].copy_from_slice(&span.location());
        let time = SECTOR_SIZE + index * 4;
        bytes[time..time + 4].copy_from_slice(&(100 + index as u32).to_be_bytes());
        let record = &mut bytes[span.byte_range()];
        record[..4].copy_from_slice(&((body.len() + 1) as u32).to_be_bytes());
        record[4] = codec;
        record[5..5 + body.len()].copy_from_slice(body);
    }

    fn poorly_compressible(length: usize) -> Vec<u8> {
        let mut rng = 0x2233_4455_u32;
        (0..length)
            .map(|_| {
                rng ^= rng << 13;
                rng ^= rng >> 17;
                rng ^= rng << 5;
                rng as u8
            })
            .collect()
    }

    fn assert_preserved(before: &[u8], after: &[u8], except: &[Range<usize>]) {
        assert!(after.len() >= before.len());
        for (index, (&a, &b)) in before.iter().zip(after).enumerate() {
            if !except.iter().any(|range| range.contains(&index)) {
                assert_eq!(a, b, "se modificó el byte {index}");
            }
        }
    }

    #[test]
    fn rewrites_in_place_and_preserves_neighbor_bytes_and_timestamps() -> Result<()> {
        let mut bytes = empty_region(7);
        add_record(&mut bytes, 0, 2, 2, 3, b"datos del objetivo");
        let mut gzip = GzEncoder::new(Vec::new(), Compression::default());
        gzip.write_all(b"vecino Gzip")?;
        add_record(&mut bytes, 1, 4, 1, 1, &gzip.finish()?);
        let mut lz4 = Vec::new();
        Lz4BlockOutput::new(&mut lz4).write_all(b"vecino LZ4")?;
        add_record(&mut bytes, 2, 6, 1, 4, &lz4);

        let mut image = RegionImage::new(bytes.clone(), REGION)?;
        assert_eq!(image.read_chunk(FIRST)?, b"datos del objetivo");
        assert_eq!(image.read_chunk(SECOND)?, b"vecino Gzip");
        assert_eq!(image.read_chunk(THIRD)?, b"vecino LZ4");
        image.write_chunk(FIRST, b"reemplazo")?;
        assert_eq!(image.read_chunk(FIRST)?, b"reemplazo");
        let changed = image.into_bytes();
        assert_eq!(bytes.len(), changed.len());
        assert_preserved(
            &bytes,
            &changed,
            &[
                SECTOR_SIZE..SECTOR_SIZE + 4,
                2 * SECTOR_SIZE..4 * SECTOR_SIZE,
            ],
        );
        assert_ne!(
            bytes[SECTOR_SIZE..SECTOR_SIZE + 4],
            changed[SECTOR_SIZE..SECTOR_SIZE + 4]
        );
        assert_eq!(changed[2 * SECTOR_SIZE + 4], ZLIB);
        assert_eq!(
            RegionImage::new(changed, REGION)?.read_chunk(FIRST)?,
            b"reemplazo"
        );
        Ok(())
    }

    #[test]
    fn grows_into_free_sectors_then_appends_without_moving_neighbors() -> Result<()> {
        let mut bytes = empty_region(9);
        add_record(&mut bytes, 0, 2, 1, 3, b"objetivo");
        add_record(&mut bytes, 1, 3, 2, 3, b"vecino");
        add_record(&mut bytes, 2, 8, 1, EXTERNAL | 4, &[]);
        let mut image = RegionImage::new(bytes.clone(), REGION)?;
        let expanded = poorly_compressible(7000);
        image.write_chunk(FIRST, &expanded)?;
        assert_eq!(image.read_chunk(FIRST)?, expanded);
        assert_eq!(
            image.spans[0].unwrap().location(),
            Span {
                start: 5,
                sectors: 2
            }
            .location()
        );
        assert_eq!(image.bytes.len(), bytes.len());
        assert_preserved(
            &bytes,
            &image.bytes,
            &[
                0..4,
                SECTOR_SIZE..SECTOR_SIZE + 4,
                5 * SECTOR_SIZE..7 * SECTOR_SIZE,
            ],
        );

        let before_append = image.bytes.clone();
        let expanded = poorly_compressible(18_000);
        image.write_chunk(FIRST, &expanded)?;
        assert_eq!(image.read_chunk(FIRST)?, expanded);
        assert_eq!(
            image.spans[0].unwrap().location(),
            Span {
                start: 9,
                sectors: 5
            }
            .location()
        );
        assert_preserved(
            &before_append,
            &image.bytes,
            &[0..4, SECTOR_SIZE..SECTOR_SIZE + 4],
        );
        assert!(
            image
                .read_chunk(THIRD)
                .unwrap_err()
                .to_string()
                .contains(".mcc")
        );
        assert_eq!(image.read_chunk(SECOND)?, b"vecino");
        assert_eq!(
            RegionImage::new(image.into_bytes(), REGION)?.read_chunk(FIRST)?,
            expanded
        );
        Ok(())
    }

    #[test]
    fn refuses_corrupt_headers_overlaps_and_invalid_lengths() {
        assert!(RegionImage::new(vec![0; SECTOR_SIZE], REGION).is_err());
        assert!(RegionImage::new(vec![0; 2 * SECTOR_SIZE + 1], REGION).is_err());
        let mut bytes = empty_region(5);
        add_record(&mut bytes, 0, 2, 2, 3, b"objetivo");
        for invalid in [
            Span {
                start: 3,
                sectors: 1,
            },
            Span {
                start: 1,
                sectors: 1,
            },
            Span {
                start: 4,
                sectors: 2,
            },
            Span {
                start: 4,
                sectors: 0,
            },
        ] {
            bytes[4..8].copy_from_slice(&invalid.location());
            assert!(RegionImage::new(bytes.clone(), REGION).is_err());
        }
        bytes[4..8].fill(0);
        bytes[2 * SECTOR_SIZE..2 * SECTOR_SIZE + 4].copy_from_slice(&u32::MAX.to_be_bytes());
        assert!(RegionImage::new(bytes, REGION).is_err());
        let mut bytes = empty_region(3);
        add_record(&mut bytes, 0, 2, 1, EXTERNAL | 2, &[1]);
        assert!(RegionImage::new(bytes, REGION).is_err());
    }

    #[test]
    fn refuses_external_missing_corrupt_and_oversized_chunks_without_changes() -> Result<()> {
        let mut bytes = empty_region(5);
        add_record(&mut bytes, 0, 2, 1, 3, b"original");
        add_record(&mut bytes, 1, 3, 1, EXTERNAL | 2, &[]);
        add_record(&mut bytes, 2, 4, 1, ZLIB, b"compresion invalida");
        let mut image = RegionImage::new(bytes.clone(), REGION)?;
        image.write_chunk(FIRST, b"original")?;
        assert!(
            image
                .write_chunk(SECOND, b"nuevo")
                .unwrap_err()
                .to_string()
                .contains(".mcc")
        );
        assert!(image.write_chunk(THIRD, b"nuevo").is_err());
        assert!(
            image
                .write_chunk(ChunkPos { x: -29, ..FIRST }, b"nuevo")
                .is_err()
        );
        assert!(
            image
                .write_chunk(ChunkPos { x: 0, ..FIRST }, b"nuevo")
                .is_err()
        );
        let oversized = poorly_compressible(256 * SECTOR_SIZE);
        assert!(
            image
                .write_chunk(FIRST, &oversized)
                .unwrap_err()
                .to_string()
                .contains(".mcc")
        );
        assert_eq!(image.into_bytes(), bytes);
        Ok(())
    }
}
