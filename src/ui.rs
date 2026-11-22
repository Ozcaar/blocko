use std::sync::Arc;

use bevy::{camera::Viewport, prelude::*};
use bevy_egui::{EguiContext, EguiFullOutput, EguiInput, egui};

use crate::{
    editor::{self, DemoSelection, Destination, Overlay, PendingOpen, SceneAssets, View},
    flight::Flight,
    paste::{self, PasteAction, PasteState, PasteSummary},
    placement::{AirMode, Placement},
    selection::{Selection, preview_origin},
    structure::{Clipboard, MAX_HORIZONTAL, MAX_Y, MIN_HORIZONTAL, MIN_Y, Volume},
};

const BAR: egui::Color32 = egui::Color32::from_rgb(20, 27, 32);
const PANEL: egui::Color32 = egui::Color32::from_rgb(25, 34, 39);
const LINE: egui::Color32 = egui::Color32::from_rgb(47, 62, 67);
const TEXT: egui::Color32 = egui::Color32::from_rgb(229, 238, 235);
const MUTED: egui::Color32 = egui::Color32::from_rgb(151, 174, 174);
const ACCENT: egui::Color32 = egui::Color32::from_rgb(112, 224, 178);
const ALERT: egui::Color32 = egui::Color32::from_rgb(244, 180, 112);

pub fn context() -> EguiContext {
    let mut context = EguiContext::default();
    let mut visuals = egui::Visuals::dark();
    visuals.widgets.inactive.bg_fill = PANEL;
    visuals.widgets.inactive.bg_stroke = egui::Stroke::new(1.0_f32, LINE);
    visuals.widgets.hovered.bg_fill = LINE;
    visuals.selection.bg_fill = egui::Color32::from_rgb(29, 95, 74);
    visuals.override_text_color = Some(TEXT);
    context.get_mut().set_visuals(visuals);
    context
}

pub fn draw(
    mut commands: Commands,
    mut windows: Query<(&Window, &mut View)>,
    mut overlays: Query<(
        &Overlay,
        &mut EguiContext,
        &mut EguiInput,
        &mut EguiFullOutput,
    )>,
    mut cameras: Query<(&mut Camera, &Transform), With<Flight>>,
    mut demo_selection: ResMut<DemoSelection>,
    mut clipboard: ResMut<Clipboard>,
    assets: Res<SceneAssets>,
) {
    let mut demo_requested = false;
    for (overlay, mut egui, mut input, mut output) in &mut overlays {
        let Ok((window, mut view)) = windows.get_mut(overlay.0) else {
            continue;
        };
        let Ok((mut camera, transform)) = cameras.get_mut(view.camera) else {
            continue;
        };
        let mut actions = WindowActions::default();
        let mut viewport = None;
        **output = Some(egui.get_mut().run(input.take(), |ctx| {
            let requested = window_ui(
                ctx,
                &mut view,
                transform.translation,
                &mut demo_selection,
                &mut demo_requested,
                &clipboard,
            );
            actions.open = actions.open.or(requested.open);
            actions.copy = actions.copy.or(requested.copy);
            actions.start_preview |= requested.start_preview;
            if let Some(action) = requested.paste {
                actions.paste.get_or_insert(action);
            }
            viewport = Some((ctx.available_rect(), ctx.pixels_per_point()));
        }));
        if let Some((rect, pixels_per_point)) = viewport {
            let logical = pixels_per_point / window.scale_factor();
            view.viewport = Some(Rect::from_corners(
                Vec2::new(rect.min.x, rect.min.y) * logical,
                Vec2::new(rect.max.x, rect.max.y) * logical,
            ));
            let min = to_pixels(rect.min, pixels_per_point, window);
            let max = to_pixels(rect.max, pixels_per_point, window).max(min);
            camera.is_active = max.x > min.x && max.y > min.y;
            if camera.is_active {
                camera.viewport = Some(Viewport {
                    physical_position: min,
                    physical_size: max - min,
                    ..default()
                });
            }
        }
        if let Some(action) = actions.paste {
            paste::apply_action(&mut view, action);
        } else if let Some(destination) = actions.open {
            view.error = None;
            view.pending = Some(PendingOpen::browse(destination));
        } else if let (Some(volume), Some(stream)) = (actions.copy, &view.stream) {
            let source = stream.source();
            match clipboard.request(overlay.0, source, volume) {
                Ok(()) => {
                    view.copying = true;
                    view.copy_notice = None;
                }
                Err(error) => view.copy_notice = Some(format!("{error:#}")),
            }
        } else if actions.start_preview
            && let (Some(source), Some(stream)) = (&clipboard.current, &view.stream)
        {
            let origin = preview_origin(stream, transform);
            view.preview = Some(Placement::new(Arc::clone(source), origin));
            stop_marking(&mut view.selection);
        }
    }
    if demo_requested {
        editor::new_demo(
            &mut commands,
            windows.iter().map(|(_, view)| view.layer),
            &assets,
        );
    }
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
        .inner_margin(egui::Margin::symmetric(16, 10))
}

