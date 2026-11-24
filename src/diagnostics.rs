use std::io;

use crate::i18n::Language;

/// A localized summary can be rendered again after the user changes language.
#[derive(Debug)]
pub struct Diagnostic {
    kind: Kind,
    location: Option<String>,
    detail: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    IncompatibleVersion,
    WorldFolder,
    InvalidLevelData,
    EmptyWorld,
    WorldLocked,
    WorldChanged,
    Recover,
    RecoveryPending,
    WriteRolledBack,
    ReadOnlyWorld,
    OutsideWorld,
    SelectionTooLarge,
    CopyInProgress,
    DoubleChest,
    CopyData,
    MissingTerrain,
    UnsupportedDestination,
    InvalidChunk,
    NoChanges,
    SaveTooLarge,
    PendingChangesLimit,
    EditConflict,
    Permission,
    MissingFile,
    DiskFull,
    FileAccess,
    Unclassified,
}

impl Diagnostic {
    pub fn from_error(error: anyhow::Error) -> Self {
        let detail = format!("{error:#}");
        let chain = error.chain().map(ToString::to_string).collect::<Vec<_>>();
        let io_error = error.downcast_ref::<io::Error>();
        Self::new(
            chain.iter().map(String::as_str),
            io_error.map(io::Error::kind),
            detail,
        )
    }

    pub fn from_detail(detail: impl Into<String>) -> Self {
        let detail = detail.into();
        Self::new(std::iter::once(detail.as_str()), None, detail.clone())
    }

    fn new<'a>(
        mut chain: impl Iterator<Item = &'a str> + Clone,
        io_error: Option<io::ErrorKind>,
        detail: String,
    ) -> Self {
        // The storage layer still returns anyhow. Classify its stable message prefixes
        // at this boundary, then use the typed OS cause before generic context.
        let kind = chain
            .clone()
            .find_map(specific_kind)
            .or_else(|| io_error.and_then(io_kind))
            .or_else(|| chain.clone().find_map(context_kind))
            .unwrap_or(Kind::Unclassified);
        let location = chain
            .clone()
            .find_map(backup_location)
            .or_else(|| chain.find_map(chunk_location));
        Self {
            kind,
            location,
            detail,
        }
    }

    pub fn message(&self, language: Language) -> String {
        match language {
            Language::Spanish => self.detail.clone(),
            Language::English => {
                let summary = self.kind.english();
                self.location.as_ref().map_or_else(
                    || summary.to_owned(),
                    |location| format!("{summary}\n{location}"),
                )
            }
        }
    }

    pub fn detail(&self) -> &str {
        &self.detail
    }
}

