use bevy::{
    input::{
        ButtonState,
        keyboard::KeyboardInput,
        mouse::{AccumulatedMouseMotion, MouseButtonInput},
    },
    prelude::*,
    window::{CursorGrabMode, CursorOptions, WindowFocused},
};

use crate::editor::View;

#[derive(Component)]
pub struct Flight {
    yaw: f32,
    pitch: f32,
    velocity: Vec3,
    keys: ButtonInput<KeyCode>,
    captured: bool,
}

impl Flight {
    pub fn new(yaw: f32, pitch: f32) -> Self {
        Self {
            yaw,
            pitch,
            velocity: Vec3::ZERO,
            keys: ButtonInput::default(),
            captured: false,
        }
    }

    pub fn rotation(&self) -> Quat {
        Quat::from_euler(EulerRot::YXZ, self.yaw, self.pitch, 0.0)
    }

    fn release(&mut self, cursor: &mut CursorOptions) {
        self.captured = false;
        self.keys.reset_all();
        self.velocity = Vec3::ZERO;
        cursor.grab_mode = CursorGrabMode::None;
        cursor.visible = true;
    }
}

pub fn fly(
    mut windows: Query<(Entity, &Window, &mut CursorOptions, &View)>,
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
    for (entity, window, mut cursor, view) in &mut windows {
        let Ok((mut flight, mut transform)) = cameras.get_mut(view.camera) else {
            continue;
        };
        if !window.focused || blurred.contains(&entity) || view.pending.is_some() {
            if flight.captured {
                flight.release(&mut cursor);
            }
            continue;
        }
        let was_captured = flight.captured;
        for click in clicks
            .iter()
            .filter(|click| click.window == entity && click.button == MouseButton::Right)
        {
            match click.state {
                ButtonState::Pressed
                    if view
                        .viewport
                        .zip(window.cursor_position())
                        .is_some_and(|(rect, position)| rect.contains(position)) =>
                {
                    flight.captured = true;
                    cursor.grab_mode = CursorGrabMode::Locked;
                    cursor.visible = false;
                }
                ButtonState::Released if flight.captured => flight.release(&mut cursor),
                _ => {}
            }
        }
        if !flight.captured {
            continue;
        }
        for key in keys.iter().filter(|key| key.window == entity) {
            match key.state {
                ButtonState::Pressed => flight.keys.press(key.key_code),
                ButtonState::Released => flight.keys.release(key.key_code),
            }
        }
        if flight.keys.pressed(KeyCode::Escape) {
            flight.release(&mut cursor);
            continue;
        }
        if was_captured {
            flight.yaw = (flight.yaw - motion.delta.x * 0.0024).rem_euclid(std::f32::consts::TAU);
            flight.pitch = (flight.pitch - motion.delta.y * 0.0024).clamp(-1.55, 1.55);
        }
        transform.rotation = flight.rotation();
        let axis = |positive, negative| {
            i32::from(flight.keys.pressed(positive)) as f32
                - i32::from(flight.keys.pressed(negative)) as f32
        };
        let direction = axis(KeyCode::KeyW, KeyCode::KeyS) * *transform.forward()
            + axis(KeyCode::KeyD, KeyCode::KeyA) * *transform.right()
            + axis(KeyCode::Space, KeyCode::ControlLeft) * Vec3::Y;
        let speed = if flight
            .keys
            .any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight])
        {
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
