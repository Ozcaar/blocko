use std::{
    path::PathBuf,
    sync::{Arc, OnceLock},
};

use bevy_egui::egui::{
    self, Color32, FontData, FontDefinitions, FontFamily, FontId, Painter, Pos2, Rect, Response,
    Sense, Shape, Stroke, StrokeKind, TextStyle, Ui, WidgetInfo, WidgetType, pos2, vec2,
};

pub(super) const BAR: Color32 = Color32::from_rgb(19, 23, 31);
pub(super) const PANEL: Color32 = Color32::from_rgb(26, 32, 43);
pub(super) const TOOLBAR: Color32 = Color32::from_rgb(33, 40, 53);
pub(super) const FIELD: Color32 = Color32::from_rgb(20, 25, 35);
pub(super) const LINE: Color32 = Color32::from_rgb(52, 61, 77);
pub(super) const TEXT: Color32 = Color32::from_rgb(231, 236, 244);
pub(super) const MUTED: Color32 = Color32::from_rgb(166, 178, 197);
pub(super) const ACCENT: Color32 = Color32::from_rgb(111, 206, 193);
pub(super) const ALERT: Color32 = Color32::from_rgb(245, 186, 115);
const SELECTED: Color32 = Color32::from_rgb(43, 69, 76);
const HOVER: Color32 = Color32::from_rgb(50, 61, 77);
const PRESSED: Color32 = Color32::from_rgb(60, 73, 90);
const DIM: Color32 = Color32::from_rgb(119, 132, 154);
const MODE_SELECTED: Color32 = Color32::from_rgb(31, 44, 51);
pub(super) const MODE_RAIL_MARGIN: i8 = 7;

pub(super) fn apply(ctx: &egui::Context) {
    static FONTS: OnceLock<FontDefinitions> = OnceLock::new();
    ctx.set_fonts(FONTS.get_or_init(fonts).clone());
    let mut style = egui::Style::default();
    style.text_styles.extend([
        (TextStyle::Small, FontId::proportional(12.0)),
        (TextStyle::Body, FontId::proportional(14.0)),
        (TextStyle::Button, FontId::proportional(13.0)),
        (TextStyle::Heading, strong(18.0)),
        (TextStyle::Monospace, FontId::monospace(12.0)),
    ]);
    style.spacing.item_spacing = vec2(7.0, 6.0);
    style.spacing.interact_size = vec2(28.0, 26.0);
    style.spacing.button_padding = vec2(8.0, 4.0);
    style.spacing.menu_margin = egui::Margin::same(7);
    style.spacing.indent = 14.0;

    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = PANEL;
    visuals.window_fill = TOOLBAR;
    visuals.window_stroke = Stroke::new(1.0_f32, LINE);
    visuals.window_corner_radius = 8.into();
    visuals.extreme_bg_color = FIELD;
    visuals.faint_bg_color = FIELD;
    visuals.error_fg_color = ALERT;
    visuals.warn_fg_color = ALERT;
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, LINE);
    visuals.widgets.noninteractive.fg_stroke.color = TEXT;
    for (widget, fill, stroke, text) in [
        (&mut visuals.widgets.inactive, TOOLBAR, LINE, TEXT),
        (&mut visuals.widgets.hovered, HOVER, DIM, TEXT),
        (&mut visuals.widgets.active, PRESSED, ACCENT, TEXT),
        (&mut visuals.widgets.open, HOVER, LINE, TEXT),
    ] {
        widget.bg_fill = fill;
        widget.weak_bg_fill = fill;
        widget.bg_stroke = Stroke::new(1.0_f32, stroke);
        widget.fg_stroke.color = text;
        widget.corner_radius = 5.into();
        widget.expansion = 0.0;
    }
    visuals.selection.bg_fill = SELECTED;
    visuals.selection.stroke = Stroke::new(1.0_f32, ACCENT);
    style.visuals = visuals;
    ctx.set_style(style);
}

fn fonts() -> FontDefinitions {
    let mut fonts = FontDefinitions::default();
    let mut regular = fonts.families[&FontFamily::Proportional].clone();
    let mut semibold = regular.clone();
    if let Some(directory) = std::env::var_os("WINDIR").map(PathBuf::from) {
        for (name, file, stack) in [
            ("Segoe UI", "segoeui.ttf", &mut regular),
            ("Segoe UI Semibold", "seguisb.ttf", &mut semibold),
        ] {
            if let Ok(bytes) = std::fs::read(directory.join("Fonts").join(file)) {
                fonts
                    .font_data
                    .insert(name.into(), Arc::new(FontData::from_owned(bytes)));
                stack.insert(0, name.into());
            }
        }
    }
    fonts.families.insert(FontFamily::Proportional, regular);
    fonts
        .families
        .insert(FontFamily::Name("semibold".into()), semibold);
    fonts
}