fn specific_kind(cause: &str) -> Option<Kind> {
    const PREFIXES: &[(&str, Kind)] = &[
        ("Falló el guardado:", Kind::RecoveryPending),
        (
            "Se restauraron las regiones tras el fallo:",
            Kind::WriteRolledBack,
        ),
        (
            "Quedó una restauración pendiente en ",
            Kind::RecoveryPending,
        ),
        ("Registro de respaldo ", Kind::RecoveryPending),
        (
            "Falta la identidad del mundo en el registro de respaldo",
            Kind::RecoveryPending,
        ),
        (
            "Falta la región en el registro de respaldo",
            Kind::RecoveryPending,
        ),
        ("Identidad del mundo inválida", Kind::RecoveryPending),
        ("Regiones pendientes de restaurar:", Kind::RecoveryPending),
        ("El mundo tiene una restauración pendiente", Kind::Recover),
        (
            "Cierra esta ventana y vuelve a abrir el mundo",
            Kind::Recover,
        ),
        ("El mundo se guardó con ", Kind::IncompatibleVersion),
        (
            "El chunk de destino debe estar guardado en ",
            Kind::IncompatibleVersion,
        ),
        ("Minecraft tiene abierto este mundo", Kind::WorldLocked),
        (
            "El mundo está abierto en Minecraft o en otra ventana",
            Kind::WorldLocked,
        ),
        (
            "No se pudo bloquear el mundo para guardar",
            Kind::WorldLocked,
        ),
        (
            "La carpeta debe contener el archivo level.dat",
            Kind::WorldFolder,
        ),
        (
            "No se encontró la carpeta region del Overworld",
            Kind::WorldFolder,
        ),
        (
            "El archivo level.dat no contiene metadatos",
            Kind::InvalidLevelData,
        ),
        ("level.dat cambió", Kind::WorldChanged),
        ("El archivo creció durante la lectura", Kind::WorldChanged),
        ("Este mundo no se puede guardar:", Kind::ReadOnlyWorld),
        (
            "Las esquinas deben estar dentro de los límites",
            Kind::OutsideWorld,
        ),
        ("La selección contiene ", Kind::SelectionTooLarge),
        ("Hay una copia en curso", Kind::CopyInProgress),
        ("Cofre doble en ", Kind::DoubleChest),
        ("Entidad de bloque no compatible en ", Kind::CopyData),
        ("No se pueden copiar los datos del chunk ", Kind::CopyData),
        ("Faltan los datos de ", Kind::CopyData),
        (
            "El Overworld no contiene chunks generados",
            Kind::EmptyWorld,
        ),
        (
            "El chunk de destino no está completamente generado",
            Kind::MissingTerrain,
        ),
        ("La sección de destino en ", Kind::MissingTerrain),
        ("Falta el bloque de destino", Kind::MissingTerrain),
        (
            "Entidades de bloque de destino incompatibles",
            Kind::UnsupportedDestination,
        ),
        (
            "Entidad de bloque de destino no compatible",
            Kind::UnsupportedDestination,
        ),
        (
            "Faltan datos compatibles de la entidad",
            Kind::UnsupportedDestination,
        ),
        (
            "La entidad de bloque no corresponde al estado solicitado",
            Kind::UnsupportedDestination,
        ),
        ("Bloque de mod no compatible", Kind::UnsupportedDestination),
        ("El cambio en ", Kind::UnsupportedDestination),
        ("El pegado afecta ", Kind::UnsupportedDestination),
        (
            "El chunk de destino tiene una actualización de versión pendiente",
            Kind::UnsupportedDestination,
        ),
        (
            "La vista previa no cambiaría ningún bloque",
            Kind::NoChanges,
        ),
        ("La operación no cambiaría ningún bloque", Kind::NoChanges),
        ("El guardado requiere preparar más de ", Kind::SaveTooLarge),
        ("El pegado requiere preparar más de ", Kind::SaveTooLarge),
        (
            "El archivo es demasiado grande para preparar el guardado",
            Kind::SaveTooLarge,
        ),
        (
            "Hay demasiados cambios pendientes.",
            Kind::PendingChangesLimit,
        ),
        ("La edición cambió durante la operación", Kind::EditConflict),
    ];
    if let Some((_, kind)) = PREFIXES
        .iter()
        .find(|(prefix, _)| cause.starts_with(prefix))
    {
        return Some(*kind);
    }
    if cause.starts_with("La región ")
        && [
            " cambió ",
            " no existía al abrir ",
            " fue sustituida externamente ",
        ]
        .iter()
        .any(|fragment| cause.contains(fragment))
    {
        return Some(Kind::WorldChanged);
    }
    if cause.starts_with("La posición ") && cause.ends_with("fuera de los límites del Overworld")
    {
        return Some(Kind::OutsideWorld);
    }
    if cause.starts_with("El bloque en ")
        && cause.ends_with("tiene etiquetas de paleta incompatibles")
    {
        return Some(Kind::UnsupportedDestination);
    }
    if let Some(chunk) = cause.strip_prefix("El chunk (")
        && let Some((_, reason)) = chunk.split_once(") ")
    {
        if reason.starts_with("no contiene terreno guardado")
            || reason.starts_with("no tiene sectores generados")
        {
            return Some(Kind::MissingTerrain);
        }
        if reason.starts_with("usa un archivo externo")
            || reason.starts_with("requeriría un archivo externo")
        {
            return Some(Kind::UnsupportedDestination);
        }
    }
    None
}

fn io_kind(kind: io::ErrorKind) -> Option<Kind> {
    match kind {
        io::ErrorKind::PermissionDenied | io::ErrorKind::ReadOnlyFilesystem => {
            Some(Kind::Permission)
        }
        io::ErrorKind::NotFound => Some(Kind::MissingFile),
        io::ErrorKind::StorageFull => Some(Kind::DiskFull),
        _ => None,
    }
}

