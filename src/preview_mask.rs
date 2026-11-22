use std::collections::BTreeMap;

use bevy::prelude::Mesh;

use crate::{
    placement::Placement,
    structure::Volume,
    terrain::mesh_chunk,
    world::{BlockState, Chunk, ChunkPos, Section},
};

const CHUNK_BOUNDS: [[i32; 2]; 2] = [[0, 15]; 2];
// The mesher samples only the facing edge of each horizontal neighbor.
const NEIGHBOR_EDGES: [[[i32; 2]; 2]; 4] = [
    [[15, 15], [0, 15]], // West
    [[0, 0], [0, 15]],   // East
    [[0, 15], [15, 15]], // North
    [[0, 15], [0, 0]],   // South
];

pub enum MaskedMesh {
    Unchanged,
    Replaced(Option<Mesh>),
}

pub fn affects_mesh(placement: &Placement, pos: ChunkPos) -> bool {
    let volume = placement.volume();
    let min = ChunkPos::from_block(volume.min[0], volume.min[2]);
    let max = ChunkPos::from_block(volume.max[0], volume.max[2]);
    let dx = pos.x.abs_diff(pos.x.clamp(min.x, max.x));
    let dz = pos.z.abs_diff(pos.z.clamp(min.z, max.z));
    dx.saturating_add(dz) <= 1
}

pub fn mesh_masked(
    placement: &Placement,
    pos: ChunkPos,
    chunk: &Chunk,
    neighbors: [Option<&Chunk>; 4],
) -> MaskedMesh {
    if !affects_mesh(placement, pos) {
        return MaskedMesh::Unchanged;
    }
    let volume = placement.volume();
    let center = mask_chunk(placement, volume, pos, chunk, CHUNK_BOUNDS);
    let neighbors_pos = pos.neighbors();
    let adjacent: [Option<Chunk>; 4] = std::array::from_fn(|index| {
        mask_chunk(
            placement,
            volume,
            neighbors_pos[index],
            neighbors[index]?,
            NEIGHBOR_EDGES[index],
        )
    });
    if center.is_none() && adjacent.iter().all(Option::is_none) {
        return MaskedMesh::Unchanged;
    }
    MaskedMesh::Replaced(mesh_chunk(
        center.as_ref().unwrap_or(chunk),
        std::array::from_fn(|index| adjacent[index].as_ref().or(neighbors[index])),
    ))
}

fn mask_chunk(
    placement: &Placement,
    volume: Volume,
    pos: ChunkPos,
    chunk: &Chunk,
    [[local_min_x, local_max_x], [local_min_z, local_max_z]]: [[i32; 2]; 2],
) -> Option<Chunk> {
    let x0 = pos.x.checked_mul(16)?;
    let z0 = pos.z.checked_mul(16)?;
    let min_x = volume.min[0].max(x0 + local_min_x);
    let max_x = volume.max[0].min(x0 + local_max_x);
    let min_z = volume.min[2].max(z0 + local_min_z);
    let max_z = volume.max[2].min(z0 + local_max_z);
    if min_x > max_x || min_z > max_z {
        return None;
    }

    let first_section = volume.min[1].div_euclid(16);
    let last_section = volume.max[1].div_euclid(16);
    let mut modified = BTreeMap::new();
    for (&sy, section) in chunk.sections.range(first_section..=last_section) {
        let y0 = sy * 16;
        let min_y = volume.min[1].max(y0);
        let max_y = volume.max[1].min(y0 + 15);
        let mut masked = None;
        for y in min_y..=max_y {
            for z in min_z..=max_z {
                for x in min_x..=max_x {
                    let index = ((y - y0) * 256 + (z - z0) * 16 + x - x0) as usize;
                    let state = &section.palette[section.blocks[index] as usize];
                    if !state.is_air() && placement.writes_block([x, y, z]).is_some() {
                        let (copy, air) = masked.get_or_insert_with(|| copy_with_air(section));
                        copy.blocks[index] = *air;
                    }
                }
            }
        }
        if let Some((section, _)) = masked {
            modified.insert(sy, section);
        }
    }
    if modified.is_empty() {
        return None;
    }
    // Only geometry participates in this temporary mesh. The original chunk
    // also remains the source of truth when the placement is cancelled.
    for (&sy, section) in &chunk.sections {
        modified.entry(sy).or_insert_with(|| copy_section(section));
    }
    Some(Chunk {
        sections: modified,
        ..Chunk::default()
    })
}

fn copy_with_air(section: &Section) -> (Section, u16) {
    let mut copy = copy_section(section);
    let air = copy
        .palette
        .iter()
        .position(BlockState::is_air)
        .unwrap_or_else(|| {
            let index = copy.palette.len();
            copy.palette.push(BlockState {
                name: "minecraft:air".into(),
                properties: BTreeMap::new(),
            });
            index
        });
    (copy, air as u16)
}