#[derive(Default)]
struct WindowActions {
    open: Option<Destination>,
    copy: Option<Volume>,
    start_preview: bool,
    paste: Option<PasteAction>,
}

fn window_ui(
    ctx: &egui::Context,
    view: &mut View,
    position: Vec3,
    demo_selection: &mut DemoSelection,
    demo_requested: &mut bool,
    clipboard: &Clipboard,
) -> WindowActions {
    let mut actions = WindowActions::default();
    let opening = view.pending.is_some();
    let paste_locked = view.paste.locks_preview();
    let navigation_locked = opening || paste_locked;
    let name = view.stream.as_ref().map(|stream| stream.name.as_str());
    egui::TopBottomPanel::top("barra")
        .frame(frame(BAR))
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("Blocko")
                        .size(21.0)
                        .strong()
                        .color(ACCENT),
                );
                ui.add_space(14.0);
                ui.separator();
                ui.add_space(8.0);
                let text = if name.is_some() {
                    "Cambiar mundo…"
                } else {
                    "Abrir mundo…"
                };
                if ui
                    .add_enabled(
                        !navigation_locked,
                        egui::Button::new(egui::RichText::new(text).color(egui::Color32::BLACK))
                            .fill(ACCENT),
                    )
                    .clicked()
                {
                    actions.open = Some(Destination::Current);
                }
                if ui
                    .add_enabled(
                        !navigation_locked,
                        egui::Button::new("Abrir en otra ventana…"),
                    )
                    .clicked()
                {
                    actions.open = Some(Destination::NewWindow);
                }
                if opening {
                    ui.spinner();
                }
            });
        });

    egui::TopBottomPanel::bottom("coordenadas")
        .frame(frame(BAR))
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(if name.is_some() {
                        "Perspectiva 3D"
                    } else {
                        "Visor 3D"
                    })
                    .color(MUTED),
                );
                if name.is_some() || view.demo_scene.is_some() {
                    ui.separator();
                    ui.monospace(format!(
                        "X {:.1}    Y {:.1}    Z {:.1}",
                        position.x, position.y, position.z
                    ));
                }
            });
        });

    let panel_width = (ctx.content_rect().width() * 0.27).clamp(165.0, 254.0);
    egui::SidePanel::left("mundo")
        .exact_width(panel_width)
        .resizable(false)
        .frame(frame(PANEL))
        .show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.add_space(8.0);
                ui.label(egui::RichText::new("Mundo activo").small().color(MUTED));
                ui.label(
                    egui::RichText::new(name.unwrap_or("Sin mundo abierto"))
                        .size(21.0)
                        .strong(),
                );
                if let Some(stream) = &view.stream {
                    ui.label(egui::RichText::new("Overworld").color(ACCENT));
                    ui.add_space(14.0);
                    let (visible, absent, failed, loading) = stream.status();
                    ui.label(format!("{visible} chunks visibles"));
                    if loading {
                        ui.label(egui::RichText::new("Cargando terreno…").color(MUTED));
                    } else if visible == 0 && absent != 0 {
                        ui.label(
                            egui::RichText::new("No hay terreno generado cerca.").color(MUTED),
                        );
                    }
                    if let Some(error) = stream.first_error() {
                        ui.label(
                            egui::RichText::new(format!("{failed} chunks con error")).color(ALERT),
                        )
                        .on_hover_text(error);
                    }
                } else if view.demo_scene.is_none() {
                    ui.add_space(7.0);
                    ui.label(
                        egui::RichText::new(
                            "Elige la carpeta de una copia de Minecraft Java 1.21.1.",
                        )
                        .color(MUTED),
                    );
                }
                if let Some(error) = &view.error {
                    ui.add_space(8.0);
                    ui.label(egui::RichText::new(error).color(ALERT));
                }
                panel_heading(ui, "Portapapeles compartido");
                clipboard_contents(ui, clipboard);
                if name.is_some()
                    && (clipboard.current.is_some()
                        || view.preview.is_some()
                        || !matches!(view.paste, PasteState::Idle))
                {
                    panel_heading(
                        ui,
                        if paste_locked {
                            "Guardar pegado"
                        } else {
                            "Vista previa de pegado"
                        },
                    );
                    let recalculating = view
                        .stream
                        .as_ref()
                        .is_some_and(|stream| stream.preview_busy());
                    ui.add_enabled_ui(!opening, |ui| {
                        actions.paste = paste_status(ui, &view.paste);
                        if !paste_locked {
                            let Some(preview) = &mut view.preview else {
                                actions.start_preview = clipboard.current.is_some()
                                    && ui.button("Previsualizar pegado").clicked();
                                return;
                            };
                            if preview_controls(
                                ui,
                                preview,
                                &mut view.selection,
                                clipboard,
                                recalculating,
                            ) {
                                view.preview = None;
                                if matches!(view.paste, PasteState::Error(_)) {
                                    actions.paste = Some(PasteAction::Discard);
                                }
                                return;
                            }
                            let prepare_label = if matches!(view.paste, PasteState::Error(_)) {
                                "Reintentar preparación"
                            } else {
                                "Preparar pegado"
                            };
                            if ui
                                .add_enabled(!preview.moving, egui::Button::new(prepare_label))
                                .clicked()
                            {
                                actions.paste = Some(PasteAction::Prepare);
                            }
                        }
                    });
                }
                if name.is_some() {
                    panel_heading(ui, "Selección 3D");
                    actions.copy = selection_controls(
                        ui,
                        &mut view.selection,
                        position,
                        navigation_locked,
                        clipboard.copying(),
                    );
                    if !paste_locked
                        && view.selection.picking.is_some()
                        && let Some(preview) = &mut view.preview
                    {
                        preview.moving = false;
                        preview.notice = None;
                    }
                    if view.copying {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(egui::RichText::new("Copiando…").color(MUTED));
                        });
                    }
                    if let Some(notice) = &view.copy_notice {
                        ui.label(egui::RichText::new(notice).color(MUTED));
                    }
                }
                if name.is_some() || view.demo_scene.is_some() {
                    panel_heading(ui, "Cámara libre");
                    ui.label(egui::RichText::new("Botón derecho + ratón para mirar").color(MUTED));
                    ui.label(egui::RichText::new("WASD para moverte").color(MUTED));
                    ui.label(egui::RichText::new("Espacio / Ctrl para subir y bajar").color(MUTED));
                    ui.label(egui::RichText::new("Mayús para acelerar").color(MUTED));
                }
                if view.demo_scene.is_some() {
                    panel_heading(ui, "Prueba de ventanas");
                    if ui.button("Otra ventana de prueba").clicked() {
                        *demo_requested = true;
                    }
                    if ui.button("Compartir selección ficticia").clicked() {
                        demo_selection.0 =
                            Some(format!("Volumen 3 × 2 × 3 · ventana {}", view.layer));
                    }
                    ui.label(
                        demo_selection
                            .0
                            .as_deref()
                            .unwrap_or("Sin selección compartida"),
                    );
                }
            });
        });
    actions
}

