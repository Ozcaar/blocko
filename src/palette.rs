use std::sync::OnceLock;

use bevy::prelude::Resource;
use bevy_egui::egui::{
    self, Align2, Color32, FontId, Id, Key, Modifiers, Rect, RichText, Sense, Stroke, Ui,
    WidgetInfo, WidgetType, pos2, vec2,
};

use crate::{i18n::Language, world::BlockState};

const ROW_HEIGHT: f32 = 39.0;
const RECENT_LIMIT: usize = 6;
const STARTERS: &[&str] = &[
    "minecraft:stone",
    "minecraft:dirt",
    "minecraft:oak_planks",
    "minecraft:cobblestone",
    "minecraft:glass",
    "minecraft:bricks",
];

#[derive(Default, Resource)]
pub struct Palette {
    selected: Option<usize>,
    recent: Vec<usize>,
}

impl Palette {
    pub fn active(&self) -> &BlockState {
        &catalog()[self.active_index()].state
    }

    fn active_index(&self) -> usize {
        self.selected.unwrap_or_else(|| {
            catalog_index(STARTERS[0]).expect("the default material is in the block catalog")
        })
    }

    fn select(&mut self, index: usize) {
        self.selected = Some(index);
        self.recent.retain(|&recent| recent != index);
        self.recent.insert(0, index);
        self.recent.truncate(RECENT_LIMIT);
    }

    fn results(&self, query: &str, lang: Language) -> Vec<usize> {
        let query = fold(query.trim());
        if query.is_empty() {
            return if self.recent.is_empty() {
                STARTERS.iter().filter_map(|id| catalog_index(id)).collect()
            } else {
                self.recent.clone()
            };
        }
        let words: Vec<_> = query.split_whitespace().collect();
        let mut results: Vec<_> = catalog()
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| {
                let name = entry.name(lang);
                let rank = entry.rank(&query, &words, lang)?;
                Some((rank, name.chars().count(), name, index))
            })
            .collect();
        results.sort_unstable();
        results.into_iter().map(|(_, _, _, index)| index).collect()
    }
}

#[derive(Default)]
pub struct Search {
    query: String,
    selected: usize,
    offset: f32,
    opened: bool,
}

impl Search {
    pub fn show(&mut self, ctx: &egui::Context, lang: Language, palette: &mut Palette) -> bool {
        let field_id = Id::new("block-palette-query");
        let (escape, enter, direction) = ctx.input_mut(|input| {
            let escape = input.consume_key(Modifiers::NONE, Key::Escape);
            let enter = input.consume_key(Modifiers::NONE, Key::Enter);
            let down = input.count_and_consume_key(Modifiers::NONE, Key::ArrowDown);
            let up = input.count_and_consume_key(Modifiers::NONE, Key::ArrowUp);
            (escape, enter, down as isize - up as isize)
        });
        if escape {
            self.dismiss(ctx, field_id);
            return false;
        }
        ctx.memory_mut(|memory| {
            if !memory.has_focus(field_id) {
                memory.request_focus(field_id);
            }
        });

        let width = (ctx.content_rect().width() - 48.0).clamp(128.0, 478.0);
        let height = (ctx.content_rect().height() - 192.0).clamp(48.0, 290.0);
        let modal = egui::Modal::new(Id::new("block-palette"))
            .backdrop_color(Color32::from_black_alpha(110))
            .frame(egui::Frame::window(&ctx.style()).inner_margin(egui::Margin::same(14)))
            .show(ctx, |ui| {
                ui.set_width(width);
                ui.heading(t(lang, Copy::Title));
                ui.add(
                    egui::Label::new(
                        RichText::new(format!(
                            "{} · {}",
                            t(lang, Copy::Active),
                            block_name(palette.active(), lang)
                        ))
                        .small()
                        .weak(),
                    )
                    .truncate(),
                );
                ui.add_space(2.0);
                let field = ui.add(
                    egui::TextEdit::singleline(&mut self.query)
                        .id(field_id)
                        .hint_text(t(lang, Copy::Hint))
                        .desired_width(f32::INFINITY)
                        .char_limit(128)
                        .margin(egui::Margin::symmetric(9, 7)),
                );
                let reset_scroll = field.changed() || !self.opened;
                if reset_scroll {
                    self.selected = 0;
                }
                self.opened = true;

                let results = palette.results(&self.query, lang);
                let last = results.len().saturating_sub(1);
                self.selected = self.selected.saturating_add_signed(direction).min(last);

                if self.query.trim().is_empty() {
                    let section = if palette.recent.is_empty() {
                        Copy::Suggested
                    } else {
                        Copy::Recent
                    };
                    ui.label(RichText::new(t(lang, section)).small().weak());
                }

                let row_stride = ROW_HEIGHT + ui.spacing().item_spacing.y;
                let mut scroll = egui::ScrollArea::vertical()
                    .id_salt("block-palette-results")
                    .auto_shrink([false, false])
                    .max_height(height);
                if reset_scroll || direction != 0 {
                    let row_top = self.selected as f32 * row_stride;
                    let offset = if reset_scroll { 0.0 } else { self.offset };
                    let offset = offset.clamp((row_top + ROW_HEIGHT - height).max(0.0), row_top);
                    scroll = scroll.vertical_scroll_offset(offset);
                }
                let response = scroll.show_rows(ui, ROW_HEIGHT, results.len(), |ui, rows| {
                    if results.is_empty() {
                        ui.label(RichText::new(t(lang, Copy::Empty)).weak());
                    }
                    let mut clicked = None;
                    for row in rows {
                        let index = results[row];
                        let response = result_row(
                            ui,
                            &catalog()[index],
                            lang,
                            row == self.selected,
                            index == palette.active_index(),
                        );
                        if response.clicked() {
                            clicked = Some(index);
                        }
                    }
                    clicked
                });
                self.offset = response.state.offset.y;
                ui.separator();
                ui.label(RichText::new(t(lang, Copy::Keys)).small().weak());
                response
                    .inner
                    .or_else(|| enter.then(|| results.get(self.selected).copied()).flatten())
            });

        if let Some(chosen) = modal.inner {
            palette.select(chosen);
            self.dismiss(ctx, field_id);
            return false;
        }
        if modal.should_close() {
            self.dismiss(ctx, field_id);
            return false;
        }
        true
    }

