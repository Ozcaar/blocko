use std::{path::PathBuf, sync::Arc};

use bevy::{
    camera::Viewport,
    prelude::*,
    window::{RawHandleWrapper, WindowPosition},
};
use bevy_egui::{EguiContext, EguiFullOutput, EguiInput, egui};
use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, WindowHandle,
};
#[cfg(windows)]
use raw_window_handle::{RawDisplayHandle, RawWindowHandle};

use crate::{
    actions::{Action, Bindings, Control, EditorContext},
    diagnostics::Diagnostic,
    editor::{
        self, DemoSelection, Destination, OpenNotice, Overlay, PendingOpen, SceneAssets, TabAction,
        TabRequest, View, Workspace,
    },
    flight::Flight,
    i18n::{Language, Text},
    paste::{self, PasteAction, PasteState, PasteSummary},
    placement::{AirMode, Placement},
    selection::{self, Selection},
    structure::{Clipboard, CopyNotice, MAX_HORIZONTAL, MAX_Y, MIN_HORIZONTAL, MIN_Y},
};

mod appearance;

use appearance::{ACCENT, ALERT, BAR, ButtonStyle, FIELD, Icon, LINE, MUTED, PANEL, TEXT, TOOLBAR};

pub fn context() -> EguiContext {
    let mut context = EguiContext::default();
    appearance::apply(context.get_mut());
    context
}

pub struct DialogParent {
    #[cfg(windows)]
    window: std::num::NonZeroIsize,
}

impl DialogParent {
    fn from_window(handle: Option<&RawHandleWrapper>) -> Option<Self> {
        #[cfg(windows)]
        if let RawWindowHandle::Win32(handle) = handle?.get_window_handle() {
            return Some(Self {
                window: handle.hwnd,
            });
        }
        let _ = handle;
        None
    }
}

impl HasWindowHandle for DialogParent {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        #[cfg(windows)]
        {
            let handle =
                RawWindowHandle::Win32(raw_window_handle::Win32WindowHandle::new(self.window));
            // The owner window stays open until the native dialog returns.
            return Ok(unsafe { WindowHandle::borrow_raw(handle) });
        }
        #[cfg(not(windows))]
        Err(HandleError::Unavailable)
    }
}

impl HasDisplayHandle for DialogParent {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        #[cfg(windows)]
        {
            let handle = RawDisplayHandle::Windows(raw_window_handle::WindowsDisplayHandle::new());
            return Ok(unsafe { DisplayHandle::borrow_raw(handle) });
        }
        #[cfg(not(windows))]
        Err(HandleError::Unavailable)
    }
}

pub fn draw(
    mut commands: Commands,
    mut windows: Query<(&Window, &mut Workspace, Option<&RawHandleWrapper>)>,
    mut views: ParamSet<(Query<&View>, Query<&mut View>)>,
    mut overlays: Query<(
        &Overlay,
        &mut Camera,
        &mut EguiContext,
        &mut EguiInput,
        &mut EguiFullOutput,
    )>,
    mut cameras: Query<(&mut Camera, &Transform, &Flight), Without<Overlay>>,
    mut demo_selection: ResMut<DemoSelection>,
    mut clipboard: ResMut<Clipboard>,
    mut language: ResMut<Language>,
    mut tab_requests: MessageWriter<TabRequest>,
    assets: Res<SceneAssets>,
    bindings: Res<Bindings>,
) {
    let mut demo_requested = false;
    for (overlay, mut overlay_camera, mut egui, mut input, mut output) in &mut overlays {
        let window_id = overlay.0;
        let Ok((window, mut workspace, handle)) = windows.get_mut(window_id) else {
            continue;
        };
        let tabs = {
            let read = views.p0();
            workspace
                .tabs
                .iter()
                .filter_map(|&tab| {
                    let stream = read.get(tab).ok()?.stream.as_ref()?;
                    Some((tab, stream.name.clone(), stream.path().to_owned()))
                })
                .collect::<Vec<_>>()
        };
        let document = workspace.active;
        let mut docs = views.p1();
        let Ok(mut view) = docs.get_mut(document) else {
            continue;
        };
        let Ok((mut camera, transform, flight)) = cameras.get_mut(view.camera) else {
            continue;
        };

        let mut result = WindowActions::default();
        let mut viewport = None;
        let ctx = egui.get_mut();
        let had_focus = input_blocked(ctx);
        let raw = input.take();
        let parent = DialogParent::from_window(handle);
        #[cfg(windows)]
        let raw = {
            let mut raw = raw;
            if had_focus
                && raw.focused
                && let Some(parent) = &parent
            {
                crate::native_clipboard::add_text_events(&mut raw, parent.window.get() as _);
            }
            raw
        };
        let focused = raw.focused && window.focused;
        let pass = ctx.run(raw, |ctx| {
            let actions = window_ui(
                ctx,
                window,
                &mut workspace,
                &mut view,
                &tabs,
                transform.translation,
                flight.captured(),
                &mut demo_selection,
                &clipboard,
                *language,
                &bindings,
            );
            let rect = ctx.available_rect();
            let scale = ctx.pixels_per_point();
            if view.stream.is_some() || view.demo_scene.is_some() {
                viewport = Some((rect, scale));
            }
            if view.selecting && view.preview.is_none() && view.viewport.is_some() {
                draw_corner_labels(
                    ctx,
                    rect,
                    scale,
                    window,
                    &camera,
                    transform,
                    &view.selection,
                );
            }
            workspace.blocks_camera = had_focus || input_blocked(ctx);
            result.merge(actions);
            if !focused
                || workspace.blocks_camera
                || workspace.pending.is_some()
                || view.paste.locks_preview()
                || result.tab.is_some()
                || result.open.is_some()
                || result.paste.is_some()
                || result.language.is_some()
            {
                return;
            }
            let view = &mut *view;
            if !flight.captured()
                && view.viewport.is_some()
                && let Some(point) = ctx
                    .pointer_hover_pos()
                    .filter(|point| rect.contains(*point))
                && let Some(preview) = &mut view.preview
                && preview.moving
                && let Some(stream) = &view.stream
            {
                let hit = selection::cursor_hit(
                    stream,
                    &camera,
                    transform,
                    to_logical(point, scale, window),
                );
                preview.set_cursor_target(hit.map(selection::Hit::adjacent));
            }
            let context = tools_context(&view);
            if let Some(action) = ctx.input(|input| {
                input.events.iter().find_map(|event| {
                    let action = bindings.match_event(context, event)?;
                    let point = match event {
                        egui::Event::PointerButton { pos, .. } => {
                            if !rect.contains(*pos) {
                                return None;
                            }
                            Some(*pos)
                        }
                        _ => None,
                    };
                    (action_enabled(action, &view, &clipboard, flight.captured())
                        || (matches!(action, Action::Place | Action::RepeatPlace)
                            && !flight.captured()
                            && view.preview.is_some()))
                    .then_some((action, point))
                })
            }) {
                result.editor.get_or_insert(action);
            }
        });
        #[cfg(windows)]
        if let Some(parent) = &parent {
            crate::native_clipboard::copy_text(
                &pass.platform_output.commands,
                parent.window.get() as _,
            );
        }
        **output = Some(pass);
        if let (Some((action, point)), Some((_, scale))) = (result.editor, viewport) {
            let hit = point.filter(|_| view.viewport.is_some()).and_then(|point| {
                selection::cursor_hit(
                    view.stream.as_ref()?,
                    &camera,
                    transform,
                    to_logical(point, scale, window),
                )
            });
            execute_action(&mut view, document, &mut clipboard, action, hit);
        }
        update_viewport(&mut view, &mut camera, viewport, window);
        let clear = if viewport.is_some() {
            ClearColorConfig::None
        } else {
            ClearColorConfig::Custom(Color::srgb_u8(FIELD.r(), FIELD.g(), FIELD.b()))
        };
        overlay_camera.clear_color = clear;
        if let Some(selected) = result.language {
            *language = selected;
        }
        if let Some((tab, action)) = result.tab {
            tab_requests.write(TabRequest {
                window: window_id,
                tab,
                action,
            });
            ctx.memory_mut(|memory| {
                if let Some(id) = memory.focused() {
                    memory.surrender_focus(id);
                }
            });
        } else if let Some(destination) = result.open {
            workspace.notice = None;
            workspace.pending = Some(PendingOpen::browse(destination, *language, parent));
        } else if let Some(action) = result.paste {
            paste::apply_action(&mut view, action);
        }
        demo_requested |= result.demo;
    }
    if demo_requested {
        editor::new_demo(
            &mut commands,
            views.p0().iter().map(|view| view.layer),
            &assets,
        );
    }
}