fn panel_heading(ui: &mut egui::Ui, label: &str) {
    ui.add_space(12.0);
    ui.separator();
    ui.add_space(5.0);
    ui.label(egui::RichText::new(label).strong());
}

fn clipboard_contents(ui: &mut egui::Ui, clipboard: &Clipboard) {
    if let Some(structure) = &clipboard.current {
        let [x, y, z] = structure.volume.dimensions();
        ui.label(egui::RichText::new(&structure.source_name).color(ACCENT));
        ui.monospace(format!("{x} × {y} × {z}"));
        let blocks = structure.block_count();
        ui.label(egui::RichText::new(format!("{blocks} bloques, incluido el aire")).color(MUTED));
        let entities = structure.block_entity_count();
        if entities != 0 {
            ui.label(egui::RichText::new(format!("{entities} entidades de bloque")).color(MUTED));
        }
    } else if !clipboard.copying() {
        ui.label(egui::RichText::new("Sin copia en esta sesión").color(MUTED));
    }
    if clipboard.copying() {
        ui.label(egui::RichText::new("Preparando una copia…").color(MUTED));
    }
}

fn paste_status(ui: &mut egui::Ui, state: &PasteState) -> Option<PasteAction> {
    match state {
        PasteState::Idle => None,
        PasteState::Preparing(..) => {
            paste_progress(ui, "Comprobando el mundo de destino…");
            ui.small_button("Cancelar preparación")
                .clicked()
                .then_some(PasteAction::Discard)
        }
        PasteState::Ready(plan) => {
            ui.label(
                egui::RichText::new("Confirmar escribirá estos cambios en el mundo.")
                    .strong()
                    .color(ALERT),
            );
            paste_summary(ui, &plan.summary);
            ui.label("Se creará un respaldo antes de escribir.");
            ui.add_space(5.0);
            if ui.button("Volver a la vista previa").clicked() {
                return Some(PasteAction::Discard);
            }
            ui.add(
                egui::Button::new(
                    egui::RichText::new("Confirmar pegado").color(egui::Color32::BLACK),
                )
                .fill(ALERT),
            )
            .clicked()
            .then_some(PasteAction::Confirm)
        }
        PasteState::Saving(..) => {
            paste_progress(ui, "Guardando el mundo…");
            None
        }
        PasteState::Saved(result) => {
            ui.label(
                egui::RichText::new(format!("Pegado guardado en {}", result.summary.world_name))
                    .strong()
                    .color(ACCENT),
            );
            ui.label("Respaldo");
            ui.add(
                egui::Label::new(
                    egui::RichText::new(result.backup.display().to_string()).monospace(),
                )
                .wrap(),
            );
            ui.small_button("Cerrar aviso")
                .clicked()
                .then_some(PasteAction::Discard)
        }
        PasteState::Error(error) => {
            ui.label(egui::RichText::new(error).color(ALERT));
            ui.small_button("Descartar aviso")
                .clicked()
                .then_some(PasteAction::Discard)
        }
    }
}