fn context_kind(cause: &str) -> Option<Kind> {
    if [
        "No se pudo leer el chunk ",
        "No se pudo leer el NBT del chunk ",
        "Chunk destino ",
        "Encabezado Anvil ",
        "Estructura NBT de chunk Java ",
        "Compresión Anvil ",
    ]
    .iter()
    .any(|prefix| cause.starts_with(prefix))
    {
        return Some(Kind::InvalidChunk);
    }
    cause.starts_with("No se pudo ").then_some(Kind::FileAccess)
}

fn backup_location(cause: &str) -> Option<String> {
    if cause.starts_with("Falló el guardado:") || cause.starts_with("Se restauraron las regiones")
    {
        return cause
            .rsplit_once(". Respaldo: ")
            .map(|(_, path)| path.to_owned());
    }
    cause
        .strip_prefix("Quedó una restauración pendiente en ")
        .map(str::to_owned)
}

fn chunk_location(cause: &str) -> Option<String> {
    const PREFIXES: &[&str] = &[
        "El chunk (",
        "Chunk destino (",
        "No se pudo leer el chunk (",
        "No se pudo leer el NBT del chunk (",
        "No se pueden copiar los datos del chunk (",
        "Cofre doble en (",
        "Entidad de bloque no compatible en (",
        "Entidad de bloque de destino no compatible en (",
        "Bloque de mod no compatible en (",
        "La sección de destino en (",
        "El cambio en (",
    ];
    let start = PREFIXES
        .iter()
        .find_map(|prefix| cause.strip_prefix(prefix))?;
    let (coordinates, _) = start.split_once(')')?;
    let parts = coordinates.split(',').collect::<Vec<_>>();
    if !(2..=3).contains(&parts.len())
        || parts.iter().any(|part| part.trim().parse::<i32>().is_err())
    {
        return None;
    }
    Some(format!("({coordinates})"))
}