fn draw_corner_labels(
    ctx: &egui::Context,
    rect: egui::Rect,
    scale: f32,
    window: &Window,
    camera: &Camera,
    transform: &Transform,
    selection: &Selection,
) {
    let painter = ctx
        .layer_painter(egui::LayerId::new(
            egui::Order::Middle,
            egui::Id::new("selection corners"),
        ))
        .with_clip_rect(rect);
    let pose = GlobalTransform::from(*transform);
    for (corner, selected) in selection.corners.into_iter().enumerate() {
        let Some(block) = selected else { continue };
        let position = Vec3::from_array(block.map(|axis| axis as f32)) + Vec3::new(0.5, 1.18, 0.5);
        let Ok(point) = camera.world_to_viewport(&pose, position) else {
            continue;
        };
        let point = point * (window.scale_factor() / scale);
        let mut point = egui::pos2(point.x, point.y);
        if !rect.shrink(12.0).contains(point) {
            continue;
        }
        let coincident = selection.corners[0] == selection.corners[1];
        if coincident {
            point.x += if corner == 0 { -10.0 } else { 10.0 };
        }
        let color = corner_color(corner);
        painter.rect_filled(
            egui::Rect::from_center_size(point, egui::vec2(18.0, 18.0)),
            4.0,
            BAR,
        );
        painter.text(
            point,
            egui::Align2::CENTER_CENTER,
            corner_name(corner),
            egui::FontId::monospace(12.0),
            color,
        );
    }
}

fn corner_color(corner: usize) -> egui::Color32 {
    if corner == 0 {
        egui::Color32::from_rgb(51, 209, 255)
    } else {
        ALERT
    }
}

fn input_blocked(ctx: &egui::Context) -> bool {
    ctx.wants_keyboard_input() || egui::Popup::is_any_open(ctx)
}

fn update_viewport(
    view: &mut View,
    camera: &mut Camera,
    viewport: Option<(egui::Rect, f32)>,
    window: &Window,
) {
    view.viewport = viewport.map(|(rect, scale)| {
        Rect::from_corners(
            to_logical(rect.min, scale, window),
            to_logical(rect.max, scale, window),
        )
    });
    camera.is_active = false;
    if let Some((rect, scale)) = viewport {
        let min = to_pixels(rect.min, scale, window);
        let max = to_pixels(rect.max, scale, window).max(min);
        camera.is_active = max.x > min.x && max.y > min.y;
        if camera.is_active {
            camera.viewport = Some(Viewport {
                physical_position: min,
                physical_size: max - min,
                ..default()
            });
        }
    }
}

fn to_logical(point: egui::Pos2, scale: f32, window: &Window) -> Vec2 {
    Vec2::new(point.x, point.y) * (scale / window.scale_factor())
}

fn to_pixels(point: egui::Pos2, scale: f32, window: &Window) -> UVec2 {
    UVec2::new(
        ((point.x * scale).round() as u32).min(window.physical_width()),
        ((point.y * scale).round() as u32).min(window.physical_height()),
    )
}

fn frame(fill: egui::Color32) -> egui::Frame {
    egui::Frame::new()
        .fill(fill)
        .inner_margin(egui::Margin::symmetric(12, 8))
}

#[derive(Default)]
struct WindowActions {
    open: Option<Destination>,
    editor: Option<(Action, Option<egui::Pos2>)>,
    paste: Option<PasteAction>,
    tab: Option<(Entity, TabAction)>,
    language: Option<Language>,
    demo: bool,
}

impl WindowActions {
    fn merge(&mut self, other: Self) {
        self.open = self.open.or(other.open);
        self.editor = self.editor.or(other.editor);
        self.paste = self.paste.or(other.paste);
        self.tab = self.tab.take().or(other.tab);
        self.language = self.language.or(other.language);
        self.demo |= other.demo;
    }
}

fn window_ui(
    ctx: &egui::Context,
    window: &Window,
    workspace: &mut Workspace,
    view: &mut View,
    tabs: &[(Entity, String, PathBuf)],
    position: Vec3,
    captured: bool,
    demo: &mut DemoSelection,
    clipboard: &Clipboard,
    lang: Language,
    bindings: &Bindings,
) -> WindowActions {
    let mut actions = WindowActions::default();
    let busy = workspace.pending.is_some() || view.paste.writes();
    let name = view.stream.as_ref().map(|stream| stream.name.clone());
    menu_bar(ctx, busy, name.is_some(), lang, &mut actions);
    if !tabs.is_empty() {
        tab_bar(ctx, window, workspace, tabs, busy, lang, &mut actions);
    }
    if name.is_some() || view.demo_scene.is_some() {
        tool_bar(ctx, view, clipboard, busy, captured, lang, &mut actions);
    }
    status_bar(
        ctx, workspace, view, position, captured, clipboard, lang, bindings,
    );

    if name.is_none() && view.demo_scene.is_none() {
        empty_workspace(ctx, workspace, busy, lang, &mut actions);
    } else {
        let width = (ctx.content_rect().width() * 0.235).clamp(194.0, 298.0);
        egui::SidePanel::right("inspector")
            .exact_width(width)
            .resizable(false)
            .frame(frame(PANEL).inner_margin(egui::Margin::symmetric(13, 14)))
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt(workspace.active)
                    .show(ui, |ui| {
                        ui.push_id(workspace.active, |ui| {
                            opening_notice(ui, workspace, lang);
                            if let Some(name) = &name {
                                inspector(
                                    ui,
                                    view,
                                    name,
                                    position,
                                    busy,
                                    clipboard,
                                    lang,
                                    &mut actions,
                                );
                            } else {
                                ui.heading(lang.text(Text::Demo));
                                if ui.button(lang.text(Text::DemoWindow)).clicked() {
                                    actions.demo = true;
                                }
                                if ui.button(lang.text(Text::DemoCopy)).clicked() {
                                    demo.0 = Some(view.layer);
                                }
                                ui.label(demo.0.map_or_else(
                                    || lang.text(Text::DemoNoSelection).into(),
                                    |layer| lang.format(Text::DemoSelection, &[&layer]),
                                ));
                            }
                        });
                    });
            });
    }
    actions
}

