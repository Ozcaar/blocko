use bevy_egui::egui::{self, Align, Id, Layout, RichText, Ui};

use crate::{
    actions::{Action, Bindings},
    i18n::{Language, Text},
};

use super::{
    appearance::{self, ACCENT, ButtonStyle, Icon, MUTED, TEXT},
    binding_label,
};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) enum Dialog {
    Preferences,
    Controls,
    About,
}

impl Dialog {
    fn title(self) -> Text {
        match self {
            Self::Preferences => Text::Preferences,
            Self::Controls => Text::Controls,
            Self::About => Text::About,
        }
    }
}

#[derive(Default)]
pub(super) struct DialogResponse {
    pub(super) language: Option<Language>,
    pub(super) close: bool,
}

pub(super) fn show(
    ctx: &egui::Context,
    dialog: Dialog,
    lang: Language,
    bindings: &Bindings,
) -> DialogResponse {
    let screen = ctx.content_rect();
    let response = egui::Modal::new(Id::new("blocko-dialog"))
        .backdrop_color(egui::Color32::from_black_alpha(150))
        .frame(egui::Frame::window(&ctx.style()).inner_margin(egui::Margin::same(16)))
        .show(ctx, |ui| {
            ui.set_width((screen.width() - 48.0).clamp(100.0, 390.0));
            ui.set_max_height((screen.height() - 52.0).max(50.0));
            let mut response = DialogResponse::default();
            ui.horizontal(|ui| {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    response.close = appearance::button(
                        ui,
                        Some(Icon::Close),
                        "",
                        lang.text(Text::CloseDialog),
                        ButtonStyle::Quiet,
                        true,
                    )
                    .clicked();
                    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                        ui.add(
                            egui::Label::new(
                                RichText::new(lang.text(dialog.title()))
                                    .font(appearance::strong(18.0))
                                    .color(TEXT),
                            )
                            .wrap(),
                        );
                    });
                });
            });
            ui.add_space(8.0);
            egui::ScrollArea::vertical()
                .id_salt(dialog)
                .show(ui, |ui| match dialog {
                    Dialog::Preferences => response.language = preferences(ui, lang),
                    Dialog::Controls => controls(ui, lang, bindings),
                    Dialog::About => about(ui, lang),
                });
            response
        });
    DialogResponse {
        close: response.should_close() || response.inner.close,
        ..response.inner
    }
}

fn preferences(ui: &mut Ui, lang: Language) -> Option<Language> {
    let mut chosen = None;
    appearance::card().show(ui, |ui| {
        ui.take_available_width();
        appearance::headline(ui, lang.text(Text::Language), 14.0);
        ui.add_space(5.0);
        ui.horizontal_wrapped(|ui| {
            for language in Language::ALL {
                if appearance::button(
                    ui,
                    None,
                    language.name(),
                    language.name(),
                    ButtonStyle::Toggle(lang == language),
                    true,
                )
                .clicked()
                    && lang != language
                {
                    chosen = Some(language);
                }
            }
        });
    });
    chosen
}

fn controls(ui: &mut Ui, lang: Language, bindings: &Bindings) {
    use Action::*;
    use Text::{
        ActionMark, ActionPlace, ActionRepeat, CancelMark, CancelOperation, ControlsCamera,
        ControlsEditing, CopySelection, PasteStructure,
    };

    control_group(
        ui,
        lang,
        bindings,
        ControlsCamera,
        &[
            (Text::CameraLook, &[CameraLook]),
            (
                Text::CameraMove,
                &[CameraForward, CameraLeft, CameraBackward, CameraRight],
            ),
            (Text::CameraHeight, &[CameraUp, CameraDown]),
            (Text::CameraBoost, &[CameraBoost]),
            (Text::CameraRelease, &[CameraRelease]),
        ],
    );
    ui.add_space(6.0);
    control_group(
        ui,
        lang,
        bindings,
        ControlsEditing,
        &[
            (ActionMark, &[MarkCorner]),
            (CancelMark, &[CancelSelection]),
            (CopySelection, &[Copy]),
            (PasteStructure, &[StartPaste]),
            (ActionPlace, &[Place]),
            (ActionRepeat, &[RepeatPlace]),
            (CancelOperation, &[CancelPaste]),
        ],
    );
}

fn control_group(
    ui: &mut Ui,
    lang: Language,
    bindings: &Bindings,
    heading: Text,
    entries: &[(Text, &[Action])],
) {
    appearance::card().show(ui, |ui| {
        appearance::headline(ui, lang.text(heading), 14.0);
        ui.add_space(4.0);
        for (label, actions) in entries {
            let key = actions
                .iter()
                .map(|&action| binding_label(bindings, action, lang, false))
                .filter(|label| !label.is_empty())
                .collect::<Vec<_>>()
                .join(" / ");
            if key.is_empty() {
                continue;
            }
            ui.columns_const(|[description, shortcut]| {
                description.label(lang.text(*label));
                shortcut.with_layout(Layout::top_down(Align::RIGHT), |ui| {
                    ui.label(RichText::new(key).monospace().color(ACCENT));
                });
            });
        }
    });
}

fn about(ui: &mut Ui, lang: Language) {
    appearance::card().show(ui, |ui| {
        ui.take_available_width();
        let version = lang.format(Text::AboutVersion, &[&env!("CARGO_PKG_VERSION")]);
        ui.label(RichText::new(version).monospace().color(ACCENT));
        ui.add_space(3.0);
        ui.label(RichText::new(lang.text(Text::AboutDescription)).color(MUTED));
    });
}
