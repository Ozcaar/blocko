use bevy::{
    input::{
        ButtonState,
        keyboard::KeyboardInput,
        mouse::{AccumulatedMouseMotion, MouseButtonInput},
    },
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
    ignore_next_motion: bool,
}

impl Flight {
    pub fn new(yaw: f32, pitch: f32) -> Self {
        Self {
            yaw,
            pitch,
            velocity: Vec3::ZERO,
            keys: ButtonInput::default(),
            capture: Capture::Free,
            ignore_next_motion: false,
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

    pub fn toggle_capture(&mut self, cursor: &mut CursorOptions) {
        if self.persistent() {
            self.release(cursor);
        } else {
            self.capture(Capture::Persistent, cursor);
        }
    }

    pub fn release(&mut self, cursor: &mut CursorOptions) {
        self.capture = Capture::Free;
        self.ignore_next_motion = false;
        self.keys.reset_all();
        self.velocity = Vec3::ZERO;
        cursor.grab_mode = CursorGrabMode::None;
        cursor.visible = true;
    }

    fn capture(&mut self, mode: Capture, cursor: &mut CursorOptions) {
        if !self.captured() {
            // Discard motion accumulated before the cursor lock takes effect.
            self.ignore_next_motion = true;
            cursor.grab_mode = CursorGrabMode::Locked;
            cursor.visible = false;
        }
        self.capture = mode;
    }

    fn look_button(&mut self, state: ButtonState, over_viewport: bool, cursor: &mut CursorOptions) {
        match state {
            ButtonState::Pressed if over_viewport && !self.captured() => {
                self.capture(Capture::Held, cursor);
            }
            ButtonState::Released if self.capture == Capture::Held => self.release(cursor),
            _ => {}
        }
    }

    fn look(&mut self, motion: Vec2) {
        if std::mem::take(&mut self.ignore_next_motion) {
            return;
        }
        self.yaw = (self.yaw - motion.x * 0.0024).rem_euclid(std::f32::consts::TAU);
        self.pitch = (self.pitch - motion.y * 0.0024).clamp(-1.55, 1.55);
    }
}

pub fn fly(
    mut windows: Query<(Entity, &Window, &mut CursorOptions, &Workspace)>,
    views: Query<&View>,
    bindings: Res<Bindings>,
    mut cameras: Query<(&mut Flight, &mut Transform)>,
    mut mouse: MessageReader<MouseButtonInput>,
    mut keyboard: MessageReader<KeyboardInput>,
    mut focus: MessageReader<WindowFocused>,
    motion: Res<AccumulatedMouseMotion>,
    time: Res<Time>,
) {
    let clicks: Vec<_> = mouse.read().collect();
    let keys: Vec<_> = keyboard.read().collect();
    let blurred: Vec<_> = focus
        .read()
        .filter(|event| !event.focused)
        .map(|event| event.window)
        .collect();
    for (entity, window, mut cursor, workspace) in &mut windows {
        let Ok(view) = views.get(workspace.active) else {
            continue;
        };
        let Ok((mut flight, mut transform)) = cameras.get_mut(view.camera) else {
            continue;
        };
        if !window.focused
            || blurred.contains(&entity)
            || workspace.pending.is_some()
            || workspace.blocks_camera
            || view.viewport.is_none()
            || view.paste.writes()
        {
            if flight.captured() {
                flight.release(&mut cursor);
            }
            continue;
        }
        let over_viewport = view
            .viewport
            .zip(window.cursor_position())
            .is_some_and(|(rect, position)| rect.contains(position));
        for click in clicks.iter().filter(|click| {
            click.window == entity && bindings.camera_mouse(Action::CameraLook, click.button)
        }) {
            flight.look_button(click.state, over_viewport, &mut cursor);
        }
        if !flight.captured() {
            continue;
        }
        for key in keys.iter().filter(|key| key.window == entity) {
            match key.state {
                ButtonState::Pressed => flight.keys.press(key.key_code),
                ButtonState::Released => flight.keys.release(key.key_code),
            }
        }
        flight.look(motion.delta);
        transform.rotation = flight.rotation();
        let axis = |positive, negative| {
            i32::from(bindings.camera_key_pressed(positive, &flight.keys)) as f32
                - i32::from(bindings.camera_key_pressed(negative, &flight.keys)) as f32
        };
        let direction = axis(Action::CameraForward, Action::CameraBackward) * *transform.forward()
            + axis(Action::CameraRight, Action::CameraLeft) * *transform.right()
            + axis(Action::CameraUp, Action::CameraDown) * Vec3::Y;
        let speed = if bindings.camera_key_pressed(Action::CameraBoost, &flight.keys) {
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
    fn capture_keeps_orientation_and_release_clears_movement() {
        let mut flight = Flight::new(0.7, -0.2);
        let mut cursor = CursorOptions::default();
        let initial = flight.rotation();

        flight.look_button(ButtonState::Pressed, false, &mut cursor);
        assert!(!flight.captured());
        flight.look_button(ButtonState::Pressed, true, &mut cursor);
        flight.look(Vec2::splat(200.0));
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
}