    fn dismiss(&mut self, ctx: &egui::Context, field_id: Id) {
        ctx.memory_mut(|memory| memory.surrender_focus(field_id));
        *self = Self::default();
    }
}

fn result_row(
    ui: &mut Ui,
    entry: &Entry,
    lang: Language,
    selected: bool,
    active: bool,
) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::click());
    response.widget_info(|| {
        WidgetInfo::selected(WidgetType::SelectableLabel, true, active, entry.name(lang))
    });
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let visuals = ui.visuals();
    let painter = ui.painter_at(rect);
    let accent = visuals.selection.stroke.color;
    if selected {
        painter.rect_filled(rect, 4, visuals.selection.bg_fill);
        painter.rect_filled(
            Rect::from_min_size(rect.min, vec2(2.0, rect.height())),
            1,
            accent,
        );
    } else if response.hovered() {
        painter.rect_filled(rect, 4, visuals.widgets.hovered.weak_bg_fill);
    }
    let swatch =
        Rect::from_center_size(pos2(rect.left() + 20.0, rect.center().y), vec2(19.0, 19.0));
    painter.rect_filled(swatch, 2, entry.swatch);
    painter.rect_stroke(
        swatch,
        2,
        Stroke::new(1.0_f32, Color32::from_white_alpha(45)),
        egui::StrokeKind::Inside,
    );
    let names = Rect::from_min_max(rect.min + vec2(37.0, 0.0), rect.max - vec2(24.0, 0.0));
    let labels = painter.with_clip_rect(names);
    labels.text(
        pos2(names.left(), rect.top() + 10.0),
        Align2::LEFT_CENTER,
        entry.name(lang),
        FontId::proportional(14.0),
        visuals.text_color(),
    );
    labels.text(
        pos2(names.left(), rect.top() + 27.0),
        Align2::LEFT_CENTER,
        &entry.state.name,
        FontId::monospace(11.0),
        visuals.weak_text_color(),
    );
    if active {
        painter.circle_filled(pos2(rect.right() - 12.0, rect.center().y), 3.0, accent);
    }
    response.on_hover_text(&entry.state.name)
}

struct Entry {
    state: BlockState,
    spanish: &'static str,
    english: String,
    spanish_folded: String,
    english_folded: String,
    swatch: Color32,
}

impl Entry {
    fn name(&self, lang: Language) -> &str {
        match lang {
            Language::Spanish => self.spanish,
            Language::English => &self.english,
        }
    }

    fn rank(&self, query: &str, words: &[&str], lang: Language) -> Option<u8> {
        let id = &self.state.name;
        let local_id = id.trim_start_matches("minecraft:");
        let name = match lang {
            Language::Spanish => &self.spanish_folded,
            Language::English => &self.english_folded,
        };
        if id == query || local_id == query || name == query {
            return Some(0);
        }
        if name.starts_with(query) || id.starts_with(query) || local_id.starts_with(query) {
            return Some(1);
        }
        if name.contains(query) || local_id.contains(query) {
            return Some(2);
        }
        words
            .iter()
            .all(|word| name.contains(word) || id.contains(word))
            .then_some(3)
    }
}

