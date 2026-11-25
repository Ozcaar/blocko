use bevy_egui::egui::{self, Align, Color32, Label, Layout, RichText, Sense, vec2};

use crate::{i18n::Language, palette, world::BlockState};

use super::appearance::{ACCENT, Icon, TEXT};

pub(super) fn show(ui: &mut egui::Ui, material: Option<&BlockState>, lang: Language) {
    let Some(material) = material else {
        return;
    };
    let color = palette::block_color(&material.name)
        .map(|[red, green, blue]| Color32::from_rgb(red, green, blue))
        .unwrap_or(ACCENT);
    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
        let (icon, icon_response) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
        Icon::Cube.paint(ui.painter(), icon, color);
        let label = ui.add(
            Label::new(
                RichText::new(palette::block_name(material, lang))
                    .small()
                    .color(TEXT),
            )
            .truncate()
            .show_tooltip_when_elided(false),
        );
        icon_response.union(label)
    })
    .inner
    .on_hover_text(material.name.as_str());
}
