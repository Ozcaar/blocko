use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use bevy::{
    camera::visibility::RenderLayers,
    light::NotShadowCaster,
    mesh::{Indices, VertexAttributeValues},
    prelude::*,
    tasks::{AsyncComputeTaskPool, Task, futures::check_ready},
};

use crate::{
    editor::{SceneAssets, View},
    placement::Placement,
    selection::box_edges,
    structure::Structure,
    terrain::mesh_chunk,
    world::{BlockState, Chunk, Section},
};

const TILE_SIZE: u32 = 16;
const RADIUS: i32 = 5;
const MAX_TILES: usize = 96;
const MAX_VERTICES: usize = 700_000;
const MAX_JOBS: usize = 4;
const MAX_JOBS_PER_VIEW: usize = 2;

type Tile = [i32; 3];

#[derive(Resource)]
pub struct PreviewAssets {
    cube: Handle<Mesh>,
    outline: Handle<StandardMaterial>,
    ghost: Handle<StandardMaterial>,
}

pub fn setup(
    mut commands: Commands,
    scene: Res<SceneAssets>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let texture = materials
        .get(&scene.terrain)
        .and_then(|terrain| terrain.base_color_texture.clone());
    commands.insert_resource(PreviewAssets {
        cube: meshes.add(Cuboid::new(1.0, 1.0, 1.0)),
        outline: materials.add(StandardMaterial {
            base_color: Color::srgb(1.0, 0.73, 0.28),
            unlit: true,
            fog_enabled: false,
            ..default()
        }),
        ghost: materials.add(StandardMaterial {
            base_color: Color::srgba(0.31, 0.94, 1.0, 0.68),
            base_color_texture: texture,
            alpha_mode: AlphaMode::Blend,
            unlit: true,
            depth_bias: 2.0,
            ..default()
        }),
    });
}

enum TileRender {
    Empty,
    Deferred,
    Visible { entity: Entity, vertices: usize },
}

struct PreviewScene {
    root: Entity,
    layers: RenderLayers,
    source: Arc<Structure>,
    origin: [i32; 3],
    center: Option<Tile>,
    priority: Vec<Tile>,
    tiles: HashMap<Tile, TileRender>,
    jobs: HashMap<Tile, Task<Option<Mesh>>>,
    vertices: usize,
}

impl PreviewScene {
    fn new(
        commands: &mut Commands,
        window: Entity,
        layer: usize,
        preview: &Placement,
        assets: &PreviewAssets,
    ) -> Self {
        let origin = preview.origin();
        let root = commands
            .spawn((
                Transform::from_translation(position(origin)),
                Visibility::default(),
                ChildOf(window),
            ))
            .id();
        let layers = RenderLayers::none().with(layer);
        let max = preview
            .source
            .volume
            .dimensions()
            .map(|size| size as i32 - 1);
        for edge in box_edges([0; 3], max, 0.055, 0.075) {
            commands.spawn((
                Mesh3d(assets.cube.clone()),
                MeshMaterial3d(assets.outline.clone()),
                edge,
                layers.clone(),
                NotShadowCaster,
                ChildOf(root),
            ));
        }
        Self {
            root,
            layers,
            source: preview.source.clone(),
            origin,
            center: None,
            priority: Vec::new(),
            tiles: HashMap::new(),
            jobs: HashMap::new(),
            vertices: 0,
        }
    }

    fn recenter(&mut self, commands: &mut Commands, origin: [i32; 3], camera: Vec3) {
        if self.origin != origin {
            self.origin = origin;
            commands
                .entity(self.root)
                .insert(Transform::from_translation(position(origin)));
        }
        let camera = camera.to_array().map(|axis| axis.floor() as i32);
        let center = std::array::from_fn(|axis| (camera[axis] - origin[axis]).div_euclid(16));
        if self.center == Some(center) {
            return;
        }
        self.center = Some(center);
        self.priority = nearby_tiles(self.source.volume.dimensions(), center);
        let keep: HashSet<_> = self.priority.iter().copied().collect();
        self.jobs.retain(|tile, _| keep.contains(tile));
        self.tiles.retain(|tile, rendered| {
            if keep.contains(tile) && !matches!(rendered, TileRender::Deferred) {
                return true;
            }
            if let TileRender::Visible { entity, vertices } = rendered {
                commands.entity(*entity).try_despawn();
                self.vertices -= *vertices;
            }
            false
        });
    }