fn catalog() -> &'static [Entry] {
    static ENTRIES: OnceLock<Vec<Entry>> = OnceLock::new();
    ENTRIES.get_or_init(|| {
        include_str!("palette/blocks.tsv")
            .lines()
            .filter(|line| !line.starts_with('#') && !line.is_empty())
            .map(|line| {
                let mut columns = line.split('\t');
                let id = columns.next().expect("block ID");
                let spanish = columns.next().expect("Spanish block name");
                let rgb = u32::from_str_radix(columns.next().expect("block swatch"), 16)
                    .expect("block swatch is RGB");
                let english = english_name(id);
                let properties = columns
                    .next()
                    .into_iter()
                    .flat_map(|properties| properties.split(','))
                    .map(|property| {
                        let (name, value) = property.split_once('=').expect("block state property");
                        (name.to_owned(), value.to_owned())
                    })
                    .collect();
                Entry {
                    state: BlockState {
                        name: format!("minecraft:{id}"),
                        properties,
                    },
                    spanish,
                    spanish_folded: fold(spanish),
                    english_folded: fold(&english),
                    english,
                    swatch: Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8),
                }
            })
            .collect()
    })
}

fn catalog_index(id: &str) -> Option<usize> {
    catalog()
        .binary_search_by_key(&id, |entry| entry.state.name.as_str())
        .ok()
}

pub fn block_color(id: &str) -> Option<[u8; 3]> {
    let [red, green, blue, _] = catalog()[catalog_index(id)?].swatch.to_array();
    Some([red, green, blue])
}

pub fn block_name(state: &BlockState, lang: Language) -> String {
    if let Some(index) = catalog_index(&state.name) {
        return catalog()[index].name(lang).to_owned();
    }
    let Some(id) = state.name.strip_prefix("minecraft:") else {
        return state.name.clone();
    };
    if lang == Language::Spanish {
        match id {
            "air" => return "Aire".into(),
            "cave_air" => return "Aire de cueva".into(),
            "void_air" => return "Aire del vacío".into(),
            "water" => return "Agua".into(),
            "lava" => return "Lava".into(),
            "sand" => return "Arena".into(),
            "red_sand" => return "Arena roja".into(),
            "gravel" => return "Grava".into(),
            "chest" => return "Cofre".into(),
            "trapped_chest" => return "Cofre trampa".into(),
            _ => {}
        }
        for (suffix, label) in [
            ("_wall_hanging_sign", "Cartel colgante"),
            ("_hanging_sign", "Cartel colgante"),
            ("_wall_sign", "Cartel"),
            ("_sign", "Cartel"),
        ] {
            if let Some(wood) = id.strip_suffix(suffix) {
                let planks = format!("minecraft:{wood}_planks");
                if let Some(index) = catalog_index(&planks) {
                    let material = catalog()[index].spanish.trim_start_matches("Tablones ");
                    return format!("{label} {material}");
                }
            }
        }
    }
    english_name(id)
}

