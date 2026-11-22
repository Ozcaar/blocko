use std::collections::{BTreeMap, HashMap, HashSet};

use anyhow::{Context, Result, bail, ensure};
use fastnbt::{LongArray, Value};

use crate::{
    java::decode_chunk,
    placement::Placement,
    structure::MIN_Y,
    world::{BlockEntity, BlockEntityRecord, BlockState, Chunk, ChunkPos, Section, SignText},
};

type Compound = HashMap<String, Value>;

const DATA_VERSION: i32 = 3955;
const SECTION_COUNT: usize = 24;

struct Change<'a> {
    coordinate: [i32; 3],
    expected_block: &'a BlockState,
    expected_entity: Option<&'a BlockEntity>,
    block_changed: bool,
    entity_changed: bool,
}

pub fn edit_chunk(nbt: &[u8], pos: ChunkPos, placement: &Placement) -> Result<Option<Vec<u8>>> {
    let [x0, z0] = [
        pos.x
            .checked_mul(16)
            .context("Coordenada de chunk X inválida")?,
        pos.z
            .checked_mul(16)
            .context("Coordenada de chunk Z inválida")?,
    ];
    let volume = placement.volume();
    let x_range = volume.min[0].max(x0)..=volume.max[0].min(x0 + 15);
    let z_range = volume.min[2].max(z0)..=volume.max[2].min(z0 + 15);
    if x_range.is_empty() || z_range.is_empty() {
        return Ok(None);
    }

    let Value::Compound(mut root) = fastnbt::from_bytes(nbt)? else {
        bail!("El NBT de destino no es un compuesto");
    };
    ensure!(
        root.get("DataVersion") == Some(&Value::Int(DATA_VERSION)),
        "El chunk de destino debe estar guardado en Minecraft Java 1.21.1"
    );
    ensure!(
        matches!(root.get("yPos"), None | Some(Value::Int(-4))),
        "El chunk usa una altura de sección incompatible con el Overworld"
    );
    let original =
        decode_chunk(nbt, pos)?.context("El chunk de destino no está completamente generado")?;

    let mut changes = Vec::new();
    for y in volume.min[1]..=volume.max[1] {
        for z in z_range.clone() {
            for x in x_range.clone() {
                let coordinate = [x, y, z];
                if let Some(state) = placement.writes_block(coordinate)
                    && let Some(change) = find_change(placement, &original, coordinate, state)?
                {
                    changes.push(change);
                }
            }
        }
    }
    if changes.is_empty() {
        return Ok(None);
    }
    let changed_positions = changes
        .iter()
        .map(|change| change.coordinate)
        .collect::<HashSet<_>>();
    check_pending_updates(&root, [x0, z0], &changed_positions)?;

    if changes.iter().any(|change| change.block_changed) {
        edit_sections(&mut root, &original, &changes)?;
        invalidate_heightmaps(&mut root)?;
        root.insert("isLightOn".into(), Value::Byte(0));
    }
    edit_entities(&mut root, &changes)?;

    let bytes = fastnbt::to_bytes(&root)?;
    let verified = decode_chunk(&bytes, pos)?.context("El chunk editado dejó de estar completo")?;
    verify_changes(&verified, &changes)?;
    Ok(Some(bytes))
}

