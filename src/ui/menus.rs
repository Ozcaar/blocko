use bevy::prelude::Entity;
use bevy_egui::egui;

use crate::{
    actions::{Action, Bindings, EditorContext},
    editor::{Destination, TabAction, View},
    i18n::{Language, Text},
    structure::Clipboard,
};

use super::{
    WindowActions, action_enabled, appearance::BAR, binding_label, dialogs::Dialog, frame,
    tools_context,
};

#[derive(Clone, Copy)]
pub(super) struct WindowState {
    pub(super) activity_bar: bool,
    pub(super) inspector: bool,
    pub(super) dialog: Option<Dialog>,
}

impl Default for WindowState {
    fn default() -> Self {
        Self {
            activity_bar: true,
            inspector: true,
            dialog: None,
        }
    }
}

pub(super) fn window_state(ctx: &egui::Context) -> WindowState {
    ctx.data(|data| {
        data.get_temp(egui::Id::new("window presentation"))
            .unwrap_or_default()
    })
}

pub(super) fn save_window_state(ctx: &egui::Context, state: WindowState) {
    ctx.data_mut(|data| data.insert_temp(egui::Id::new("window presentation"), state));
}

pub(super) fn bar(
    ctx: &egui::Context,
    chrome: &mut WindowState,
    document: Entity,
    view: &View,
    clipboard: &Clipboard,
    busy: bool,
    captured: bool,
    lang: Language,
    bindings: &Bindings,
    actions: &mut WindowActions,
) {
    let world_open = view.stream.is_some();
    let scene_open = world_open || view.demo_scene.is_some();
    egui::TopBottomPanel::top("application menu")
        .exact_height(36.0)
        .frame(frame(BAR).inner_margin(egui::Margin::symmetric(10, 4)))
        .show(ctx, |ui| {
            ui.horizontal_centered(|ui| {
                egui::MenuBar::new()
                    .style(|style: &mut egui::Style| {
                        egui::containers::menu::menu_style(style);
                        style.spacing.button_padding.x = 8.0;
                    })
                    .ui(ui, |ui| {
                        ui.spacing_mut().item_spacing.x = 1.0;
                        let editor = &mut actions.editor;
                        let mut command = |ui: &mut egui::Ui, label, action| {
                            let binding = binding_label(bindings, action, lang, false);
                            if entry(
                                ui,
                                lang.text(label),
                                &binding,
                                !busy && action_enabled(action, view, clipboard, captured),
                            ) {
                                *editor = Some((action, None));
                            }
                        };
                        ui.menu_button(lang.text(Text::FileMenu), |ui| {
                            menu_width(ui, 240.0);
                            for (label, destination, available) in [
                                (Text::OpenWorld, Destination::Tab, true),
                                (Text::OpenWindow, Destination::NewWindow, true),
                                (Text::OpenReplace, Destination::Replace, world_open),
                            ] {
                                if entry(ui, lang.text(label), "", !busy && available) {
                                    actions.open = Some(destination);
                                }
                            }
                            ui.separator();
                            command(ui, Text::SaveWorld, Action::Save);
                            if entry(ui, lang.text(Text::CloseTab), "", !busy && world_open) {
                                actions.tab = Some((document, TabAction::Close));
                            }
                        });
                        ui.menu_button(lang.text(Text::EditMenu), |ui| {
                            menu_width(ui, 235.0);
                            command(ui, Text::UndoEdit, Action::Undo);
                            command(ui, Text::RedoEdit, Action::Redo);
                            ui.separator();
                            let cancel = if tools_context(view) == EditorContext::Place {
                                Action::CancelPaste
                            } else {
                                Action::CancelSelection
                            };
                            for (label, action) in [
                                (Text::CopySelection, Action::Copy),
                                (Text::PasteStructure, Action::StartPaste),
                                (Text::CancelOperation, cancel),
                            ] {
                                command(ui, label, action);
                            }
                            ui.separator();
                            command(ui, Text::BlockPalette, Action::OpenPalette);
                            let tools_enabled = !busy && world_open && !view.paste.locks_preview();
                            for (label, tool) in [
                                (Text::InspectBlock, EditorContext::Inspect),
                                (Text::PaintBlock, EditorContext::Paint),
                                (Text::EraseBlock, EditorContext::Erase),
                            ] {
                                if entry(ui, lang.text(label), "", tools_enabled) {
                                    actions.tool = Some(tool);
                                }
                            }
                            let region_enabled = tools_enabled
                                && tools_context(view) == EditorContext::Select
                                && view
                                    .selection
                                    .volume()
                                    .is_some_and(|volume| volume.validate().is_ok());
                            if entry(ui, lang.text(Text::RegionEdit), "", region_enabled) {
                                chrome.inspector = true;
                                actions.region_menu = true;
                            }
                            ui.separator();
                            if entry(ui, lang.text(Text::Preferences), "", true) {
                                chrome.dialog = Some(Dialog::Preferences);
                            }
                        });
                        ui.menu_button(lang.text(Text::ViewMenu), |ui| {
                            menu_width(ui, 174.0);
                            if ui
                                .checkbox(&mut chrome.activity_bar, lang.text(Text::ActivityBar))
                                .changed()
                            {
                                ui.close();
                            }
                            if ui
                                .add_enabled(
                                    scene_open,
                                    egui::Checkbox::new(
                                        &mut chrome.inspector,
                                        lang.text(Text::Inspector),
                                    ),
                                )
                                .changed()
                            {
                                ui.close();
                            }
                            ui.separator();
                            command(ui, Text::CameraCapture, Action::ToggleCameraCapture);
                        });
                        ui.menu_button(lang.text(Text::HelpMenu), |ui| {
                            menu_width(ui, 190.0);
                            if entry(ui, lang.text(Text::Controls), "", true) {
                                chrome.dialog = Some(Dialog::Controls);
                            }
                            if entry(ui, lang.text(Text::About), "", true) {
                                chrome.dialog = Some(Dialog::About);
                            }
                        });
                    });
            });
        });
}

fn menu_width(ui: &mut egui::Ui, width: f32) {
    ui.set_min_width(width);
    ui.spacing_mut().interact_size.y = 26.0;
    ui.spacing_mut().item_spacing.y = 2.0;
}

fn entry(ui: &mut egui::Ui, text: &str, shortcut: &str, enabled: bool) -> bool {
    let width = ui.available_width();
    let button = egui::Button::new(text).shortcut_text(shortcut);
    let clicked = ui
        .add_enabled(enabled, |ui: &mut egui::Ui| {
            ui.add_sized([width, ui.spacing().interact_size.y], button)
        })
        .clicked();
    if clicked {
        ui.close();
    }
    clicked
}
