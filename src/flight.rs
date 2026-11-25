use bevy::{
    input::{ButtonState, keyboard::KeyboardInput, mouse::AccumulatedMouseMotion},
    prelude::*,
    window::{CursorGrabMode, CursorOptions, WindowFocused},
};

use crate::{
    actions::{Action, Bindings},
    editor::{View, Workspace},
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Capture {
    Free,
    Held,
    Persistent,
}

#[derive(Component)]
pub struct Flight {
    yaw: f32,
    pitch: f32,
    velocity: Vec3,
    keys: ButtonInput<KeyCode>,
    capture: Capture,
    just_captured: bool,
    focus_changed: bool,
    suppress_boost: bool,
}

impl Flight {
    pub fn new(yaw: f32, pitch: f32) -> Self {
        Self {
            yaw,
            pitch,
            velocity: Vec3::ZERO,
            keys: ButtonInput::default(),
            capture: Capture::Free,
            just_captured: false,
            focus_changed: false,
            suppress_boost: false,
        }
    }

    pub fn rotation(&self) -> Quat {
        Quat::from_euler(EulerRot::YXZ, self.yaw, self.pitch, 0.0)
    }

    pub fn captured(&self) -> bool {
        self.capture != Capture::Free
    }

    pub fn persistent(&self) -> bool {
        self.capture == Capture::Persistent
    }

    pub fn focus_changed(&self) -> bool {
        self.focus_changed
    }

    pub fn suppress_boost_once(&mut self) {
        self.suppress_boost = true;
    }

    pub fn toggle_capture(&mut self, cursor: &mut CursorOptions) {
        if self.persistent() {
            self.release(cursor);
        } else if !self.focus_changed {
            self.capture(Capture::Persistent, cursor);
        }
    }

    pub fn change_tool(&mut self, cursor: &mut CursorOptions) {
        if self.capture == Capture::Held {
            self.release(cursor);
        }
    }

    pub fn release(&mut self, cursor: &mut CursorOptions) {
        self.capture = Capture::Free;
        self.just_captured = false;
        self.keys.reset_all();
        self.velocity = Vec3::ZERO;
        cursor.grab_mode = CursorGrabMode::None;
        cursor.visible = true;
    }

    fn capture(&mut self, mode: Capture, cursor: &mut CursorOptions) {
        if !self.captured() {
            self.just_captured = true;
            cursor.grab_mode = CursorGrabMode::Locked;
            cursor.visible = false;
        }
        self.capture = mode;
    }

    pub fn look_button(
        &mut self,
        state: ButtonState,
        over_viewport: bool,
        cursor: &mut CursorOptions,
    ) {
        match state {
            ButtonState::Pressed if over_viewport && !self.focus_changed && !self.captured() => {
                self.capture(Capture::Held, cursor);
            }
            ButtonState::Released if self.capture == Capture::Held => self.release(cursor),
            _ => {}
        }
    }

    fn look(&mut self, motion: Vec2) {
        self.yaw = (self.yaw - motion.x * 0.0024).rem_euclid(std::f32::consts::TAU);
        self.pitch = (self.pitch - motion.y * 0.0024).clamp(-1.55, 1.55);
    }

    fn record_key(&mut self, key: &KeyboardInput, bindings: &Bindings) {
        match key.state {
            ButtonState::Pressed if key.repeat => {}
            ButtonState::Pressed if Bindings::is_camera_modifier(key.key_code) => {
                self.keys.press(key.key_code);
                self.velocity = Vec3::ZERO;
                let motion_keys: Vec<_> = self
                    .keys
                    .get_pressed()
                    .copied()
                    .filter(|key| !Bindings::is_camera_modifier(*key))
                    .collect();
                for key in motion_keys {
                    self.keys.reset(key);
                }
            }
            ButtonState::Pressed if !bindings.camera_motion_blocked(&self.keys) => {
                self.keys.press(key.key_code);
            }
            ButtonState::Pressed => {}
            ButtonState::Released => self.keys.release(key.key_code),
        }
    }
}

pub fn handle_focus(
    mut windows: Query<(Entity, &Window, &mut CursorOptions, &Workspace)>,
    views: Query<&View>,
    mut cameras: Query<&mut Flight>,
    mut focus: MessageReader<WindowFocused>,
) {
    let changes: Vec<_> = focus.read().map(|event| event.window).collect();
    for (entity, window, mut cursor, workspace) in &mut windows {
        let Ok(view) = views.get(workspace.active) else {
            continue;
        };
        let Ok(mut flight) = cameras.get_mut(view.camera) else {
            continue;
        };
        flight.focus_changed = changes.contains(&entity);
        if flight.focus_changed() || !window.focused {
            if flight.captured() {
                flight.release(&mut cursor);
            } else {
                flight.keys.reset_all();
            }
        }
    }
}

pub fn fly(
    mut windows: Query<(Entity, &Window, &mut CursorOptions, &Workspace)>,
    views: Query<&View>,
    bindings: Res<Bindings>,
    mut cameras: Query<(&mut Flight, &mut Transform)>,
    mut keyboard: MessageReader<KeyboardInput>,
    motion: Res<AccumulatedMouseMotion>,
    time: Res<Time>,
) {
    let keys: Vec<_> = keyboard.read().collect();
    for (entity, window, mut cursor, workspace) in &mut windows {
        let Ok(view) = views.get(workspace.active) else {
            continue;
        };
        let Ok((mut flight, mut transform)) = cameras.get_mut(view.camera) else {
            continue;
        };
        let suppress_boost = std::mem::take(&mut flight.suppress_boost);
        if !window.focused
            || flight.focus_changed()
            || workspace.pending.is_some()
            || workspace.blocks_camera
            || view.viewport.is_none()
            || view.paste.writes()
        {
            if flight.captured() {
                flight.release(&mut cursor);
            } else {
                flight.keys.reset_all();
            }
            continue;
        }
        flight.keys.clear();
        for key in keys.iter().filter(|key| key.window == entity) {
            flight.record_key(key, &bindings);
        }
        if !flight.captured() {
            continue;
        }
        if cursor.grab_mode == CursorGrabMode::None {
            flight.release(&mut cursor);
            continue;
        }
        // This frame's accumulated motion was collected before the UI requested capture.
        if !std::mem::take(&mut flight.just_captured) {
            flight.look(motion.delta);
        }
        transform.rotation = flight.rotation();
        if bindings.camera_motion_blocked(&flight.keys) {
            flight.velocity = Vec3::ZERO;
            continue;
        }
        let axis = |positive, negative| {
            i32::from(bindings.camera_key_pressed(positive, &flight.keys)) as f32
                - i32::from(bindings.camera_key_pressed(negative, &flight.keys)) as f32
        };
        let heading = Quat::from_rotation_y(flight.yaw);
        let horizontal = heading
            * Vec3::new(
                axis(Action::CameraRight, Action::CameraLeft),
                0.0,
                -axis(Action::CameraForward, Action::CameraBackward),
            );
        let direction = horizontal + axis(Action::CameraUp, Action::CameraDown) * Vec3::Y;
        let speed =
            if !suppress_boost && bindings.camera_key_pressed(Action::CameraBoost, &flight.keys) {
                52.0
            } else {
                16.0
            };
        let dt = time.delta_secs().min(0.05);
        flight.velocity = flight.velocity.lerp(
            direction.normalize_or_zero() * speed,
            1.0 - (-14.0 * dt).exp(),
        );
        transform.translation += flight.velocity * dt;
        transform.translation = transform.translation.clamp(
            Vec3::new(-30_000_000.0, -4096.0, -30_000_000.0),
            Vec3::new(30_000_000.0, 4096.0, 30_000_000.0),
        );
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use bevy::input::keyboard::Key;

    use super::*;

    #[test]
    fn persistent_capture_survives_right_button_release() {
        let mut flight = Flight::new(0.0, 0.0);
        let mut cursor = CursorOptions::default();

        flight.look_button(ButtonState::Pressed, true, &mut cursor);
        assert!(flight.captured());
        assert!(!flight.persistent());
        flight.look_button(ButtonState::Released, false, &mut cursor);
        assert!(!flight.captured());

        flight.toggle_capture(&mut cursor);
        flight.look_button(ButtonState::Pressed, true, &mut cursor);
        flight.look_button(ButtonState::Released, false, &mut cursor);
        assert!(flight.persistent());
        assert_eq!(cursor.grab_mode, CursorGrabMode::Locked);
        assert!(!cursor.visible);

        flight.toggle_capture(&mut cursor);
        assert!(!flight.captured());
        assert_eq!(cursor.grab_mode, CursorGrabMode::None);
        assert!(cursor.visible);
    }

    #[test]
    fn changing_tool_preserves_persistent_navigation_and_releases_temporary_capture() {
        let mut flight = Flight::new(0.7, -0.2);
        let mut cursor = CursorOptions::default();
        let rotation = flight.rotation();
        flight.toggle_capture(&mut cursor);
        flight.keys.press(KeyCode::KeyW);
        flight.velocity = Vec3::Z;

        flight.change_tool(&mut cursor);
        assert!(flight.persistent());
        assert_eq!(cursor.grab_mode, CursorGrabMode::Locked);
        assert!(!cursor.visible);
        assert_eq!(flight.rotation(), rotation);
        assert!(flight.keys.pressed(KeyCode::KeyW));
        assert_eq!(flight.velocity, Vec3::Z);

        flight.release(&mut cursor);
        flight.change_tool(&mut cursor);
        assert!(!flight.captured());

        flight.look_button(ButtonState::Pressed, true, &mut cursor);
        flight.change_tool(&mut cursor);
        assert!(!flight.captured());
        assert_eq!(cursor.grab_mode, CursorGrabMode::None);
        assert!(cursor.visible);
    }

    #[test]
    fn capture_keeps_orientation_and_release_clears_movement() {
        let mut flight = Flight::new(0.7, -0.2);
        let mut cursor = CursorOptions::default();
        let initial = flight.rotation();

        flight.look_button(ButtonState::Pressed, false, &mut cursor);
        assert!(!flight.captured());
        flight.look_button(ButtonState::Pressed, true, &mut cursor);
        assert_eq!(flight.rotation(), initial);
        flight.keys.press(KeyCode::KeyW);
        flight.velocity = Vec3::Z;

        flight.toggle_capture(&mut cursor);
        flight.look(Vec2::splat(1.0));
        assert_ne!(flight.rotation(), initial);
        assert!(flight.keys.pressed(KeyCode::KeyW));
        assert_eq!(flight.velocity, Vec3::Z);

        let changed = flight.rotation();
        flight.release(&mut cursor);
        assert!(!flight.captured());
        assert!(!flight.keys.pressed(KeyCode::KeyW));
        assert_eq!(flight.velocity, Vec3::ZERO);
        assert_eq!(flight.rotation(), changed);
    }

    fn camera_app() -> (App, Entity, Entity) {
        let mut app = App::new();
        app.init_resource::<Bindings>()
            .init_resource::<Time>()
            .init_resource::<AccumulatedMouseMotion>()
            .add_message::<KeyboardInput>()
            .add_message::<WindowFocused>()
            .add_systems(Update, (handle_focus, fly).chain());
        let world = app.world_mut();
        world
            .resource_mut::<Time>()
            .advance_by(Duration::from_millis(16));
        let window = world
            .spawn(Window {
                focused: true,
                ..default()
            })
            .id();
        let flight = Flight::new(0.0, 0.9);
        let camera = world
            .spawn((Transform::from_rotation(flight.rotation()), flight))
            .id();
        let view = world
            .spawn(View {
                owner: window,
                camera,
                layer: 1,
                stream: None,
                viewport: Some(Rect::from_corners(Vec2::ZERO, Vec2::splat(100.0))),
                demo_scene: None,
                selection: default(),
                copying: false,
                copy_notice: None,
                tool: crate::actions::EditorContext::Explore,
                edits: default(),
                replace_source: String::new(),
                inspected: None,
                inspection_pinned: false,
                air_mode: default(),
                preview_cancelled: false,
                preview: None,
                paste: default(),
            })
            .id();
        world.entity_mut(window).insert(Workspace::new(view));
        (app, window, camera)
    }

    fn toggle(app: &mut App, window: Entity, camera: Entity) {
        let world = app.world_mut();
        let mut cursor = world.get::<CursorOptions>(window).unwrap().clone();
        world
            .get_mut::<Flight>(camera)
            .unwrap()
            .toggle_capture(&mut cursor);
        world.entity_mut(window).insert(cursor);
    }

    fn key(app: &mut App, window: Entity, key_code: KeyCode, state: ButtonState, repeat: bool) {
        app.world_mut().write_message(KeyboardInput {
            key_code,
            logical_key: Key::Character("".into()),
            state,
            text: None,
            repeat,
            window,
        });
    }

    #[test]
    fn quick_focus_round_trip_releases_capture_and_ignores_old_input() {
        let (mut app, window, camera) = camera_app();
        toggle(&mut app, window, camera);
        app.update();
        key(&mut app, window, KeyCode::KeyW, ButtonState::Pressed, false);
        app.update();
        let before = *app.world().get::<Transform>(camera).unwrap();
        assert!(before.translation.z < 0.0);

        let ecs = app.world_mut();
        ecs.write_message(WindowFocused {
            window,
            focused: false,
        });
        ecs.write_message(WindowFocused {
            window,
            focused: true,
        });
        ecs.resource_mut::<AccumulatedMouseMotion>().delta = Vec2::splat(200.0);
        key(&mut app, window, KeyCode::KeyW, ButtonState::Pressed, true);
        app.update();

        let flight = app.world().get::<Flight>(camera).unwrap();
        assert!(flight.focus_changed());
        assert!(!flight.captured());
        assert_eq!(flight.velocity, Vec3::ZERO);
        let cursor = app.world().get::<CursorOptions>(window).unwrap();
        assert!(cursor.visible);
        assert_eq!(cursor.grab_mode, CursorGrabMode::None);
        toggle(&mut app, window, camera);
        assert!(!app.world().get::<Flight>(camera).unwrap().captured());
        app.update();

        toggle(&mut app, window, camera);
        key(&mut app, window, KeyCode::KeyW, ButtonState::Pressed, true);
        app.update();
        assert_eq!(*app.world().get::<Transform>(camera).unwrap(), before);

        app.world_mut()
            .resource_mut::<AccumulatedMouseMotion>()
            .delta = Vec2::new(12.0, 1.0);
        app.update();
        let after = app.world().get::<Transform>(camera).unwrap();
        assert_ne!(after.rotation, before.rotation);
        assert_eq!(after.translation, before.translation);
    }

    #[test]
    fn movement_uses_heading_and_modifier_chords_do_not_leak_into_navigation() {
        let (mut app, window, camera) = camera_app();
        toggle(&mut app, window, camera);
        key(&mut app, window, KeyCode::KeyW, ButtonState::Pressed, false);
        app.update();
        let walk = app.world().get::<Transform>(camera).unwrap().translation;
        assert!(walk.z < 0.0);
        assert_eq!(walk.y, 0.0);

        for code in [KeyCode::ControlLeft, KeyCode::KeyS, KeyCode::KeyQ] {
            key(&mut app, window, code, ButtonState::Pressed, false);
        }
        app.update();
        assert_eq!(
            app.world().get::<Transform>(camera).unwrap().translation,
            walk
        );

        key(
            &mut app,
            window,
            KeyCode::ControlLeft,
            ButtonState::Released,
            false,
        );
        for code in [KeyCode::KeyS, KeyCode::KeyQ] {
            key(&mut app, window, code, ButtonState::Pressed, true);
        }
        app.update();
        assert_eq!(
            app.world().get::<Transform>(camera).unwrap().translation,
            walk
        );

        key(
            &mut app,
            window,
            KeyCode::KeyS,
            ButtonState::Released,
            false,
        );
        key(&mut app, window, KeyCode::KeyS, ButtonState::Pressed, false);
        app.update();
        assert!(app.world().get::<Transform>(camera).unwrap().translation.z > walk.z);

        key(
            &mut app,
            window,
            KeyCode::Space,
            ButtonState::Pressed,
            false,
        );
        app.update();
        assert!(app.world().get::<Transform>(camera).unwrap().translation.y > walk.y);
    }

    #[test]
    fn a_shift_gesture_suppresses_only_the_current_frame_boost() {
        let (mut app, window, camera) = camera_app();
        toggle(&mut app, window, camera);
        app.world_mut()
            .get_mut::<Flight>(camera)
            .unwrap()
            .suppress_boost_once();
        for key_code in [KeyCode::KeyW, KeyCode::ShiftLeft] {
            key(&mut app, window, key_code, ButtonState::Pressed, false);
        }
        app.update();
        let speed = app.world().get::<Flight>(camera).unwrap().velocity.length();
        let unboosted_speed = 16.0 * (1.0 - (-14.0_f32 * 0.016).exp());
        assert!((speed - unboosted_speed).abs() < 0.0001);

        app.update();
        assert!(app.world().get::<Flight>(camera).unwrap().velocity.length() > 10.0);
    }
}
