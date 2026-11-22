use std::{collections::HashSet, path::PathBuf};

use bevy::{
    camera::{RenderTarget, visibility::RenderLayers},
    core_pipeline::tonemapping::Tonemapping,
    light::GlobalAmbientLight,
    pbr::{DistanceFog, FogFalloff},
    prelude::*,
    tasks::{IoTaskPool, Task, futures::check_ready},
    window::{CursorOptions, ExitCondition, WindowRef, WindowResolution},
};
use bevy_egui::{EguiContext, EguiContextSettings, EguiGlobalSettings, EguiPlugin};

use crate::{
    flight::{self, Flight},
    java::JavaWorld,
    streaming::Stream,
    terrain, ui,
};

const SKY: Color = Color::srgb(0.60, 0.75, 0.84);
const STUDIO: Color = Color::srgb(0.075, 0.102, 0.116);

#[derive(Component)]
pub struct View {
    pub camera: Entity,
    pub layer: usize,
    pub stream: Option<Stream>,
    pub pending: Option<PendingOpen>,
    pub error: Option<String>,
    pub viewport: Option<Rect>,
    pub demo_scene: Option<Entity>,
}

#[derive(Component)]
pub struct Overlay(pub Entity);

#[derive(Clone, Copy)]
pub enum Destination {
    Current,
    NewWindow,
}

pub struct PendingOpen {
    destination: Destination,
    task: Task<anyhow::Result<Option<JavaWorld>>>,
}

impl PendingOpen {
    pub fn browse(destination: Destination) -> Self {
        Self::start(destination, None)
    }

    fn start(destination: Destination, path: Option<PathBuf>) -> Self {
        let task = IoTaskPool::get().spawn(async move {
            let path = if path.is_some() {
                path
            } else {
                let mut dialog =
                    rfd::AsyncFileDialog::new().set_title("Abrir carpeta del mundo de Minecraft");
                if let Some(saves) = std::env::var_os("APPDATA")
                    .map(|path| PathBuf::from(path).join(".minecraft").join("saves"))
                    .filter(|path| path.is_dir())
                {
                    dialog = dialog.set_directory(saves);
                }
                dialog
                    .pick_folder()
                    .await
                    .map(|folder| folder.path().to_path_buf())
            };
            path.as_deref().map(JavaWorld::open).transpose()
        });
        Self { destination, task }
    }
}

#[derive(Resource)]
struct Launch {
    path: Option<PathBuf>,
    demo: bool,
}

#[derive(Default, Resource)]
pub struct DemoSelection(pub Option<String>);

#[derive(Resource)]
pub struct SceneAssets {
    terrain: Handle<StandardMaterial>,
    cube: Handle<Mesh>,
    pedestal: Handle<StandardMaterial>,
    teal: Handle<StandardMaterial>,
    amber: Handle<StandardMaterial>,
}

pub fn run(path: Option<PathBuf>, demo: bool) {
    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: None,
            exit_condition: ExitCondition::OnAllClosed,
            ..default()
        }))
        .add_plugins(EguiPlugin::default())
        .insert_resource(Launch { path, demo })
        .insert_resource(GlobalAmbientLight {
            brightness: 260.0,
            ..default()
        })
        .init_resource::<DemoSelection>()
        .add_systems(Startup, setup)
        .add_systems(
            Update,
            (flight::fly, finish_opening, stream_worlds, ui::draw).chain(),
        )
        .run();
}

fn setup(
    mut commands: Commands,
    mut launch: ResMut<Launch>,
    mut egui_settings: ResMut<EguiGlobalSettings>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    egui_settings.auto_create_primary_context = false;
    let assets = SceneAssets {
        terrain: materials.add(terrain::terrain_material(&mut images)),
        cube: meshes.add(Cuboid::new(1.0, 1.0, 1.0)),
        pedestal: materials.add(Color::srgb(0.13, 0.19, 0.21)),
        teal: materials.add(Color::srgb(0.24, 0.67, 0.53)),
        amber: materials.add(Color::srgb(0.87, 0.59, 0.30)),
    };
    let (window, mut view) = new_view(&mut commands, 1, None);
    if let Some(path) = launch.path.take() {
        view.pending = Some(PendingOpen::start(Destination::Current, Some(path)));
    }
    if launch.demo {
        view.demo_scene = Some(spawn_demo(&mut commands, window, 1, &assets));
    }
    commands.entity(window).insert(view);
    commands.insert_resource(assets);
}

fn new_view(commands: &mut Commands, layer: usize, world_name: Option<&str>) -> (Entity, View) {
    let window = commands
        .spawn((
            Window {
                title: world_name.map_or_else(|| "Blocko".into(), world_title),
                resolution: WindowResolution::new(1280, 800),
                ..default()
            },
            Transform::default(),
            Visibility::default(),
        ))
        .id();
    let target = RenderTarget::Window(WindowRef::Entity(window));
    let yaw = if layer % 2 == 1 { 0.62 } else { -0.62 };
    let flight = Flight::new(yaw, -0.34);
    let rotation = flight.rotation();
    let camera = commands
        .spawn((
            Camera3d::default(),
            Camera {
                clear_color: ClearColorConfig::Custom(if world_name.is_some() {
                    SKY
                } else {
                    STUDIO
                }),
                ..default()
            },
            target.clone(),
            Tonemapping::None,
            Transform::from_translation(rotation * Vec3::new(0.0, 0.0, 14.0))
                .with_rotation(rotation),
            flight,
            RenderLayers::none().with(layer),
            ChildOf(window),
        ))
        .id();
    commands.spawn((
        Camera2d,
        Camera {
            order: 1,
            clear_color: ClearColorConfig::None,
            ..default()
        },
        target,
        ui::context(),
        EguiContextSettings {
            run_manually: true,
            ..default()
        },
        Overlay(window),
        ChildOf(window),
    ));
    commands.spawn((
        DirectionalLight {
            illuminance: 12_000.0,
            ..default()
        },
        Transform::from_xyz(-3.0, 8.0, 4.0).looking_at(Vec3::ZERO, Vec3::Y),
        RenderLayers::none().with(layer),
        ChildOf(window),
    ));
    let view = View {
        camera,
        layer,
        stream: None,
        pending: None,
        error: None,
        viewport: None,
        demo_scene: None,
    };
    (window, view)
}

