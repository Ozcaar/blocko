use std::collections::HashMap;

use bevy::{
    camera::visibility::RenderLayers,
    input::{ButtonState, keyboard::KeyboardInput, mouse::MouseButtonInput},
    prelude::*,
    window::{CursorGrabMode, CursorOptions, WindowFocused},
};

use crate::{
    editor::View, placement::Placement, streaming::Stream, structure::Volume, world::ChunkPos,
};

const PICK_DISTANCE: f64 = 80.0;

#[derive(Default)]
pub struct Selection {
    pub corners: [Option<[i32; 3]>; 2],
    pub picking: Option<usize>,
    pub pick_b_after_a: bool,
    pub notice: Option<String>,
}

impl Selection {
    pub fn volume(&self) -> Option<Volume> {
        let [a, b] = self.corners;
        Some(Volume::between(a?, b?))
    }
}

pub fn pick(
    mut windows: Query<(&Window, &CursorOptions, &mut View)>,
    cameras: Query<(&Camera, &Transform)>,
    mut mouse: MessageReader<MouseButtonInput>,
    mut keyboard: MessageReader<KeyboardInput>,
    mut focus: MessageReader<WindowFocused>,
) {
    let blurred: Vec<_> = focus
        .read()
        .filter(|event| !event.focused)
        .map(|event| event.window)
        .collect();
    for key in keyboard.read() {
        if key.key_code == KeyCode::Escape
            && key.state == ButtonState::Pressed
            && !blurred.contains(&key.window)
            && let Ok((window, _, mut view)) = windows.get_mut(key.window)
            && window.focused
        {
            if view.selection.picking.is_some() {
                view.selection.picking = None;
                view.selection.pick_b_after_a = false;
                view.selection.notice = None;
            }
            if let Some(preview) = &mut view.preview
                && preview.moving
            {
                preview.moving = false;
                preview.notice = None;
            }
        }
    }

    for click in mouse.read() {
        if click.button != MouseButton::Left
            || click.state != ButtonState::Pressed
            || blurred.contains(&click.window)
        {
            continue;
        }
        let Ok((window, cursor, mut view)) = windows.get_mut(click.window) else {
            continue;
        };
        if !window.focused || cursor.grab_mode != CursorGrabMode::None || view.pending.is_some() {
            continue;
        }
        let moving = view.preview.as_ref().is_some_and(|preview| preview.moving);
        let corner = view
            .selection
            .picking
            .filter(|&corner| corner < view.selection.corners.len());
        if !moving && corner.is_none() {
            continue;
        }
        let (Some(stream), Some(viewport), Some(position)) = (
            view.stream.as_ref(),
            view.viewport,
            window.cursor_position(),
        ) else {
            continue;
        };
        if !viewport.contains(position) {
            continue;
        }
        let Ok((camera, transform)) = cameras.get(view.camera) else {
            continue;
        };
        if !camera.is_active {
            continue;
        }
        // The window parent has the identity transform. Use the current pose before propagation.
        let hit = camera
            .viewport_to_world(&GlobalTransform::from(*transform), position)
            .ok()
            .and_then(|ray| {
                trace_terrain(
                    stream,
                    ray.origin,
                    *ray.direction,
                    PICK_DISTANCE,
                    view.preview.as_ref().filter(|preview| preview.moving),
                )
            });
        let notice = "Acerca la cámara y haz clic en un bloque visible.";
        if let Some(preview) = view.preview.as_mut().filter(|preview| preview.moving) {
            if let Some(hit) = hit {
                preview.set_origin(hit.adjacent());
                preview.moving = false;
                preview.notice = None;
            } else {
                preview.notice = Some(notice.into());
            }
            continue;
        }
        let Some(corner) = corner else {
            continue;
        };
        let Some(hit) = hit else {
            view.selection.notice = Some(notice.into());
            continue;
        };
        let selection = &mut view.selection;
        selection.corners[corner] = Some(hit.position);
        selection.picking = (corner == 0 && selection.pick_b_after_a).then_some(1);
        selection.pick_b_after_a = false;
        selection.notice = None;
    }
}