fn empty_workspace(
    ctx: &egui::Context,
    workspace: &mut Workspace,
    busy: bool,
    lang: Language,
    actions: &mut WindowActions,
) {
    egui::CentralPanel::default()
        .frame(frame(FIELD).inner_margin(egui::Margin::same(20)))
        .show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(((ui.available_height() - 338.0) * 0.43).max(8.0));
                    let width = ui.available_width().min(396.0);
                    ui.allocate_ui_with_layout(
                        egui::vec2(width, 0.0),
                        egui::Layout::top_down(egui::Align::Center),
                        |ui| {
                            appearance::card()
                                .fill(PANEL)
                                .inner_margin(egui::Margin::same(23))
                                .show(ui, |ui| {
                                    ui.set_width(width - 48.0);
                                    appearance::empty_illustration(ui);
                                    ui.add_space(9.0);
                                    appearance::headline(ui, lang.text(Text::EmptyTitle), 24.0);
                                    ui.label(
                                        egui::RichText::new(lang.text(Text::EmptyHelp))
                                            .color(MUTED),
                                    );
                                    ui.add_space(9.0);
                                    let label = lang.text(Text::OpenWorld);
                                    if appearance::button(
                                        ui,
                                        Some(Icon::Folder),
                                        label,
                                        label,
                                        ButtonStyle::Primary,
                                        !busy,
                                    )
                                    .clicked()
                                    {
                                        actions.open = Some(Destination::Tab);
                                    }
                                    if workspace.notice.is_some() {
                                        ui.add_space(7.0);
                                        opening_notice(ui, workspace, lang);
                                    }
                                });
                        },
                    );
                });
            });
        });
}

fn menu_bar(
    ctx: &egui::Context,
    busy: bool,
    world_open: bool,
    lang: Language,
    actions: &mut WindowActions,
) {
    egui::TopBottomPanel::top("menu")
        .frame(frame(BAR).inner_margin(egui::Margin::symmetric(12, 6)))
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                let (logo, _) =
                    ui.allocate_exact_size(egui::vec2(27.0, 27.0), egui::Sense::hover());
                ui.painter().rect_filled(logo, 7, TOOLBAR);
                Icon::Cube.paint(ui.painter(), logo.shrink(5.0), ACCENT);
                appearance::headline(ui, "Blocko", 18.0);
                ui.add_space(6.0);
                ui.separator();
                ui.add_enabled_ui(!busy, |ui| {
                    ui.menu_button(lang.text(Text::WorldMenu), |ui| {
                        let commands = [
                            (true, Text::OpenWorld, Destination::Tab),
                            (world_open, Text::OpenReplace, Destination::Replace),
                            (true, Text::OpenWindow, Destination::NewWindow),
                        ];
                        for (enabled, text, destination) in commands {
                            if ui
                                .add_enabled(enabled, egui::Button::new(lang.text(text)))
                                .clicked()
                            {
                                actions.open = Some(destination);
                                ui.close();
                            }
                        }
                    });
                });
                ui.menu_button(lang.text(Text::Language), |ui| {
                    for language in Language::ALL {
                        if ui
                            .selectable_label(language == lang, language.name())
                            .clicked()
                        {
                            actions.language = Some(language);
                            ui.close();
                        }
                    }
                });
            });
        });
}

fn tool_bar(
    ctx: &egui::Context,
    view: &mut View,
    clipboard: &Clipboard,
    busy: bool,
    captured: bool,
    lang: Language,
    actions: &mut WindowActions,
) {
    egui::TopBottomPanel::top("tools")
        .exact_height(45.0)
        .frame(frame(TOOLBAR).inner_margin(egui::Margin::symmetric(11, 7)))
        .show(ctx, |ui| {
            horizontal_strip(ui, |ui| {
                if view.stream.is_none() {
                    return;
                }
                if view.paste.locks_preview() {
                    ui.label(egui::RichText::new(lang.text(Text::Placement)).color(MUTED));
                } else if let Some(preview) = &mut view.preview {
                    ui.add_enabled_ui(!busy, |ui| {
                        tool_bar_placement(ui, preview, lang, actions);
                    });
                    view.air_mode = preview.mode;
                } else {
                    ui.add_enabled_ui(!busy, |ui| {
                        tool_bar_editing(ui, view, clipboard, captured, lang, actions);
                    });
                }
            });
        });
}

fn tool_bar_editing(
    ui: &mut egui::Ui,
    view: &mut View,
    clipboard: &Clipboard,
    captured: bool,
    lang: Language,
    actions: &mut WindowActions,
) {
    let explore = lang.text(Text::Explore);
    let select = lang.text(Text::Select);
    if appearance::button(
        ui,
        Some(Icon::Pointer),
        explore,
        explore,
        ButtonStyle::Toggle(!view.selecting),
        true,
    )
    .clicked()
    {
        view.selecting = false;
    }
    if appearance::button(
        ui,
        Some(Icon::Selection),
        select,
        select,
        ButtonStyle::Toggle(view.selecting),
        true,
    )
    .clicked()
    {
        view.selecting = true;
    }
    ui.separator();
    if view.selecting {
        if appearance::button(
            ui,
            Some(Icon::Target),
            lang.text(Text::PickBothTool),
            lang.text(Text::PickBoth),
            ButtonStyle::Tool,
            true,
        )
        .clicked()
        {
            arm_pick(&mut view.selection, 0, true);
        }
        if view.selection.picking.is_some() {
            let label = lang.text(Text::CancelMark);
            if appearance::button(
                ui,
                Some(Icon::Close),
                label,
                label,
                ButtonStyle::Quiet,
                true,
            )
            .clicked()
            {
                stop_marking(&mut view.selection);
            }
        } else if appearance::button(
            ui,
            Some(Icon::Trash),
            "",
            lang.text(Text::ClearSelection),
            ButtonStyle::Quiet,
            view.selection.corners.iter().any(Option::is_some),
        )
        .clicked()
        {
            view.selection = Selection::default();
        }
        if appearance::button(
            ui,
            Some(Icon::Copy),
            lang.text(Text::ActionCopy),
            lang.text(Text::CopyButton),
            ButtonStyle::Tool,
            action_enabled(Action::Copy, view, clipboard, captured),
        )
        .clicked()
        {
            actions.editor = Some((Action::Copy, None));
        }
        ui.separator();
    } else {
        stop_marking(&mut view.selection);
    }
    if appearance::button(
        ui,
        Some(Icon::Clipboard),
        lang.text(Text::PreviewTool),
        lang.text(Text::PreviewButton),
        ButtonStyle::Tool,
        action_enabled(Action::StartPaste, view, clipboard, captured),
    )
    .clicked()
    {
        actions.editor = Some((Action::StartPaste, None));
    }
}