fn find_change<'a>(
    placement: &'a Placement,
    chunk: &Chunk,
    coordinate: [i32; 3],
    expected_block: &'a BlockState,
) -> Result<Option<Change<'a>>> {
    let [x, y, z] = coordinate;
    if let Some(error) = &chunk.block_entity_error {
        bail!("Entidades de bloque de destino incompatibles: {error}");
    }
    let old_block = chunk.block(x.rem_euclid(16), y, z.rem_euclid(16));
    ensure!(
        old_block.is_some() || expected_block.is_air(),
        "La sección de destino en ({x}, {y}, {z}) carece de datos de bloques. No se pueden inferir sus biomas"
    );
    let block_changed = old_block.is_some_and(|block| block != expected_block);
    if block_changed {
        ensure!(
            expected_block.name.starts_with("minecraft:")
                && old_block.is_none_or(|block| block.name.starts_with("minecraft:")),
            "Bloque de mod no compatible en ({x}, {y}, {z})"
        );
        ensure!(
            !is_point_of_interest(expected_block) && !old_block.is_some_and(is_point_of_interest),
            "El cambio en ({x}, {y}, {z}) afecta un punto de interés de Minecraft"
        );
    }

    let relative = std::array::from_fn(|axis| (coordinate[axis] - placement.origin()[axis]) as u32);
    let expected_entity = placement.source.block_entity_at(relative);
    ensure!(
        expected_entity.is_some() == entity_id(expected_block).is_some(),
        "Faltan datos compatibles de la entidad de bloque que se intenta pegar en ({x}, {y}, {z})"
    );
    let old_entity = match chunk.block_entities.get(&coordinate) {
        Some(BlockEntityRecord::Supported(entity)) => Some(entity),
        Some(BlockEntityRecord::Unsupported { kind, reason }) => {
            bail!(
                "Entidad de bloque de destino no compatible en ({x}, {y}, {z}): {kind}. {reason}"
            );
        }
        None => None,
    };
    ensure!(
        old_entity.is_some() == old_block.and_then(entity_id).is_some(),
        "Faltan datos compatibles de la entidad de bloque de destino en ({x}, {y}, {z})"
    );
    let entity_changed =
        old_entity != expected_entity || (block_changed && expected_entity.is_some());
    Ok((block_changed || entity_changed).then_some(Change {
        coordinate,
        expected_block,
        expected_entity,
        block_changed,
        entity_changed,
    }))
}