fn english_name(id: &str) -> String {
    match id {
        "jack_o_lantern" => return "Jack o'Lantern".into(),
        "lapis_block" => return "Block of Lapis Lazuli".into(),
        "lapis_ore" => return "Lapis Lazuli Ore".into(),
        "deepslate_lapis_ore" => return "Deepslate Lapis Lazuli Ore".into(),
        _ => {}
    }
    let name = id
        .split('_')
        .map(|word| {
            let mut chars = word.chars();
            chars
                .next()
                .into_iter()
                .flat_map(char::to_uppercase)
                .chain(chars)
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join(" ");
    match id {
        "coal_block" | "iron_block" | "gold_block" | "diamond_block" | "emerald_block"
        | "netherite_block" | "copper_block" | "amethyst_block" | "raw_iron_block"
        | "raw_gold_block" | "raw_copper_block" => {
            format!("Block of {}", name.trim_end_matches(" Block"))
        }
        _ => name,
    }
}

fn fold(text: &str) -> String {
    text.chars()
        .flat_map(char::to_lowercase)
        .map(|ch| match ch {
            'á' | 'ä' => 'a',
            'é' | 'ë' => 'e',
            'í' | 'ï' => 'i',
            'ó' | 'ö' => 'o',
            'ú' | 'ü' => 'u',
            'ñ' => 'n',
            _ => ch,
        })
        .collect()
}

#[derive(Clone, Copy)]
enum Copy {
    Title,
    Active,
    Hint,
    Recent,
    Suggested,
    Empty,
    Keys,
}

fn t(lang: Language, key: Copy) -> &'static str {
    let (es, en) = match key {
        Copy::Title => ("Paleta de bloques", "Block palette"),
        Copy::Active => ("Material activo", "Active material"),
        Copy::Hint => ("Buscar por nombre o ID…", "Search by name or ID…"),
        Copy::Recent => ("Bloques recientes", "Recent blocks"),
        Copy::Suggested => ("Materiales sugeridos", "Suggested materials"),
        Copy::Empty => ("No se encontraron bloques.", "No matching blocks."),
        Copy::Keys => (
            "↑ ↓ Navegar   Enter Elegir   Esc Cerrar",
            "↑ ↓ Navigate   Enter Select   Esc Close",
        ),
    };
    match lang {
        Language::Spanish => es,
        Language::English => en,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_translated_names_and_prioritizes_exact_ids() {
        let palette = Palette::default();
        let first = |query, lang| &catalog()[palette.results(query, lang)[0]].state.name;
        assert_eq!(
            first("tablones roble", Language::Spanish),
            "minecraft:oak_planks"
        );
        assert_eq!(
            first("minecraft:oak_planks", Language::Spanish),
            "minecraft:oak_planks"
        );
        assert_eq!(
            first("oak planks", Language::English),
            "minecraft:oak_planks"
        );
        assert_eq!(first("cesped", Language::Spanish), "minecraft:grass_block");
        assert_eq!(
            first("mosaico bambu", Language::Spanish),
            "minecraft:bamboo_mosaic"
        );
    }

    #[test]
    fn selection_keeps_default_state_and_recent_materials_across_languages() {
        let mut palette = Palette::default();
        assert_eq!(palette.active().name, "minecraft:stone");
        let stairs = catalog_index("minecraft:oak_stairs").unwrap();
        let slab = catalog_index("minecraft:oak_slab").unwrap();
        palette.select(stairs);
        palette.select(slab);
        palette.select(stairs);
        assert_eq!(palette.results("", Language::Spanish), [stairs, slab]);
        assert_eq!(palette.results("", Language::English), [stairs, slab]);
        assert_eq!(
            palette
                .active()
                .properties
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str()))
                .collect::<Vec<_>>(),
            [
                ("facing", "north"),
                ("half", "bottom"),
                ("shape", "straight"),
                ("waterlogged", "false")
            ]
        );
        assert_eq!(
            block_name(palette.active(), Language::Spanish),
            "Escaleras de roble"
        );
        assert_eq!(
            block_name(palette.active(), Language::English),
            "Oak Stairs"
        );
    }

    #[test]
    fn catalog_is_sorted_and_excludes_unsupported_materials() {
        assert!(catalog().len() > 400);
        assert!(
            catalog()
                .windows(2)
                .all(|pair| pair[0].state.name < pair[1].state.name)
        );
        for excluded in [
            "air",
            "water",
            "copper_door",
            "chest",
            "oak_sign",
            "sand",
            "bell",
        ] {
            assert!(catalog_index(&format!("minecraft:{excluded}")).is_none());
        }
        assert!(catalog().iter().all(|entry| {
            !entry.state.is_air()
                && entry
                    .state
                    .properties
                    .get("waterlogged")
                    .is_none_or(|value| value == "false")
        }));
    }

    #[test]
    fn overlay_captures_typing_arrows_and_escape_without_changing_the_active_block() {
        let ctx = egui::Context::default();
        let mut search = Search::default();
        let mut palette = Palette::default();
        let mut frame = |events: Vec<egui::Event>| {
            let mut open = false;
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(900.0, 640.0))),
                    events,
                    ..Default::default()
                },
                |ctx| open = search.show(ctx, Language::Spanish, &mut palette),
            );
            (
                open,
                search.selected,
                search.query.clone(),
                palette.active().name.clone(),
            )
        };
        let key = |key| egui::Event::Key {
            key,
            physical_key: Some(key),
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        };

        assert!(frame(Vec::new()).0);
        let browsing = frame(vec![egui::Event::Text("roble".into()), key(Key::ArrowDown)]);
        assert_eq!(
            browsing,
            (true, 1, "roble".into(), "minecraft:stone".into())
        );
        assert_eq!(
            frame(vec![key(Key::Escape)]),
            (false, 0, String::new(), "minecraft:stone".into())
        );
        assert!(frame(Vec::new()).0);
        let choosing = frame(vec![
            egui::Event::Text("minecraft:oak_planks".into()),
            key(Key::Enter),
        ]);
        assert_eq!(
            choosing,
            (false, 0, String::new(), "minecraft:oak_planks".into())
        );
        assert!(!ctx.memory(|memory| memory.has_focus(Id::new("block-palette-query"))));
    }
}
