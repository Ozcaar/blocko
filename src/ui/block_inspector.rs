use bevy_egui::egui;

use crate::{
    i18n::{Language, Text},
    palette::block_name,
    world::{BlockEntity, BlockEntityRecord, BlockState, SignText},
};

use super::appearance::{self, ACCENT, MUTED};

pub(super) fn show(
    ui: &mut egui::Ui,
    lang: Language,
    coordinate: [i32; 3],
    state: &BlockState,
    entity: Option<&BlockEntityRecord>,
    pending: bool,
) {
    ui.push_id("inspected block", |ui| {
        let name = block_name(state, lang);
        appearance::headline(ui, &name, 17.0);
        data(ui, &state.name);
        let [x, y, z] = coordinate;
        ui.label(lang.format(Text::Coordinates, &[&x, &y, &z]));
        let (origin, color) = if pending {
            (Label::Pending, ACCENT)
        } else {
            (Label::Saved, MUTED)
        };
        ui.label(egui::RichText::new(origin.get(lang)).small().color(color));

        ui.add_space(4.0);
        ui.separator();
        appearance::headline(ui, Label::Properties.get(lang), 14.0);
        if state.properties.is_empty() {
            muted(ui, Label::NoProperties.get(lang));
        } else {
            egui::Grid::new("block properties")
                .spacing(egui::vec2(8.0, 4.0))
                .max_col_width((ui.available_width() - 8.0).max(1.0) / 2.0)
                .show(ui, |ui| {
                    for (key, value) in &state.properties {
                        ui.add(
                            egui::Label::new(egui::RichText::new(key).monospace().color(MUTED))
                                .wrap(),
                        );
                        data(ui, value);
                        ui.end_row();
                    }
                });
        }
        if let Some(entity) = entity {
            ui.add_space(4.0);
            egui::CollapsingHeader::new(
                egui::RichText::new(Label::Entity.get(lang)).font(appearance::strong(14.0)),
            )
            .id_salt("block entity")
            .show_unindented(ui, |ui| show_entity(ui, lang, entity));
        }
    });
}

fn show_entity(ui: &mut egui::Ui, lang: Language, record: &BlockEntityRecord) {
    match record {
        BlockEntityRecord::Supported(BlockEntity::Chest {
            items,
            custom_name,
            lock,
        }) => {
            if let Some(name) = custom_name {
                muted(ui, Label::CustomName.get(lang));
                message(ui, name);
            }
            if let Some(lock) = lock {
                muted(ui, Label::Lock.get(lang));
                data(ui, lock);
            }
            muted(ui, Label::Inventory.get(lang));
            if items.is_empty() {
                ui.label(Label::Empty.get(lang));
            }
            for item in items {
                data(
                    ui,
                    format!("{:02} · {} ×{}", item.slot, item.id, item.count),
                );
            }
        }
        BlockEntityRecord::Supported(BlockEntity::Sign { front, back, waxed }) => {
            flag(ui, lang, Label::Waxed, *waxed);
            sign_face(ui, lang, Label::Front, front);
            sign_face(ui, lang, Label::Back, back);
        }
        BlockEntityRecord::Unsupported { kind, .. } => {
            data(ui, kind);
            muted(ui, Label::Unsupported.get(lang));
        }
    }
}

fn sign_face(ui: &mut egui::Ui, lang: Language, face: Label, text: &SignText) {
    ui.push_id(face, |ui| {
        ui.add_space(4.0);
        appearance::headline(ui, face.get(lang), 13.0);
        for line in &text.messages {
            message(ui, line);
        }
        ui.horizontal_wrapped(|ui| {
            muted(ui, Label::Color.get(lang));
            data(ui, &text.color);
        });
        flag(ui, lang, Label::Glowing, text.glowing);
        if let Some(lines) = &text.filtered_messages {
            egui::CollapsingHeader::new(Label::Filtered.get(lang))
                .id_salt("filtered messages")
                .show_unindented(ui, |ui| {
                    for line in lines {
                        message(ui, line);
                    }
                });
        }
    });
}

fn message(ui: &mut egui::Ui, encoded: &str) {
    let text = literal_component(encoded);
    ui.add(egui::Label::new(text.as_deref().unwrap_or(encoded)).wrap());
}

