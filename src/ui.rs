use bevy::{camera::Viewport, prelude::*};
use bevy_egui::{EguiContext, EguiFullOutput, EguiInput, egui};

use crate::{
    editor::{self, DemoSelection, Destination, Overlay, PendingOpen, SceneAssets, View},
    flight::Flight,
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
    visuals.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, LINE);
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
        let mut open = None;
        let mut viewport = None;
        **output = Some(egui.get_mut().run(input.take(), |ctx| {
            open = window_ui(
                ctx,
                &view,
                transform.translation,
                &mut demo_selection,
                &mut demo_requested,
            );
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
        if let Some(destination) = open {
            view.error = None;
            view.pending = Some(PendingOpen::browse(destination));
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

fn window_ui(
    ctx: &egui::Context,
    view: &View,
    position: Vec3,
    selection: &mut DemoSelection,
    demo_requested: &mut bool,
) -> Option<Destination> {
    let mut open = None;
    let busy = view.pending.is_some();
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
                        !busy,
                        egui::Button::new(egui::RichText::new(text).color(egui::Color32::BLACK))
                            .fill(ACCENT),
                    )
                    .clicked()
                {
                    open = Some(Destination::Current);
                }
                if ui
                    .add_enabled(!busy, egui::Button::new("Abrir en otra ventana…"))
                    .clicked()
                {
                    open = Some(Destination::NewWindow);
                }
                if busy {
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
                    ui.label(egui::RichText::new("Overworld · Solo lectura").color(ACCENT));
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
                if name.is_some() || view.demo_scene.is_some() {
                    ui.add_space(15.0);
                    ui.separator();
                    ui.add_space(8.0);
                    ui.label(egui::RichText::new("Cámara libre").strong());
                    ui.label(egui::RichText::new("Botón derecho + ratón para mirar").color(MUTED));
                    ui.label(egui::RichText::new("WASD para moverte").color(MUTED));
                    ui.label(egui::RichText::new("Espacio / Ctrl para subir y bajar").color(MUTED));
                    ui.label(egui::RichText::new("Mayús para acelerar").color(MUTED));
                }
                if view.demo_scene.is_some() {
                    ui.add_space(15.0);
                    ui.separator();
                    ui.label(egui::RichText::new("Prueba de ventanas").strong());
                    if ui.button("Otra ventana de prueba").clicked() {
                        *demo_requested = true;
                    }
                    if ui.button("Compartir selección ficticia").clicked() {
                        selection.0 = Some(format!("Volumen 3 × 2 × 3 · ventana {}", view.layer));
                    }
                    ui.label(selection.0.as_deref().unwrap_or("Sin selección compartida"));
                }
            });
        });
    open
}