pub(super) fn strong(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("semibold".into()))
}

pub(super) fn headline(ui: &mut Ui, text: &str, size: f32) -> Response {
    ui.label(egui::RichText::new(text).font(strong(size)).color(TEXT))
}

pub(super) fn card() -> egui::Frame {
    egui::Frame::new()
        .fill(FIELD)
        .stroke(Stroke::new(1.0_f32, LINE))
        .corner_radius(8)
        .inner_margin(egui::Margin::same(10))
}

#[derive(Clone, Copy)]
pub(super) enum Icon {
    Cube,
    Folder,
    Plus,
    Close,
    Pointer,
    Selection,
    Clipboard,
    Target,
    Pin,
    Trash,
    Inspect,
    Pencil,
    Layers,
}

impl Icon {
    pub(super) fn paint(self, painter: &Painter, rect: Rect, color: Color32) {
        let p = |x: f32, y: f32| rect.min + vec2(x * rect.width() / 16.0, y * rect.height() / 16.0);
        let stroke = Stroke::new((rect.width() * 0.085).max(1.12), color);
        let path = |points: &[[f32; 2]]| {
            painter.add(Shape::line(
                points.iter().map(|&[x, y]| p(x, y)).collect(),
                stroke,
            ));
        };
        let outline = |a: Pos2, b: Pos2, radius| {
            painter.rect_stroke(Rect::from_min_max(a, b), radius, stroke, StrokeKind::Inside);
        };
        match self {
            Self::Cube => {
                path(&[
                    [8., 1.],
                    [14., 4.5],
                    [14., 11.5],
                    [8., 15.],
                    [2., 11.5],
                    [2., 4.5],
                    [8., 1.],
                ]);
                path(&[[2., 4.5], [8., 8.], [14., 4.5]]);
                path(&[[8., 8.], [8., 15.]]);
            }
            Self::Folder => path(&[
                [1., 12.5],
                [1., 3.],
                [6., 3.],
                [7.5, 5.],
                [15., 5.],
                [14., 12.5],
                [1., 12.5],
            ]),
            Self::Plus => {
                path(&[[3., 8.], [13., 8.]]);
                path(&[[8., 3.], [8., 13.]]);
            }
            Self::Close => {
                path(&[[4., 4.], [12., 12.]]);
                path(&[[12., 4.], [4., 12.]]);
            }
            Self::Pointer => path(&[[3., 1.5], [13., 9.], [8.5, 9.], [6., 13.5], [3., 1.5]]),
            Self::Selection => {
                for (x, y, sx, sy) in [
                    (2., 2., 1., 1.),
                    (14., 2., -1., 1.),
                    (2., 14., 1., -1.),
                    (14., 14., -1., -1.),
                ] {
                    path(&[[x + 4. * sx, y], [x, y], [x, y + 4. * sy]]);
                }
            }
            Self::Clipboard => {
                path(&[
                    [5., 3.],
                    [3., 3.],
                    [3., 14.],
                    [13., 14.],
                    [13., 3.],
                    [11., 3.],
                ]);
                outline(p(5., 1.5), p(11., 5.), 1.0_f32);
                path(&[[6., 8.], [10., 8.]]);
                path(&[[6., 11.], [10., 11.]]);
            }
            Self::Inspect => {
                painter.circle_stroke(p(6.5, 6.5), rect.width() * 0.27, stroke);
                path(&[[10., 10.], [14.5, 14.5]]);
            }
            Self::Pencil => {
                path(&[
                    [2.5, 13.5],
                    [3.5, 9.5],
                    [11., 2.],
                    [14., 5.],
                    [6.5, 12.5],
                    [2.5, 13.5],
                ]);
                path(&[[9.5, 3.5], [12.5, 6.5]]);
                path(&[[3.5, 9.5], [6.5, 12.5]]);
            }
            Self::Target => {
                painter.circle_stroke(p(8., 8.), rect.width() * 0.30, stroke);
                path(&[[8., 1.], [8., 5.]]);
                path(&[[8., 11.], [8., 15.]]);
                path(&[[1., 8.], [5., 8.]]);
                path(&[[11., 8.], [15., 8.]]);
            }
            Self::Pin => {
                path(&[
                    [4., 2.],
                    [12., 2.],
                    [10.5, 4.],
                    [10.5, 7.],
                    [12., 9.],
                    [4., 9.],
                    [5.5, 7.],
                    [5.5, 4.],
                    [4., 2.],
                ]);
                path(&[[8., 9.], [8., 14.]]);
            }
            Self::Trash => {
                path(&[[2., 4.], [14., 4.]]);
                path(&[[6., 4.], [6., 2.], [10., 2.], [10., 4.]]);
                path(&[[4., 4.], [5., 14.], [11., 14.], [12., 4.]]);
            }
            Self::Layers => {
                path(&[[1., 5.], [8., 1.5], [15., 5.], [8., 8.5], [1., 5.]]);
                path(&[[1., 8.], [8., 11.5], [15., 8.]]);
                path(&[[1., 11.], [8., 14.5], [15., 11.]]);
            }
        }
    }
}

