use std::{collections::HashSet, fs, path::PathBuf};

use bevy::{
    camera::{RenderTarget, visibility::RenderLayers},
    core_pipeline::tonemapping::Tonemapping,
    light::GlobalAmbientLight,
    prelude::*,
    tasks::{IoTaskPool, Task, futures::check_ready},
    window::{
        CursorOptions, ExitCondition, WindowCloseRequested, WindowPosition, WindowRef,
        WindowResizeConstraints, WindowResolution,
    },
};
use bevy_egui::{EguiContextSettings, EguiGlobalSettings, EguiPlugin};

use crate::{
    actions::Bindings,
    diagnostics::Diagnostic,
    flight::{self, Flight},
    i18n::{Language, Text},
    java::JavaWorld,
    paste::{self, PasteState},
    placement::{AirMode, Placement},
    preview_visual,
    selection::{self, Selection},
    streaming::Stream,
    structure::{self, Clipboard},
    terrain, ui,
};

const STUDIO: Color = Color::srgb(0.075, 0.102, 0.116);

#[derive(Component)]
pub struct View {
    pub owner: Entity,
    pub camera: Entity,
    pub layer: usize,
    pub stream: Option<Stream>,
    pub viewport: Option<Rect>,
    pub demo_scene: Option<Entity>,
    pub selection: Selection,
    pub copying: bool,
    pub copy_notice: Option<structure::CopyNotice>,
    pub selecting: bool,
    pub air_mode: AirMode,
    pub preview_cancelled: bool,
    pub preview: Option<Placement>,
    pub paste: PasteState,
}

#[derive(Component)]
pub struct Overlay(pub Entity);

#[derive(Component)]
pub struct Workspace {
    pub tabs: Vec<Entity>,
    pub active: Entity,
    pub pending: Option<PendingOpen>,
    pub notice: Option<OpenNotice>,
    pub dragging: Option<Entity>,
    pub blocks_camera: bool,
}

impl Workspace {
    pub(crate) fn new(active: Entity) -> Self {
        Self {
            tabs: vec![active],
            active,
            pending: None,
            notice: None,
            dragging: None,
            blocks_camera: false,
        }
    }
}

#[derive(Clone, Copy)]
pub enum Destination {
    Tab,
    Replace,
    NewWindow,
}

#[derive(Message)]
pub struct TabRequest {
    pub window: Entity,
    pub tab: Entity,
    pub action: TabAction,
}

pub enum TabAction {
    Activate,
    Close,
    Detach(Option<IVec2>),
}

pub enum OpenNotice {
    Opened(String),
    Cancelled,
    Duplicate(String),
    Error(Option<PathBuf>, Diagnostic),
}

pub struct PendingOpen {
    destination: Destination,
    stage: OpenStage,
}

enum OpenStage {
    Locate(Task<anyhow::Result<Option<PathBuf>>>),
    Load(PathBuf, Task<anyhow::Result<JavaWorld>>),
}

impl PendingOpen {
    pub fn browse(
        destination: Destination,
        language: Language,
        parent: Option<ui::DialogParent>,
    ) -> Self {
        Self::start(destination, None, language, parent)
    }