fn edit_sections(root: &mut Compound, original: &Chunk, changes: &[Change<'_>]) -> Result<()> {
    let mut changes_by_y = BTreeMap::<i32, Vec<&Change<'_>>>::new();
    for change in changes.iter().filter(|change| change.block_changed) {
        changes_by_y
            .entry(change.coordinate[1].div_euclid(16))
            .or_default()
            .push(change);
    }
    let Value::List(sections) = root
        .get_mut("sections")
        .context("Faltan las secciones del chunk de destino")?
    else {
        bail!("Las secciones del chunk tienen un formato incompatible");
    };
    for section in sections {
        let Value::Compound(section) = section else {
            bail!("Sección NBT de destino inválida");
        };
        // Java 1.21.1 still queues old arrays even when isLightOn is false.
        section.remove("BlockLight");
        section.remove("SkyLight");
        let Some(&Value::Byte(sy)) = section.get("Y") else {
            bail!("Sección sin coordenada Y válida");
        };
        let sy = i32::from(sy);
        let Some(Value::Compound(states)) = section.get_mut("block_states") else {
            continue;
        };
        if let Some(edits) = changes_by_y.remove(&sy) {
            rewrite_block_states(states, &original.sections[&sy], &edits)?;
        }
    }
    ensure!(
        changes_by_y.is_empty(),
        "Faltan datos NBT para las secciones modificadas"
    );
    Ok(())
}

fn rewrite_block_states(
    states: &mut Compound,
    original: &Section,
    edits: &[&Change<'_>],
) -> Result<()> {
    let Some(Value::List(mut palette)) = states.remove("palette") else {
        bail!("Paleta NBT de destino inválida");
    };
    ensure!(
        palette.len() == original.palette.len(),
        "La paleta NBT no corresponde a los datos decodificados"
    );
    let mut by_state = original
        .palette
        .iter()
        .enumerate()
        .map(|(index, state)| (state, index))
        .collect::<HashMap<_, _>>();
    let mut indices = original.blocks.clone();
    for edit in edits {
        let id = *by_state.entry(edit.expected_block).or_insert_with(|| {
            let id = palette.len();
            palette.push(block_state_nbt(edit.expected_block));
            id
        });
        let [x, y, z] = edit.coordinate.map(|axis| axis.rem_euclid(16) as usize);
        indices[y * 256 + z * 16 + x] = u16::try_from(id)?;
    }
    let mut used = vec![false; palette.len()];
    for &id in indices.iter() {
        used[usize::from(id)] = true;
    }
    let mut remap = vec![0; palette.len()];
    let mut compact = Vec::new();
    for (id, value) in palette.into_iter().enumerate() {
        if used[id] {
            remap[id] = compact.len();
            compact.push(value);
        }
    }
    if compact.len() == 1 {
        states.remove("data");
    } else {
        // Modern Anvil does not split palette indices across adjacent longs.
        let bits = ((compact.len() - 1).ilog2() + 1).max(4) as usize;
        let per_word = 64 / bits;
        let packed = indices
            .chunks(per_word)
            .map(|indices| {
                indices
                    .iter()
                    .enumerate()
                    .fold(0_u64, |word, (slot, index)| {
                        word | ((remap[usize::from(*index)] as u64) << (slot * bits))
                    }) as i64
            })
            .collect();
        states.insert("data".into(), Value::LongArray(LongArray::new(packed)));
    }
    states.insert("palette".into(), Value::List(compact));
    Ok(())
}

fn block_state_nbt(state: &BlockState) -> Value {
    let mut compound = Compound::from([("Name".into(), Value::String(state.name.clone()))]);
    if !state.properties.is_empty() {
        let properties = state
            .properties
            .iter()
            .map(|(key, value)| (key.clone(), Value::String(value.clone())))
            .collect();
        compound.insert("Properties".into(), Value::Compound(properties));
    }
    Value::Compound(compound)
}

fn edit_entities(root: &mut Compound, changes: &[Change<'_>]) -> Result<()> {
    let replacements = changes
        .iter()
        .filter(|change| change.entity_changed)
        .map(|change| (change.coordinate, change))
        .collect::<BTreeMap<_, _>>();
    if replacements.is_empty() {
        return Ok(());
    }
    let Value::List(entities) = root
        .entry("block_entities".into())
        .or_insert_with(|| Value::List(Vec::new()))
    else {
        bail!("La lista de entidades de bloque de destino es inválida");
    };
    entities.retain(|tag| {
        !tag_coordinates(tag).is_some_and(|position| replacements.contains_key(&position))
    });
    for change in replacements.values() {
        if let Some(entity) = change.expected_entity {
            entities.push(entity_nbt(
                change.coordinate,
                change.expected_block,
                entity,
            )?);
        }
    }
    Ok(())
}

fn entity_id(state: &BlockState) -> Option<&'static str> {
    if state.name == "minecraft:chest" {
        Some("minecraft:chest")
    } else if state.name.starts_with("minecraft:") && state.name.ends_with("_hanging_sign") {
        Some("minecraft:hanging_sign")
    } else if state.name.starts_with("minecraft:") && state.name.ends_with("_sign") {
        Some("minecraft:sign")
    } else {
        None
    }
}

fn entity_nbt([x, y, z]: [i32; 3], state: &BlockState, entity: &BlockEntity) -> Result<Value> {
    let id = entity_id(state).context("La entidad pegada no corresponde al tipo de bloque")?;
    let mut tags = Compound::from([
        ("id".into(), Value::String(id.into())),
        ("x".into(), Value::Int(x)),
        ("y".into(), Value::Int(y)),
        ("z".into(), Value::Int(z)),
    ]);
    match entity {
        BlockEntity::Chest {
            items,
            custom_name,
            lock,
        } => {
            ensure!(
                id == "minecraft:chest",
                "El inventario no corresponde a un cofre"
            );
            let items = items
                .iter()
                .map(|item| {
                    Value::Compound(Compound::from([
                        ("Slot".into(), Value::Byte(item.slot as i8)),
                        ("id".into(), Value::String(item.id.clone())),
                        ("count".into(), Value::Int(item.count)),
                    ]))
                })
                .collect();
            tags.insert("Items".into(), Value::List(items));
            if let Some(name) = custom_name {
                tags.insert("CustomName".into(), Value::String(name.clone()));
            }
            if let Some(lock) = lock {
                tags.insert("Lock".into(), Value::String(lock.clone()));
            }
        }
        BlockEntity::Sign { front, back, waxed } => {
            ensure!(
                id != "minecraft:chest",
                "El texto no corresponde a un cartel"
            );
            tags.insert("front_text".into(), sign_face_nbt(front));
            tags.insert("back_text".into(), sign_face_nbt(back));
            tags.insert("is_waxed".into(), Value::Byte(i8::from(*waxed)));
        }
    }
    Ok(Value::Compound(tags))
}

fn sign_face_nbt(text: &SignText) -> Value {
    let lines =
        |lines: &[String; 4]| Value::List(lines.iter().cloned().map(Value::String).collect());
    let mut tags = Compound::from([
        ("messages".into(), lines(&text.messages)),
        ("color".into(), Value::String(text.color.clone())),
        (
            "has_glowing_text".into(),
            Value::Byte(i8::from(text.glowing)),
        ),
    ]);
    if let Some(filtered) = &text.filtered_messages {
        tags.insert("filtered_messages".into(), lines(filtered));
    }
    Value::Compound(tags)
}

fn tag_coordinates(tag: &Value) -> Option<[i32; 3]> {
    let Value::Compound(tag) = tag else {
        return None;
    };
    match (tag.get("x"), tag.get("y"), tag.get("z")) {
        (Some(&Value::Int(x)), Some(&Value::Int(y)), Some(&Value::Int(z))) => Some([x, y, z]),
        _ => None,
    }
}

fn check_pending_updates(
    root: &Compound,
    [x0, z0]: [i32; 2],
    affected: &HashSet<[i32; 3]>,
) -> Result<()> {
    for key in ["block_ticks", "fluid_ticks"] {
        let Some(ticks) = root.get(key) else {
            continue;
        };
        let Value::List(ticks) = ticks else {
            bail!("Lista de actualizaciones {key} inválida");
        };
        for tick in ticks {
            let position = tag_coordinates(tick)
                .with_context(|| format!("Actualización {key} sin coordenadas válidas"))?;
            ensure!(
                !affected.contains(&position),
                "El pegado afecta una actualización de Minecraft pendiente en {position:?} ({key})"
            );
        }
    }
    if let Some(post) = root.get("PostProcessing") {
        let Value::List(sections) = post else {
            bail!("Datos de postprocesado inválidos");
        };
        ensure!(
            sections.len() <= SECTION_COUNT,
            "El postprocesado no corresponde al Overworld"
        );
        for (index, section) in sections.iter().enumerate() {
            let Value::List(entries) = section else {
                bail!("Datos de postprocesado de sección inválidos");
            };
            for entry in entries {
                let Value::Short(offset @ 0..=4095) = *entry else {
                    bail!("Posición de postprocesado inválida");
                };
                let [x, y, z] =
                    [offset & 15, (offset >> 4) & 15, (offset >> 8) & 15].map(i32::from);
                let position = [x0 + x, MIN_Y + index as i32 * 16 + y, z0 + z];
                ensure!(
                    !affected.contains(&position),
                    "El pegado afecta un bloque pendiente de postprocesado en {position:?}"
                );
            }
        }
    }
    if let Some(upgrade) = root.get("UpgradeData") {
        let Value::Compound(tags) = upgrade else {
            bail!("Datos de actualización de versión inválidos");
        };
        ensure!(
            tags.iter().all(|(key, value)| match (key.as_str(), value) {
                ("Sides", Value::Byte(0)) => true,
                ("Indices", Value::Compound(indices)) => indices.is_empty(),
                _ => false,
            }),
            "El chunk de destino tiene una actualización de versión pendiente"
        );
    }
    Ok(())
}

fn is_point_of_interest(block: &BlockState) -> bool {
    let Some(name) = block.name.strip_prefix("minecraft:") else {
        return false;
    };
    // Bed heads are POI. Protect both halves so the bed remains consistent.
    name.ends_with("_bed")
        || matches!(
            name,
            "blast_furnace"
                | "smoker"
                | "cartography_table"
                | "brewing_stand"
                | "composter"
                | "barrel"
                | "fletching_table"
                | "cauldron"
                | "water_cauldron"
                | "lava_cauldron"
                | "powder_snow_cauldron"
                | "lectern"
                | "stonecutter"
                | "loom"
                | "smithing_table"
                | "grindstone"
                | "bell"
                | "beehive"
                | "bee_nest"
                | "nether_portal"
                | "lodestone"
                | "lightning_rod"
        )
}

fn invalidate_heightmaps(chunk: &mut Compound) -> Result<()> {
    if let Some(tag) = chunk.get_mut("Heightmaps") {
        let Value::Compound(heightmaps) = tag else {
            bail!("Los mapas de altura del destino son inválidos");
        };
        for key in [
            "WORLD_SURFACE_WG",
            "WORLD_SURFACE",
            "OCEAN_FLOOR_WG",
            "OCEAN_FLOOR",
            "MOTION_BLOCKING",
            "MOTION_BLOCKING_NO_LEAVES",
        ] {
            heightmaps.remove(key);
        }
    }
    Ok(())
}

fn verify_changes(chunk: &Chunk, changes: &[Change<'_>]) -> Result<()> {
    ensure!(
        chunk.block_entity_error.is_none(),
        "El chunk editado contiene entidades malformadas"
    );
    for change in changes {
        let [x, y, z] = change.coordinate;
        let block = chunk.block(x.rem_euclid(16), y, z.rem_euclid(16));
        ensure!(
            block == Some(change.expected_block),
            "La verificación del bloque en ({x}, {y}, {z}) no coincide"
        );
        let entity = match chunk.block_entities.get(&change.coordinate) {
            None => None,
            Some(BlockEntityRecord::Supported(entity)) => Some(entity),
            Some(BlockEntityRecord::Unsupported { kind, reason }) => {
                bail!("Entidad inválida al verificar ({x}, {y}, {z}): {kind}. {reason}");
            }
        };
        ensure!(
            entity == change.expected_entity,
            "La verificación de la entidad en ({x}, {y}, {z}) no coincide"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::{
        placement::AirMode,
        structure::{Structure, Volume},
        world::ChestItem,
    };
    use fastnbt::ByteArray;

    const TARGET: ChunkPos = ChunkPos { x: -2, z: -3 };
    const ORIGIN: [i32; 3] = [-32, -49, -48];

    fn state(name: &str, properties: &[(&str, &str)]) -> BlockState {
        BlockState {
            name: format!("minecraft:{name}"),
            properties: properties
                .iter()
                .map(|&(key, value)| (key.into(), value.into()))
                .collect(),
        }
    }

    fn text(value: &str) -> Value {
        Value::String(value.into())
    }

    fn compound<const N: usize>(entries: [(&str, Value); N]) -> Compound {
        entries
            .into_iter()
            .map(|(name, value)| (name.into(), value))
            .collect()
    }

    fn states_tag(palette: &[BlockState], indices: &[u16; 4096]) -> Value {
        let mut data = compound([(
            "palette",
            Value::List(palette.iter().map(block_state_nbt).collect()),
        )]);
        if palette.len() > 1 {
            assert!(
                palette.len() <= 16,
                "la prueba prepara una paleta de hasta cuatro bits"
            );
            let longs = indices
                .chunks(16)
                .map(|group| {
                    group.iter().enumerate().fold(0_u64, |word, (i, index)| {
                        word | (u64::from(*index) << (i * 4))
                    }) as i64
                })
                .collect();
            data.insert("data".into(), Value::LongArray(LongArray::new(longs)));
        }
        Value::Compound(data)
    }

    fn destination(palette: &[BlockState], indices: &[u16; 4096]) -> Compound {
        let biomes = Value::Compound(compound([(
            "palette",
            Value::List(vec![text("minecraft:plains")]),
        )]));
        let blocks = states_tag(palette, indices);
        let light = Value::ByteArray(ByteArray::new(vec![0; 2048]));
        let section = Value::Compound(compound([
            ("Y", Value::Byte(-4)),
            ("block_states", blocks),
            ("biomes", biomes),
            ("BlockLight", light.clone()),
            ("SkyLight", light),
            ("FutureSectionTag", text("preserved")),
        ]));
        compound([
            ("DataVersion", Value::Int(DATA_VERSION)),
            ("Status", text("minecraft:full")),
            ("xPos", Value::Int(TARGET.x)),
            ("yPos", Value::Int(-4)),
            ("zPos", Value::Int(TARGET.z)),
            ("isLightOn", Value::Byte(1)),
            ("sections", Value::List(vec![section])),
            ("block_entities", Value::List(vec![])),
            (
                "Heightmaps",
                Value::Compound(compound([
                    (
                        "WORLD_SURFACE",
                        Value::LongArray(LongArray::new(vec![42; 37])),
                    ),
                    ("FutureMap", Value::ByteArray(ByteArray::new(vec![2, 3]))),
                ])),
            ),
            (
                "FutureRootTag",
                Value::LongArray(LongArray::new(vec![1, -2, 3])),
            ),
        ])
    }

    fn source(
        palette: Vec<BlockState>,
        entities: BTreeMap<[i32; 3], BlockEntityRecord>,
    ) -> Result<Arc<Structure>> {
        let last = i32::try_from(palette.len())? - 1;
        Structure::capture(
            "Bosque".into(),
            Volume::between([0; 3], [last, 0, 0]),
            |_| {
                let mut blocks = Box::new([0; 4096]);
                for (index, block) in blocks.iter_mut().take(palette.len()).enumerate() {
                    *block = index as u16;
                }
                Ok(Some(Chunk {
                    sections: BTreeMap::from([(
                        0,
                        Section {
                            palette: palette.clone(),
                            blocks,
                        },
                    )]),
                    block_entities: entities
                        .iter()
                        .map(|(pos, record)| {
                            let BlockEntityRecord::Supported(entity) = record else {
                                panic!("entidad de prueba inválida")
                            };
                            (*pos, BlockEntityRecord::Supported(entity.clone()))
                        })
                        .collect(),
                    ..Chunk::default()
                }))
            },
        )
        .map(Arc::new)
    }

    fn read_entity(chunk: &Chunk, [x, y, z]: [i32; 3]) -> &BlockEntity {
        let Some(BlockEntityRecord::Supported(entity)) = chunk.block_entities.get(&[x, y, z])
        else {
            panic!("Falta entidad esperada en ({x}, {y}, {z})");
        };
        entity
    }

    #[test]
    fn copies_oriented_blocks_and_entities_and_preserves_other_nbt_in_both_modes() -> Result<()> {
        let stairs = state(
            "oak_stairs",
            &[
                ("facing", "east"),
                ("half", "top"),
                ("shape", "straight"),
                ("waterlogged", "false"),
            ],
        );
        let chest = BlockEntity::Chest {
            items: vec![ChestItem {
                slot: 7,
                id: "minecraft:diamond".into(),
                count: 6,
            }],
            custom_name: Some(r#"{"text":"Materiales"}"#.into()),
            lock: Some("llave".into()),
        };
        let face = |first: &str, color: &str, glowing: bool| SignText {
            messages: [first.into(), "\"\"".into(), "\"\"".into(), "\"\"".into()],
            filtered_messages: glowing.then(|| {
                [
                    r#""******""#.into(),
                    "\"\"".into(),
                    "\"\"".into(),
                    "\"\"".into(),
                ]
            }),
            color: color.into(),
            glowing,
        };
        let sign = BlockEntity::Sign {
            front: face(r#"{"text": "Entrada"}"#, "red", true),
            back: face(r#""Salida""#, "black", false),
            waxed: true,
        };
        let structure = source(
            vec![
                stairs.clone(),
                state("chest", &[("type", "single")]),
                state("oak_sign", &[("rotation", "11")]),
                state("cave_air", &[]),
            ],
            BTreeMap::from([
                ([1, 0, 0], BlockEntityRecord::Supported(chest.clone())),
                ([2, 0, 0], BlockEntityRecord::Supported(sign.clone())),
            ]),
        )?;
        let stone = state("stone", &[]);
        let mut original = destination(std::slice::from_ref(&stone), &[0; 4096]);
        let other = Value::Compound(compound([
            ("id", text("minecraft:command_block")),
            ("x", Value::Int(-26)),
            ("y", Value::Int(-49)),
            ("z", Value::Int(-48)),
            ("UnknownEntityField", Value::Int(781)),
        ]));
        original.insert("block_entities".into(), Value::List(vec![other.clone()]));
        let distant_tick = Value::List(vec![Value::Compound(compound([
            ("x", Value::Int(-28)),
            ("y", Value::Int(-49)),
            ("z", Value::Int(-48)),
            ("i", text("minecraft:water")),
            ("t", Value::Int(91)),
            ("p", Value::Int(0)),
        ]))]);
        original.insert("fluid_ticks".into(), distant_tick.clone());
        let nbt = fastnbt::to_bytes(&original)?;

        let mut placement = Placement::new(structure, ORIGIN);
        for (mode, remaining) in [
            (AirMode::Ignore, &stone),
            (AirMode::Include, &state("cave_air", &[])),
        ] {
            placement.mode = mode;
            let edited = edit_chunk(&nbt, TARGET, &placement)?.expect("debe editar el chunk");
            let decoded = decode_chunk(&edited, TARGET)?.expect("chunk completo");
            assert_eq!(decoded.block(0, -49, 0), Some(&stairs));
            assert_eq!(decoded.block(3, -49, 0), Some(remaining));
            assert_eq!(decoded.block(4, -49, 0), Some(&stone));
            assert_eq!(read_entity(&decoded, [-31, -49, -48]), &chest);
            assert_eq!(read_entity(&decoded, [-30, -49, -48]), &sign);

            let result: Compound = fastnbt::from_bytes(&edited)?;
            assert_eq!(result["FutureRootTag"], original["FutureRootTag"]);
            assert_eq!(result["fluid_ticks"], distant_tick);
            assert_eq!(result["isLightOn"], Value::Byte(0));
            let Value::Compound(heightmaps) = &result["Heightmaps"] else {
                panic!("mapa de altura");
            };
            assert!(!heightmaps.contains_key("WORLD_SURFACE"));
            assert_eq!(
                heightmaps["FutureMap"],
                Value::ByteArray(ByteArray::new(vec![2, 3]))
            );
            let Value::List(sections) = &result["sections"] else {
                panic!("secciones");
            };
            let Value::Compound(section) = &sections[0] else {
                panic!("sección");
            };
            assert!(!section.contains_key("BlockLight") && !section.contains_key("SkyLight"));
            assert_eq!(
                section["biomes"],
                Value::Compound(compound([(
                    "palette",
                    Value::List(vec![text("minecraft:plains")])
                )]))
            );
            assert_eq!(section["FutureSectionTag"], text("preserved"));
            let Value::List(entities) = &result["block_entities"] else {
                panic!("entidades");
            };
            assert_eq!(entities[0], other);
            assert!(edit_chunk(&edited, TARGET, &placement)?.is_none());
        }
        Ok(())
    }

    #[test]
    fn repacks_when_the_palette_crosses_a_padded_word_boundary() -> Result<()> {
        let mut indices = [0; 4096];
        let palette = [
            "stone",
            "dirt",
            "coarse_dirt",
            "rooted_dirt",
            "granite",
            "diorite",
            "andesite",
            "cobblestone",
            "bedrock",
            "calcite",
            "tuff",
            "sand",
            "gravel",
            "stone_bricks",
            "deepslate",
            "cobbled_deepslate",
        ]
        .map(|name| state(name, &[]));
        for (index, block) in indices.iter_mut().take(16).enumerate() {
            *block = index as u16;
        }
        let old = destination(&palette, &indices);
        let planks = state("oak_planks", &[]);
        let placement = Placement::new(
            source(vec![planks.clone()], BTreeMap::new())?,
            [-32, -64, -47],
        );
        let output = edit_chunk(&fastnbt::to_bytes(&old)?, TARGET, &placement)?.unwrap();
        let chunk = decode_chunk(&output, TARGET)?.unwrap();
        for (x, block) in palette.iter().enumerate() {
            assert_eq!(chunk.block(x as i32, -64, 0), Some(block));
        }
        assert_eq!(chunk.block(0, -64, 1), Some(&planks));
        let tags: Compound = fastnbt::from_bytes(&output)?;
        let Value::List(sections) = &tags["sections"] else {
            panic!("secciones");
        };
        let Value::Compound(section) = &sections[0] else {
            panic!("sección");
        };
        let Value::Compound(block_states) = &section["block_states"] else {
            panic!("bloques");
        };
        let Value::LongArray(packed) = &block_states["data"] else {
            panic!("paleta empaquetada");
        };
        assert_eq!(packed.len(), 4096_usize.div_ceil(12));
        Ok(())
    }

    #[test]
    fn refuses_incompatible_destinations_before_serializing_edits() -> Result<()> {
        let placement = Placement::new(
            source(vec![state("oak_planks", &[])], BTreeMap::new())?,
            ORIGIN,
        );
        let stone = state("stone", &[]);
        let tick = Value::Compound(compound([
            ("x", Value::Int(ORIGIN[0])),
            ("y", Value::Int(ORIGIN[1])),
            ("z", Value::Int(ORIGIN[2])),
        ]));
        let unsupported_entity = Value::Compound(compound([
            ("x", Value::Int(ORIGIN[0])),
            ("y", Value::Int(ORIGIN[1])),
            ("z", Value::Int(ORIGIN[2])),
            ("id", text("minecraft:command_block")),
        ]));
        for (key, value, message) in [
            ("DataVersion", Value::Int(DATA_VERSION - 1), "1.21.1"),
            ("Status", text("minecraft:light"), "generado"),
            ("sections", Value::List(vec![]), "biomas"),
            (
                "block_ticks",
                Value::List(vec![tick.clone()]),
                "block_ticks",
            ),
            ("fluid_ticks", Value::List(vec![tick]), "fluid_ticks"),
            (
                "PostProcessing",
                Value::List(vec![Value::List(vec![Value::Short(15 << 4)])]),
                "postprocesado",
            ),
            (
                "UpgradeData",
                Value::Compound(compound([("Sides", Value::Byte(1))])),
                "actualización",
            ),
            (
                "block_entities",
                Value::List(vec![unsupported_entity]),
                "command_block",
            ),
        ] {
            let mut dst = destination(std::slice::from_ref(&stone), &[0; 4096]);
            dst.insert(key.into(), value);
            let error = edit_chunk(&fastnbt::to_bytes(&dst)?, TARGET, &placement).unwrap_err();
            assert!(format!("{error:#}").contains(message), "{key}: {error:#}");
        }
        let poi = destination(&[state("bell", &[])], &[0; 4096]);
        assert!(
            edit_chunk(&fastnbt::to_bytes(&poi)?, TARGET, &placement)
                .unwrap_err()
                .to_string()
                .contains("punto de interés")
        );
        Ok(())
    }

    #[test]
    fn overwriting_a_supported_destination_entity_removes_only_that_entity() -> Result<()> {
        let chest = state("chest", &[("type", "single")]);
        let existing = BlockEntity::Chest {
            items: vec![],
            custom_name: None,
            lock: None,
        };
        let a = entity_nbt(ORIGIN, &chest, &existing)?;
        let b = entity_nbt([-31, -49, -48], &chest, &existing)?;
        let mut dst = destination(&[chest], &[0; 4096]);
        dst.insert("block_entities".into(), Value::List(vec![a, b.clone()]));
        let mut placement =
            Placement::new(source(vec![state("air", &[])], BTreeMap::new())?, ORIGIN);
        let nbt = fastnbt::to_bytes(&dst)?;
        assert!(edit_chunk(&nbt, TARGET, &placement)?.is_none());
        placement.mode = AirMode::Include;
        let output = edit_chunk(&nbt, TARGET, &placement)?.unwrap();
        let result: Compound = fastnbt::from_bytes(&output)?;
        assert_eq!(result["block_entities"], Value::List(vec![b]));
        Ok(())
    }
}