#[derive(Clone, Copy)]
pub(super) enum ButtonStyle {
    Primary,
    Tool,
    Toggle(bool),
    Mode(bool),
    Quiet,
}

pub(super) fn button(
    ui: &mut Ui,
    icon: Option<Icon>,
    label: &str,
    accessible_name: &str,
    style: ButtonStyle,
    enabled: bool,
) -> Response {
    let enabled = enabled && ui.is_enabled();
    let galley = ui
        .painter()
        .layout_no_wrap(label.into(), strong(13.0), Color32::PLACEHOLDER);
    let gap = if icon.is_some() && !label.is_empty() {
        6.0
    } else {
        0.0
    };
    let mode = matches!(style, ButtonStyle::Mode(_));
    let icon_width = if icon.is_none() {
        0.0
    } else if mode {
        20.0
    } else {
        15.0
    };
    let size = if mode {
        vec2(38.0, 38.0)
    } else {
        vec2((galley.size().x + gap + icon_width + 16.0).max(27.0), 28.0)
    };
    let mut response = ui.add_enabled(enabled, |ui: &mut Ui| {
        ui.allocate_exact_size(size, Sense::click()).1
    });
    let selected = matches!(style, ButtonStyle::Toggle(true) | ButtonStyle::Mode(true));
    response.widget_info(|| {
        if mode || matches!(style, ButtonStyle::Toggle(_)) {
            WidgetInfo::selected(
                WidgetType::SelectableLabel,
                enabled,
                selected,
                accessible_name,
            )
        } else {
            WidgetInfo::labeled(WidgetType::Button, enabled, accessible_name)
        }
    });
    let primary = matches!(style, ButtonStyle::Primary);
    let fill = match (
        enabled,
        primary,
        response.is_pointer_button_down_on(),
        response.hovered(),
        selected,
        style,
    ) {
        (false, _, _, _, true, ButtonStyle::Mode(_)) => FIELD,
        (false, ..) => Color32::TRANSPARENT,
        (_, true, _, true, _, _) => Color32::from_rgb(149, 225, 211),
        (_, true, ..) => ACCENT,
        (_, _, true, ..) => PRESSED,
        (_, _, _, true, false, ButtonStyle::Mode(_)) => TOOLBAR,
        (_, _, _, true, ..) => HOVER,
        (_, _, _, _, true, ButtonStyle::Mode(_)) => MODE_SELECTED,
        (_, _, _, _, true, _) => SELECTED,
        (_, _, _, _, _, ButtonStyle::Tool) => PANEL,
        _ => Color32::TRANSPARENT,
    };
    let text = if !enabled {
        DIM
    } else if primary {
        BAR
    } else if selected {
        ACCENT
    } else if mode && !response.hovered() && !response.has_focus() {
        MUTED
    } else {
        TEXT
    };
    let focused = response.has_focus() && (enabled || !mode);
    let stroke = if focused || (selected && !mode) {
        Stroke::new(1.0_f32, ACCENT)
    } else if enabled && matches!(style, ButtonStyle::Tool) {
        Stroke::new(1.0_f32, LINE)
    } else {
        Stroke::NONE
    };
    let rect = response.rect;
    let corner_radius = if mode { 0_u8 } else { 6 };
    let painter = ui.painter();
    painter.rect(rect, corner_radius, fill, stroke, StrokeKind::Inside);
    if mode && selected {
        painter.rect_filled(
            Rect::from_min_size(
                pos2(rect.left() - f32::from(MODE_RAIL_MARGIN), rect.top()),
                vec2(2.0, rect.height()),
            ),
            0,
            if enabled {
                ACCENT
            } else {
                ACCENT.gamma_multiply(0.5)
            },
        );
    }
    let mut x = rect.left() + (rect.width() - galley.size().x - gap - icon_width) / 2.0;
    if let Some(icon) = icon {
        icon.paint(
            ui.painter(),
            Rect::from_min_size(
                pos2(x, rect.center().y - icon_width * 0.5),
                vec2(icon_width, icon_width),
            ),
            text,
        );
        x += icon_width + gap;
    }
    ui.painter().galley(
        pos2(x, rect.center().y - galley.size().y / 2.0),
        galley,
        text,
    );
    if !accessible_name.is_empty() && accessible_name != label {
        response = response
            .on_hover_text(accessible_name)
            .on_disabled_hover_text(accessible_name);
    }
    response
}

