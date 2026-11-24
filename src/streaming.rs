use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use bevy::{
    camera::visibility::RenderLayers,
    prelude::*,
    tasks::{AsyncComputeTaskPool, IoTaskPool, Task, futures::check_ready},
};

use crate::{
    edit::EditSession,
    java::JavaWorld,
    placement::Placement,
    preview_mask::{self, MaskedMesh},
    terrain::mesh_chunk,
    world::{Chunk, ChunkPos},
};

const VIEW_RADIUS: u32 = 5;
const LOAD_RADIUS: u32 = VIEW_RADIUS + 1;
const READ_JOBS: usize = 6;
const MESH_JOBS: usize = 3;

struct PreviewMeshes {
    placement: Placement,
    jobs: HashMap<ChunkPos, Task<MaskedMesh>>,
    results: HashMap<ChunkPos, MaskResult>,
}

enum MaskResult {
    Unchanged,
    Replacement(Option<Entity>),
}

pub struct Stream {
    pub name: String,
    pub root: Entity,
    source: Arc<JavaWorld>,
    center: Option<ChunkPos>,
    priority: Vec<ChunkPos>,
    loaded: HashMap<ChunkPos, Arc<Chunk>>,
    overlaid: HashMap<ChunkPos, Arc<Chunk>>,
    remesh: HashSet<ChunkPos>,
    absent: HashSet<ChunkPos>,
    errors: HashMap<ChunkPos, String>,
    rendered: HashMap<ChunkPos, Option<Entity>>,
    reads: HashMap<ChunkPos, Task<anyhow::Result<Option<Chunk>>>>,
    meshes: HashMap<ChunkPos, Task<Option<Mesh>>>,
    preview: Option<PreviewMeshes>,
}

impl Stream {
    pub fn new(source: JavaWorld, root: Entity) -> Self {
        Self::from_source(Arc::new(source), root)
    }

    fn from_source(source: Arc<JavaWorld>, root: Entity) -> Self {
        Self {
            name: source.name.clone(),
            root,
            source,
            center: None,
            priority: Vec::new(),
            loaded: HashMap::new(),
            overlaid: HashMap::new(),
            remesh: HashSet::new(),
            absent: HashSet::new(),
            errors: HashMap::new(),
            rendered: HashMap::new(),
            reads: HashMap::new(),
            meshes: HashMap::new(),
            preview: None,
        }
    }

    pub fn reload(&mut self, commands: &mut Commands) {
        commands.entity(self.root).despawn_children();
        *self = Self::from_source(self.source.clone(), self.root);
    }

    pub fn status(&self) -> (usize, usize, usize, bool) {
        let visible = self.rendered.values().flatten().count();
        let busy = !self.reads.is_empty() || !self.meshes.is_empty();
        (visible, self.absent.len(), self.errors.len(), busy)
    }

    pub fn path(&self) -> &std::path::Path {
        &self.source.root
    }

    pub fn source(&self) -> Arc<JavaWorld> {
        Arc::clone(&self.source)
    }

    pub fn loaded_chunk(&self, pos: ChunkPos) -> Option<&Chunk> {
        self.visible_chunk(pos).map(Arc::as_ref)
    }

    fn visible_chunk(&self, pos: ChunkPos) -> Option<&Arc<Chunk>> {
        self.overlaid.get(&pos).or_else(|| self.loaded.get(&pos))
    }

    fn sync_edits(&mut self, commands: &mut Commands, edits: &mut EditSession) {
        let invalidated = edits.take_invalidated();
        if invalidated.is_empty() {
            return;
        }
        self.clear_preview(commands);
        for pos in invalidated {
            if let Some(base) = self.loaded.get(&pos) {
                if let Some(patch) = edits.patch_for(pos) {
                    let mut chunk = (**base).clone();
                    chunk.apply_patch(&patch);
                    self.overlaid.insert(pos, Arc::new(chunk));
                } else {
                    self.overlaid.remove(&pos);
                }
            }
            for target in std::iter::once(pos).chain(pos.neighbors()) {
                self.meshes.remove(&target);
                if self.rendered.contains_key(&target) {
                    self.remesh.insert(target);
                }
            }
        }
    }

    pub fn preview_busy(&self) -> bool {
        self.preview
            .as_ref()
            .is_some_and(|preview| !preview.jobs.is_empty())
    }

    pub fn first_error(&self) -> Option<&str> {
        self.priority
            .iter()
            .find_map(|pos| self.errors.get(pos).map(String::as_str))
    }