fn tool_bar_placement(
    ui: &mut egui::Ui,
    preview: &mut Placement,
    lang: Language,
    actions: &mut WindowActions,
) {
    for (mode, text, help) in [
        (AirMode::Include, Text::IncludeAir, Text::IncludeAirHelp),
        (AirMode::Ignore, Text::IgnoreAir, Text::IgnoreAirHelp),
    ] {
        if appearance::button(
            ui,
            None,
            lang.text(text),
            lang.text(help),
            ButtonStyle::Toggle(preview.mode == mode),
            true,
        )
        .clicked()
        {
            preview.mode = mode;
        }
    }
    ui.separator();
    for (track, icon, key, help) in [
        (true, Icon::Target, Text::FollowTool, Text::Follow),
        (false, Icon::Pin, Text::PreciseTool, Text::Precise),
    ] {
        let label = lang.text(key);
        if appearance::button(
            ui,
            Some(icon),
            label,
            lang.text(help),
            ButtonStyle::Toggle(preview.moving == track),
            true,
        )
        .clicked()
            && preview.moving != track
        {
            preview.moving = track;
            preview.target_valid = !track;
        }
    }
    ui.separator();
    let prepare = lang.text(Text::PrepareTool);
    if appearance::button(
        ui,
        None,
        prepare,
        lang.text(Text::PreparePaste),
        ButtonStyle::Tool,
        preview.target_valid,
    )
    .clicked()
    {
        actions.paste = Some(PasteAction::Prepare);
    }
    if appearance::button(
        ui,
        Some(Icon::Close),
        "",
        lang.text(Text::CancelPreview),
        ButtonStyle::Quiet,
        true,
    )
    .clicked()
    {
        actions.editor = Some((Action::CancelPaste, None));
    }
}

fn horizontal_strip(ui: &mut egui::Ui, contents: impl FnOnce(&mut egui::Ui)) {
    egui::ScrollArea::horizontal()
        .max_height(ui.available_height())
        .auto_shrink([false, true])
        .scroll_source(egui::scroll_area::ScrollSource {
            drag: false,
            ..Default::default()
        })
        .show(ui, |ui| {
            ui.horizontal(contents);
        });
}

fn tab_bar(
    ctx: &egui::Context,
    window: &Window,
    workspace: &mut Workspace,
    tabs: &[(Entity, String, PathBuf)],
    busy: bool,
    lang: Language,
    actions: &mut WindowActions,
) {
    egui::TopBottomPanel::top("tabs")
        .exact_height(38.0)
        .frame(frame(BAR).inner_margin(egui::Margin {
            left: 11,
            right: 11,
            top: 4,
            bottom: 0,
        }))
        .show(ctx, |ui| {
            horizontal_strip(ui, |ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                for (tab, name, path) in tabs {
                    ui.push_id(*tab, |ui| {
                        let active = *tab == workspace.active;
                        let (label, close) = world_tab(ui, name, active, busy, lang);
                        let label = label.on_hover_ui(|ui| {
                            appearance::headline(ui, name, 14.0);
                            ui.label(lang.text(Text::DragTab));
                            ui.label(
                                egui::RichText::new(path.display().to_string())
                                    .monospace()
                                    .color(MUTED),
                            );
                        });
                        if label.clicked() {
                            actions.tab = Some((*tab, TabAction::Activate));
                        }
                        if label.drag_started_by(egui::PointerButton::Primary) {
                            workspace.dragging = Some(*tab);
                        }
                        if close.clicked() {
                            actions.tab = Some((*tab, TabAction::Close));
                        }
                    });
                }
                let label = lang.text(Text::OpenWorld);
                if appearance::button(ui, Some(Icon::Plus), "", label, ButtonStyle::Quiet, !busy)
                    .clicked()
                {
                    actions.open = Some(Destination::Tab);
                }
            });
        });
    if let Some(tab) = workspace.dragging {
        let (released, position) = ctx.input(|input| {
            let release = input.events.iter().rev().find_map(|event| match event {
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    ..
                } => Some(*pos),
                _ => None,
            });
            (
                !input.pointer.primary_down(),
                release.or(input.pointer.latest_pos()),
            )
        });
        if released {
            workspace.dragging = None;
            if !busy && let Some(point) = position.filter(|p| !ctx.viewport_rect().contains(*p)) {
                let position = match window.position {
                    WindowPosition::At(origin) => Some(
                        origin
                            + IVec2::new(
                                (point.x * ctx.pixels_per_point()) as i32 - 36,
                                (point.y * ctx.pixels_per_point()) as i32 - 18,
                            ),
                    ),
                    _ => None,
                };
                actions.tab = Some((tab, TabAction::Detach(position)));
            }
        } else if !window.focused {
            workspace.dragging = None;
        }
    }
}

fn world_tab(
    ui: &mut egui::Ui,
    name: &str,
    active: bool,
    busy: bool,
    lang: Language,
) -> (egui::Response, egui::Response) {
    let mut job = egui::text::LayoutJob::simple(
        name.to_owned(),
        appearance::strong(13.0),
        egui::Color32::PLACEHOLDER,
        145.0,
    );
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    let text = ui.painter().layout_job(job);
    let (id, rect) = ui.allocate_space(egui::vec2((text.size().x + 67.0).max(110.0), 32.0));
    let close_rect = egui::Rect::from_center_size(
        egui::pos2(rect.right() - 15.0, rect.center().y),
        egui::vec2(23.0, 23.0),
    );
    let main_rect =
        egui::Rect::from_min_max(rect.min, egui::pos2(close_rect.left(), rect.bottom()));
    let (label, close) = ui
        .add_enabled_ui(!busy, |ui| {
            (
                ui.interact(main_rect, id.with("world"), egui::Sense::click_and_drag()),
                ui.interact(close_rect, id.with("close"), egui::Sense::click()),
            )
        })
        .inner;
    let close_text = lang.text(Text::CloseTab);
    label.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, !busy, active, name)
    });
    close.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, !busy, close_text));
    appearance::tab_decoration(ui, rect, active, label.hovered() || close.hovered());
    if label.has_focus() {
        ui.painter().rect_stroke(
            main_rect.shrink(2.0),
            5,
            egui::Stroke::new(1.0_f32, ACCENT),
            egui::StrokeKind::Inside,
        );
    }
    let icon_rect = egui::Rect::from_center_size(
        egui::pos2(rect.left() + 18.0, rect.center().y),
        egui::vec2(15.0, 15.0),
    );
    appearance::tab_text(ui, icon_rect, text, active);
    if close.hovered() || close.has_focus() {
        ui.painter().rect_filled(close_rect, 5, LINE);
    }
    Icon::Close.paint(
        ui.painter(),
        close_rect.shrink(4.5),
        if close.hovered() { TEXT } else { MUTED },
    );
    (label, close.on_hover_text(close_text))
}