pub fn preview_origin(stream: &Stream, transform: &Transform) -> [i32; 3] {
    let direction = *transform.forward();
    trace_terrain(stream, transform.translation, direction, 48.0, None).map_or_else(
        || {
            (transform.translation + direction * 10.0)
                .floor()
                .as_ivec3()
                .to_array()
        },
        Hit::adjacent,
    )
}

fn trace_terrain(
    stream: &Stream,
    origin: Vec3,
    direction: Vec3,
    reach: f64,
    masked: Option<&Placement>,
) -> Option<Hit> {
    first_block(origin, direction, reach, |[x, y, z]| {
        !masked.is_some_and(|preview| preview.writes_block([x, y, z]).is_some())
            && stream
                .loaded_chunk(ChunkPos::from_block(x, z))
                .and_then(|chunk| chunk.block(x.rem_euclid(16), y, z.rem_euclid(16)))
                .is_some_and(|block| !block.is_air())
    })
}

#[derive(Debug, Eq, PartialEq)]
struct Hit {
    position: [i32; 3],
    face: [i32; 3],
}

impl Hit {
    fn adjacent(self) -> [i32; 3] {
        std::array::from_fn(|axis| self.position[axis] + self.face[axis])
    }
}

fn first_block(
    origin: Vec3,
    direction: Vec3,
    reach: f64,
    mut occupied: impl FnMut([i32; 3]) -> bool,
) -> Option<Hit> {
    if !origin.is_finite() || !direction.is_finite() || direction == Vec3::ZERO {
        return None;
    }
    let origin = origin.to_array().map(f64::from);
    let direction = direction.to_array().map(f64::from);
    let mut cell = origin.map(|value| value.floor() as i32);
    let mut step = [0; 3];
    let mut next = [f64::INFINITY; 3];
    let mut interval = [f64::INFINITY; 3];
    let mut face = [0, 1, 0];
    for axis in 0..3 {
        let dir = direction[axis];
        if dir == 0.0 {
            continue;
        }
        step[axis] = if dir > 0.0 { 1 } else { -1 };
        if origin[axis] == f64::from(cell[axis]) {
            face = [0; 3];
            face[axis] = -step[axis];
            if dir < 0.0 {
                cell[axis] -= 1;
            }
        }
        let boundary = f64::from(cell[axis]) + if dir > 0.0 { 1.0 } else { 0.0 };
        next[axis] = (boundary - origin[axis]) / dir;
        interval[axis] = dir.abs().recip();
    }

    loop {
        if occupied(cell) {
            return Some(Hit {
                position: cell,
                face,
            });
        }
        let distance = next[0].min(next[1]).min(next[2]);
        if !distance.is_finite() || distance > reach {
            return None;
        }
        // Step every tied axis together. The side cells only touch the ray at the boundary.
        let entry_axis = next
            .iter()
            .position(|&next| next == distance)
            .expect("el rayo cruza un eje");
        face = [0; 3];
        face[entry_axis] = -step[entry_axis];
        for axis in 0..3 {
            if next[axis] == distance {
                cell[axis] += step[axis];
                next[axis] += interval[axis];
            }
        }
    }
}

#[derive(Resource)]
pub struct SelectionAssets {
    cube: Handle<Mesh>,
    volume: Handle<StandardMaterial>,
    corners: [Handle<StandardMaterial>; 2],
}

pub fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let mut unlit = |base_color| {
        materials.add(StandardMaterial {
            base_color,
            unlit: true,
            fog_enabled: false,
            ..default()
        })
    };
    commands.insert_resource(SelectionAssets {
        cube: meshes.add(Cuboid::new(1.0, 1.0, 1.0)),
        volume: unlit(Color::srgb(0.46, 0.92, 0.81)),
        corners: [
            unlit(Color::srgb(0.29, 0.92, 0.67)),
            unlit(Color::srgb(1.0, 0.68, 0.28)),
        ],
    });
}

struct Outline {
    corners: [Option<[i32; 3]>; 2],
    root: Entity,
}

#[derive(Default)]
pub struct Outlines(HashMap<Entity, Outline>);