fn activate(commands: &mut Commands, window: Entity, view: &mut View, world: JavaWorld) {
    if let Some(previous) = view.stream.take() {
        commands.entity(previous.root).despawn();
    }
    if let Some(demo) = view.demo_scene.take() {
        commands.entity(demo).despawn();
    }
    let [x, y, z] = world.spawn;
    let flight = Flight::new(0.0, -0.42);
    let position = Vec3::new(x as f32 + 0.5, y as f32 + 9.0, z as f32 + 0.5);
    commands.entity(view.camera).insert((
        Transform::from_translation(position).with_rotation(flight.rotation()),
        flight,
        DistanceFog {
            color: SKY,
            falloff: FogFalloff::Linear {
                start: 58.0,
                end: 93.0,
            },
            ..default()
        },
    ));
    commands.entity(window).insert(CursorOptions::default());
    let root = commands
        .spawn((Transform::default(), Visibility::default(), ChildOf(window)))
        .id();
    view.stream = Some(Stream::new(world, root));
    view.error = None;
}

fn world_title(name: &str) -> String {
    format!("{name} · Blocko")
}

fn finish_opening(
    mut commands: Commands,
    mut windows: Query<(Entity, &mut Window, &mut View)>,
    mut cameras: Query<&mut Camera, With<Camera3d>>,
) {
    let mut occupied = windows
        .iter()
        .map(|(_, _, view)| view.layer)
        .collect::<HashSet<_>>();
    for (entity, mut window, mut view) in &mut windows {
        let Some(pending) = &mut view.pending else {
            continue;
        };
        let Some(result) = check_ready(&mut pending.task) else {
            continue;
        };
        let destination = pending.destination;
        view.pending = None;
        let world = match result {
            Ok(Some(world)) => world,
            Ok(None) => continue,
            Err(error) => {
                view.error = Some(format!("{error:#}"));
                continue;
            }
        };
        match destination {
            Destination::Current => {
                window.title = world_title(&world.name);
                activate(&mut commands, entity, &mut view, world);
                if let Ok(mut camera) = cameras.get_mut(view.camera) {
                    camera.clear_color = ClearColorConfig::Custom(SKY);
                }
            }
            Destination::NewWindow => {
                let layer = next_layer(&mut occupied);
                let (entity, mut new_view) = new_view(&mut commands, layer, Some(&world.name));
                activate(&mut commands, entity, &mut new_view, world);
                commands.entity(entity).insert(new_view);
            }
        }
    }
}

fn next_layer(occupied: &mut HashSet<usize>) -> usize {
    (1..)
        .find(|&layer| occupied.insert(layer))
        .expect("hay capas disponibles")
}

pub fn new_demo(commands: &mut Commands, views: impl Iterator<Item = usize>, assets: &SceneAssets) {
    let layer = next_layer(&mut views.collect());
    let (window, mut view) = new_view(commands, layer, None);
    view.demo_scene = Some(spawn_demo(commands, window, layer, assets));
    commands.entity(window).insert(view);
}

fn spawn_demo(
    commands: &mut Commands,
    window: Entity,
    layer: usize,
    assets: &SceneAssets,
) -> Entity {
    let root = commands
        .spawn((Transform::default(), Visibility::default(), ChildOf(window)))
        .id();
    let mut cuboid = |position, size, material: &Handle<StandardMaterial>| {
        commands.spawn((
            Mesh3d(assets.cube.clone()),
            MeshMaterial3d(material.clone()),
            Transform::from_translation(position).with_scale(size),
            RenderLayers::none().with(layer),
            ChildOf(root),
        ));
    };
    let (base, accent) = if layer % 2 == 1 {
        (&assets.teal, &assets.amber)
    } else {
        (&assets.amber, &assets.teal)
    };
    cuboid(
        Vec3::new(0.0, -1.8, 0.0),
        Vec3::new(8.0, 0.3, 8.0),
        &assets.pedestal,
    );
    cuboid(Vec3::new(0.0, -0.4, 0.0), Vec3::new(3.0, 2.5, 3.0), base);
    cuboid(Vec3::new(-0.65, 1.35, -0.6), Vec3::new(1.7, 1.0, 1.6), base);
    cuboid(Vec3::new(2.1, -0.9, 0.9), Vec3::splat(1.5), accent);
    root
}

fn stream_worlds(
    mut commands: Commands,
    mut views: Query<&mut View>,
    cameras: Query<&Transform, With<Flight>>,
    assets: Res<SceneAssets>,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    for mut view in &mut views {
        let layer = view.layer;
        let Ok(camera) = cameras.get(view.camera) else {
            continue;
        };
        if let Some(stream) = &mut view.stream {
            stream.update(
                &mut commands,
                &mut meshes,
                &assets.terrain,
                layer,
                camera.translation,
            );
        }
    }
}
