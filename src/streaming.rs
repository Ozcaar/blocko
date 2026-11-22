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
    java::JavaWorld,
    terrain::mesh_chunk,
    world::{Chunk, ChunkPos},
};

const VIEW_RADIUS: u32 = 5;
const LOAD_RADIUS: u32 = VIEW_RADIUS + 1;
const READ_JOBS: usize = 6;
const MESH_JOBS: usize = 3;

pub struct Stream {
    pub name: String,
    pub root: Entity,
    source: Arc<JavaWorld>,
    center: Option<ChunkPos>,
    priority: Vec<ChunkPos>,
    loaded: HashMap<ChunkPos, Arc<Chunk>>,
    absent: HashSet<ChunkPos>,
    errors: HashMap<ChunkPos, String>,
    rendered: HashMap<ChunkPos, Option<Entity>>,
    reads: HashMap<ChunkPos, Task<anyhow::Result<Option<Chunk>>>>,
    meshes: HashMap<ChunkPos, Task<Option<Mesh>>>,
}

impl Stream {
    pub fn new(source: JavaWorld, root: Entity) -> Self {
        Self {
            name: source.name.clone(),
            root,
            source: Arc::new(source),
            center: None,
            priority: Vec::new(),
            loaded: HashMap::new(),
            absent: HashSet::new(),
            errors: HashMap::new(),
            rendered: HashMap::new(),
            reads: HashMap::new(),
            meshes: HashMap::new(),
        }
    }

    pub fn status(&self) -> (usize, usize, usize, bool) {
        let visible = self.rendered.values().flatten().count();
        let busy = !self.reads.is_empty() || !self.meshes.is_empty();
        (visible, self.absent.len(), self.errors.len(), busy)
    }

    pub fn source(&self) -> Arc<JavaWorld> {
        Arc::clone(&self.source)
    }

    pub fn loaded_chunk(&self, pos: ChunkPos) -> Option<&Chunk> {
        self.loaded.get(&pos).map(Arc::as_ref)
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
    ) {
        let center = ChunkPos::from_block(position.x.floor() as i32, position.z.floor() as i32);
        if self.center != Some(center) {
            self.recenter(commands, center);
        }
        self.collect_reads();
        self.schedule_reads();
        self.collect_meshes(commands, assets, material, layer);
        self.schedule_meshes(center);
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

    fn collect_reads(&mut self) {
        self.reads.retain(|&pos, task| {
            let Some(result) = check_ready(task) else {
                return true;
            };
            match result {
                Ok(Some(chunk)) => {
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
                commands
                    .spawn((
                        Mesh3d(assets.add(mesh)),
                        MeshMaterial3d(material.clone()),
                        Transform::from_xyz(pos.x as f32 * 16.0, 0.0, pos.z as f32 * 16.0),
                        RenderLayers::none().with(layer),
                        ChildOf(self.root),
                    ))
                    .id()
            });
            self.rendered.insert(pos, entity);
            false
        });
    }

    fn schedule_meshes(&mut self, center: ChunkPos) {
        for &pos in &self.priority {
            if self.meshes.len() >= MESH_JOBS {
                break;
            }
            if pos.distance(center) > VIEW_RADIUS
                || self.rendered.contains_key(&pos)
                || self.meshes.contains_key(&pos)
            {
                continue;
            }
            let Some(chunk) = self.loaded.get(&pos).cloned() else {
                continue;
            };
            let neighbors = pos.neighbors();
            if !neighbors
                .iter()
                .all(|neighbor| self.read_complete(neighbor))
            {
                continue;
            }
            let neighbors = neighbors.map(|neighbor| self.loaded.get(&neighbor).cloned());
            self.meshes.insert(
                pos,
                AsyncComputeTaskPool::get().spawn(async move {
                    mesh_chunk(&chunk, std::array::from_fn(|i| neighbors[i].as_deref()))
                }),
            );
        }
    }
}