pub fn draw(
    mut commands: Commands,
    views: Query<(Entity, &View)>,
    assets: Res<SelectionAssets>,
    mut outlines: Local<Outlines>,
) {
    // Closing the window recursively removes its outline and meshes via ChildOf.
    outlines.0.retain(|window, _| views.get(*window).is_ok());
    for (window, view) in &views {
        let corners = if view.stream.is_some() {
            view.selection.corners
        } else {
            [None; 2]
        };
        if outlines
            .0
            .get(&window)
            .is_some_and(|old| old.corners == corners)
        {
            continue;
        }
        if let Some(old) = outlines.0.remove(&window) {
            commands.entity(old.root).try_despawn();
        }
        if corners == [None; 2] {
            continue;
        }

        let root = commands
            .spawn((Transform::default(), Visibility::default(), ChildOf(window)))
            .id();
        outlines.0.insert(window, Outline { corners, root });
        let layers = RenderLayers::none().with(view.layer);
        let mut outline = |min, max, padding, width, material: &Handle<StandardMaterial>| {
            for transform in box_edges(min, max, padding, width) {
                commands.spawn((
                    Mesh3d(assets.cube.clone()),
                    MeshMaterial3d(material.clone()),
                    transform,
                    layers.clone(),
                    ChildOf(root),
                ));
            }
        };
        if corners[0] != corners[1]
            && let Some(volume) = view.selection.volume()
        {
            outline(volume.min, volume.max, 0.025, 0.037, &assets.volume);
        }
        for (index, corner) in corners.into_iter().enumerate() {
            if index == 0 && corners[0] == corners[1] {
                continue;
            }
            if let Some(position) = corner {
                outline(position, position, 0.085, 0.062, &assets.corners[index]);
            }
        }
    }
}

pub(crate) fn box_edges(
    min: [i32; 3],
    max: [i32; 3],
    padding: f32,
    width: f32,
) -> impl Iterator<Item = Transform> {
    let min = Vec3::from(min.map(|value| value as f32)) - Vec3::splat(padding);
    let max = Vec3::from(max.map(|value| value as f32)) + Vec3::splat(1.0 + padding);
    let center = (min + max) * 0.5;
    (0..3).flat_map(move |axis| {
        let u = (axis + 1) % 3;
        let v = (axis + 2) % 3;
        let mut size = Vec3::splat(width);
        size[axis] = max[axis] - min[axis] + width;
        [
            (min[u], min[v]),
            (min[u], max[v]),
            (max[u], min[v]),
            (max[u], max[v]),
        ]
        .map(|(u_pos, v_pos)| {
            let mut position = center;
            position[u] = u_pos;
            position[v] = v_pos;
            Transform::from_translation(position).with_scale(size)
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ray_picks_nearest_block_across_negative_chunk_border() {
        let hit = first_block(Vec3::new(0.75, 5.5, -2.5), Vec3::NEG_X, 80.0, |cell| {
            [[-17, 5, -3], [-33, 5, -3]].contains(&cell)
        });
        let hit = hit.unwrap();
        assert_eq!(hit.position, [-17, 5, -3]);
        assert_eq!(hit.adjacent(), [-16, 5, -3]);
    }

    #[test]
    fn negative_ray_starts_on_the_correct_side_of_a_block_boundary() {
        let hit = first_block(Vec3::new(0.0, 0.5, 0.5), Vec3::NEG_X, 1.0, |_| true);
        let hit = hit.unwrap();
        assert_eq!(hit.position, [-1, 0, 0]);
        assert_eq!(hit.adjacent(), [0, 0, 0]);
    }

    #[test]
    fn ray_ignores_cells_touched_only_at_an_edge_and_stops_at_its_reach() {
        let direction = Vec3::new(1.0, 1.0, 0.0).normalize();
        let blocks = |cell| [[0, 1, 0], [1, 0, 0], [2, 2, 0]].contains(&cell);
        let origin = Vec3::splat(0.5);
        assert_eq!(first_block(origin, direction, 2.0, blocks), None);
        assert_eq!(
            first_block(origin, direction, 3.0, blocks)
                .unwrap()
                .position,
            [2, 2, 0]
        );
    }

    #[test]
    fn places_the_origin_on_the_clicked_face_above_ground() {
        let hit = first_block(Vec3::new(1.5, 8.0, -0.5), Vec3::NEG_Y, 20.0, |cell| {
            cell == [1, 2, -1]
        })
        .unwrap();
        assert_eq!(hit.adjacent(), [1, 3, -1]);
    }
}