fn paste_progress(ui: &mut egui::Ui, message: &str) {
    ui.horizontal(|ui| {
        ui.spinner();
        ui.label(egui::RichText::new(message).color(MUTED));
    });
}

fn paste_summary(ui: &mut egui::Ui, summary: &PasteSummary) {
    ui.label(egui::RichText::new(&summary.world_name).strong());
    ui.add(
        egui::Label::new(egui::RichText::new(summary.world_path.display().to_string()).monospace())
            .wrap(),
    );
    let [x, y, z] = summary.volume.min;
    ui.monospace(format!("Desde ({x}, {y}, {z})"));
    let [x, y, z] = summary.volume.max;
    ui.monospace(format!("Hasta ({x}, {y}, {z})"));
    let mode = match summary.mode {
        AirMode::Include => "Con aire",
        AirMode::Ignore => "Sin aire",
    };
    ui.label(format!(
        "{mode} · {} chunks · {} regiones",
        summary.chunks, summary.regions
    ));
}

fn preview_controls(
    ui: &mut egui::Ui,
    preview: &mut Placement,
    selection: &mut Selection,
    clipboard: &Clipboard,
    recalculating: bool,
) -> bool {
    let uses_current_copy = clipboard
        .current
        .as_ref()
        .is_some_and(|current| Arc::ptr_eq(&preview.source, current));
    if !uses_current_copy {
        let [x, y, z] = preview.volume().dimensions();
        ui.label(
            egui::RichText::new(format!(
                "Copia de {} · {x} × {y} × {z}",
                preview.source.source_name
            ))
            .color(MUTED),
        );
    }
    ui.horizontal(|ui| {
        ui.selectable_value(&mut preview.mode, AirMode::Include, "Con aire");
        ui.selectable_value(&mut preview.mode, AirMode::Ignore, "Sin aire");
    });
    ui.label(
        egui::RichText::new(match preview.mode {
            AirMode::Include => "El aire de la copia vacía esos bloques del destino.",
            AirMode::Ignore => "Los huecos de la copia conservan los bloques del destino.",
        })
        .color(MUTED),
    );

    ui.add_space(5.0);
    ui.label("Origen (esquina mínima)");
    let mut origin = preview.origin();
    if edit_coordinates(ui, &mut origin, preview.limits()) {
        preview.set_origin(origin);
        preview.moving = false;
        preview.notice = None;
    }
    if preview.moving {
        ui.label(
            egui::RichText::new("Haz clic sobre el terreno para fijar el origen.").color(ACCENT),
        );
        if ui.small_button("Cancelar movimiento").clicked() {
            preview.moving = false;
            preview.notice = None;
        }
    } else if ui.button("Mover en el visor").clicked() {
        preview.moving = true;
        preview.notice = None;
        stop_marking(selection);
    }
    if let Some(notice) = &preview.notice {
        ui.label(egui::RichText::new(notice).color(ALERT));
    }
    if recalculating {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(egui::RichText::new("Actualizando vista previa…").color(MUTED));
        });
    }
    ui.small_button("Cancelar vista previa").clicked()
}