fn inspector(
    ui: &mut egui::Ui,
    view: &mut View,
    name: &str,
    position: Vec3,
    busy: bool,
    clipboard: &Clipboard,
    lang: Language,
    actions: &mut WindowActions,
) {
    world_heading(ui, name);
    ui.add_space(8.0);
    ui.separator();
    ui.add_space(6.0);
    let paste_locked = view.paste.locks_preview();
    let placing = view.preview.is_some() || paste_locked;
    let section = if placing {
        Some(Text::Placement)
    } else if view.selecting {
        Some(Text::Select)
    } else {
        None
    };
    if let Some(section) = section {
        appearance::headline(ui, lang.text(section), 17.0);
        ui.add_space(4.0);
    }
    ui.add_enabled_ui(!busy, |ui| {
        if let Some(action) = paste_status(ui, &view.paste, name, lang) {
            actions.paste.get_or_insert(action);
        }
        if !paste_locked {
            if let Some(preview) = &mut view.preview {
                let recalculating = view
                    .stream
                    .as_ref()
                    .is_some_and(|stream| stream.preview_busy());
                preview_controls(ui, preview, recalculating, lang);
            } else if view.selecting {
                selection_details(ui, &mut view.selection, position, lang);
            }
        }
    });
    if let Some(CopyNotice::Error(error)) = &view.copy_notice {
        diagnostic(ui, &lang.format(Text::CopyFailed, &[&name]), error, lang);
    }
    if section.is_some() || !matches!(view.paste, PasteState::Idle) {
        ui.add_space(8.0);
        ui.separator();
        ui.add_space(6.0);
    }
    egui::CollapsingHeader::new(
        egui::RichText::new(lang.text(Text::Clipboard)).font(appearance::strong(14.0)),
    )
    .id_salt(("clipboard", placing))
    .default_open(!placing)
    .show_unindented(ui, |ui| {
        inspector_card(ui, |ui| clipboard_contents(ui, clipboard, lang));
    });
}

fn inspector_card<R>(ui: &mut egui::Ui, contents: impl FnOnce(&mut egui::Ui) -> R) -> R {
    appearance::card()
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            contents(ui)
        })
        .inner
}

fn world_heading(ui: &mut egui::Ui, name: &str) {
    ui.horizontal(|ui| {
        let (icon, _) = ui.allocate_exact_size(egui::vec2(36.0, 36.0), egui::Sense::hover());
        ui.painter().rect_filled(icon, 8, TOOLBAR);
        Icon::Cube.paint(ui.painter(), icon.shrink(8.0), ACCENT);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            ui.add(
                egui::Label::new(
                    egui::RichText::new(name)
                        .font(appearance::strong(19.0))
                        .color(TEXT),
                )
                .truncate(),
            )
            .on_hover_text(name);
            ui.label(egui::RichText::new("Overworld").small().color(MUTED));
        });
    });
}

fn tools_context(view: &View) -> EditorContext {
    if view.preview.is_some() {
        EditorContext::Place
    } else if view.selecting {
        EditorContext::Select
    } else {
        EditorContext::Explore
    }
}

fn action_enabled(action: Action, view: &View, clipboard: &Clipboard, captured: bool) -> bool {
    if view.stream.is_none() || view.paste.locks_preview() {
        return false;
    }
    match action {
        Action::Copy => {
            !captured
                && !clipboard.copying()
                && view.selection.picking.is_none()
                && view
                    .selection
                    .volume()
                    .is_some_and(|volume| volume.validate().is_ok())
        }
        Action::StartPaste => !captured && clipboard.current.is_some(),
        Action::Place | Action::RepeatPlace => {
            !captured
                && view
                    .preview
                    .as_ref()
                    .is_some_and(|preview| preview.target_valid)
        }
        Action::CancelPaste => view.preview.is_some(),
        Action::MarkCorner => !captured && view.selection.picking.is_some(),
        Action::CancelSelection => view.selection.picking.is_some(),
        _ => false,
    }
}

pub(crate) fn execute_action(
    view: &mut View,
    document: Entity,
    clipboard: &mut Clipboard,
    action: Action,
    hit: Option<selection::Hit>,
) {
    let Some(stream) = &view.stream else { return };
    match action {
        Action::Copy => {
            if let Some(volume) = view.selection.volume() {
                match clipboard.request(document, stream.source(), volume) {
                    Ok(()) => {
                        view.copying = true;
                        view.copy_notice = None;
                        view.preview_cancelled = false;
                        view.paste = PasteState::Idle;
                    }
                    Err(error) => {
                        view.copy_notice = Some(CopyNotice::Error(Diagnostic::from_error(error)))
                    }
                }
            }
        }
        Action::StartPaste => {
            if let Some(source) = &clipboard.current {
                let mut preview = Placement::new(Arc::clone(source), stream.source().spawn);
                preview.mode = view.air_mode;
                preview.moving = true;
                preview.target_valid = false;
                view.preview = Some(preview);
                view.paste = PasteState::Idle;
                view.preview_cancelled = false;
                stop_marking(&mut view.selection);
            }
        }
        Action::CancelPaste => {
            view.preview = None;
            if !matches!(view.paste, PasteState::Saved(_)) {
                view.paste = PasteState::Idle;
            }
            view.preview_cancelled = true;
        }
        Action::Place | Action::RepeatPlace => {
            if let Some(preview) = &mut view.preview
                && preview.moving
            {
                preview.set_cursor_target(hit.map(selection::Hit::adjacent));
            }
            paste::apply_action(
                view,
                PasteAction::Place {
                    repeat: action == Action::RepeatPlace,
                },
            );
        }
        Action::MarkCorner => {
            selection::mark_corner(&mut view.selection, hit.map(|hit| hit.position))
        }
        Action::CancelSelection => stop_marking(&mut view.selection),
        _ => {}
    }
}