    pub fn update(
        &mut self,
        commands: &mut Commands,
        assets: &mut Assets<Mesh>,
        material: &Handle<StandardMaterial>,
        layer: usize,
        position: Vec3,
        placement: Option<&Placement>,
        edits: &mut EditSession,
    ) {
        let center = ChunkPos::from_block(position.x.floor() as i32, position.z.floor() as i32);
        if self.center != Some(center) {
            self.recenter(commands, center);
        }
        self.sync_edits(commands, edits);
        self.collect_reads(edits);
        self.schedule_reads();
        self.collect_meshes(commands, assets, material, layer);
        self.schedule_meshes(center);
        self.update_preview(commands, assets, material, layer, placement);
    }

    fn recenter(&mut self, commands: &mut Commands, center: ChunkPos) {
        self.center = Some(center);
        let radius = LOAD_RADIUS as i32;
        self.priority = (-radius..=radius)
            .flat_map(|z| (-radius..=radius).map(move |x| (x, z)))
            .map(|(x, z)| ChunkPos {
                x: center.x + x,
                z: center.z + z,
            })
            .collect();
        self.priority.sort_by_key(|pos| {
            let (x, z) = (pos.x - center.x, pos.z - center.z);
            (x * x + z * z, z, x)
        });
        self.reads
            .retain(|pos, _| pos.distance(center) <= LOAD_RADIUS);
        self.loaded
            .retain(|pos, _| pos.distance(center) <= LOAD_RADIUS);
        self.overlaid
            .retain(|pos, _| pos.distance(center) <= LOAD_RADIUS);
        self.remesh
            .retain(|pos| pos.distance(center) <= VIEW_RADIUS);
        self.absent
            .retain(|pos| pos.distance(center) <= LOAD_RADIUS);
        self.errors
            .retain(|pos, _| pos.distance(center) <= LOAD_RADIUS);
        self.meshes
            .retain(|pos, _| pos.distance(center) <= VIEW_RADIUS);
        self.rendered.retain(|pos, entity| {
            if pos.distance(center) <= VIEW_RADIUS {
                return true;
            }
            if let Some(entity) = entity {
                commands.entity(*entity).despawn();
            }
            false
        });
    }

    fn collect_reads(&mut self, edits: &EditSession) {
        self.reads.retain(|&pos, task| {
            let Some(result) = check_ready(task) else {
                return true;
            };
            match result {
                Ok(Some(chunk)) => {
                    if let Some(patch) = edits.patch_for(pos) {
                        let mut visible = chunk.clone();
                        visible.apply_patch(&patch);
                        self.overlaid.insert(pos, Arc::new(visible));
                    }
                    self.loaded.insert(pos, Arc::new(chunk));
                }
                Ok(None) => {
                    self.absent.insert(pos);
                }
                Err(error) => {
                    warn!("Chunk ({}, {}) en {}: {error:#}", pos.x, pos.z, self.name);
                    self.errors
                        .insert(pos, format!("Chunk {}, {} · {error:#}", pos.x, pos.z));
                }
            }
            false
        });
    }

    fn read_complete(&self, pos: &ChunkPos) -> bool {
        self.loaded.contains_key(pos) || self.absent.contains(pos) || self.errors.contains_key(pos)
    }

    fn schedule_reads(&mut self) {
        for &pos in &self.priority {
            if self.reads.len() >= READ_JOBS {
                break;
            }
            if self.read_complete(&pos) || self.reads.contains_key(&pos) {
                continue;
            }
            let source = self.source.clone();
            self.reads.insert(
                pos,
                IoTaskPool::get().spawn(async move { source.load_chunk(pos) }),
            );
        }
    }

    fn collect_meshes(
        &mut self,
        commands: &mut Commands,
        assets: &mut Assets<Mesh>,
        material: &Handle<StandardMaterial>,
        layer: usize,
    ) {
        self.meshes.retain(|&pos, task| {
            let Some(mesh) = check_ready(task) else {
                return true;
            };
            let entity = mesh.map(|mesh| {
                spawn_terrain(commands, assets.add(mesh), material, pos, layer, self.root)
            });
            if let Some(Some(old)) = self.rendered.insert(pos, entity) {
                commands.entity(old).try_despawn();
            }
            self.remesh.remove(&pos);
            false
        });
    }