    fn collect(
        &mut self,
        commands: &mut Commands,
        meshes: &mut Assets<Mesh>,
        assets: &PreviewAssets,
    ) {
        for rank in 0..self.priority.len() {
            let tile = self.priority[rank];
            let Some(mesh) = self.jobs.get_mut(&tile).and_then(check_ready) else {
                continue;
            };
            self.jobs.remove(&tile);
            let Some(mesh) = mesh else {
                self.tiles.insert(tile, TileRender::Empty);
                continue;
            };
            let vertices = mesh.count_vertices();
            if self.vertices + vertices > MAX_VERTICES {
                self.evict_farther(commands, rank, vertices);
            }
            if self.vertices + vertices > MAX_VERTICES {
                self.tiles.insert(tile, TileRender::Deferred);
                continue;
            }
            let entity = commands
                .spawn((
                    Mesh3d(meshes.add(mesh)),
                    MeshMaterial3d(assets.ghost.clone()),
                    Transform::from_translation(position(tile.map(|axis| axis * TILE_SIZE as i32))),
                    self.layers.clone(),
                    NotShadowCaster,
                    ChildOf(self.root),
                ))
                .id();
            self.vertices += vertices;
            self.tiles
                .insert(tile, TileRender::Visible { entity, vertices });
        }
    }

    fn evict_farther(&mut self, commands: &mut Commands, rank: usize, needed: usize) {
        for tile in self.priority[rank + 1..].iter().rev() {
            let Some(rendered @ TileRender::Visible { .. }) = self.tiles.get_mut(tile) else {
                continue;
            };
            if let TileRender::Visible { entity, vertices } =
                std::mem::replace(rendered, TileRender::Deferred)
            {
                commands.entity(entity).try_despawn();
                self.vertices -= vertices;
            }
            if self.vertices + needed <= MAX_VERTICES {
                break;
            }
        }
    }

    fn schedule(&mut self, available: &mut usize) {
        for &tile in &self.priority {
            if *available == 0 || self.jobs.len() >= MAX_JOBS_PER_VIEW {
                return;
            }
            if self.tiles.contains_key(&tile) || self.jobs.contains_key(&tile) {
                continue;
            }
            let source = Arc::clone(&self.source);
            self.jobs.insert(
                tile,
                AsyncComputeTaskPool::get()
                    .spawn(async move { mesh_tile(tile, |position| source.block_at(position)) }),
            );
            *available -= 1;
        }
    }
}

#[derive(Default)]
pub struct PreviewScenes(HashMap<Entity, PreviewScene>);

pub fn draw(
    mut commands: Commands,
    views: Query<(Entity, &View)>,
    cameras: Query<&Transform>,
    assets: Res<PreviewAssets>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut scenes: Local<PreviewScenes>,
) {
    scenes.0.retain(|window, scene| {
        let match_preview = views
            .get(*window)
            .ok()
            .and_then(|(_, view)| view.preview.as_ref())
            .is_some_and(|preview| Arc::ptr_eq(&preview.source, &scene.source));
        if !match_preview {
            commands.entity(scene.root).try_despawn();
        }
        match_preview
    });

    for (window, view) in &views {
        let (Some(preview), Ok(camera)) = (&view.preview, cameras.get(view.camera)) else {
            continue;
        };
        let scene = scenes.0.entry(window).or_insert_with(|| {
            PreviewScene::new(&mut commands, window, view.layer, preview, &assets)
        });
        scene.recenter(&mut commands, preview.origin(), camera.translation);
        scene.collect(&mut commands, &mut meshes, &assets);
    }
    let pending: usize = scenes.0.values().map(|scene| scene.jobs.len()).sum();
    let mut available = MAX_JOBS.saturating_sub(pending);
    for scene in scenes.0.values_mut() {
        scene.schedule(&mut available);
    }
}

fn position(position: [i32; 3]) -> Vec3 {
    Vec3::from(position.map(|axis| axis as f32))
}

fn nearby_tiles(dimensions: [u32; 3], center: Tile) -> Vec<Tile> {
    let limits = dimensions.map(|size| size.saturating_sub(1).div_euclid(TILE_SIZE) as i32);
    let [x, y, z] = std::array::from_fn(|axis| {
        (center[axis] - RADIUS).max(0)..=(center[axis] + RADIUS).min(limits[axis])
    });
    let mut tiles = Vec::new();
    for x in x {
        for y in y.clone() {
            for z in z.clone() {
                tiles.push([x, y, z]);
            }
        }
    }
    tiles.sort_unstable_by_key(|tile| {
        let [dx, dy, dz] = std::array::from_fn(|axis| tile[axis] - center[axis]);
        (dx * dx + dy * dy + dz * dz, *tile)
    });
    tiles.truncate(MAX_TILES);
    tiles
}

const FULL_SECTION: [(u32, u32); 3] = [(0, TILE_SIZE); 3];