fn status_bar(
    ctx: &egui::Context,
    workspace: &Workspace,
    view: &View,
    position: Vec3,
    captured: bool,
    clipboard: &Clipboard,
    lang: Language,
    bindings: &Bindings,
) {
    let busy = workspace.pending.is_some() || view.paste.writes();
    let scene = view.stream.is_some() || view.demo_scene.is_some();
    let compact = ctx.content_rect().width() < 640.0;
    command_strip(ctx, |ui| {
        if scene && !busy && !input_blocked(ctx) {
            help_bar(ui, view, captured, clipboard, lang, bindings, compact);
        }
    });
    egui::TopBottomPanel::bottom("status")
        .exact_height(26.0)
        .frame(frame(BAR).inner_margin(egui::Margin::symmetric(11, 3)))
        .show(ctx, |ui| {
            let notice = status_notice(workspace, view, lang);
            if scene || notice.is_some() {
                ui.horizontal(|ui| {
                    if scene {
                        ui.monospace(lang.format(
                            Text::Coordinates,
                            &[
                                &format_args!("{:.1}", position.x),
                                &format_args!("{:.1}", position.y),
                                &format_args!("{:.1}", position.z),
                            ],
                        ));
                    }
                    if let Some(stream) = &view.stream {
                        let (visible, absent, failed, loading) = stream.status();
                        ui.separator();
                        let terrain = if loading {
                            lang.text(Text::LoadingTerrain).into()
                        } else if visible == 0 && absent > 0 {
                            lang.text(Text::EmptyTerrain).into()
                        } else {
                            lang.format(Text::VisibleChunks, &[&visible])
                        };
                        ui.label(egui::RichText::new(terrain).color(MUTED));
                        if let Some(error) = stream.first_error() {
                            ui.label(
                                egui::RichText::new(lang.format(Text::FailedChunks, &[&failed]))
                                    .color(ALERT),
                            )
                            .on_hover_text(Diagnostic::from_detail(error).message(lang));
                        }
                    }
                    if let Some((status, color, spinner)) = notice {
                        if scene {
                            ui.separator();
                        }
                        if spinner {
                            ui.spinner();
                        }
                        ui.add(
                            egui::Label::new(egui::RichText::new(&status).color(color)).truncate(),
                        )
                        .on_hover_text(status);
                    }
                });
            }
        });
}

fn command_strip(ctx: &egui::Context, contents: impl FnOnce(&mut egui::Ui)) {
    egui::TopBottomPanel::bottom("contextual commands")
        .exact_height(33.0)
        .frame(frame(PANEL).inner_margin(egui::Margin::symmetric(11, 4)))
        .show(ctx, |ui| horizontal_strip(ui, contents));
}

fn status_notice(
    workspace: &Workspace,
    view: &View,
    lang: Language,
) -> Option<(String, egui::Color32, bool)> {
    let name = view
        .stream
        .as_ref()
        .map_or("", |stream| stream.name.as_str());
    let world = |key| lang.format(key, &[&name]);
    if let Some(pending) = &workspace.pending {
        let message = pending.path().map_or_else(
            || lang.text(Text::OpenFolder).into(),
            |path| lang.format(Text::Opening, &[&path.display()]),
        );
        return Some((message, MUTED, true));
    }
    let notice = match &view.paste {
        PasteState::Saving(..) => (world(Text::PasteWriting), MUTED, true),
        PasteState::Preparing(..) => (world(Text::PasteChecking), MUTED, true),
        PasteState::Error(_) | PasteState::TargetChanged => {
            (world(Text::PasteFailed), ALERT, false)
        }
        _ if view.copying => (world(Text::CopyingWorld), MUTED, true),
        _ if view.preview_cancelled => (lang.text(Text::PasteCancelled).into(), MUTED, false),
        PasteState::Saved(result) => (
            lang.format(Text::PasteSaved, &[&result.summary.world_name]),
            ACCENT,
            false,
        ),
        _ if view.preview.is_some() => (world(Text::PreviewDestination), MUTED, false),
        _ => match &view.copy_notice {
            Some(CopyNotice::Copied([x, y, z])) => (
                lang.format(Text::CopyDone, &[&name, x, y, z]),
                ACCENT,
                false,
            ),
            Some(CopyNotice::Error(_)) => (world(Text::CopyFailed), ALERT, false),
            None => match &workspace.notice {
                Some(OpenNotice::Opened(name))
                    if view
                        .stream
                        .as_ref()
                        .is_some_and(|stream| stream.name == *name) =>
                {
                    (lang.format(Text::Opened, &[name]), MUTED, false)
                }
                Some(OpenNotice::Cancelled) => {
                    (lang.text(Text::OpenCancelled).into(), MUTED, false)
                }
                _ => return None,
            },
        },
    };
    Some(notice)
}

fn help_bar(
    ui: &mut egui::Ui,
    view: &View,
    captured: bool,
    clipboard: &Clipboard,
    lang: Language,
    bindings: &Bindings,
    compact: bool,
) {
    if compact {
        ui.spacing_mut().item_spacing.x = 5.0;
    }
    let binding = |action| {
        bindings.label(action, |key| {
            lang.text(match key {
                Control::PrimaryMouse if compact => Text::KeyLeftClickShort,
                Control::PrimaryMouse => Text::KeyLeftClick,
                Control::SecondaryMouse if compact => Text::KeyRightClickShort,
                Control::SecondaryMouse => Text::KeyRightClick,
                Control::MiddleMouse => Text::KeyMiddleClick,
                Control::ExtraMouse1 => Text::KeyMouse4,
                Control::ExtraMouse2 => Text::KeyMouse5,
                Control::Backspace => Text::KeyBackspace,
                Control::Space => Text::KeySpace,
                Control::LeftControl => Text::KeyLeftCtrl,
                Control::RightControl => Text::KeyRightCtrl,
            })
        })
    };
    let context = tools_context(view);
    let mut hint = |controls: String, label: Text| {
        if !controls.is_empty() {
            egui::Frame::new()
                .fill(TOOLBAR)
                .stroke(egui::Stroke::new(1.0_f32, LINE))
                .corner_radius(4)
                .inner_margin(egui::Margin::symmetric(5, 2))
                .show(ui, |ui| {
                    ui.label(egui::RichText::new(controls).small().strong())
                });
            ui.label(egui::RichText::new(lang.text(label)).small().color(MUTED));
            ui.add_space(if compact { 4.0 } else { 9.0 });
        }
    };
    let editor = [
        (Action::MarkCorner, Text::ActionMark),
        (Action::CancelSelection, Text::ActionCancel),
        (Action::Copy, Text::ActionCopy),
        (Action::StartPaste, Text::ActionPaste),
        (Action::Place, Text::ActionPlace),
        (Action::RepeatPlace, Text::ActionRepeat),
        (Action::CancelPaste, Text::ActionCancel),
    ];
    for (action, label) in editor {
        if bindings.available(action, context) && action_enabled(action, view, clipboard, captured)
        {
            hint(binding(action), label);
        }
    }
    if captured {
        hint(
            [
                Action::CameraForward,
                Action::CameraLeft,
                Action::CameraBackward,
                Action::CameraRight,
            ]
            .map(binding)
            .join(" "),
            Text::CameraMove,
        );
        hint(
            [Action::CameraUp, Action::CameraDown]
                .map(binding)
                .join(" / "),
            Text::CameraHeight,
        );
        hint(binding(Action::CameraBoost), Text::CameraBoost);
        hint(binding(Action::CameraRelease), Text::CameraRelease);
    } else {
        hint(binding(Action::CameraLook), Text::CameraLook);
    }
}