    fn start(
        destination: Destination,
        path: Option<PathBuf>,
        language: Language,
        parent: Option<ui::DialogParent>,
    ) -> Self {
        let task = IoTaskPool::get().spawn(async move {
            let path = if let Some(path) = path {
                Some(path)
            } else {
                let mut dialog =
                    rfd::AsyncFileDialog::new().set_title(language.text(Text::DialogOpen));
                if let Some(parent) = &parent {
                    dialog = dialog.set_parent(parent);
                }
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
            path.map(|path| {
                use anyhow::Context;
                fs::canonicalize(&path)
                    .with_context(|| format!("No se pudo abrir {}", path.display()))
            })
            .transpose()
        });
        Self {
            destination,
            stage: OpenStage::Locate(task),
        }
    }

    pub fn path(&self) -> Option<&std::path::Path> {
        match &self.stage {
            OpenStage::Locate(_) => None,
            OpenStage::Load(path, _) => Some(path),
        }
    }
}

#[derive(Resource)]
struct Launch {
    path: Option<PathBuf>,
    demo: bool,
}

#[derive(Default, Resource)]
pub struct DemoSelection(pub Option<usize>);

#[derive(Resource)]
pub struct SceneAssets {
    pub terrain: Handle<StandardMaterial>,
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
            close_when_requested: false,
            ..default()
        }))
        .add_plugins(EguiPlugin::default())
        .insert_resource(Launch { path, demo })
        .insert_resource(GlobalAmbientLight {
            brightness: 260.0,
            ..default()
        })
        .init_resource::<DemoSelection>()
        .init_resource::<Bindings>()
        .init_resource::<Language>()
        .add_message::<TabRequest>()
        .init_resource::<Clipboard>()
        .add_systems(Startup, (setup, preview_visual::setup).chain())
        .add_systems(Startup, selection::setup)
        .add_systems(
            Update,
            (
                paste::finish,
                close_windows,
                finish_opening,
                manage_tabs,
                flight::fly,
                structure::finish_copying,
                ui::draw,
                stream_worlds,
                selection::draw,
                preview_visual::draw,
            )
                .chain(),
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
    let window = new_window(&mut commands, None);
    let document = new_view(&mut commands, window, 1);
    let mut workspace = Workspace::new(document);
    if let Some(path) = launch.path.take() {
        workspace.pending = Some(PendingOpen::start(
            Destination::Tab,
            Some(path),
            Language::default(),
            None,
        ));
    }
    if launch.demo {
        let demo = spawn_demo(&mut commands, document, 1, &assets);
        commands.queue(move |world: &mut World| {
            world.get_mut::<View>(document).unwrap().demo_scene = Some(demo);
        });
    }
    commands.entity(window).insert(workspace);
    commands.insert_resource(assets);
}

fn new_window(commands: &mut Commands, position: Option<IVec2>) -> Entity {
    let window = commands
        .spawn((
            Window {
                title: "Blocko".into(),
                resolution: WindowResolution::new(1280, 800),
                position: position.map_or(WindowPosition::Automatic, WindowPosition::At),
                resize_constraints: WindowResizeConstraints {
                    min_width: 520.0,
                    min_height: 400.0,
                    ..default()
                },
                ..default()
            },
            Transform::default(),
            Visibility::default(),
        ))
        .id();
    commands.spawn((
        Camera2d,
        Camera {
            order: 1,
            clear_color: ClearColorConfig::None,
            ..default()
        },
        window_target(window),
        ui::context(),
        EguiContextSettings {
            run_manually: true,
            ..default()
        },
        Overlay(window),
        ChildOf(window),
    ));
    window
}

fn window_target(window: Entity) -> RenderTarget {
    RenderTarget::Window(WindowRef::Entity(window))
}

fn new_view(commands: &mut Commands, owner: Entity, layer: usize) -> Entity {
    let document = commands
        .spawn((Transform::default(), Visibility::default(), ChildOf(owner)))
        .id();
    let yaw = if layer % 2 == 1 { 0.62 } else { -0.62 };
    let flight = Flight::new(yaw, -0.34);
    let rotation = flight.rotation();
    let camera = commands
        .spawn((
            Camera3d::default(),
            Camera {
                clear_color: ClearColorConfig::Custom(STUDIO),
                is_active: false,
                ..default()
            },
            window_target(owner),
            Tonemapping::None,
            Transform::from_translation(rotation * Vec3::new(0.0, 0.0, 14.0))
                .with_rotation(rotation),
            flight,
            RenderLayers::none().with(layer),
            ChildOf(document),
        ))
        .id();
    let (sun, pose) = terrain::overworld_sun();
    commands.spawn((
        sun,
        pose,
        RenderLayers::none().with(layer),
        ChildOf(document),
    ));
    commands.entity(document).insert(View {
        owner,
        camera,
        layer,
        stream: None,
        viewport: None,
        demo_scene: None,
        selection: Selection::default(),
        copying: false,
        copy_notice: None,
        selecting: false,
        air_mode: AirMode::default(),
        preview_cancelled: false,
        preview: None,
        paste: PasteState::Idle,
    });
    document
}