impl Kind {
    fn english(self) -> &'static str {
        match self {
            Self::IncompatibleVersion => "Choose a world saved in Minecraft Java 1.21.1.",
            Self::WorldFolder => {
                "Choose a Minecraft Java world folder containing level.dat and the Overworld region folder."
            }
            Self::InvalidLevelData => {
                "Blocko could not read the Java metadata in level.dat. Try an intact copy of the world."
            }
            Self::EmptyWorld => {
                "The Overworld has no complete chunks. Open the world in Minecraft Java to generate terrain, then use a fresh copy."
            }
            Self::WorldLocked => {
                "The world is in use. Close it in Minecraft or in the other Blocko window, then try again."
            }
            Self::WorldChanged => {
                "World files changed outside this operation. Reopen the world in Blocko before trying again."
            }
            Self::Recover => {
                "A save still needs recovery. Close this world in Blocko and reopen it before loading it in Minecraft."
            }
            Self::RecoveryPending => {
                "A save could not be recovered. Keep the backup and reopen the world in Blocko before loading it in Minecraft."
            }
            Self::WriteRolledBack => {
                "Saving failed and Blocko restored the original regions. Keep the backup and check the file permissions before retrying."
            }
            Self::ReadOnlyWorld => {
                "Blocko could not verify this world's files for writing. Check that the world is an intact, writable copy and reopen it."
            }
            Self::OutsideWorld => "Choose coordinates inside the Overworld boundaries.",
            Self::SelectionTooLarge => "Reduce the selection to at most 1,048,576 blocks.",
            Self::CopyInProgress => "Wait for the current copy to finish, then try again.",
            Self::DoubleChest => {
                "The selection contains a double chest. Adjust the selection to exclude it. Blocko supports single chests."
            }
            Self::CopyData => {
                "The selection contains unsupported or incomplete block data. Adjust the selection to exclude the affected blocks."
            }
            Self::MissingTerrain => {
                "Some blocks are in terrain that has not been fully generated. Choose a generated area."
            }
            Self::UnsupportedDestination => {
                "The edit affects data Blocko cannot safely change. Adjust the target or the selected material."
            }
            Self::InvalidChunk => {
                "Blocko could not read compatible chunk data. Try a different area or an intact copy of the world."
            }
            Self::NoChanges => "This operation would leave the blocks unchanged.",
            Self::SaveTooLarge => {
                "The affected region data exceeds the save size limit. Undo edits to save a smaller area."
            }
            Self::PendingChangesLimit => {
                "Too many blocks have pending changes. Save the world before continuing."
            }
            Self::EditConflict => "The edit state changed during the operation. Try again.",
            Self::Permission => {
                "Access to a required file was denied. Check the world folder permissions and close other apps using it."
            }
            Self::MissingFile => {
                "A required file is missing. Check that the world folder is complete and choose an area with generated terrain."
            }
            Self::DiskFull => {
                "There is not enough disk space. Free space in the drive containing the world before retrying."
            }
            Self::FileAccess => {
                "Blocko could not access a world file. Check the world folder and the diagnostic details."
            }
            Self::Unclassified => {
                "Blocko could not complete the operation. Use the diagnostic details to identify the affected file or data."
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use anyhow::{Context, anyhow};

    use super::*;

    #[test]
    fn keeps_the_backup_path_and_distinguishes_recovery_from_a_rollback() {
        let path = r"C:\Mundos de prueba\Bosque\blocko-backups\pegado-123";
        let pending = Diagnostic::from_error(anyhow!(
            "Falló el guardado: disco. La restauración quedó pendiente: archivo. Respaldo: {path}"
        ));
        let rolled_back = Diagnostic::from_error(anyhow!(
            "Se restauraron las regiones tras el fallo: archivo. Respaldo: {path}"
        ));
        assert_eq!(pending.kind, Kind::RecoveryPending);
        assert_eq!(rolled_back.kind, Kind::WriteRolledBack);
        assert!(pending.message(Language::English).ends_with(path));
        assert!(rolled_back.message(Language::English).ends_with(path));
        assert_eq!(pending.message(Language::Spanish), pending.detail());
    }

    #[test]
    fn uses_specific_causes_and_keeps_context_coordinates() {
        let error = Err::<(), _>(anyhow!(
            "El chunk de destino no está completamente generado"
        ))
        .context("Chunk destino (-3, 18)")
        .unwrap_err();
        let diagnostic = Diagnostic::from_error(error);
        assert_eq!(diagnostic.kind, Kind::MissingTerrain);
        assert!(diagnostic.message(Language::English).ends_with("(-3, 18)"));
    }

    #[test]
    fn uses_the_os_error_kind_under_a_file_context() {
        let error = Err::<(), _>(io::Error::from(io::ErrorKind::PermissionDenied))
            .context("No se pudo abrir C:\\World\\region\\r.0.0.mca")
            .unwrap_err();
        let diagnostic = Diagnostic::from_error(error);
        assert_eq!(diagnostic.kind, Kind::Permission);
        assert!(diagnostic.detail().contains("r.0.0.mca"));
    }

    #[test]
    fn distinguishes_no_change_from_the_limits_on_editing_and_saving() {
        let no_change =
            Diagnostic::from_detail("La operación no cambiaría ningún bloque ni entidad");
        let pending = Diagnostic::from_detail(
            "Hay demasiados cambios pendientes. Guarda el mundo antes de seguir",
        );
        let save_limit =
            Diagnostic::from_detail("El guardado requiere preparar más de 256 MiB de regiones");
        assert_eq!(no_change.kind, Kind::NoChanges);
        assert_eq!(pending.kind, Kind::PendingChangesLimit);
        assert_eq!(save_limit.kind, Kind::SaveTooLarge);
        assert!(
            pending
                .message(Language::English)
                .contains("Save the world")
        );
    }

    #[test]
    fn recognizes_legacy_terrain_diagnostics_without_hiding_the_full_details() {
        let detail = "No se pudo leer el chunk (-12, 3): Paleta inválida en la sección Y=4";
        let diagnostic = Diagnostic::from_detail(detail);
        assert_eq!(diagnostic.kind, Kind::InvalidChunk);
        assert!(diagnostic.message(Language::English).ends_with("(-12, 3)"));
        assert_eq!(diagnostic.detail(), detail);
    }
}