pub(super) fn empty_illustration(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(vec2(156.0, 96.0), Sense::hover());
    let painter = ui.painter();
    let origin = rect.center();
    for dx in [-2, -1, 0, 1, 2] {
        let x = dx as f32 * 14.0;
        let stroke = Stroke::new(1.0_f32, LINE);
        painter.line_segment(
            [origin + vec2(x - 36.0, 22.0), origin + vec2(x + 36.0, 52.0)],
            stroke,
        );
        painter.line_segment(
            [origin + vec2(x + 36.0, 22.0), origin + vec2(x - 36.0, 52.0)],
            stroke,
        );
    }
    for (offset, width, color) in [
        (vec2(-29.0, 15.0), 28.0, DIM),
        (vec2(28.0, 16.0), 25.0, MUTED),
        (vec2(0.0, -5.0), 56.0, ACCENT),
    ] {
        let tile = Rect::from_center_size(origin + offset, vec2(width, width));
        painter.add(Shape::convex_polygon(
            [
                [0.5, 0.0625],
                [0.875, 0.28],
                [0.875, 0.72],
                [0.5, 0.9375],
                [0.125, 0.72],
                [0.125, 0.28],
            ]
            .map(|[x, y]| tile.min + vec2(x, y) * width)
            .to_vec(),
            PANEL,
            Stroke::NONE,
        ));
        Icon::Cube.paint(painter, tile, color);
    }
}

pub(super) fn tab_decoration(ui: &Ui, rect: Rect, active: bool, hovered: bool) {
    let fill = if active {
        TOOLBAR
    } else if hovered {
        PANEL
    } else {
        BAR
    };
    let p = ui.painter();
    p.rect_filled(
        rect,
        egui::CornerRadius {
            nw: 7,
            ne: 7,
            ..egui::CornerRadius::ZERO
        },
        fill,
    );
    if active {
        p.hline(
            rect.left() + 7.0..=rect.right() - 7.0,
            rect.top() + 1.0,
            Stroke::new(2.0_f32, ACCENT),
        );
    }
}

pub(super) fn tab_text(ui: &Ui, icon_rect: Rect, galley: Arc<egui::Galley>, active: bool) {
    let color = if active { TEXT } else { MUTED };
    Icon::Cube.paint(ui.painter(), icon_rect, if active { ACCENT } else { MUTED });
    ui.painter().galley_with_override_text_color(
        pos2(
            icon_rect.right() + 7.0,
            icon_rect.center().y - galley.size().y / 2.0,
        ),
        galley,
        color,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn styled_buttons_wrap_within_a_narrow_inspector() {
        let context = egui::Context::default();
        apply(&context);
        let _ = context.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(184.0, 150.0))),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let right = ui.max_rect().right();
                    ui.horizontal_wrapped(|ui| {
                        let cursor = button(
                            ui,
                            Some(Icon::Target),
                            "Cursor",
                            "Cursor",
                            ButtonStyle::Toggle(false),
                            true,
                        );
                        let coords = button(
                            ui,
                            Some(Icon::Pin),
                            "Coordinates",
                            "Coordinates",
                            ButtonStyle::Toggle(true),
                            true,
                        );
                        assert!(coords.rect.top() >= cursor.rect.bottom());
                        assert!(coords.rect.right() <= right);
                    });
                });
            },
        );
    }

    #[test]
    fn styled_buttons_preserve_keyboard_activation_and_respect_disabled_state() {
        for (style, icon, label, enabled) in [
            (ButtonStyle::Primary, Icon::Folder, "Abrir", true),
            (ButtonStyle::Primary, Icon::Folder, "Abrir", false),
            (ButtonStyle::Mode(false), Icon::Selection, "", true),
            (ButtonStyle::Mode(true), Icon::Selection, "", false),
        ] {
            let context = egui::Context::default();
            apply(&context);
            let frame = |events: Vec<egui::Event>, focus: bool| {
                let mut clicked = false;
                let _ = context.run(
                    egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(400.0, 300.0))),
                        events,
                        ..Default::default()
                    },
                    |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| {
                            let reply =
                                button(ui, Some(icon), label, "Herramienta", style, enabled);
                            if matches!(style, ButtonStyle::Mode(_)) {
                                assert_eq!(reply.rect.size(), vec2(38.0, 38.0));
                            }
                            if focus {
                                reply.request_focus();
                            }
                            clicked = reply.clicked();
                        });
                    },
                );
                clicked
            };
            assert!(!frame(vec![], true));
            assert_eq!(
                frame(
                    vec![egui::Event::Key {
                        key: egui::Key::Enter,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: egui::Modifiers::NONE,
                    }],
                    false
                ),
                enabled
            );
        }
    }
}