fn add_world(commands: &mut Commands, owner: Entity, layer: usize, world: JavaWorld) -> Entity {
    let document = new_view(commands, owner, layer);
    let root = commands
        .spawn((
            Transform::default(),
            Visibility::default(),
            ChildOf(document),
        ))
        .id();
    let [x, y, z] = world.spawn;
    let flight = Flight::new(0.0, -0.42);
    let position = Vec3::new(x as f32 + 0.5, y as f32 + 9.0, z as f32 + 0.5);
    commands.queue(move |ecs: &mut World| {
        let mut view = ecs.get_mut::<View>(document).unwrap();
        view.stream = Some(Stream::new(world, root));
        let camera = view.camera;
        ecs.entity_mut(camera).insert((
            Transform::from_translation(position).with_rotation(flight.rotation()),
            flight,
            terrain::overworld_fog(),
            terrain::overworld_ambient(),
        ));
        ecs.get_mut::<Camera>(camera).unwrap().clear_color =
            ClearColorConfig::Custom(terrain::OVERWORLD_SKY);
    });
    document
}

fn close_windows(
    mut commands: Commands,
    mut requests: MessageReader<WindowCloseRequested>,
    windows: Query<&Workspace>,
    views: Query<&View>,
) {
    for request in requests.read() {
        if !windows.get(request.window).is_ok_and(|workspace| {
            workspace.pending.is_some()
                || workspace
                    .tabs
                    .iter()
                    .any(|tab| views.get(*tab).is_ok_and(|view| view.paste.writes()))
        }) {
            commands.entity(request.window).try_despawn();
        }
    }
}

fn world_title(name: &str) -> String {
    format!("{name} · Blocko")
}

fn finish_opening(
    mut commands: Commands,
    mut windows: Query<(Entity, &mut Window, &mut Workspace)>,
    views: Query<&View>,
    mut cameras: Query<(&mut Camera, &mut RenderTarget), With<Camera3d>>,
) {
    let mut occupied = views.iter().map(|view| view.layer).collect::<HashSet<_>>();
    let mut reserved = windows
        .iter()
        .filter_map(|(_, _, workspace)| workspace.pending.as_ref()?.path().map(ToOwned::to_owned))
        .collect::<HashSet<_>>();
    for (entity, mut window, mut workspace) in &mut windows {
        let Some(pending) = &mut workspace.pending else {
            continue;
        };
        let destination = pending.destination;
        let world = match &mut pending.stage {
            OpenStage::Locate(task) => {
                let Some(result) = check_ready(task) else {
                    continue;
                };
                match result {
                    Ok(Some(path)) => {
                        if let Some(view) = views.iter().find(|view| {
                            view.stream
                                .as_ref()
                                .is_some_and(|stream| stream.path() == path)
                        }) {
                            workspace.notice = Some(OpenNotice::Duplicate(
                                view.stream.as_ref().unwrap().name.clone(),
                            ));
                        } else if !reserved.insert(path.clone()) {
                            workspace.notice =
                                Some(OpenNotice::Duplicate(path.display().to_string()));
                        } else {
                            let load_path = path.clone();
                            pending.stage = OpenStage::Load(
                                path,
                                IoTaskPool::get().spawn(async move { JavaWorld::open(&load_path) }),
                            );
                            continue;
                        }
                    }
                    Ok(None) => workspace.notice = Some(OpenNotice::Cancelled),
                    Err(error) => {
                        workspace.notice =
                            Some(OpenNotice::Error(None, Diagnostic::from_error(error)))
                    }
                }
                workspace.pending = None;
                continue;
            }
            OpenStage::Load(path, task) => {
                let Some(result) = check_ready(task) else {
                    continue;
                };
                match result {
                    Ok(world) => world,
                    Err(error) => {
                        workspace.notice = Some(OpenNotice::Error(
                            Some(path.clone()),
                            Diagnostic::from_error(error),
                        ));
                        workspace.pending = None;
                        continue;
                    }
                }
            }
        };
        workspace.pending = None;
        let name = world.name.clone();
        let layer = next_layer(&mut occupied);
        if matches!(destination, Destination::NewWindow) {
            let owner = new_window(&mut commands, None);
            let document = add_world(&mut commands, owner, layer, world);
            let mut new_workspace = Workspace::new(document);
            new_workspace.notice = Some(OpenNotice::Opened(name.clone()));
            commands.entity(owner).insert(new_workspace);
            commands.queue(move |ecs: &mut World| {
                ecs.get_mut::<Window>(owner).unwrap().title = world_title(&name)
            });
            continue;
        }
        let active = workspace.active;
        let replace = matches!(destination, Destination::Replace)
            || views.get(active).is_ok_and(|view| view.stream.is_none());
        let document = add_world(&mut commands, entity, layer, world);
        deactivate(active, &views, &mut cameras);
        if replace {
            if let Some(tab) = workspace.tabs.iter_mut().find(|tab| **tab == active) {
                *tab = document;
            }
            commands.entity(active).try_despawn();
        } else {
            workspace.tabs.push(document);
        }
        workspace.active = document;
        workspace.notice = Some(OpenNotice::Opened(name.clone()));
        window.title = world_title(&name);
    }
}