fn selection_controls(
    ui: &mut egui::Ui,
    selection: &mut Selection,
    position: Vec3,
    opening: bool,
    copying: bool,
) -> Option<Volume> {
    ui.add_enabled_ui(!opening, |ui| {
        if ui.button("Marcar A y B en el visor").clicked() {
            arm_pick(selection, 0, true);
        }
        if let Some(corner) = selection.picking {
            ui.label(
                egui::RichText::new(format!(
                    "Haz clic sobre un bloque para marcar la esquina {}.",
                    corner_name(corner)
                ))
                .color(ACCENT),
            );
            if ui.small_button("Cancelar marca").clicked() {
                stop_marking(selection);
            }
        }
        if let Some(notice) = &selection.notice {
            ui.label(egui::RichText::new(notice).color(ALERT));
        }

        for corner in 0..2 {
            ui.push_id(corner, |ui| {
                corner_controls(ui, selection, corner, position)
            });
        }

        if selection.corners.iter().any(Option::is_some)
            && ui.small_button("Limpiar selección").clicked()
        {
            *selection = Selection::default();
        }

        let volume = selection.volume()?;
        let [x, y, z] = volume.dimensions();
        ui.monospace(format!("{x} × {y} × {z} bloques"));
        if let Err(error) = volume.validate() {
            ui.label(egui::RichText::new(format!("{error:#}")).color(ALERT));
            return None;
        }
        ui.add_enabled(
            !copying && selection.picking.is_none(),
            egui::Button::new("Copiar volumen"),
        )
        .clicked()
        .then_some(volume)
    })
    .inner
}

const COORDINATE_LIMITS: [(i32, i32); 3] = [
    (MIN_HORIZONTAL, MAX_HORIZONTAL),
    (MIN_Y, MAX_Y),
    (MIN_HORIZONTAL, MAX_HORIZONTAL),
];

fn corner_controls(ui: &mut egui::Ui, selection: &mut Selection, corner: usize, position: Vec3) {
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.label(format!("Esquina {}", corner_name(corner)));
        if ui
            .small_button("Marcar")
            .on_hover_text("Marcar esta esquina en el visor")
            .clicked()
        {
            arm_pick(selection, corner, false);
        }
    });
    if let Some(coords) = &mut selection.corners[corner] {
        if edit_coordinates(ui, coords, COORDINATE_LIMITS) {
            stop_marking(selection);
        }
    } else if ui.small_button("Editar coordenadas").clicked() {
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

fn edit_coordinates(ui: &mut egui::Ui, coords: &mut [i32; 3], limits: [(i32, i32); 3]) -> bool {
    egui::Grid::new("coordenadas")
        .show(ui, |ui| {
            let mut changed = false;
            for (axis, ((min, max), coordinate)) in ["X", "Y", "Z"]
                .into_iter()
                .zip(limits.into_iter().zip(coords))
            {
                ui.monospace(axis);
                changed |= ui
                    .add_sized(
                        [82.0, ui.spacing().interact_size.y],
                        egui::DragValue::new(coordinate)
                            .range(min..=max)
                            .update_while_editing(false),
                    )
                    .changed();
                ui.end_row();
            }
            changed
        })
        .inner
}

fn corner_name(corner: usize) -> &'static str {
    ["A", "B"][corner]
}

fn arm_pick(selection: &mut Selection, corner: usize, then_b: bool) {
    selection.picking = Some(corner);
    selection.pick_b_after_a = then_b;
    selection.notice = None;
}

fn stop_marking(selection: &mut Selection) {
    selection.picking = None;
    selection.pick_b_after_a = false;
    selection.notice = None;
}