fn opening_notice(ui: &mut egui::Ui, workspace: &mut Workspace, lang: Language) {
    match &workspace.notice {
        Some(OpenNotice::Error(path, error)) => {
            let name = path.as_ref().map_or_else(
                || lang.text(Text::WorldFolder).into(),
                |path| path.display().to_string(),
            );
            diagnostic(ui, &lang.format(Text::OpenFailed, &[&name]), error, lang);
        }
        Some(OpenNotice::Duplicate(name)) => {
            ui.label(egui::RichText::new(lang.format(Text::Duplicate, &[name])).color(ALERT));
        }
        _ => return,
    }
    if ui.small_button(lang.text(Text::CloseNotice)).clicked() {
        workspace.notice = None;
    }
    ui.separator();
}

fn diagnostic(ui: &mut egui::Ui, title: &str, error: &Diagnostic, lang: Language) {
    ui.label(egui::RichText::new(title).strong().color(ALERT));
    let message = error.message(lang);
    ui.label(&message);
    if message != error.detail() {
        ui.collapsing(lang.text(Text::ErrorDetails), |ui| {
            ui.label(error.detail());
        });
    }
}

fn clipboard_contents(ui: &mut egui::Ui, clipboard: &Clipboard, lang: Language) {
    if let Some(structure) = &clipboard.current {
        let [x, y, z] = structure.volume.dimensions();
        ui.horizontal(|ui| {
            let (icon, _) = ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::hover());
            Icon::Clipboard.paint(ui.painter(), icon, ACCENT);
            appearance::headline(ui, &structure.source_name, 14.0);
        });
        ui.label(
            egui::RichText::new(lang.format(Text::Dimensions, &[&x, &y, &z]))
                .monospace()
                .color(MUTED),
        );
        ui.label(
            egui::RichText::new(lang.format(Text::ClipboardBlocks, &[&structure.block_count()]))
                .small()
                .color(MUTED),
        );
        let entities = structure.block_entity_count();
        if entities != 0 {
            let key = if entities == 1 {
                Text::ClipboardEntity
            } else {
                Text::ClipboardEntities
            };
            ui.label(
                egui::RichText::new(lang.format(key, &[&entities]))
                    .small()
                    .color(MUTED),
            );
        }
    } else if !clipboard.copying() {
        ui.horizontal(|ui| {
            let (icon, _) = ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::hover());
            Icon::Clipboard.paint(ui.painter(), icon, MUTED);
            ui.label(
                egui::RichText::new(lang.text(Text::EmptyClipboard))
                    .small()
                    .color(MUTED),
            );
        });
    }
    if clipboard.copying() {
        ui.label(
            egui::RichText::new(lang.text(Text::CopyingClipboard))
                .small()
                .color(MUTED),
        );
    }
}

fn paste_status(
    ui: &mut egui::Ui,
    state: &PasteState,
    name: &str,
    lang: Language,
) -> Option<PasteAction> {
    match state {
        PasteState::Idle | PasteState::Saving(..) | PasteState::Preparing(Some(_), _) => None,
        PasteState::Preparing(None, _) => ui
            .small_button(lang.text(Text::CancelPreparation))
            .clicked()
            .then_some(PasteAction::Discard),
        PasteState::Ready(plan) => {
            ui.label(
                egui::RichText::new(lang.text(Text::ManualConfirm))
                    .strong()
                    .color(ALERT),
            );
            paste_summary(ui, &plan.summary, lang);
            ui.label(lang.text(Text::BackupBeforeWrite));
            if ui.button(lang.text(Text::ReturnToPreview)).clicked() {
                return Some(PasteAction::Discard);
            }
            appearance::button(
                ui,
                Some(Icon::Clipboard),
                lang.text(Text::ConfirmPaste),
                lang.text(Text::ConfirmPaste),
                ButtonStyle::Primary,
                true,
            )
            .clicked()
            .then_some(PasteAction::Confirm)
        }
        PasteState::Saved(result) => {
            ui.collapsing(lang.text(Text::Backup), |ui| {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(result.backup.display().to_string()).monospace(),
                    )
                    .wrap(),
                );
            });
            ui.small_button(lang.text(Text::CloseNotice))
                .clicked()
                .then_some(PasteAction::Discard)
        }
        PasteState::Error(error) => {
            diagnostic(ui, &lang.format(Text::PasteFailed, &[&name]), error, lang);
            ui.small_button(lang.text(Text::CloseNotice))
                .clicked()
                .then_some(PasteAction::Discard)
        }
        PasteState::TargetChanged => {
            ui.label(egui::RichText::new(lang.text(Text::PasteTargetChanged)).color(ALERT));
            ui.small_button(lang.text(Text::CloseNotice))
                .clicked()
                .then_some(PasteAction::Discard)
        }
    }
}

fn paste_summary(ui: &mut egui::Ui, summary: &PasteSummary, lang: Language) {
    ui.label(egui::RichText::new(&summary.world_name).strong());
    ui.add(
        egui::Label::new(egui::RichText::new(summary.world_path.display().to_string()).monospace())
            .wrap(),
    );
    let [x, y, z] = summary.volume.min;
    ui.monospace(lang.format(Text::From, &[&x, &y, &z]));
    let [x, y, z] = summary.volume.max;
    ui.monospace(lang.format(Text::To, &[&x, &y, &z]));
    let mode = lang.text(if summary.mode == AirMode::Include {
        Text::IncludeAir
    } else {
        Text::IgnoreAir
    });
    ui.label(lang.format(
        Text::PasteSummary,
        &[&mode, &summary.chunks, &summary.regions],
    ));
}

fn preview_controls(
    ui: &mut egui::Ui,
    preview: &mut Placement,
    recalculating: bool,
    lang: Language,
) {
    inspector_card(ui, |ui| {
        ui.label(
            egui::RichText::new(lang.text(Text::PasteSource))
                .small()
                .color(MUTED),
        );
        appearance::headline(ui, &preview.source.source_name, 16.0);
        let [x, y, z] = preview.volume().dimensions();
        ui.label(
            egui::RichText::new(lang.format(Text::Dimensions, &[&x, &y, &z]))
                .monospace()
                .color(ACCENT),
        );
    });
    ui.add_space(5.0);
    let mut origin = preview.origin();
    let (changed, focused) = inspector_card(ui, |ui| {
        appearance::headline(ui, lang.text(Text::Origin), 13.0);
        edit_coordinates(ui, &mut origin, preview.limits())
    });
    if changed {
        preview.set_origin(origin);
    }
    if changed || focused {
        preview.moving = false;
        preview.target_valid = true;
    }
    if preview.moving && !preview.target_valid {
        ui.label(egui::RichText::new(lang.text(Text::NoPasteTarget)).color(MUTED));
    } else if recalculating {
        ui.label(egui::RichText::new(lang.text(Text::PreviewUpdating)).color(MUTED));
    }
}