fn deactivate(
    document: Entity,
    views: &Query<&View>,
    cameras: &mut Query<(&mut Camera, &mut RenderTarget), With<Camera3d>>,
) {
    if let Ok(view) = views.get(document)
        && let Ok((mut camera, _)) = cameras.get_mut(view.camera)
    {
        camera.is_active = false;
    }
}

fn manage_tabs(
    mut commands: Commands,
    mut requests: MessageReader<TabRequest>,
    mut windows: Query<(&mut Window, &mut CursorOptions, &mut Workspace)>,
    mut views: ParamSet<(Query<&View>, Query<&mut View>)>,
    mut cameras: Query<(&mut Camera, &mut RenderTarget), With<Camera3d>>,
    mut flights: Query<&mut Flight>,
) {
    let mut occupied = views
        .p0()
        .iter()
        .map(|view| view.layer)
        .collect::<HashSet<_>>();
    for request in requests.read() {
        let Ok((mut window, mut cursor, mut workspace)) = windows.get_mut(request.window) else {
            continue;
        };
        let old = workspace.active;
        let Some(index) = workspace.tabs.iter().position(|&tab| tab == request.tab) else {
            continue;
        };
        let read = views.p0();
        if workspace.pending.is_some()
            || read.get(old).is_ok_and(|view| view.paste.writes())
            || read.get(request.tab).is_ok_and(|view| view.paste.writes())
        {
            continue;
        }
        if matches!(request.action, TabAction::Activate) && request.tab == old {
            continue;
        }
        if request.tab == old || matches!(request.action, TabAction::Activate) {
            deactivate(old, &read, &mut cameras);
            if let Ok(view) = read.get(old)
                && let Ok(mut flight) = flights.get_mut(view.camera)
            {
                flight.release(&mut cursor);
            }
        }
        match request.action {
            TabAction::Activate => workspace.active = request.tab,
            TabAction::Close => {
                workspace.tabs.remove(index);
                commands.entity(request.tab).try_despawn();
            }
            TabAction::Detach(position) => {
                workspace.tabs.remove(index);
                let owner = new_window(&mut commands, position);
                commands.entity(request.tab).insert(ChildOf(owner));
                commands.entity(owner).insert(Workspace::new(request.tab));
                if let Ok(mut view) = views.p1().get_mut(request.tab) {
                    view.owner = owner;
                    view.viewport = None;
                    if let Ok((mut camera, mut target)) = cameras.get_mut(view.camera) {
                        camera.is_active = false;
                        camera.viewport = None;
                        *target = window_target(owner);
                    }
                    if let Some(stream) = &view.stream {
                        let title = world_title(&stream.name);
                        commands.queue(move |ecs: &mut World| {
                            ecs.get_mut::<Window>(owner).unwrap().title = title
                        });
                    }
                }
            }
        }
        if workspace.tabs.is_empty() {
            let blank = new_view(&mut commands, request.window, next_layer(&mut occupied));
            workspace.tabs.push(blank);
        }
        if !workspace.tabs.contains(&workspace.active) {
            workspace.active = workspace.tabs[index.min(workspace.tabs.len() - 1)];
        }
        workspace.dragging = None;
        window.title = views
            .p0()
            .get(workspace.active)
            .ok()
            .and_then(|view| view.stream.as_ref())
            .map_or_else(|| "Blocko".into(), |stream| world_title(&stream.name));
    }
}

