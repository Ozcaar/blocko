use bevy::prelude::{ButtonInput, KeyCode, MouseButton, Resource};
use bevy_egui::{
    egui::{Event, Key, ModifierNames, Modifiers, PointerButton},
    helpers::bevy_to_egui_physical_key,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    Copy,
    StartPaste,
    Place,
    RepeatPlace,
    CancelPaste,
    MarkCorner,
    CancelSelection,
    CameraLook,
    CameraForward,
    CameraBackward,
    CameraLeft,
    CameraRight,
    CameraUp,
    CameraDown,
    CameraBoost,
    CameraRelease,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EditorContext {
    Explore,
    Select,
    Place,
}

impl Action {
    pub fn allowed_in(self, context: EditorContext) -> bool {
        use Action::*;
        match self {
            Copy | StartPaste => context != EditorContext::Place,
            Place | RepeatPlace | CancelPaste => context == EditorContext::Place,
            MarkCorner | CancelSelection => context == EditorContext::Select,
            CameraLook | CameraForward | CameraBackward | CameraLeft | CameraRight | CameraUp
            | CameraDown | CameraBoost | CameraRelease => true,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Control {
    PrimaryMouse,
    SecondaryMouse,
    MiddleMouse,
    ExtraMouse1,
    ExtraMouse2,
    Backspace,
    Space,
    LeftControl,
    RightControl,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Trigger {
    Key(Modifiers, Key),
    Click(Modifiers, MouseButton),
    HeldMouse(MouseButton),
    HeldKey(KeyCode),
}

impl Trigger {
    fn matches(self, event: &Event) -> bool {
        match (self, event) {
            (
                Self::Key(chord, target),
                Event::Key {
                    key,
                    pressed: true,
                    repeat: false,
                    modifiers,
                    ..
                },
            ) => target == *key && modifiers.matches_exact(chord),
            (
                Self::Click(chord, target),
                Event::PointerButton {
                    button,
                    pressed: true,
                    modifiers,
                    ..
                },
            ) => target == bevy_mouse(*button) && modifiers.matches_exact(chord),
            _ => false,
        }
    }

    fn label(self, localize: impl Fn(Control) -> &'static str) -> String {
        match self {
            Self::Key(chord, key) => chord_label(chord, key_name(key, localize)),
            Self::Click(chord, button) => chord_label(chord, &mouse_name(button, localize)),
            Self::HeldMouse(button) => mouse_name(button, localize),
            Self::HeldKey(key) => match key {
                KeyCode::ControlLeft => localize(Control::LeftControl).into(),
                KeyCode::ControlRight => localize(Control::RightControl).into(),
                KeyCode::ShiftLeft | KeyCode::ShiftRight => "Shift".into(),
                key => bevy_to_egui_physical_key(&key)
                    .map_or_else(|| format!("{key:?}"), |key| key_name(key, localize).into()),
            },
        }
    }
}

fn chord_label(chord: Modifiers, name: &str) -> String {
    let modifiers = ModifierNames::NAMES.format(&chord, cfg!(target_os = "macos"));
    if modifiers.is_empty() {
        name.into()
    } else {
        format!("{modifiers}+{name}")
    }
}

fn key_name(key: Key, localize: impl Fn(Control) -> &'static str) -> &'static str {
    match key {
        Key::Escape => "Esc",
        Key::Backspace => localize(Control::Backspace),
        Key::Space => localize(Control::Space),
        key => key.name(),
    }
}

fn bevy_mouse(button: PointerButton) -> MouseButton {
    match button {
        PointerButton::Primary => MouseButton::Left,
        PointerButton::Secondary => MouseButton::Right,
        PointerButton::Middle => MouseButton::Middle,
        PointerButton::Extra1 => MouseButton::Back,
        PointerButton::Extra2 => MouseButton::Forward,
    }
}

fn mouse_name(button: MouseButton, localize: impl Fn(Control) -> &'static str) -> String {
    let control = match button {
        MouseButton::Left => Control::PrimaryMouse,
        MouseButton::Right => Control::SecondaryMouse,
        MouseButton::Middle => Control::MiddleMouse,
        MouseButton::Back => Control::ExtraMouse1,
        MouseButton::Forward => Control::ExtraMouse2,
        MouseButton::Other(index) => return format!("Mouse {index}"),
    };
    localize(control).into()
}

#[derive(Resource)]
pub struct Bindings(Vec<(Action, Trigger)>);

impl Default for Bindings {
    fn default() -> Self {
        use Action::*;
        use Trigger::{Click, HeldKey, HeldMouse};
        let plain = Modifiers::NONE;
        let ctrl = Modifiers::CTRL;
        let shift = Modifiers::SHIFT;

        Self(vec![
            (Copy, Trigger::Key(ctrl, Key::C)),
            (StartPaste, Trigger::Key(ctrl, Key::V)),
            (Place, Click(plain, MouseButton::Left)),
            (RepeatPlace, Click(shift, MouseButton::Left)),
            (CancelPaste, Trigger::Key(plain, Key::Escape)),
            (CancelPaste, Trigger::Key(plain, Key::Backspace)),
            (MarkCorner, Click(plain, MouseButton::Left)),
            (CancelSelection, Trigger::Key(plain, Key::Escape)),
            (CameraLook, HeldMouse(MouseButton::Right)),
            (CameraForward, HeldKey(KeyCode::KeyW)),
            (CameraBackward, HeldKey(KeyCode::KeyS)),
            (CameraLeft, HeldKey(KeyCode::KeyA)),
            (CameraRight, HeldKey(KeyCode::KeyD)),
            (CameraUp, HeldKey(KeyCode::Space)),
            (CameraDown, HeldKey(KeyCode::ControlLeft)),
            (CameraBoost, HeldKey(KeyCode::ShiftLeft)),
            (CameraBoost, HeldKey(KeyCode::ShiftRight)),
            (CameraRelease, HeldKey(KeyCode::Escape)),
        ])
    }
}

impl Bindings {
    fn triggers(&self, action: Action) -> impl Iterator<Item = Trigger> + '_ {
        self.0
            .iter()
            .filter_map(move |&(bound_action, trigger)| (bound_action == action).then_some(trigger))
    }

    pub fn available(&self, action: Action, context: EditorContext) -> bool {
        action.allowed_in(context) && self.triggers(action).next().is_some()
    }

    pub fn match_event(&self, context: EditorContext, event: &Event) -> Option<Action> {
        self.0.iter().find_map(|&(action, trigger)| {
            (action.allowed_in(context) && trigger.matches(event)).then_some(action)
        })
    }

    pub fn label(&self, action: Action, localize: impl Fn(Control) -> &'static str) -> String {
        let mut labels = Vec::new();
        for trigger in self.triggers(action) {
            let label = trigger.label(&localize);
            if !labels.contains(&label) {
                labels.push(label);
            }
        }
        labels.join(" / ")
    }

    pub fn camera_mouse(&self, action: Action, button: MouseButton) -> bool {
        self.triggers(action)
            .any(|trigger| trigger == Trigger::HeldMouse(button))
    }

    pub fn camera_key_pressed(&self, action: Action, keys: &ButtonInput<KeyCode>) -> bool {
        self.triggers(action)
            .any(|trigger| matches!(trigger, Trigger::HeldKey(key) if keys.pressed(key)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(key: Key, modifiers: Modifiers, repeat: bool) -> Event {
        Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat,
            modifiers,
        }
    }

    fn click(modifiers: Modifiers, pressed: bool) -> Event {
        Event::PointerButton {
            pos: bevy_egui::egui::Pos2::ZERO,
            button: PointerButton::Primary,
            pressed,
            modifiers,
        }
    }

    #[test]
    fn changing_a_binding_changes_dispatch_and_help_together() {
        let mut bindings = Bindings::default();
        let copy = bindings
            .0
            .iter_mut()
            .find(|entry| entry.0 == Action::Copy)
            .unwrap();
        copy.1 = Trigger::Key(Modifiers::CTRL, Key::K);
        let context = EditorContext::Select;

        assert_eq!(bindings.label(Action::Copy, |_| ""), "Ctrl+K");
        assert_eq!(
            bindings.match_event(context, &key(Key::C, Modifiers::CTRL, false)),
            None
        );
        assert_eq!(
            bindings.match_event(context, &key(Key::K, Modifiers::CTRL, false)),
            Some(Action::Copy)
        );
        assert_eq!(
            bindings.match_event(context, &key(Key::K, Modifiers::CTRL, true)),
            None
        );
    }

    #[test]
    fn context_and_modifiers_distinguish_mark_place_and_repeat() {
        let bindings = Bindings::default();
        let plain = click(Modifiers::NONE, true);
        let shifted = click(Modifiers::SHIFT, true);

        assert_eq!(bindings.match_event(EditorContext::Explore, &plain), None);
        assert_eq!(
            bindings.match_event(EditorContext::Select, &plain),
            Some(Action::MarkCorner)
        );
        assert_eq!(
            bindings.match_event(EditorContext::Place, &plain),
            Some(Action::Place)
        );
        assert_eq!(
            bindings.match_event(EditorContext::Place, &shifted),
            Some(Action::RepeatPlace)
        );
        assert_eq!(
            bindings.match_event(EditorContext::Place, &click(Modifiers::SHIFT, false)),
            None
        );
    }

    #[test]
    fn cancellation_aliases_share_one_action_and_display_together() {
        let bindings = Bindings::default();
        let place = EditorContext::Place;
        let select = EditorContext::Select;
        let escape = key(Key::Escape, Modifiers::NONE, false);
        let backspace = key(Key::Backspace, Modifiers::NONE, false);

        assert_eq!(
            bindings.match_event(place, &escape),
            Some(Action::CancelPaste)
        );
        assert_eq!(
            bindings.match_event(place, &backspace),
            Some(Action::CancelPaste)
        );
        assert_eq!(
            bindings.match_event(select, &escape),
            Some(Action::CancelSelection)
        );
        assert_eq!(bindings.match_event(select, &backspace), None);
        assert_eq!(
            bindings.label(Action::CancelPaste, |_| "Retroceso"),
            "Esc / Retroceso"
        );
    }
}