fn selection_details(ui: &mut egui::Ui, selection: &mut Selection, position: Vec3, lang: Language) {
    if let Some(corner) = selection.picking {
        ui.label(
            egui::RichText::new(lang.format(Text::PickCorner, &[&corner_name(corner)]))
                .color(ACCENT),
        );
    }
    if selection.missed {
        ui.label(egui::RichText::new(lang.text(Text::PickMiss)).color(ALERT));
    }
    if let Some(volume) = selection.volume() {
        let [x, y, z] = volume.dimensions();
        inspector_card(ui, |ui| {
            ui.horizontal(|ui| {
                let (icon, _) =
                    ui.allocate_exact_size(egui::vec2(17.0, 17.0), egui::Sense::hover());
                Icon::Layers.paint(ui.painter(), icon, ACCENT);
                ui.label(
                    egui::RichText::new(lang.format(Text::Dimensions, &[&x, &y, &z]))
                        .monospace()
                        .color(ACCENT),
                );
            });
            if let Err(error) = volume.validate() {
                ui.label(
                    egui::RichText::new(Diagnostic::from_error(error).message(lang)).color(ALERT),
                );
            }
        });
    }
    for corner in 0..2 {
        ui.push_id(corner, |ui| {
            inspector_card(ui, |ui| {
                corner_controls(ui, selection, corner, position, lang)
            });
        });
    }
}

const COORDINATE_LIMITS: [(i32, i32); 3] = [
    (MIN_HORIZONTAL, MAX_HORIZONTAL),
    (MIN_Y, MAX_Y),
    (MIN_HORIZONTAL, MAX_HORIZONTAL),
];

fn corner_controls(
    ui: &mut egui::Ui,
    selection: &mut Selection,
    corner: usize,
    position: Vec3,
    lang: Language,
) {
    ui.horizontal(|ui| {
        let color = corner_color(corner);
        ui.label(
            egui::RichText::new(lang.format(Text::Corner, &[&corner_name(corner)]))
                .font(appearance::strong(14.0))
                .color(color),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let mark = lang.text(Text::Mark);
            if appearance::button(ui, Some(Icon::Target), mark, mark, ButtonStyle::Quiet, true)
                .clicked()
            {
                arm_pick(selection, corner, false);
            }
        });
    });
    if let Some(coords) = &mut selection.corners[corner] {
        if edit_coordinates(ui, coords, COORDINATE_LIMITS).0 {
            stop_marking(selection);
        }
    } else if ui.button(lang.text(Text::EditCoordinates)).clicked() {
        let near_camera = position.floor().as_ivec3().to_array();
        selection.corners[corner] = Some(selection.corners[1 - corner].unwrap_or_else(|| {
            std::array::from_fn(|axis| {
                let (min, max) = COORDINATE_LIMITS[axis];
                near_camera[axis].clamp(min, max)
            })
        }));
        stop_marking(selection);
    }
}

fn edit_coordinates(
    ui: &mut egui::Ui,
    coords: &mut [i32; 3],
    limits: [(i32, i32); 3],
) -> (bool, bool) {
    let horizontal = ui.available_width() >= 202.0;
    let width = (ui.available_width() - 2.0 * ui.spacing().item_spacing.x) / 3.0;
    let layout = if horizontal {
        egui::Layout::left_to_right(egui::Align::Min)
    } else {
        egui::Layout::top_down(egui::Align::Min)
    };
    ui.with_layout(layout, |ui| {
        let mut changed = false;
        let mut focused = false;
        for (axis, ((min, max), coordinate)) in ["X", "Y", "Z"]
            .into_iter()
            .zip(limits.into_iter().zip(coords))
        {
            let label = egui::RichText::new(axis)
                .font(appearance::strong(12.0))
                .color(MUTED);
            let mut field = |ui: &mut egui::Ui, width: f32| {
                let response = ui.add_sized(
                    [width, 27.0],
                    egui::DragValue::new(coordinate)
                        .range(min..=max)
                        .update_while_editing(false),
                );
                changed |= response.changed();
                focused |= response.has_focus();
            };
            if horizontal {
                ui.vertical(|ui| {
                    ui.set_width(width);
                    ui.label(label);
                    field(ui, width);
                });
            } else {
                ui.horizontal(|ui| {
                    ui.add_sized([18.0, 27.0], egui::Label::new(label));
                    field(ui, ui.available_width());
                });
            }
        }
        (changed, focused)
    })
    .inner
}

fn corner_name(corner: usize) -> &'static str {
    ["A", "B"][corner]
}

fn arm_pick(selection: &mut Selection, corner: usize, then_b: bool) {
    selection.picking = Some(corner);
    selection.pick_b_after_a = then_b;
    selection.missed = false;
}

fn stop_marking(selection: &mut Selection) {
    selection.picking = None;
    selection.pick_b_after_a = false;
    selection.missed = false;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contextual_commands_reserve_the_same_viewport_space_when_empty_or_overflowing() {
        for width in [520.0, 1280.0] {
            let ctx = egui::Context::default();
            appearance::apply(&ctx);
            let mut bottom_edge = 0.0;
            let mut draw = |count| {
                let _ = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(width, 600.0),
                        )),
                        ..Default::default()
                    },
                    |ctx| {
                        command_strip(ctx, |ui| {
                            for _ in 0..count {
                                ui.label("Shift+Clic izquierdo · Repetir");
                            }
                        });
                        bottom_edge = ctx.available_rect().bottom();
                    },
                );
                bottom_edge
            };
            let empty = draw(0);
            assert!(empty < 600.0);
            assert_eq!(draw(12), empty);
            assert_eq!(draw(12), empty);
            assert_eq!(draw(0), empty);
        }
    }

    #[test]
    fn dragging_a_world_tab_only_detaches_when_released_outside() {
        for (release, detaches) in [
            (egui::pos2(40.0, 520.0), false),
            (egui::pos2(40.0, 680.0), true),
        ] {
            let ctx = egui::Context::default();
            appearance::apply(&ctx);
            let window = Window {
                focused: true,
                position: WindowPosition::At(IVec2::ZERO),
                ..default()
            };
            let world = Entity::from_raw_u32(1).unwrap();
            let mut workspace = Workspace::new(world);
            let tabs = [(world, "Bosque".into(), PathBuf::from("Bosque"))];
            let mut frame = |events| {
                let mut actions = WindowActions::default();
                let _ = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(800.0, 600.0),
                        )),
                        events,
                        ..Default::default()
                    },
                    |ctx| {
                        tab_bar(
                            ctx,
                            &window,
                            &mut workspace,
                            &tabs,
                            false,
                            Language::Spanish,
                            &mut actions,
                        )
                    },
                );
                actions.tab
            };
            let button = |pos, pressed| egui::Event::PointerButton {
                pos,
                pressed,
                button: egui::PointerButton::Primary,
                modifiers: egui::Modifiers::NONE,
            };
            let start = egui::pos2(34.0, 12.0);
            assert!(frame(vec![]).is_none());
            assert!(frame(vec![egui::Event::PointerMoved(start), button(start, true)]).is_none());
            assert!(frame(vec![egui::Event::PointerMoved(egui::pos2(38.0, 40.0))]).is_none());
            assert!(frame(vec![egui::Event::PointerMoved(release)]).is_none());
            let action = frame(vec![button(release, false)]);
            if detaches {
                assert!(matches!(action, Some((tab, TabAction::Detach(_))) if tab == world));
            } else {
                assert!(action.is_none());
            }
        }
    }
}