fn mesh_tile<'a>(tile: Tile, sample: impl Fn([u32; 3]) -> Option<&'a BlockState>) -> Option<Mesh> {
    let mut chunk = Chunk::default();
    chunk
        .sections
        .insert(0, sample_section(&sample, tile, FULL_SECTION)?);
    for neighbor in [-1, 1] {
        if let Some(section) = sample_border(&sample, tile, 1, neighbor) {
            chunk.sections.insert(neighbor, section);
        }
    }
    let neighbors = [(0, -1), (0, 1), (2, -1), (2, 1)].map(|(axis, offset)| {
        let section = sample_border(&sample, tile, axis, offset)?;
        let mut chunk = Chunk::default();
        chunk.sections.insert(0, section);
        Some(chunk)
    });
    let mesh = mesh_chunk(&chunk, std::array::from_fn(|side| neighbors[side].as_ref()))?;
    visible_tile(mesh)
}

fn sample_border<'a>(
    sample: &impl Fn([u32; 3]) -> Option<&'a BlockState>,
    mut tile: Tile,
    axis: usize,
    offset: i32,
) -> Option<Section> {
    tile[axis] += offset;
    let mut ranges = FULL_SECTION;
    ranges[axis] = if offset < 0 {
        (TILE_SIZE - 1, TILE_SIZE)
    } else {
        (0, 1)
    };
    sample_section(sample, tile, ranges)
}

fn sample_section<'a>(
    sample: &impl Fn([u32; 3]) -> Option<&'a BlockState>,
    tile: Tile,
    [(x0, x1), (y0, y1), (z0, z1)]: [(u32, u32); 3],
) -> Option<Section> {
    if tile.iter().any(|&axis| axis < 0) {
        return None;
    }
    let [tx, ty, tz] = tile.map(|axis| axis as u32 * TILE_SIZE);
    let mut palette = vec![BlockState {
        name: "minecraft:air".into(),
        properties: Default::default(),
    }];
    let mut blocks = Box::new([0; 4096]);
    let mut seen = HashMap::new();
    for y in y0..y1 {
        for z in z0..z1 {
            for x in x0..x1 {
                let Some(block) = sample([tx + x, ty + y, tz + z]).filter(|block| !block.is_air())
                else {
                    continue;
                };
                let index = seen.entry(block).or_insert_with(|| {
                    let index = palette.len() as u16;
                    palette.push(block.clone());
                    index
                });
                blocks[(y * 256 + z * 16 + x) as usize] = *index;
            }
        }
    }
    (palette.len() > 1).then_some(Section { palette, blocks })
}

fn visible_tile(mut mesh: Mesh) -> Option<Mesh> {
    let VertexAttributeValues::Float32x3(positions) = mesh.attribute(Mesh::ATTRIBUTE_POSITION)?
    else {
        return None;
    };
    let VertexAttributeValues::Float32x3(normals) = mesh.attribute(Mesh::ATTRIBUTE_NORMAL)? else {
        return None;
    };
    let Indices::U32(indices) = mesh.indices()? else {
        return None;
    };
    // The two Y slices provide occlusion. Only emit triangles owned by the center section.
    let visible: Vec<_> = indices
        .chunks_exact(3)
        .filter(|triangle| {
            let [a, b, c] = [triangle[0], triangle[1], triangle[2]].map(|index| index as usize);
            let middle = (positions[a][1] + positions[b][1] + positions[c][1]) / 3.0;
            let inside_face = middle - normals[a][1] * 0.001;
            (0.0..TILE_SIZE as f32).contains(&inside_face)
        })
        .flatten()
        .copied()
        .collect();
    if visible.is_empty() {
        return None;
    }
    mesh.insert_indices(Indices::U32(visible));
    if let Some(VertexAttributeValues::Float32x4(colors)) =
        mesh.attribute_mut(Mesh::ATTRIBUTE_COLOR)
    {
        for color in colors {
            let luminance = 0.2126 * color[0] + 0.7152 * color[1] + 0.0722 * color[2];
            let shade = 0.56 + 0.44 * luminance;
            *color = [shade, shade, shade, color[3]];
        }
    }
    Some(mesh)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_tiles_cull_both_horizontal_and_vertical_seams() {
        let stone = BlockState {
            name: "minecraft:stone".into(),
            properties: Default::default(),
        };
        let sample = |position| {
            [[15, 15, 0], [16, 15, 0], [15, 16, 0]]
                .contains(&position)
                .then_some(&stone)
        };
        let face_count = |tile| mesh_tile(tile, sample).unwrap().indices().unwrap().len() / 6;
        assert_eq!(face_count([0, 0, 0]), 4);
        assert_eq!(face_count([1, 0, 0]), 5);
        assert_eq!(face_count([0, 1, 0]), 5);
    }

    #[test]
    fn relevant_tiles_stay_bounded_and_follow_the_camera() {
        let near = nearby_tiles([256, 256, 16], [4, 8, -1]);
        assert_eq!(near.len(), MAX_TILES);
        assert_eq!(near[0], [4, 8, 0]);
        assert!(nearby_tiles([256, 256, 16], [4, 8, -20]).is_empty());
    }
}