// A literal can be a JSON string or a component with a single "text" member.
// Keep more involved components intact so the inspector never hides their data.
fn literal_component(encoded: &str) -> Option<String> {
    let encoded = encoded.trim();
    let value = if let Some(object) = encoded.strip_prefix('{') {
        object
            .strip_suffix('}')?
            .trim_start()
            .strip_prefix("\"text\"")?
            .trim_start()
            .strip_prefix(':')?
            .trim()
    } else {
        encoded
    };
    let mut chars = value.strip_prefix('"')?.strip_suffix('"')?.chars();
    let mut decoded = Vec::new();
    while let Some(ch) = chars.next() {
        let ch = match ch {
            '\\' => match chars.next()? {
                '"' => '"',
                '\\' => '\\',
                '/' => '/',
                'b' => '\u{0008}',
                'f' => '\u{000c}',
                'n' => '\n',
                'r' => '\r',
                't' => '\t',
                'u' => {
                    let hex = [chars.next()?, chars.next()?, chars.next()?, chars.next()?];
                    let unit = hex.into_iter().try_fold(0_u16, |unit, digit| {
                        Some((unit << 4) | digit.to_digit(16)? as u16)
                    })?;
                    decoded.push(unit);
                    continue;
                }
                _ => return None,
            },
            '"' | '\u{0000}'..='\u{001f}' => return None,
            _ => ch,
        };
        decoded.extend_from_slice(ch.encode_utf16(&mut [0; 2]));
    }
    String::from_utf16(&decoded).ok()
}

fn flag(ui: &mut egui::Ui, lang: Language, label: Label, value: bool) {
    ui.horizontal_wrapped(|ui| {
        muted(ui, label.get(lang));
        ui.label(if value {
            Label::Yes.get(lang)
        } else {
            Label::No.get(lang)
        });
    });
}

fn data(ui: &mut egui::Ui, text: impl Into<String>) {
    ui.add(egui::Label::new(egui::RichText::new(text).monospace()).wrap());
}

fn muted(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).small().color(MUTED));
}

#[derive(Clone, Copy, Hash)]
enum Label {
    Pending,
    Saved,
    Properties,
    NoProperties,
    Entity,
    CustomName,
    Lock,
    Inventory,
    Empty,
    Waxed,
    Front,
    Back,
    Color,
    Glowing,
    Filtered,
    Unsupported,
    Yes,
    No,
}

impl Label {
    fn get(self, lang: Language) -> &'static str {
        let (es, en) = match self {
            Self::Pending => ("Cambio sin guardar", "Unsaved change"),
            Self::Saved => ("Estado guardado", "Saved state"),
            Self::Properties => ("Propiedades del bloque", "Block properties"),
            Self::NoProperties => ("Sin propiedades", "No properties"),
            Self::Entity => ("Entidad de bloque", "Block entity"),
            Self::CustomName => ("Nombre personalizado", "Custom name"),
            Self::Lock => ("Cerradura", "Lock"),
            Self::Inventory => ("Inventario", "Inventory"),
            Self::Empty => ("Vacío", "Empty"),
            Self::Waxed => ("Encerado", "Waxed"),
            Self::Front => ("Frente", "Front"),
            Self::Back => ("Reverso", "Back"),
            Self::Color => ("Color", "Color"),
            Self::Glowing => ("Texto brillante", "Glowing text"),
            Self::Filtered => ("Texto filtrado", "Filtered text"),
            Self::Unsupported => (
                "Detalles no disponibles para esta entidad.",
                "Details are unavailable for this entity.",
            ),
            Self::Yes => ("Sí", "Yes"),
            Self::No => ("No", "No"),
        };
        match lang {
            Language::Spanish => es,
            Language::English => en,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn displays_plain_text_for_literal_components_without_discarding_complex_data() {
        assert_eq!(
            literal_component(r#"{"text":"Casa \uD83C\uDFE0"}"#).as_deref(),
            Some("Casa 🏠")
        );
        assert_eq!(
            literal_component(r#""Dijo \"hola\"""#).as_deref(),
            Some("Dijo \"hola\"")
        );
        assert_eq!(
            literal_component(r#"{"text":"Entrada","color":"red"}"#),
            None
        );
    }

    #[test]
    fn marks_the_visible_block_with_its_save_state_and_preserves_game_values() {
        let ctx = egui::Context::default();
        appearance::apply(&ctx);
        let state = BlockState {
            name: "minecraft:oak_stairs".into(),
            properties: [("facing".into(), "north".into())].into(),
        };
        for (lang, pending, origin) in [
            (Language::Spanish, true, "Cambio sin guardar"),
            (Language::English, false, "Saved state"),
        ] {
            let output = ctx.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    show(ui, lang, [-18, 70, 4], &state, None, pending);
                });
            });
            let texts: Vec<_> = output
                .shapes
                .iter()
                .filter_map(|clipped| match &clipped.shape {
                    egui::Shape::Text(text) => Some(text.galley.job.text.as_str()),
                    _ => None,
                })
                .collect();
            for label in [origin, &state.name, "X -18   Y 70   Z 4", "facing", "north"] {
                assert!(texts.contains(&label), "{label}: {texts:?}");
            }
        }
    }
}