fn copy_section(section: &Section) -> Section {
    Section {
        palette: section.palette.clone(),
        blocks: section.blocks.clone(),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::{placement::AirMode, structure::Structure};

    fn test_chunk<const N: usize>(blocks: [([i32; 3], u16); N]) -> Chunk {
        let palette = [
            "minecraft:air",
            "minecraft:stone",
            "minecraft:cave_air",
            "minecraft:void_air",
        ]
        .map(|name| BlockState {
            name: name.into(),
            properties: BTreeMap::new(),
        })
        .to_vec();
        let mut chunk = Chunk::default();
        for ([x, y, z], state) in blocks {
            let section = chunk
                .sections
                .entry(y.div_euclid(16))
                .or_insert_with(|| Section {
                    blocks: Box::new([0; 4096]),
                    palette: palette.clone(),
                });
            section.blocks[(y.rem_euclid(16) * 256 + z * 16 + x) as usize] = state;
        }
        chunk
    }

    fn copied_structure(
        blocks: impl FnMut(ChunkPos) -> anyhow::Result<Option<Chunk>>,
        end: [i32; 3],
    ) -> anyhow::Result<Arc<Structure>> {
        Structure::capture("Origen".into(), Volume::between([0; 3], end), blocks).map(Arc::new)
    }

    #[test]
    fn only_effective_writes_clear_the_destination_and_existing_blocks_stay_intact()
    -> anyhow::Result<()> {
        let source = copied_structure(
            |_| {
                Ok(Some(test_chunk([
                    ([0, 0, 0], 1),
                    ([2, 0, 0], 2),
                    ([3, 0, 0], 3),
                ])))
            },
            [3, 0, 0],
        )?;
        let mut placement = Placement::new(source, [-16, -1, 0]);
        let destination = test_chunk([
            ([0, -1, 0], 1),
            ([1, -1, 0], 1),
            ([2, -1, 0], 1),
            ([3, -1, 0], 1),
        ]);
        let pos = ChunkPos { x: -1, z: 0 };
        let baseline = mesh_chunk(&destination, [None; 4])
            .unwrap()
            .count_vertices();

        placement.mode = AirMode::Include;
        assert!(matches!(
            mesh_masked(&placement, pos, &destination, [None; 4]),
            MaskedMesh::Replaced(None)
        ));

        placement.mode = AirMode::Ignore;
        let MaskedMesh::Replaced(Some(mesh)) =
            mesh_masked(&placement, pos, &destination, [None; 4])
        else {
            panic!("la copia sin aire debe dejar tres bloques del destino");
        };
        assert_eq!(mesh.count_vertices(), 56);
        for x in 0..4 {
            assert_eq!(destination.block(x, -1, 0).unwrap().name, "minecraft:stone");
        }
        assert_eq!(
            mesh_chunk(&destination, [None; 4])
                .unwrap()
                .count_vertices(),
            baseline
        );
        Ok(())
    }

    #[test]
    fn exposes_faces_in_adjacent_chunks_when_the_volume_clears_their_shared_edge()
    -> anyhow::Result<()> {
        let source = copied_structure(|_| Ok(Some(test_chunk([([0, 0, 0], 1)]))), [0; 3])?;
        let pos = ChunkPos { x: -1, z: -1 };
        let touching_blocks = [
            ([0, 0, 8], [15, 0, 8]),
            ([15, 0, 8], [0, 0, 8]),
            ([8, 0, 0], [8, 0, 15]),
            ([8, 0, 15], [8, 0, 0]),
        ];
        for (index, (center_cell, neighbor_cell)) in touching_blocks.into_iter().enumerate() {
            let neighbor_pos = pos.neighbors()[index];
            let [x, y, z] = neighbor_cell;
            let origin = [neighbor_pos.x * 16 + x, y, neighbor_pos.z * 16 + z];
            let mut placement = Placement::new(source.clone(), origin);
            placement.mode = AirMode::Ignore;
            assert!(affects_mesh(&placement, pos));

            let center = test_chunk([(center_cell, 1)]);
            let neighbor = test_chunk([(neighbor_cell, 1)]);
            let mut neighbors = [None; 4];
            neighbors[index] = Some(&neighbor);
            assert_eq!(mesh_chunk(&center, neighbors).unwrap().count_vertices(), 20);
            let MaskedMesh::Replaced(Some(mesh)) = mesh_masked(&placement, pos, &center, neighbors)
            else {
                panic!("el vecino debe dejar visible la cara compartida");
            };
            assert_eq!(mesh.count_vertices(), 24);
            assert_eq!(neighbor.block(x, y, z).unwrap().name, "minecraft:stone");
        }

        let diagonal = Placement::new(source, [-17, 0, -17]);
        assert!(!affects_mesh(&diagonal, pos));
        Ok(())
    }

    #[test]
    fn placements_inside_a_neighbor_leave_the_center_mesh_unchanged() -> anyhow::Result<()> {
        let source = copied_structure(|_| Ok(Some(test_chunk([([0, 0, 0], 1)]))), [0; 3])?;
        let pos = ChunkPos { x: 0, z: 0 };
        let placement = Placement::new(source, [17, 0, 0]);
        let center = test_chunk([([15, 0, 0], 1)]);
        let east = test_chunk([([0, 0, 0], 1), ([1, 0, 0], 1)]);
        assert!(affects_mesh(&placement, pos));
        assert!(matches!(
            mesh_masked(&placement, pos, &center, [None, Some(&east), None, None]),
            MaskedMesh::Unchanged
        ));
        Ok(())
    }

    #[test]
    fn previews_a_cutout_inside_a_fully_solid_section() -> anyhow::Result<()> {
        let source = copied_structure(|_| Ok(Some(test_chunk([([0, 0, 0], 1)]))), [0; 3])?;
        let placement = Placement::new(source, [7, 7, 7]);
        let destination = Chunk {
            sections: BTreeMap::from([(
                0,
                Section {
                    palette: vec![BlockState {
                        name: "minecraft:stone".into(),
                        properties: BTreeMap::new(),
                    }],
                    blocks: Box::new([0; 4096]),
                },
            )]),
            ..Chunk::default()
        };
        let orig = mesh_chunk(&destination, [None; 4])
            .unwrap()
            .count_vertices();
        let MaskedMesh::Replaced(Some(mesh)) =
            mesh_masked(&placement, ChunkPos { x: 0, z: 0 }, &destination, [None; 4])
        else {
            panic!("la sección sólida debe mostrar las caras del hueco");
        };
        assert_eq!(mesh.count_vertices(), orig + 24);
        assert_eq!(destination.sections[&0].palette.len(), 1);
        Ok(())
    }
}