    fn schedule_meshes(&mut self, center: ChunkPos) {
        for &pos in &self.priority {
            if self.meshes.len() >= MESH_JOBS {
                break;
            }
            if pos.distance(center) > VIEW_RADIUS
                || (self.rendered.contains_key(&pos) && !self.remesh.contains(&pos))
                || self.meshes.contains_key(&pos)
            {
                continue;
            }
            let Some(chunk) = self.visible_chunk(pos).cloned() else {
                continue;
            };
            let neighbors = pos.neighbors();
            if !neighbors
                .iter()
                .all(|neighbor| self.read_complete(neighbor))
            {
                continue;
            }
            let neighbors = neighbors.map(|neighbor| self.visible_chunk(neighbor).cloned());
            self.meshes.insert(
                pos,
                AsyncComputeTaskPool::get().spawn(async move {
                    mesh_chunk(&chunk, std::array::from_fn(|i| neighbors[i].as_deref()))
                }),
            );
        }
    }

    fn clear_preview(&mut self, commands: &mut Commands) {
        if let Some(active) = self.preview.take() {
            for (pos, result) in active.results {
                if let MaskResult::Replacement(entity) = result {
                    if let Some(entity) = entity {
                        commands.entity(entity).try_despawn();
                    }
                    if let Some(Some(base)) = self.rendered.get(&pos) {
                        commands.entity(*base).insert(Visibility::Inherited);
                    }
                }
            }
        }
    }

    fn update_preview(
        &mut self,
        commands: &mut Commands,
        assets: &mut Assets<Mesh>,
        material: &Handle<StandardMaterial>,
        layer: usize,
        placement: Option<&Placement>,
    ) {
        let changed = self.preview.as_ref().is_some_and(|active| {
            !placement.is_some_and(|placement| active.placement.same_mask(placement))
        });
        if changed {
            self.clear_preview(commands);
        }
        let Some(placement) = placement else {
            return;
        };
        let active = self.preview.get_or_insert_with(|| PreviewMeshes {
            placement: placement.clone(),
            jobs: HashMap::new(),
            results: HashMap::new(),
        });
        active.jobs.retain(|pos, _| self.rendered.contains_key(pos));
        active.results.retain(|pos, result| {
            if self.rendered.contains_key(pos) {
                return true;
            }
            if let MaskResult::Replacement(Some(entity)) = result {
                commands.entity(*entity).try_despawn();
            }
            false
        });

        active.jobs.retain(|&pos, task| {
            let Some(mesh) = check_ready(task) else {
                return true;
            };
            let result = match mesh {
                MaskedMesh::Unchanged => MaskResult::Unchanged,
                MaskedMesh::Replaced(mesh) => {
                    if let Some(Some(base)) = self.rendered.get(&pos) {
                        commands.entity(*base).insert(Visibility::Hidden);
                    }
                    let entity = mesh.map(|mesh| {
                        spawn_terrain(commands, assets.add(mesh), material, pos, layer, self.root)
                    });
                    MaskResult::Replacement(entity)
                }
            };
            active.results.insert(pos, result);
            false
        });

        for &pos in &self.priority {
            if active.jobs.len() >= MESH_JOBS {
                break;
            }
            if !self.rendered.contains_key(&pos)
                || self.remesh.contains(&pos)
                || active.results.contains_key(&pos)
                || active.jobs.contains_key(&pos)
                || !preview_mask::affects_mesh(placement, pos)
            {
                continue;
            }
            let Some(chunk) = self
                .overlaid
                .get(&pos)
                .or_else(|| self.loaded.get(&pos))
                .cloned()
            else {
                continue;
            };
            let neighbors = pos.neighbors().map(|neighbor| {
                self.overlaid
                    .get(&neighbor)
                    .or_else(|| self.loaded.get(&neighbor))
                    .cloned()
            });
            let placement = placement.clone();
            active.jobs.insert(
                pos,
                AsyncComputeTaskPool::get().spawn(async move {
                    preview_mask::mesh_masked(
                        &placement,
                        pos,
                        &chunk,
                        std::array::from_fn(|i| neighbors[i].as_deref()),
                    )
                }),
            );
        }
    }
}

fn spawn_terrain(
    commands: &mut Commands,
    mesh: Handle<Mesh>,
    material: &Handle<StandardMaterial>,
    pos: ChunkPos,
    layer: usize,
    root: Entity,
) -> Entity {
    commands
        .spawn((
            Mesh3d(mesh),
            MeshMaterial3d(material.clone()),
            Transform::from_xyz(pos.x as f32 * 16.0, 0.0, pos.z as f32 * 16.0),
            RenderLayers::none().with(layer),
            ChildOf(root),
        ))
        .id()
}
