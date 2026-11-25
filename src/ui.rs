use std::{path::PathBuf, sync::Arc};

use bevy::{
    camera::Viewport,
    input::{ButtonState, keyboard::KeyboardInput},
    prelude::*,
    window::{CursorOptions, RawHandleWrapper, WindowPosition},
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
        self, DemoSelection, Destination, LeaveChoice, LeaveDecision, OpenNotice, Overlay,
        SceneAssets, TabAction, TabRequest, View, Workspace,
    },
    flight::Flight,
    i18n::{Language, Text},
    manual_edit,
    palette::{self, Palette},
    paste::{self, EditIntent, PasteAction, PasteState},
    placement::{AirMode, Placement},
    selection::{self, Selection},
    structure::{Clipboard, CopyNotice, MAX_HORIZONTAL, MAX_Y, MIN_HORIZONTAL, MIN_Y, Volume},
    world::{BlockState, ChunkPos},
};

mod active_material;
mod appearance;
mod block_inspector;
mod dialogs;
mod menus;

use appearance::{ACCENT, ALERT, BAR, ButtonStyle, FIELD, Icon, LINE, MUTED, PANEL, TEXT, TOOLBAR};

const TOOL_BUTTONS: [(Action, Icon, Text); 4] = [
    (Action::UseExplore, Icon::Pointer, Text::Explore),
    (Action::UseSelection, Icon::Selection, Text::Select),
    (Action::UseInspect, Icon::Inspect, Text::InspectBlock),
    (Action::UseManual, Icon::Pencil, Text::ManualEdit),
];

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
    mut windows: Query<(
        &Window,
        &mut CursorOptions,
        &mut Workspace,
        Option<&RawHandleWrapper>,
    )>,
    mut views: ParamSet<(Query<&View>, Query<&mut View>)>,
    mut overlays: Query<(
        &Overlay,
        &mut Camera,
        &mut EguiContext,
        &mut EguiInput,
        &mut EguiFullOutput,
    )>,
    mut cameras: Query<(&mut Camera, &Transform, &mut Flight), Without<Overlay>>,
    mut demo_selection: ResMut<DemoSelection>,
    mut clipboard: ResMut<Clipboard>,
    mut language: ResMut<Language>,
    mut palette: ResMut<Palette>,
    mut leave_decisions: MessageWriter<LeaveDecision>,
    mut tab_requests: MessageWriter<TabRequest>,
    mut keyboard: MessageReader<KeyboardInput>,
    assets: Res<SceneAssets>,
    bindings: Res<Bindings>,
) {
    let mut demo_requested = false;
    let keys: Vec<_> = keyboard.read().collect();
    for (overlay, mut overlay_camera, mut egui, mut input, mut output) in &mut overlays {
        let window_id = overlay.0;
        let native_keys: Vec<_> = keys
            .iter()
            .copied()
            .filter(|key| key.window == window_id)
            .collect();
        let Ok((window, mut cursor, mut workspace, handle)) = windows.get_mut(window_id) else {
            continue;
        };
        let tabs = {
            let read = views.p0();
            workspace
                .tabs
                .iter()
                .filter_map(|&tab| {
                    let view = read.get(tab).ok()?;
                    let stream = view.stream.as_ref()?;
                    let name = if view.edits.dirty() {
                        format!("{} *", stream.name)
                    } else {
                        stream.name.clone()
                    };
                    Some((tab, name, stream.path().to_owned()))
                })
                .collect::<Vec<_>>()
        };
        let leaving = workspace.leave.as_ref().and_then(|leave| {
            let document = leave.document()?;
            let read = views.p0();
            let view = read.get(document).ok()?;
            let stream = view.stream.as_ref()?;
            let error = match &view.paste {
                PasteState::Error(error) => Some(error.message(*language)),
                _ => None,
            };
            Some((document, stream.name.clone(), leave.saving, error))
        });
        let document = workspace.active;
        let mut docs = views.p1();
        let Ok(mut view) = docs.get_mut(document) else {
            continue;
        };
        let Ok((mut camera, transform, mut flight)) = cameras.get_mut(view.camera) else {
            continue;
        };

        let mut result = WindowActions::default();
        let mut viewport = None;
        let mut look_buttons = Vec::new();
        let mut consumes_boost = false;
        let ctx = egui.get_mut();
        let had_focus = input_blocked(ctx);
        let was_modal = workspace.search.is_some()
            || workspace.leave.is_some()
            || menus::window_state(ctx).dialog.is_some();
        let mut raw = input.take();
        raw.events
            .retain(|event| !Bindings::is_system_shortcut(event));
        if flight.captured()
            && let Some(viewport) = view.viewport
        {
            let center = viewport.center() * (window.scale_factor() / ctx.pixels_per_point());
            center_captured_pointer(&mut raw, egui::pos2(center.x, center.y));
        }
        let parent = DialogParent::from_window(handle);
        #[cfg(windows)]
        if had_focus
            && raw.focused
            && let Some(parent) = &parent
        {
            crate::native_clipboard::add_text_events(&mut raw, parent.window.get() as _);
        }
        let focused = raw.focused && window.focused && !flight.focus_changed();
        let pass = ctx.run(raw, |ctx| {
            // The camera still uses the preceding frame's viewport while egui lays out this one.
            let ray_point = view.viewport.and_then(|viewport| {
                if flight.captured() {
                    Some(viewport.center())
                } else {
                    let point =
                        to_logical(ctx.pointer_hover_pos()?, ctx.pixels_per_point(), window);
                    viewport.contains(point).then_some(point)
                }
            });
            let aimed = ray_point.and_then(|point| {
                selection::cursor_hit(view.stream.as_ref()?, &camera, transform, point)
            });
            if view.tool == EditorContext::Inspect
                && !view.inspection_pinned
                && ray_point.is_some()
                && focused
                && !was_modal
                && !had_focus
            {
                view.inspected = aimed.map(|hit| hit.position);
            }
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
                &palette,
                aimed,
                *language,
                &bindings,
            );
            let rect = ctx.available_rect();
            let scale = ctx.pixels_per_point();
            if view.stream.is_some() || view.demo_scene.is_some() {
                viewport = Some((rect, scale));
            }
            if view.preview.is_some() {
                draw_placement_badge(ctx, rect, *language);
            } else if view.tool == EditorContext::Select && view.viewport.is_some() {
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
            if !was_modal && !had_focus && view.preview.is_none() {
                let highlight = match view.tool {
                    EditorContext::Inspect => view.inspected,
                    EditorContext::Manual => aimed.map(|hit| hit.position),
                    _ => None,
                };
                if let Some(target) = highlight {
                    draw_block_outline(
                        ctx,
                        rect,
                        scale,
                        window,
                        &camera,
                        transform,
                        target,
                        view.tool == EditorContext::Manual,
                    );
                }
            }
            result.merge(actions);
            if let Some((document, name, saving, error)) = &leaving {
                if let Some(choice) = confirm_leave(ctx, *language, name, *saving, error.as_deref())
                {
                    leave_decisions.write(LeaveDecision {
                        window: window_id,
                        document: *document,
                        choice,
                    });
                }
            } else if let Some(search) = &mut workspace.search
                && !search.show(ctx, *language, &mut palette)
            {
                workspace.search = None;
                ctx.request_repaint();
            }
            workspace.blocks_camera = had_focus
                || input_blocked(ctx)
                || was_modal
                || result.open.is_some()
                || result.tab.is_some()
                || workspace.pending.is_some()
                || menus::window_state(ctx).dialog.is_some();
            if !focused
                || workspace.blocks_camera
                || view.paste.writes()
                || result.tab.is_some()
                || result.paste.is_some()
                || result.language.is_some()
                || result.region.is_some()
            {
                return;
            }
            let view = &mut *view;
            if !view.paste.locks_preview()
                && let Some(preview) = &mut view.preview
                && preview.moving
            {
                preview.set_cursor_target(aimed.map(selection::Hit::adjacent));
            }
            let context = tools_context(view);
            let viewport_focused = flight.captured()
                || ctx
                    .pointer_hover_pos()
                    .is_some_and(|point| rect.contains(point));
            if let Some((action, point, shift)) = ctx.input(|input| {
                look_buttons.clear();
                look_buttons.extend(input.events.iter().filter_map(|event| {
                    let (pressed, point) = bindings.camera_look_event(context, event)?;
                    Some((pressed, rect.contains(point)))
                }));
                input.events.iter().find_map(|event| {
                    if Bindings::only_native_repeats(event, &native_keys) {
                        return None;
                    }
                    let action = bindings.match_enabled(context, event, |action| {
                        let needs_viewport =
                            action.tool().is_some() || action == Action::ToggleCameraCapture;
                        (!needs_viewport || viewport_focused)
                            && action_enabled(action, view, &clipboard, flight.captured())
                    })?;
                    let (point, shift) = match event {
                        egui::Event::PointerButton { pos, modifiers, .. } => {
                            let point = if flight.captured() {
                                rect.center()
                            } else if rect.contains(*pos) {
                                *pos
                            } else {
                                return None;
                            };
                            (Some(point), modifiers.shift)
                        }
                        egui::Event::Key { modifiers, .. } => (None, modifiers.shift),
                        _ => return None,
                    };
                    Some((action, point, shift))
                })
            }) && result.editor.is_none()
            {
                result.editor = Some((action, point));
                consumes_boost = shift;
            }
            let releasing = result.editor.is_some_and(|(action, _)| match action {
                Action::CameraRelease | Action::OpenPalette | Action::Save | Action::StartPaste => {
                    true
                }
                Action::ToggleCameraCapture => flight.persistent(),
                action if action.tool().is_some() => !flight.persistent(),
                _ => false,
            });
            if flight.captured()
                && !releasing
                && (flight.persistent() || look_buttons.iter().all(|(pressed, _)| *pressed))
            {
                draw_reticle(ctx, rect);
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
        if workspace.blocks_camera || !focused {
            flight.release(&mut cursor);
        } else {
            for (pressed, over_viewport) in look_buttons {
                if !pressed || result.editor.is_none() {
                    let state = if pressed {
                        ButtonState::Pressed
                    } else {
                        ButtonState::Released
                    };
                    flight.look_button(state, over_viewport, &mut cursor);
                }
            }
        }
        if consumes_boost {
            flight.suppress_boost_once();
        }
        if let (Some((action, point)), Some((_, scale))) = (result.editor, viewport) {
            match action {
                Action::UseExplore
                | Action::UseSelection
                | Action::UseInspect
                | Action::UseManual => {
                    if focused && let Some(tool) = action.tool() {
                        flight.change_tool(&mut cursor);
                        view.tool = tool;
                        view.inspection_pinned = false;
                        view.inspected = None;
                        view.selection.cancel_marking();
                    }
                }
                Action::OpenPalette => {
                    flight.release(&mut cursor);
                    workspace.search = Some(palette::Search::default());
                    workspace.blocks_camera = true;
                }
                Action::ToggleCameraCapture => {
                    if focused {
                        flight.toggle_capture(&mut cursor);
                        workspace.blocks_camera = false;
                    }
                }
                Action::CameraRelease => flight.release(&mut cursor),
                Action::StartPaste => {
                    flight.release(&mut cursor);
                    execute_action(&mut view, document, &mut clipboard, action, None);
                }
                Action::PaintBlock | Action::EraseBlock => {
                    if let Some(hit) = point.and_then(|point| {
                        selection::cursor_hit(
                            view.stream.as_ref()?,
                            &camera,
                            transform,
                            to_logical(point, scale, window),
                        )
                    }) {
                        manual_edit::apply(&mut view, action, hit, palette.active());
                    }
                }
                _ => {
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
            }
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
            workspace.request_open(destination, *language, parent, view.edits.dirty());
        } else if let Some(action) = result.paste {
            paste::apply_action(&mut view, action);
        } else if let Some((volume, state, matching)) = result.region {
            paste::set_blocks(&mut view, volume, state, matching, true);
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

fn center_captured_pointer(input: &mut egui::RawInput, center: egui::Pos2) {
    input.events.retain_mut(|event| match event {
        egui::Event::PointerMoved(point) | egui::Event::PointerButton { pos: point, .. } => {
            *point = center;
            true
        }
        egui::Event::PointerGone => false,
        _ => true,
    });
    // The hidden OS cursor may still be over a toolbar when capture begins.
    input.events.insert(0, egui::Event::PointerMoved(center));
}

fn draw_reticle(ctx: &egui::Context, viewport: egui::Rect) {
    let painter = ctx
        .layer_painter(egui::LayerId::new(
            egui::Order::Middle,
            egui::Id::new("camera reticle"),
        ))
        .with_clip_rect(viewport);
    let scale = ctx.pixels_per_point();
    let snap = |points: f32| (points * scale).round() / scale;
    let center = viewport.center();
    let center = egui::pos2(snap(center.x), snap(center.y));
    let radius = snap(13.0);
    let gap = snap(3.0);
    let half_width = snap(1.0).max(scale.recip());
    let color = egui::Color32::from_white_alpha(190);

    for (x, y) in [(1.0_f32, 0.0_f32), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)] {
        let axis = egui::vec2(x, y);
        let width = egui::vec2(y.abs(), x.abs()) * half_width;
        let arm =
            egui::Rect::from_two_pos(center + axis * gap - width, center + axis * radius + width);
        painter.rect_filled(arm, 0, color);
    }
}

fn draw_block_outline(
    ctx: &egui::Context,
    viewport: egui::Rect,
    scale: f32,
    window: &Window,
    camera: &Camera,
    transform: &Transform,
    block: [i32; 3],
    erase: bool,
) {
    let origin = Vec3::from_array(block.map(|value| value as f32));
    let camera_pose = GlobalTransform::from(*transform);
    let corners: [Option<egui::Pos2>; 8] = std::array::from_fn(|corner| {
        let world = origin
            + Vec3::new(
                (corner & 1) as f32,
                ((corner >> 1) & 1) as f32,
                ((corner >> 2) & 1) as f32,
            );
        let point =
            camera.world_to_viewport(&camera_pose, world).ok()? * (window.scale_factor() / scale);
        Some(egui::pos2(point.x, point.y))
    });
    let painter = ctx
        .layer_painter(egui::LayerId::new(
            egui::Order::Middle,
            egui::Id::new("pointed block"),
        ))
        .with_clip_rect(viewport);
    let color = if erase { ALERT } else { ACCENT };
    for from in 0..8 {
        for bit in [1, 2, 4] {
            if from & bit == 0
                && let (Some(a), Some(b)) = (corners[from], corners[from | bit])
            {
                painter.line_segment([a, b], egui::Stroke::new(1.0_f32, color));
            }
        }
    }
}

fn confirm_leave(
    ctx: &egui::Context,
    lang: Language,
    name: &str,
    saving: bool,
    error: Option<&str>,
) -> Option<LeaveChoice> {
    let modal = egui::Modal::new(egui::Id::new("unsaved world")).show(ctx, |ui| {
        ui.set_width(340.0_f32.min(ctx.content_rect().width() - 36.0));
        appearance::headline(ui, lang.text(Text::SaveChangesTitle), 18.0);
        ui.add_space(6.0);
        ui.label(lang.format(Text::UnsavedPrompt, &[&name]));
        if saving {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(lang.format(Text::SavingWorld, &[&name]));
            });
            return None;
        }
        if let Some(error) = error {
            ui.label(egui::RichText::new(error).color(ALERT));
        }
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            [
                (Text::SaveWorld, LeaveChoice::Save, ButtonStyle::Primary),
                (
                    Text::DiscardChanges,
                    LeaveChoice::Discard,
                    ButtonStyle::Quiet,
                ),
                (Text::ActionCancel, LeaveChoice::Cancel, ButtonStyle::Quiet),
            ]
            .into_iter()
            .filter_map(|(label, choice, style)| {
                let label = lang.text(label);
                appearance::button(ui, None, label, label, style, true)
                    .clicked()
                    .then_some(choice)
            })
            .last()
        })
        .inner
    });
    if modal.should_close() && !saving {
        Some(LeaveChoice::Cancel)
    } else {
        modal.inner
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

fn draw_placement_badge(ctx: &egui::Context, viewport: egui::Rect, lang: Language) {
    let painter = ctx
        .layer_painter(egui::LayerId::new(
            egui::Order::Middle,
            egui::Id::new("placement mode"),
        ))
        .with_clip_rect(viewport);
    let text = painter.layout_no_wrap(
        lang.text(Text::Placement).into(),
        appearance::strong(13.0),
        TEXT,
    );
    let rect = egui::Rect::from_min_size(
        viewport.min + egui::vec2(12.0, 12.0),
        text.size() + egui::vec2(31.0, 12.0),
    );
    painter.rect(
        rect,
        4,
        FIELD,
        egui::Stroke::new(1.0_f32, LINE),
        egui::StrokeKind::Inside,
    );
    painter.circle_filled(egui::pos2(rect.left() + 11.0, rect.center().y), 3.0, ALERT);
    painter.galley(rect.min + egui::vec2(20.0, 6.0), text, TEXT);
}

fn input_blocked(ctx: &egui::Context) -> bool {
    ctx.wants_keyboard_input()
        || egui::Popup::is_any_open(ctx)
        || menus::window_state(ctx).dialog.is_some()
        || ctx.memory(|memory| memory.top_modal_layer().is_some())
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
    region: Option<(Volume, BlockState, Option<String>)>,
    region_menu: bool,
    demo: bool,
}

impl WindowActions {
    fn merge(&mut self, other: Self) {
        self.open = self.open.or(other.open);
        self.editor = self.editor.or(other.editor);
        self.paste = self.paste.or(other.paste);
        self.tab = self.tab.take().or(other.tab);
        self.language = self.language.or(other.language);
        self.region = self.region.take().or(other.region);
        self.region_menu |= other.region_menu;
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
    palette: &Palette,
    aimed: Option<selection::Hit>,
    lang: Language,
    bindings: &Bindings,
) -> WindowActions {
    let mut actions = WindowActions::default();
    let busy = workspace.pending.is_some()
        || workspace.leave.is_some()
        || workspace.search.is_some()
        || view.paste.busy();
    let name = view.stream.as_ref().map(|stream| stream.name.clone());
    let scene = name.is_some() || view.demo_scene.is_some();
    let mut chrome = menus::window_state(ctx);
    menus::bar(
        ctx,
        &mut chrome,
        workspace.active,
        view,
        clipboard,
        busy,
        captured,
        lang,
        bindings,
        &mut actions,
    );
    if !tabs.is_empty() {
        tab_bar(ctx, window, workspace, tabs, busy, lang, &mut actions);
    }
    status_bar(
        ctx, workspace, view, position, captured, clipboard, palette, lang, bindings,
    );
    if chrome.activity_bar {
        tool_rail(ctx, view, busy, lang, bindings, &mut actions);
    }

    if !scene {
        empty_workspace(ctx, workspace, busy, lang, &mut actions);
    } else if chrome.inspector {
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
                                    palette,
                                    aimed,
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
    if let Some(dialog) = chrome.dialog {
        let result = dialogs::show(ctx, dialog, lang, bindings);
        actions.language = result.language;
        if result.close {
            chrome.dialog = None;
        }
    }
    menus::save_window_state(ctx, chrome);
    actions
}

fn empty_workspace(
    ctx: &egui::Context,
    workspace: &mut Workspace,
    busy: bool,
    lang: Language,
    actions: &mut WindowActions,
) {
    let available = egui::CentralPanel::default()
        .frame(frame(FIELD).inner_margin(egui::Margin::same(20)))
        .show(ctx, |ui| ui.max_rect())
        .inner;
    let width = available.width().min(396.0);
    let inset = 23.0;
    egui::Area::new(egui::Id::new("welcome card"))
        .pivot(egui::Align2::CENTER_CENTER)
        .fixed_pos(available.center())
        .constrain_to(available)
        .default_width(width)
        .fade_in(false)
        .show(ctx, |ui| {
            appearance::card()
                .fill(PANEL)
                .inner_margin(egui::Margin::same(inset as i8))
                .show(ui, |ui| {
                    ui.set_width(width - 2.0 * inset);
                    egui::ScrollArea::vertical()
                        .max_height(available.height() - 2.0 * inset)
                        .show(ui, |ui| {
                            ui.vertical_centered(|ui| {
                                appearance::empty_illustration(ui);
                                ui.add_space(9.0);
                                appearance::headline(ui, lang.text(Text::EmptyTitle), 24.0);
                                ui.label(
                                    egui::RichText::new(lang.text(Text::EmptyHelp)).color(MUTED),
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
                        });
                });
        });
}

fn tool_rail(
    ctx: &egui::Context,
    view: &View,
    busy: bool,
    lang: Language,
    bindings: &Bindings,
    actions: &mut WindowActions,
) {
    egui::SidePanel::left("primary tools")
        .exact_width(52.0)
        .resizable(false)
        .frame(frame(BAR).inner_margin(egui::Margin::symmetric(appearance::MODE_RAIL_MARGIN, 9)))
        .show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.spacing_mut().item_spacing.y = 5.0;
                let active = tools_context(view);
                let has_world = view.stream.is_some();
                for (action, icon, label) in TOOL_BUTTONS {
                    let Some(mode) = action.tool() else { continue };
                    let selected = mode == active && (has_world || view.demo_scene.is_some());
                    let enabled = has_world
                        && !busy
                        && !view.paste.locks_preview()
                        && action.allowed_in(active);
                    let label = lang.text(label);
                    let chord = binding_label(bindings, action, lang, false);
                    let tooltip = if chord.is_empty() {
                        label.to_owned()
                    } else {
                        format!("{label} ({chord})")
                    };
                    if appearance::button(
                        ui,
                        Some(icon),
                        "",
                        &tooltip,
                        ButtonStyle::Mode(selected),
                        enabled,
                    )
                    .clicked()
                        && mode != active
                    {
                        actions.editor = Some((action, None));
                    }
                    if matches!(mode, EditorContext::Explore | EditorContext::Inspect) {
                        ui.separator();
                    }
                }
            });
        });
}

fn selection_actions(ui: &mut egui::Ui, selection: &mut Selection, lang: Language) {
    ui.horizontal_wrapped(|ui| {
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
            arm_pick(selection, 0, true);
        }
        if selection.picking.is_some() {
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
                selection.cancel_marking();
            }
        } else if appearance::button(
            ui,
            Some(Icon::Trash),
            "",
            lang.text(Text::ClearSelection),
            ButtonStyle::Quiet,
            selection.corners.iter().any(Option::is_some),
        )
        .clicked()
        {
            selection.clear();
        }
    });
}

fn placement_actions(
    ui: &mut egui::Ui,
    preview: &mut Placement,
    lang: Language,
    actions: &mut WindowActions,
) {
    ui.horizontal_wrapped(|ui| {
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
    });
    ui.horizontal_wrapped(|ui| {
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
    });
    ui.horizontal_wrapped(|ui| {
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
    });
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
            let tab_width = (ui.available_width() * 0.28).clamp(176.0, 240.0);
            horizontal_strip(ui, |ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                for (tab, name, path) in tabs {
                    ui.push_id(*tab, |ui| {
                        let active = *tab == workspace.active;
                        let (label, close) = world_tab(ui, name, tab_width, active, busy, lang);
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
    width: f32,
    active: bool,
    busy: bool,
    lang: Language,
) -> (egui::Response, egui::Response) {
    let mut job = egui::text::LayoutJob::simple(
        name.to_owned(),
        appearance::strong(13.0),
        egui::Color32::PLACEHOLDER,
        width - 67.0,
    );
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    let text = ui.painter().layout_job(job);
    let (id, rect) = ui.allocate_space(egui::vec2(width, 32.0));
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
    palette: &Palette,
    aimed: Option<selection::Hit>,
    lang: Language,
    actions: &mut WindowActions,
) {
    world_heading(ui, name);
    ui.add_space(8.0);
    ui.separator();
    ui.add_space(6.0);
    let locked = view.paste.locks_preview();
    let context = tools_context(view);
    let section = match context {
        EditorContext::Explore => None,
        EditorContext::Select => Some(Text::Select),
        EditorContext::Inspect => Some(Text::InspectBlock),
        EditorContext::Manual => Some(Text::ManualEdit),
        EditorContext::Place => Some(Text::Placement),
    };
    if let Some(section) = section {
        appearance::headline(ui, lang.text(section), 17.0);
        ui.add_space(4.0);
    }
    if let Some(action) = paste_status(ui, &view.paste, name, lang) {
        actions.paste.get_or_insert(action);
    }
    ui.add_enabled_ui(!busy && !locked, |ui| {
        if let Some(preview) = &mut view.preview {
            placement_actions(ui, preview, lang, actions);
            view.air_mode = preview.mode;
            ui.add_space(4.0);
            let recalculating = view
                .stream
                .as_ref()
                .is_some_and(|stream| stream.preview_busy());
            preview_controls(ui, preview, recalculating, lang);
        } else {
            match context {
                EditorContext::Select => {
                    selection_actions(ui, &mut view.selection, lang);
                    ui.add_space(4.0);
                    selection_details(ui, &mut view.selection, position, lang);
                    if let Some(volume) = view
                        .selection
                        .volume()
                        .filter(|volume| volume.validate().is_ok())
                    {
                        ui.add_space(7.0);
                        ui.separator();
                        let jump = actions.region_menu;
                        let region = egui::CollapsingHeader::new(
                            egui::RichText::new(lang.text(Text::RegionEdit))
                                .font(appearance::strong(14.0)),
                        )
                        .id_salt("region operations")
                        .open(jump.then_some(true))
                        .show_unindented(ui, |ui| {
                            region_controls(ui, view, volume, palette, lang, actions)
                        });
                        if jump {
                            region
                                .header_response
                                .scroll_to_me(Some(egui::Align::Center));
                        }
                    }
                }
                EditorContext::Inspect => {
                    if let Some(([x, y, z], chunk)) = view.inspected.and_then(|position| {
                        let [x, _, z] = position;
                        let chunk = view
                            .stream
                            .as_ref()?
                            .loaded_chunk(ChunkPos::from_block(x, z))?;
                        Some((position, chunk))
                    }) && let Some(state) = chunk.block(x.rem_euclid(16), y, z.rem_euclid(16))
                    {
                        block_inspector::show(
                            ui,
                            lang,
                            [x, y, z],
                            state,
                            chunk.block_entities.get(&[x, y, z]),
                            view.edits.contains([x, y, z]),
                        );
                        let source = lang.text(Text::UseAsReplaceSource);
                        if ui.small_button(source).clicked() {
                            view.replace_source.clone_from(&state.name);
                        }
                    } else {
                        ui.label(egui::RichText::new(lang.text(Text::NoTargetBlock)).color(MUTED));
                    }
                }
                EditorContext::Manual => {
                    material_control(ui, palette, lang, actions);
                    if aimed.is_none() {
                        ui.label(egui::RichText::new(lang.text(Text::NoTargetBlock)).color(MUTED));
                    }
                }
                _ => {}
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
    .id_salt("shared clipboard")
    .default_open(false)
    .show_unindented(ui, |ui| {
        inspector_card(ui, |ui| clipboard_contents(ui, clipboard, lang));
    });
}

fn material_control(
    ui: &mut egui::Ui,
    palette: &Palette,
    lang: Language,
    actions: &mut WindowActions,
) {
    ui.label(
        egui::RichText::new(lang.text(Text::ActiveMaterial))
            .small()
            .color(MUTED),
    );
    let state = palette.active();
    let label = palette::block_name(state, lang);
    if appearance::button(
        ui,
        Some(Icon::Cube),
        &label,
        lang.text(Text::BlockPalette),
        ButtonStyle::Quiet,
        true,
    )
    .on_hover_text(&state.name)
    .clicked()
    {
        actions.editor = Some((Action::OpenPalette, None));
    }
}

fn region_controls(
    ui: &mut egui::Ui,
    view: &mut View,
    volume: Volume,
    palette: &Palette,
    lang: Language,
    actions: &mut WindowActions,
) {
    material_control(ui, palette, lang, actions);
    ui.label(
        egui::RichText::new(lang.text(Text::ReplaceSource))
            .small()
            .color(MUTED),
    );
    ui.add(
        egui::TextEdit::singleline(&mut view.replace_source)
            .hint_text("minecraft:stone")
            .desired_width(f32::INFINITY),
    );
    let source = view.replace_source.trim();
    let matching = (!source.is_empty()).then(|| {
        if source.contains(':') {
            source.to_owned()
        } else {
            format!("minecraft:{source}")
        }
    });
    ui.horizontal_wrapped(|ui| {
        for (key, allowed, filter) in [
            (Text::RegionFill, true, None),
            (Text::RegionReplace, matching.is_some(), matching),
        ] {
            let label = lang.text(key);
            if appearance::button(ui, None, label, label, ButtonStyle::Quiet, allowed).clicked() {
                actions.region = Some((volume, palette.active().clone(), filter));
            }
        }
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
    } else {
        view.tool
    }
}

fn action_enabled(action: Action, view: &View, clipboard: &Clipboard, captured: bool) -> bool {
    if action == Action::CameraRelease {
        return captured;
    }
    if view.stream.is_none() || !action.allowed_in(tools_context(view)) {
        return false;
    }
    match action {
        Action::ToggleCameraCapture if captured => return !view.paste.writes(),
        Action::CancelPaste => return !view.paste.writes() && view.preview.is_some(),
        _ if view.paste.locks_preview() => return false,
        _ => {}
    }
    if let Some(tool) = action.tool() {
        return view.tool != tool;
    }
    match action {
        Action::Save => !view.copying && view.edits.dirty(),
        Action::Undo => view.edits.can_undo(),
        Action::Redo => view.edits.can_redo(),
        Action::OpenPalette
        | Action::ToggleCameraCapture
        | Action::PinInspection
        | Action::PaintBlock
        | Action::EraseBlock => true,
        Action::Copy => {
            !clipboard.copying()
                && view.selection.picking.is_none()
                && view
                    .selection
                    .volume()
                    .is_some_and(|volume| volume.validate().is_ok())
        }
        Action::StartPaste => clipboard.current.is_some(),
        Action::Place | Action::RepeatPlace => view
            .preview
            .as_ref()
            .is_some_and(|preview| preview.target_valid || preview.moving),
        Action::MarkCorner => view.selection.can_mark(),
        Action::UnmarkCorner => view.selection.corners.iter().any(Option::is_some),
        Action::ClearSelection => {
            view.selection.corners.iter().any(Option::is_some)
                || view.selection.picking.is_some()
                || view.selection.missed
        }
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
    if view.paste.writes() {
        return;
    }
    let Some(stream) = &view.stream else { return };
    match action {
        Action::Save => {
            paste::start_save(view);
        }
        Action::Undo | Action::Redo => {
            let result = if action == Action::Undo {
                view.edits.undo()
            } else {
                view.edits.redo()
            };
            match result {
                Ok(Some(count)) => {
                    view.paste = if action == Action::Undo {
                        PasteState::Undone(count)
                    } else {
                        PasteState::Redone(count)
                    }
                }
                Ok(None) => {}
                Err(error) => view.paste = PasteState::Error(Diagnostic::from_error(error)),
            }
            view.preview_cancelled = false;
        }
        Action::Copy => {
            if let Some(volume) = view.selection.volume() {
                match clipboard.request(
                    document,
                    stream.source(),
                    volume,
                    view.edits.patch_in(volume),
                ) {
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
                view.selection.cancel_marking();
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
        Action::PinInspection => {
            if view.inspection_pinned && view.inspected == hit.map(|hit| hit.position) {
                view.inspection_pinned = false;
            } else if let Some(hit) = hit {
                view.inspected = Some(hit.position);
                view.inspection_pinned = true;
            }
        }
        Action::MarkCorner => {
            selection::mark_corner(&mut view.selection, hit.map(|hit| hit.position))
        }
        Action::UnmarkCorner => view.selection.unmark_last(),
        Action::ClearSelection => view.selection.clear(),
        Action::CancelSelection => view.selection.cancel_marking(),
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
    palette: &Palette,
    lang: Language,
    bindings: &Bindings,
) {
    let busy = workspace.pending.is_some() || view.paste.writes();
    let scene = view.stream.is_some() || view.demo_scene.is_some();
    if scene {
        let compact = ctx.content_rect().width() < 640.0;
        command_strip(ctx, |ui| {
            if !busy && !input_blocked(ctx) {
                help_bar(ui, view, captured, clipboard, lang, bindings, compact);
            }
        });
    }
    egui::TopBottomPanel::bottom("status")
        .exact_height(26.0)
        .frame(frame(BAR).inner_margin(egui::Margin::symmetric(11, 3)))
        .show(ctx, |ui| {
            let notice = status_notice(workspace, view, lang);
            if scene || notice.is_some() {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if view.stream.is_some() {
                        let width = (ui.available_width() * 0.23).clamp(100.0, 160.0);
                        ui.allocate_ui_with_layout(
                            egui::vec2(width, ui.available_height()),
                            egui::Layout::left_to_right(egui::Align::Center),
                            |ui| active_material::show(ui, Some(palette.active()), lang),
                        );
                        ui.separator();
                    }
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        horizontal_strip(ui, |ui| {
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
                                        egui::RichText::new(
                                            lang.format(Text::FailedChunks, &[&failed]),
                                        )
                                        .color(ALERT),
                                    )
                                    .on_hover_text(Diagnostic::from_detail(error).message(lang));
                                }
                            }
                            if view.edits.dirty() {
                                ui.separator();
                                ui.label(
                                    egui::RichText::new(
                                        lang.format(
                                            Text::SavePending,
                                            &[&view.edits.pending_count()],
                                        ),
                                    )
                                    .color(ALERT),
                                );
                            }
                            if let Some((status, color, spinner)) = notice {
                                if scene {
                                    ui.separator();
                                }
                                if spinner {
                                    ui.spinner();
                                }
                                ui.add(
                                    egui::Label::new(egui::RichText::new(&status).color(color))
                                        .truncate(),
                                )
                                .on_hover_text(status);
                            }
                        });
                    });
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
        PasteState::Saving(..) => (world(Text::SavingWorld), MUTED, true),
        PasteState::Preparing(..) => (world(Text::EditChecking), MUTED, true),
        PasteState::Error(error) => (error.message(lang), ALERT, false),
        PasteState::Edited(count) => (lang.format(Text::BlocksEdited, &[count]), ACCENT, false),
        PasteState::Undone(count) => (lang.format(Text::BlocksUndone, &[count]), ACCENT, false),
        PasteState::Redone(count) => (lang.format(Text::BlocksRedone, &[count]), ACCENT, false),
        _ if view.copying => (world(Text::CopyingWorld), MUTED, true),
        _ if view.preview_cancelled => (lang.text(Text::PasteCancelled).into(), MUTED, false),
        PasteState::Saved(_) => (world(Text::WorldSaved), ACCENT, false),
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

fn binding_label(bindings: &Bindings, action: Action, lang: Language, compact: bool) -> String {
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
        })
    })
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
    let binding = |action| binding_label(bindings, action, lang, compact);
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
    for (action, label) in [
        (Action::PinInspection, Text::InspectBlock),
        (Action::EraseBlock, Text::EraseBlock),
        (Action::PaintBlock, Text::PaintBlock),
        (Action::MarkCorner, Text::ActionMark),
        (Action::UnmarkCorner, Text::UnmarkCorner),
        (Action::ClearSelection, Text::ClearSelection),
        (Action::CancelSelection, Text::ActionCancel),
        (Action::Place, Text::ActionPlace),
        (Action::RepeatPlace, Text::ActionRepeat),
        (Action::CancelPaste, Text::ActionCancel),
    ] {
        if bindings.available(action, context) && action_enabled(action, view, clipboard, captured)
        {
            hint(binding(action), label);
        }
    }
    if !captured && bindings.available(Action::CameraLook, context) {
        hint(binding(Action::CameraLook), Text::CameraLook);
    }
    if captured || context == EditorContext::Explore {
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
        hint(binding(Action::CameraUp), Text::CameraRise);
        hint(binding(Action::CameraDown), Text::CameraDescend);
        if captured {
            hint(binding(Action::CameraBoost), Text::CameraBoost);
            if !(view.preview.is_some() || view.selection.picking.is_some()) {
                hint(binding(Action::CameraRelease), Text::CameraRelease);
            }
        }
    }
    if action_enabled(Action::ToggleCameraCapture, view, clipboard, captured) {
        hint(binding(Action::ToggleCameraCapture), Text::CameraCapture);
    }
    for (action, label) in [
        (Action::OpenPalette, Text::BlockPalette),
        (Action::Copy, Text::ActionCopy),
        (Action::StartPaste, Text::ActionPaste),
        (Action::Save, Text::SaveWorld),
        (Action::Undo, Text::UndoEdit),
        (Action::Redo, Text::RedoEdit),
    ] {
        if bindings.available(action, context) && action_enabled(action, view, clipboard, captured)
        {
            hint(binding(action), label);
        }
    }
    if view.stream.is_some() && !view.paste.locks_preview() {
        let tools = TOOL_BUTTONS
            .into_iter()
            .filter_map(|(action, _, _)| {
                bindings.available(action, context).then(|| binding(action))
            })
            .filter(|label| !label.is_empty())
            .collect::<Vec<_>>();
        hint(tools.join(" / "), Text::Tools);
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
        PasteState::Idle
        | PasteState::Saving(_)
        | PasteState::Edited(_)
        | PasteState::Undone(_)
        | PasteState::Redone(_) => None,
        PasteState::Preparing(EditIntent::Single, _)
        | PasteState::Preparing(EditIntent::Paste { confirm: false, .. }, _) => None,
        PasteState::Preparing(..) => ui
            .small_button(lang.text(Text::CancelPreparation))
            .clicked()
            .then_some(PasteAction::Discard),
        PasteState::Ready(intent, operation) => {
            if let EditIntent::Region {
                volume,
                material,
                matching,
            } = intent
            {
                let [x, y, z] = volume.dimensions();
                ui.label(lang.format(Text::Dimensions, &[&x, &y, &z]));
                if let Some(matching) = matching {
                    ui.label(lang.format(Text::RegionFilter, &[matching, material]));
                } else {
                    ui.label(lang.format(Text::RegionMaterial, &[material]));
                }
                if operation.len() >= 32_768 {
                    ui.label(egui::RichText::new(lang.text(Text::RegionLarge)).color(ALERT));
                }
            }
            ui.label(
                egui::RichText::new(lang.format(Text::RegionAffected, &[&operation.len()]))
                    .strong(),
            );
            ui.horizontal_wrapped(|ui| {
                let apply = lang.text(Text::ApplyMemory);
                let cancel = lang.text(Text::ActionCancel);
                if appearance::button(ui, None, apply, apply, ButtonStyle::Primary, true).clicked()
                {
                    Some(PasteAction::Confirm)
                } else if appearance::button(ui, None, cancel, cancel, ButtonStyle::Quiet, true)
                    .clicked()
                {
                    Some(PasteAction::Discard)
                } else {
                    None
                }
            })
            .inner
        }
        PasteState::Saved(result) => {
            ui.label(lang.format(Text::WorldSavedCount, &[&result.blocks]));
            if let Some(backup) = &result.backup {
                ui.collapsing(lang.text(Text::Backup), |ui| {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(backup.display().to_string()).monospace(),
                        )
                        .wrap(),
                    );
                });
            }
            ui.small_button(lang.text(Text::CloseNotice))
                .clicked()
                .then_some(PasteAction::Discard)
        }
        PasteState::Error(error) => {
            diagnostic(ui, &lang.format(Text::EditFailed, &[&name]), error, lang);
            ui.small_button(lang.text(Text::CloseNotice))
                .clicked()
                .then_some(PasteAction::Discard)
        }
    }
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
    if let Some(corner) = selection.next_corner() {
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
            selection.cancel_marking();
        }
    } else if ui.button(lang.text(Text::EditCoordinates)).clicked() {
        let near_camera = position.floor().as_ivec3().to_array();
        selection.corners[corner] = Some(selection.corners[1 - corner].unwrap_or_else(|| {
            std::array::from_fn(|axis| {
                let (min, max) = COORDINATE_LIMITS[axis];
                near_camera[axis].clamp(min, max)
            })
        }));
        selection.cancel_marking();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captured_pointer_buttons_cannot_hit_widgets_under_the_os_cursor() {
        let center = egui::pos2(300.0, 240.0);
        let chrome = egui::pos2(19.0, 8.0);
        let mut input = egui::RawInput {
            events: vec![
                egui::Event::PointerGone,
                egui::Event::PointerButton {
                    pos: chrome,
                    button: egui::PointerButton::Secondary,
                    pressed: true,
                    modifiers: egui::Modifiers::SHIFT,
                },
            ],
            ..Default::default()
        };
        center_captured_pointer(&mut input, center);
        assert!(matches!(input.events.as_slice(), [
            egui::Event::PointerMoved(point),
            egui::Event::PointerButton { pos, modifiers, .. }
        ] if *point == center && *pos == center && modifiers.shift));
    }

    #[test]
    fn fields_popups_and_preferences_block_viewport_shortcuts() {
        let ctx = egui::Context::default();
        appearance::apply(&ctx);
        let mut text = String::from("18");
        let field = egui::Id::new("coordinate");
        assert!(!input_blocked(&ctx));
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.add(egui::TextEdit::singleline(&mut text).id(field))
                    .request_focus();
                assert!(input_blocked(ctx));
            });
        });
        ctx.memory_mut(|memory| memory.surrender_focus(field));
        assert!(!input_blocked(&ctx));
        let popup = egui::Id::new("edit popup");
        egui::Popup::open_id(&ctx, popup);
        assert!(input_blocked(&ctx));
        egui::Popup::close_id(&ctx, popup);
        assert!(!input_blocked(&ctx));
        let mut chrome = menus::window_state(&ctx);
        chrome.dialog = Some(dialogs::Dialog::Preferences);
        menus::save_window_state(&ctx, chrome);
        assert!(input_blocked(&ctx));
    }

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