fn next_layer(occupied: &mut HashSet<usize>) -> usize {
    (1..)
        .find(|&layer| occupied.insert(layer))
        .expect("hay capas disponibles")
}

pub fn new_demo(commands: &mut Commands, views: impl Iterator<Item = usize>, assets: &SceneAssets) {
    let layer = next_layer(&mut views.collect());
    let window = new_window(commands, None);
    let document = new_view(commands, window, layer);
    let demo = spawn_demo(commands, document, layer, assets);
    commands.entity(window).insert(Workspace::new(document));
    commands.queue(move |ecs: &mut World| {
        ecs.get_mut::<View>(document).unwrap().demo_scene = Some(demo)
    });
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
    windows: Query<&Workspace>,
    mut views: Query<&mut View>,
    cameras: Query<&Transform, With<Flight>>,
    assets: Res<SceneAssets>,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    for workspace in &windows {
        let Ok(mut view) = views.get_mut(workspace.active) else {
            continue;
        };
        if view.paste.is_saving() {
            continue;
        }
        let layer = view.layer;
        let Ok(camera) = cameras.get(view.camera) else {
            continue;
        };
        let view = &mut *view;
        if let Some(stream) = &mut view.stream {
            stream.update(
                &mut commands,
                &mut meshes,
                &assets.terrain,
                layer,
                camera.translation,
                view.preview.as_ref().filter(|preview| preview.target_valid),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        thread,
        time::{Duration, Instant},
    };

    use anyhow::{Context, Result, ensure};
    use bevy::ecs::system::RunSystemOnce;

    use super::*;

    fn settle(app: &mut App, mut ready: impl FnMut(&mut World) -> bool) -> Result<()> {
        let until = Instant::now() + Duration::from_secs(10);
        loop {
            app.update();
            if ready(app.world_mut()) {
                return Ok(());
            }
            ensure!(Instant::now() < until, "La operación no terminó");
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn select_and_copy(app: &mut App, document: Entity) -> Result<()> {
        app.world_mut()
            .resource_scope(|world, mut clipboard: Mut<Clipboard>| -> Result<()> {
                let mut view = world
                    .get_mut::<View>(document)
                    .context("Missing document")?;
                let stream = view.stream.as_ref().context("Missing world")?;
                let volume = view.selection.volume().context("Missing selection")?;
                clipboard.request(document, stream.source(), volume)?;
                view.copying = true;
                Ok(())
            })?;
        settle(app, |world| !world.resource::<Clipboard>().copying())?;
        let view = app.world().get::<View>(document).unwrap();
        ensure!(
            matches!(view.copy_notice, Some(structure::CopyNotice::Copied(_))),
            "Copy failed"
        );
        Ok(())
    }

    fn test_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_message::<TabRequest>()
            .init_resource::<Clipboard>()
            .add_message::<WindowCloseRequested>()
            .add_systems(
                Update,
                (
                    paste::finish,
                    close_windows,
                    finish_opening,
                    structure::finish_copying,
                    manage_tabs,
                )
                    .chain(),
            );
        app
    }

    fn workspace(app: &mut App) -> Entity {
        app.world_mut()
            .run_system_once(|mut commands: Commands| {
                let window = new_window(&mut commands, None);
                let blank = new_view(&mut commands, window, 1);
                commands.entity(window).insert(Workspace::new(blank));
                window
            })
            .unwrap()
    }

    fn tab(app: &mut App, window: Entity, document: Entity, action: TabAction) {
        app.world_mut().write_message(TabRequest {
            window,
            tab: document,
            action,
        });
        app.update();
    }

    fn open(
        app: &mut App,
        window: Entity,
        path: PathBuf,
        destination: Destination,
    ) -> Result<Entity> {
        app.world_mut()
            .get_mut::<Workspace>(window)
            .unwrap()
            .pending = Some(PendingOpen::start(
            destination,
            Some(path),
            Language::English,
            None,
        ));
        settle(app, |world| {
            world.get::<Workspace>(window).unwrap().pending.is_none()
        })?;
        let workspace = app.world().get::<Workspace>(window).unwrap();
        if let Some(OpenNotice::Error(_, error)) = &workspace.notice {
            anyhow::bail!("{}", error.detail());
        }
        Ok(workspace.active)
    }

    #[test]
    fn moving_a_tab_retargets_its_camera_without_rebuilding_document_state() {
        let mut app = test_app();
        let window = workspace(&mut app);
        let document = app.world().get::<Workspace>(window).unwrap().active;
        let camera = app.world().get::<View>(document).unwrap().camera;
        let position = Vec3::new(22.0, 83.0, -52.0);
        app.world_mut()
            .get_mut::<Transform>(camera)
            .unwrap()
            .translation = position;
        let corners = [Some([2, 3, 4]), Some([5, 6, 7])];
        app.world_mut()
            .get_mut::<View>(document)
            .unwrap()
            .selection
            .corners = corners;

        tab(&mut app, window, document, TabAction::Detach(None));
        let view = app.world().get::<View>(document).unwrap();
        assert_ne!(view.owner, window);
        let detached = view.owner;
        assert_eq!(
            app.world().get::<Workspace>(detached).unwrap().active,
            document
        );
        assert_ne!(
            app.world().get::<Workspace>(window).unwrap().active,
            document
        );
        assert!(matches!(
            app.world().get::<RenderTarget>(camera),
            Some(RenderTarget::Window(WindowRef::Entity(entity))) if *entity == detached
        ));
        assert_eq!(view.selection.corners, corners);
        assert_eq!(
            app.world().get::<Transform>(camera).unwrap().translation,
            position
        );
        app.world_mut().entity_mut(window).despawn();
        assert!(app.world().get::<View>(document).is_some());
    }

    #[test]
    #[ignore = "requires BLOCKO_TEST_WORLDS with copies of Bosque and Pradera in Java 1.21.1"]
    fn keeps_real_clipboard_and_world_state_across_tabs_and_detachment() -> Result<()> {
        let root =
            PathBuf::from(std::env::var_os("BLOCKO_TEST_WORLDS").context("Missing test worlds")?);
        let bosque_path = root.join("Bosque");
        let pradera_path = root.join("Pradera");
        let mut app = test_app();
        let window = workspace(&mut app);
        let bosque = open(&mut app, window, bosque_path.clone(), Destination::Tab)?;
        let selection = [Some([14, 66, -50]), Some([17, 73, -47])];
        app.world_mut()
            .get_mut::<View>(bosque)
            .unwrap()
            .selection
            .corners = selection;
        select_and_copy(&mut app, bosque)?;
        let copy = app
            .world()
            .resource::<Clipboard>()
            .current
            .as_ref()
            .unwrap();
        assert_eq!(copy.source_name, "Bosque");
        assert_eq!(copy.block_count(), 128);
        assert!(copy.block_at([0, 7, 0]).unwrap().is_air());

        let chest = [Some([86, -23, 119]); 2];
        app.world_mut()
            .get_mut::<View>(bosque)
            .unwrap()
            .selection
            .corners = chest;
        select_and_copy(&mut app, bosque)?;
        let copy = app
            .world()
            .resource::<Clipboard>()
            .current
            .as_ref()
            .unwrap();
        assert_eq!(copy.block_at([0; 3]).unwrap().properties["facing"], "east");
        assert!(matches!(
            copy.block_entity_at([0; 3]),
            Some(crate::world::BlockEntity::Chest { items, .. })
                if items.iter().any(|item| item.slot == 16 && item.id == "minecraft:diamond" && item.count == 1)
        ));

        let pradera = open(&mut app, window, pradera_path, Destination::Tab)?;
        assert_eq!(
            app.world().get::<Workspace>(window).unwrap().tabs,
            [bosque, pradera]
        );
        let camera = app.world().get::<View>(pradera).unwrap().camera;
        let position = Vec3::new(22.0, 83.0, -52.0);
        app.world_mut()
            .get_mut::<Transform>(camera)
            .unwrap()
            .translation = position;
        app.world_mut()
            .get_mut::<View>(pradera)
            .unwrap()
            .selection
            .corners = selection;
        tab(&mut app, window, bosque, TabAction::Activate);
        assert_eq!(
            app.world().get::<View>(bosque).unwrap().selection.corners,
            chest
        );
        let active = open(
            &mut app,
            window,
            bosque_path.clone(),
            Destination::NewWindow,
        )?;
        assert_eq!(active, bosque);
        assert!(matches!(
            app.world().get::<Workspace>(window).unwrap().notice,
            Some(OpenNotice::Duplicate(_))
        ));
        tab(&mut app, window, pradera, TabAction::Detach(None));
        let detached = app.world().get::<View>(pradera).unwrap().owner;
        assert_ne!(detached, window);
        assert_eq!(app.world().get::<Workspace>(window).unwrap().tabs, [bosque]);
        assert_eq!(
            app.world().get::<Workspace>(detached).unwrap().active,
            pradera
        );
        assert_eq!(
            app.world().get::<View>(pradera).unwrap().selection.corners,
            selection
        );
        assert_eq!(
            app.world().get::<Transform>(camera).unwrap().translation,
            position
        );
        tab(&mut app, window, bosque, TabAction::Close);
        app.world_mut().entity_mut(window).despawn();
        let copy = app
            .world()
            .resource::<Clipboard>()
            .current
            .as_ref()
            .unwrap();
        assert_eq!(copy.source_name, "Bosque");
        assert_eq!(copy.block_entity_count(), 1);
        select_and_copy(&mut app, pradera)?;
        assert_eq!(
            app.world()
                .resource::<Clipboard>()
                .current
                .as_ref()
                .unwrap()
                .source_name,
            "Pradera"
        );
        Ok(())
    }

    #[test]
    #[ignore = "requires BLOCKO_TEST_WORLDS with copies of Bosque and Pradera in Java 1.21.1"]
    fn cancels_without_writing_and_commits_each_repeated_placement_to_its_document() -> Result<()> {
        use crate::paste::PasteAction;
        use crate::world::{BlockEntity, BlockEntityRecord, ChunkPos};

        let fixtures =
            PathBuf::from(std::env::var_os("BLOCKO_TEST_WORLDS").context("Missing test worlds")?);
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let scratch =
            std::env::temp_dir().join(format!("blocko-v020-paste-{}-{unique}", std::process::id()));
        fs::create_dir(&scratch)?;
        let original = fixtures.join("Pradera");
        for file in ["level.dat", "session.lock"] {
            fs::copy(original.join(file), scratch.join(file))?;
        }
        for folder in ["region", "poi"] {
            let source = original.join(folder);
            if source.is_dir() {
                fs::create_dir(scratch.join(folder))?;
                for entry in fs::read_dir(source)? {
                    let entry = entry?;
                    if entry.file_type()?.is_file() {
                        fs::copy(entry.path(), scratch.join(folder).join(entry.file_name()))?;
                    }
                }
            }
        }

        let inventory = |root: &std::path::Path| -> Result<_> {
            fs::read_dir(root.join("region"))?
                .map(|entry| {
                    let entry = entry?;
                    Ok((
                        entry.file_name(),
                        crate::transaction::hash_file(&entry.path())?,
                    ))
                })
                .collect::<Result<std::collections::BTreeMap<_, _>>>()
        };
        let before = inventory(&scratch)?;
        let mut app = test_app();
        let window = workspace(&mut app);
        let bosque_path = fixtures.join("Bosque");
        let source_before = inventory(&bosque_path)?;
        let bosque = open(&mut app, window, bosque_path.clone(), Destination::Tab)?;
        app.world_mut()
            .get_mut::<View>(bosque)
            .unwrap()
            .selection
            .corners = [Some([86, -23, 119]); 2];
        select_and_copy(&mut app, bosque)?;
        let pradera = open(&mut app, window, scratch.clone(), Destination::Tab)?;
        let clipboard = app.world().resource::<Clipboard>().current.clone().unwrap();
        let preview = Placement::new(clipboard.clone(), [4, 90, -46]);
        let cancel = |app: &mut App| {
            app.world_mut()
                .resource_scope(|world, mut clipboard: Mut<Clipboard>| {
                    crate::ui::execute_action(
                        &mut world.get_mut::<View>(pradera).unwrap(),
                        pradera,
                        &mut clipboard,
                        crate::actions::Action::CancelPaste,
                        None,
                    );
                });
        };
        for prepared in [false, true] {
            let mut view = app.world_mut().get_mut::<View>(pradera).unwrap();
            view.preview = Some(preview.clone());
            if prepared {
                paste::apply_action(&mut view, PasteAction::Prepare);
                settle(&mut app, |world| {
                    !matches!(
                        world.get::<View>(pradera).unwrap().paste,
                        PasteState::Preparing(..)
                    )
                })?;
                assert!(matches!(
                    app.world().get::<View>(pradera).unwrap().paste,
                    PasteState::Ready(_)
                ));
            }
            cancel(&mut app);
            let view = app.world().get::<View>(pradera).unwrap();
            assert!(view.preview.is_none());
            assert!(matches!(view.paste, PasteState::Idle));
            assert_eq!(inventory(&scratch)?, before);
            assert!(!scratch.join("blocko-backups").exists());
            assert!(std::sync::Arc::ptr_eq(
                app.world()
                    .resource::<Clipboard>()
                    .current
                    .as_ref()
                    .unwrap(),
                &clipboard,
            ));
        }

        tab(&mut app, window, pradera, TabAction::Detach(None));
        let detached = app.world().get::<View>(pradera).unwrap().owner;
        app.world_mut().get_mut::<View>(pradera).unwrap().preview = Some(preview);
        let origins = [[4, 90, -46], [8, 90, -46], [12, 90, -46], [15, 90, -46]];
        let mut backups = HashSet::new();
        for (index, origin) in origins.into_iter().enumerate() {
            let repeat = index != origins.len() - 1;
            {
                let mut view = app.world_mut().get_mut::<View>(pradera).unwrap();
                view.preview.as_mut().unwrap().set_origin(origin);
                paste::apply_action(&mut view, PasteAction::Place { repeat });
                paste::apply_action(&mut view, PasteAction::Place { repeat: false });
            }
            cancel(&mut app);
            assert!(matches!(
                &app.world().get::<View>(pradera).unwrap().paste,
                PasteState::Preparing(Some(value), _) if *value == repeat
            ));
            app.world_mut()
                .write_message(WindowCloseRequested { window: detached });
            settle(&mut app, |world| {
                world
                    .get::<View>(pradera)
                    .is_some_and(|view| !view.paste.writes())
            })?;
            let view = app.world().get::<View>(pradera).unwrap();
            let PasteState::Saved(result) = &view.paste else {
                if let PasteState::Error(error) = &view.paste {
                    anyhow::bail!("{}", error.detail());
                }
                anyhow::bail!("Placement did not save");
            };
            assert_eq!(result.summary.volume.min, origin);
            assert!(backups.insert(result.backup.clone()));
            assert_eq!(view.preview.is_some(), repeat);
        }
        assert_eq!(
            fs::read_dir(scratch.join("blocko-backups"))?.count(),
            origins.len()
        );
        assert_eq!(inventory(&bosque_path)?, source_before);
        assert_eq!(inventory(&original)?, before);
        for (region, hash) in inventory(&scratch)? {
            if region != "r.0.-1.mca" {
                assert_eq!(before.get(&region), Some(&hash));
            }
        }

        let world = app
            .world()
            .get::<View>(pradera)
            .unwrap()
            .stream
            .as_ref()
            .unwrap()
            .source();
        for [x, y, z] in origins {
            let chunk = world
                .load_chunk_for_copy(ChunkPos::from_block(x, z))?
                .unwrap();
            let block = chunk.block(x.rem_euclid(16), y, z.rem_euclid(16)).unwrap();
            assert_eq!(block.properties["facing"], "east");
            assert!(matches!(
                chunk.block_entities.get(&[x, y, z]),
                Some(BlockEntityRecord::Supported(BlockEntity::Chest { items, .. }))
                    if items.iter().any(|item| item.slot == 16 && item.id == "minecraft:diamond" && item.count == 1)
            ));
        }
        Ok(())
    }
}
