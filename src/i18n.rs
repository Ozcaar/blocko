use std::fmt::Display;

use bevy::prelude::Resource;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Resource)]
pub enum Language {
    #[default]
    Spanish,
    English,
}

macro_rules! catalog {
    ($($key:ident => $es:literal, $en:literal;)*) => {
        #[derive(Clone, Copy)]
        pub enum Text { $($key),* }

        impl Language {
            pub fn text(self, key: Text) -> &'static str {
                let (es, en) = match key { $(Text::$key => ($es, $en),)* };
                let translated = match self { Self::Spanish => es, Self::English => en };
                if translated.is_empty() { es } else { translated }
            }
        }
    };
}

catalog! {
    DialogOpen => "Abrir carpeta del mundo de Minecraft", "Open Minecraft world folder";
    OpenWorld => "Abrir mundo…", "Open world…";
    SaveWorld => "Guardar", "Save";
    FileMenu => "Archivo", "File";
    EditMenu => "Editar", "Edit";
    ViewMenu => "Ver", "View";
    HelpMenu => "Ayuda", "Help";
    ActivityBar => "Barra de herramientas", "Tool bar";
    Inspector => "Inspector", "Inspector";
    Preferences => "Preferencias", "Preferences";
    Controls => "Controles y atajos", "Controls and shortcuts";
    About => "Acerca de Blocko", "About Blocko";
    CloseDialog => "Cerrar", "Close";
    AboutVersion => "Versión {0}", "Version {0}";
    AboutDescription => "Editor de escritorio para mundos de Minecraft Java Edition 1.21.1.", "Desktop editor for Minecraft Java Edition 1.21.1 worlds.";
    ControlsCamera => "Cámara", "Camera";
    ControlsEditing => "Edición", "Editing";
    UndoEdit => "Deshacer", "Undo";
    RedoEdit => "Rehacer", "Redo";
    BlockPalette => "Buscar bloque", "Find block";
    CopySelection => "Copiar selección", "Copy selection";
    PasteStructure => "Pegar construcción", "Paste structure";
    CancelOperation => "Cancelar operación", "Cancel operation";
    OpenReplace => "Reemplazar mundo de esta pestaña…", "Replace world in this tab…";
    OpenWindow => "Abrir en otra ventana…", "Open in new window…";
    Language => "Idioma", "Language";
    EmptyTitle => "Abre un mundo de Minecraft", "Open a Minecraft world";
    EmptyHelp => "Elige la carpeta de una copia de Minecraft Java 1.21.1.", "Choose a copy of a Minecraft Java 1.21.1 world.";
    OpenFolder => "Elige la carpeta del mundo…", "Choose the world folder…";
    Opening => "Abriendo {0}…", "Opening {0}…";
    Opened => "Mundo abierto · {0}", "World opened · {0}";
    OpenCancelled => "Apertura cancelada", "Opening cancelled";
    Duplicate => "{0} ya está abierto o cargando. Busca su pestaña o ventana.", "{0} is already open or loading. Find its tab or window.";
    OpenFailed => "No se pudo abrir {0}.", "Could not open {0}.";
    WorldFolder => "la carpeta elegida", "the selected folder";
    CloseTab => "Cerrar pestaña", "Close tab";
    DragTab => "Arrastra la pestaña fuera para separarla.", "Drag the tab outside the window to detach it.";
    SaveChangesTitle => "Cambios sin guardar", "Unsaved changes";
    UnsavedPrompt => "¿Guardar los cambios de «{0}» antes de continuar?", "Save changes to “{0}” before continuing?";
    DiscardChanges => "Descartar cambios", "Discard changes";
    SavePending => "Bloques sin guardar · {0}", "Unsaved blocks · {0}";
    EditChecking => "Comprobando cambios en {0}…", "Checking changes in {0}…";
    WorldSaved => "Mundo guardado · {0}", "World saved · {0}";
    WorldSavedCount => "Bloques guardados · {0}", "Blocks saved · {0}";
    EditFailed => "No se pudo completar la operación en {0}.", "Could not complete the operation in {0}.";
    BlocksEdited => "Bloques modificados · {0}", "Blocks edited · {0}";
    BlocksUndone => "Cambios deshechos · {0}", "Block changes undone · {0}";
    BlocksRedone => "Cambios rehechos · {0}", "Block changes redone · {0}";
    Explore => "Explorar", "Explore";
    Select => "Selección 3D", "3D selection";
    InspectBlock => "Inspeccionar bloque", "Inspect block";
    ManualEdit => "Edición manual", "Manual editing";
    PaintBlock => "Colocar bloque", "Place block";
    EraseBlock => "Borrar bloque", "Erase block";
    Placement => "Colocación", "Placement";
    NoTargetBlock => "Apunta a un bloque visible del terreno.", "Point at a visible terrain block.";
    ActiveMaterial => "Material activo", "Active material";
    Clipboard => "Portapapeles compartido", "Shared clipboard";
    EmptyClipboard => "Sin copia en esta sesión", "No copy in this session";
    CopyingClipboard => "Preparando una copia…", "Preparing a copy…";
    ClipboardBlocks => "{0} bloques, incluido el aire", "{0} blocks, including air";
    ClipboardEntity => "{0} entidad de bloque", "{0} block entity";
    ClipboardEntities => "{0} entidades de bloque", "{0} block entities";
    LoadingTerrain => "Cargando terreno…", "Loading terrain…";
    EmptyTerrain => "No hay terreno generado cerca", "No generated terrain nearby";
    VisibleChunks => "{0} chunks visibles", "{0} visible chunks";
    FailedChunks => "{0} chunks con error", "{0} chunks failed";
    Coordinates => "X {0}   Y {1}   Z {2}", "X {0}   Y {1}   Z {2}";
    CopyingWorld => "Copiando desde {0}…", "Copying from {0}…";
    CopyDone => "Copia de {0} · {1} × {2} × {3}", "Copied from {0} · {1} × {2} × {3}";
    CopyFailed => "No se pudo copiar desde {0}.", "Could not copy from {0}.";
    PickBothTool => "Marcar A/B", "Mark A/B";
    PickBoth => "Marcar A y B en el visor", "Mark A and B in the viewport";
    PickCorner => "Marca la esquina {0} en un bloque del visor.", "Mark corner {0} on a block in the viewport.";
    CancelMark => "Cancelar marca", "Cancel marking";
    PickMiss => "Acerca la cámara y elige un bloque visible.", "Move closer and choose a visible block.";
    UnmarkCorner => "Desmarcar esquina", "Unmark corner";
    ClearSelection => "Limpiar selección", "Clear selection";
    Dimensions => "{0} × {1} × {2} bloques", "{0} × {1} × {2} blocks";
    Corner => "Esquina {0}", "Corner {0}";
    Mark => "Marcar", "Mark";
    EditCoordinates => "Editar coordenadas", "Edit coordinates";
    RegionEdit => "Editar selección", "Edit selection";
    RegionFill => "Rellenar", "Fill";
    RegionReplace => "Reemplazar", "Replace";
    ReplaceSource => "Tipo de bloque que se reemplazará", "Block type to replace";
    UseAsReplaceSource => "Usar como tipo de origen", "Use as source type";
    RegionFilter => "De {0} a {1}", "From {0} to {1}";
    RegionMaterial => "Rellenar con {0}", "Fill with {0}";
    RegionLarge => "Esta edición es grande. Revisa el alcance antes de aplicarla.", "This is a large edit. Review its scope before applying.";
    RegionAffected => "Bloques afectados · {0}", "Affected blocks · {0}";
    ApplyMemory => "Aplicar cambios", "Apply changes";
    SavingWorld => "Guardando · {0}…", "Saving · {0}…";
    PasteCancelled => "Vista previa cancelada", "Preview cancelled";
    CancelPreparation => "Cancelar preparación", "Cancel preparation";
    PrepareTool => "Preparar", "Prepare";
    PreparePaste => "Preparar pegado", "Prepare paste";
    Backup => "Respaldo", "Backup";
    CloseNotice => "Cerrar aviso", "Dismiss notice";
    ErrorDetails => "Detalles", "Details";
    IncludeAir => "Con aire", "With air";
    IgnoreAir => "Sin aire", "Without air";
    IncludeAirHelp => "El aire de la copia vacía esos bloques del destino.", "Air in the copy clears those blocks in the destination.";
    IgnoreAirHelp => "Los huecos conservan los bloques del destino.", "Gaps preserve the destination blocks.";
    PasteSource => "Origen de la copia", "Clipboard source";
    Origin => "Origen de pegado (esquina mínima)", "Paste origin (minimum corner)";
    FollowTool => "Cursor", "Cursor";
    Follow => "Seguir cursor", "Follow cursor";
    PreciseTool => "Coordenadas", "Coordinates";
    Precise => "Fijar coordenadas", "Pin coordinates";
    NoPasteTarget => "Apunta a una cara visible del terreno o fija las coordenadas.", "Point at a visible terrain face or pin the coordinates.";
    PreviewUpdating => "Actualizando vista previa…", "Updating preview…";
    PreviewDestination => "Vista previa · {0}", "Preview · {0}";
    CameraLook => "Mirar", "Look";
    CameraMove => "Mover", "Move";
    CameraRise => "Subir", "Rise";
    CameraDescend => "Bajar", "Descend";
    CameraBoost => "Acelerar", "Boost";
    CameraRelease => "Soltar cámara", "Release camera";
    CameraCapture => "Cámara persistente", "Persistent camera";
    ActionCopy => "Copiar", "Copy";
    ActionPaste => "Pegar", "Paste";
    ActionPlace => "Colocar", "Place";
    ActionRepeat => "Repetir", "Repeat";
    ActionCancel => "Cancelar", "Cancel";
    ActionMark => "Marcar esquina", "Mark corner";
    KeyLeftClickShort => "Clic izq.", "Left click";
    KeyLeftClick => "Clic izquierdo", "Left click";
    KeyRightClickShort => "Botón der.", "Right mouse";
    KeyRightClick => "Botón derecho", "Right mouse";
    KeyMiddleClick => "Botón central", "Middle mouse";
    KeyMouse4 => "Botón 4", "Mouse 4";
    KeyMouse5 => "Botón 5", "Mouse 5";
    KeyBackspace => "Retroceso", "Backspace";
    KeySpace => "Espacio", "Space";
    Demo => "Prueba de ventanas", "Window demo";
    DemoWindow => "Otra ventana de prueba", "Another demo window";
    DemoCopy => "Compartir selección ficticia", "Share demo selection";
    DemoSelection => "Volumen 3 × 2 × 3 · ventana {0}", "Volume 3 × 2 × 3 · window {0}";
    DemoNoSelection => "Sin selección compartida", "No shared selection";
}

impl Language {
    pub const ALL: [Self; 2] = [Self::Spanish, Self::English];

    pub fn name(self) -> &'static str {
        match self {
            Self::Spanish => "Español",
            Self::English => "English",
        }
    }

    pub fn format(self, key: Text, values: &[&dyn Display]) -> String {
        let template = self.text(key);
        let mut output = String::with_capacity(template.len());
        for part in template.split_inclusive('}') {
            let value = part
                .strip_suffix('}')
                .and_then(|part| part.rsplit_once('{'))
                .and_then(|(prefix, index)| {
                    Some((prefix, values.get(index.parse::<usize>().ok()?)?))
                });
            if let Some((prefix, value)) = value {
                use std::fmt::Write;
                output.push_str(prefix);
                let _ = write!(output, "{value}");
            } else {
                output.push_str(part);
            }
        }
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_world_names_verbatim_when_formatting_messages() {
        assert_eq!(
            Language::English.format(Text::CopyDone, &[&"Bosque {1}", &1, &2, &3]),
            "Copied from Bosque {1} · 1 × 2 × 3"
        );
    }
}
