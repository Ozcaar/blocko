use std::{
    collections::BTreeSet,
    fs::{self, File, Metadata},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail, ensure};
use blake3::Hash;

const BACKUPS: &str = "blocko-backups";
const PENDING: &str = "pendiente.txt";
const COMMITTED: &str = "confirmado.txt";
const REVERTED: &str = "restaurado.txt";
pub const MAX_REGION_BYTES: u64 = 256 * 1024 * 1024;

pub struct RegionEdit {
    pub name: String,
    pub before: Hash,
    pub bytes: Vec<u8>,
}

#[derive(Debug)]
struct Entry {
    name: String,
    before: Hash,
    after: Hash,
}

pub fn commit(world: &Path, level: Hash, edits: Vec<RegionEdit>) -> Result<PathBuf> {
    commit_with(world, level, edits, |_| Ok(()))
}

fn commit_with(
    world: &Path,
    level: Hash,
    edits: Vec<RegionEdit>,
    mut before_replace: impl FnMut(usize) -> Result<()>,
) -> Result<PathBuf> {
    let _level_pin = pin_level(world, level)?;
    let region = plain_dir(&world.join("region"))?;
    ensure!(!edits.is_empty(), "La operación no contiene cambios");
    let mut used = BTreeSet::new();
    for edit in &edits {
        ensure!(
            region_name(&edit.name) && used.insert(&edit.name),
            "Región inválida o duplicada: {}",
            edit.name
        );
    }
    let directory = new_backup_directory(world)?;
    let mut handles = Vec::with_capacity(edits.len());
    let mut entries = Vec::with_capacity(edits.len());
    for edit in edits {
        let path = region.join(&edit.name);
        let mut original = read_only(&path)?;
        let bytes = bounded_read(&mut original, MAX_REGION_BYTES, &path)?;
        ensure!(
            blake3::hash(&bytes) == edit.before,
            "La región {} cambió desde la preparación. Vuelve a abrir el mundo",
            edit.name
        );
        write_new(&directory.join(&edit.name), &bytes)?;
        write_new(&directory.join(format!("nuevo-{}", edit.name)), &edit.bytes)?;
        let entry = Entry {
            name: edit.name,
            before: edit.before,
            after: blake3::hash(&edit.bytes),
        };
        handles.push(original);
        entries.push(entry);
    }
    let manifest = format!("nivel {level}\n")
        + &entries
            .iter()
            .map(|entry| format!("{} {} {}\n", entry.name, entry.before, entry.after))
            .collect::<String>();
    write_new(&directory.join("registro.tmp"), manifest.as_bytes())?;
    atomic_replace(&directory.join("registro.tmp"), &directory.join(PENDING))?;

    let mut attempted = 0;
    let apply = || -> Result<()> {
        for (index, entry) in entries.iter().enumerate() {
            before_replace(index)?;
            let destination = region.join(&entry.name);
            ensure!(
                hash_file(&destination)? == entry.before,
                "La región {} cambió durante la confirmación",
                entry.name
            );
            attempted = index + 1;
            let displaced = directory.join(format!("desplazada-{}", entry.name));
            replace_region(
                &directory.join(format!("nuevo-{}", entry.name)),
                &destination,
                &displaced,
            )?;
            #[cfg(windows)]
            ensure!(
                hash_file(&displaced)? == entry.before,
                "La región {} fue sustituida externamente durante la confirmación",
                entry.name
            );
        }
        for entry in &entries {
            ensure!(
                hash_file(&region.join(&entry.name))? == entry.after,
                "La verificación de {} falló tras el reemplazo",
                entry.name
            );
        }
        atomic_replace(&directory.join(PENDING), &directory.join(COMMITTED))
    };
    let mut apply = apply;
    let result = apply();
    drop(handles);
    if let Err(failure) = result {
        if let Err(recovery) = restore(&region, &directory, &entries[..attempted]) {
            bail!(
                "Falló el guardado: {failure:#}. La restauración quedó pendiente: {recovery:#}. Respaldo: {}",
                directory.display()
            );
        }
        bail!(
            "Se restauraron las regiones tras el fallo: {failure:#}. Respaldo: {}",
            directory.display()
        );
    }
    for entry in &entries {
        let _ = fs::remove_file(directory.join(format!("desplazada-{}", entry.name)));
    }
    Ok(directory)
}

pub fn pending(world: &Path) -> Result<Vec<PathBuf>> {
    let root = world.join(BACKUPS);
    if !root.try_exists()? {
        return Ok(Vec::new());
    }
    let mut pending = Vec::new();
    for item in fs::read_dir(plain_dir(&root)?)? {
        let dir = item?.path();
        if !dir
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with("pegado-"))
        {
            continue;
        }
        let dir = plain_dir(&dir)?;
        if dir.join(PENDING).try_exists()? {
            pending.push(dir);
        }
    }
    pending.sort();
    Ok(pending)
}

pub fn recover(world: &Path) -> Result<()> {
    for directory in pending(world)? {
        let region = plain_dir(&world.join("region"))?;
        let (level, entries) = read_manifest(&directory)?;
        let _level_pin = pin_level(world, level)?;
        restore(&region, &directory, &entries).with_context(|| {
            format!(
                "Quedó una restauración pendiente en {}",
                directory.display()
            )
        })?;
    }
    Ok(())
}

fn restore(region: &Path, directory: &Path, entries: &[Entry]) -> Result<()> {
    let mut errors = Vec::new();
    for entry in entries {
        if let Err(error) = restore_region(region, directory, entry) {
            errors.push(format!("{}: {error:#}", entry.name));
        }
    }
    ensure!(
        errors.is_empty(),
        "Regiones pendientes de restaurar: {}",
        errors.join(". ")
    );
    atomic_replace(&directory.join(PENDING), &directory.join(REVERTED))
}

fn restore_region(region: &Path, directory: &Path, entry: &Entry) -> Result<()> {
    let destination = region.join(&entry.name);
    let original = directory.join(&entry.name);
    ensure!(
        hash_file(&original)? == entry.before,
        "El respaldo original está dañado"
    );
    let displaced = directory.join(format!("desplazada-{}", entry.name));
    #[cfg(windows)]
    if !displaced.try_exists()?
        && directory
            .join(format!("nuevo-{}", entry.name))
            .try_exists()?
    {
        return Ok(());
    }
    // ReplaceFileW captures the file that actually occupied the path.
    let preimage = if displaced.try_exists()? {
        &displaced
    } else {
        &original
    };
    let bytes = read_bounded(preimage, MAX_REGION_BYTES)?;
    let expected = blake3::hash(&bytes);
    let temporary = directory.join(format!("restaurar-{}-{}", unique_suffix()?, entry.name));
    if destination.try_exists()? {
        let current = hash_file(&destination)?;
        if current == expected {
            return Ok(());
        }
        ensure!(
            current == entry.after,
            "La región cambió externamente. Su respaldo permanece disponible"
        );
        write_new(&temporary, &bytes)?;
        let preserved = directory.join(format!(
            "previa-restauracion-{}-{}",
            unique_suffix()?,
            entry.name
        ));
        replace_region(&temporary, &destination, &preserved)?;
        #[cfg(windows)]
        {
            let observed = hash_file(&preserved)?;
            if observed != entry.after {
                // Restore the external edit. Both displaced versions stay on disk.
                ensure!(
                    hash_file(&destination)? == expected,
                    "La región volvió a cambiar durante la restauración"
                );
                let copied = read_bounded(&preserved, MAX_REGION_BYTES)?;
                let temp = directory.join(format!("recuperar-{}-{}", unique_suffix()?, entry.name));
                write_new(&temp, &copied)?;
                let second = directory.join(format!("externa-{}-{}", unique_suffix()?, entry.name));
                replace_region(&temp, &destination, &second)?;
                bail!("Se conservó una modificación externa detectada durante la restauración");
            }
            let _ = fs::remove_file(preserved);
        }
    } else {
        // ReplaceFileW can leave the destination absent on certain failures.
        ensure!(
            displaced.try_exists()?,
            "La región desapareció por fuera de Blocko"
        );
        write_new(&temporary, &bytes)?;
        fs::hard_link(&temporary, &destination)
            .context("No se pudo restaurar la región ausente")?;
        let _ = fs::remove_file(temporary);
    }
    ensure!(
        hash_file(&destination)? == expected,
        "No se pudo validar la restauración"
    );
    Ok(())
}

fn read_manifest(directory: &Path) -> Result<(Hash, Vec<Entry>)> {
    let manifest = read_bounded(&directory.join(PENDING), 1024 * 1024)?;
    let manifest = std::str::from_utf8(&manifest).context("Registro de respaldo ilegible")?;
    let mut used = BTreeSet::new();
    let mut entries = Vec::new();
    let mut lines = manifest.lines();
    let level = lines
        .next()
        .and_then(|line| line.strip_prefix("nivel "))
        .context("Falta la identidad del mundo en el registro de respaldo")?;
    let level = Hash::from_hex(level).context("Identidad del mundo inválida")?;
    for line in lines {
        let mut fields = line.split_ascii_whitespace();
        let name = fields
            .next()
            .context("Falta la región en el registro de respaldo")?;
        let before = fields.next().context("Falta la huella original")?;
        let after = fields.next().context("Falta la huella modificada")?;
        ensure!(
            region_name(name) && used.insert(name) && fields.next().is_none(),
            "Registro de región inválido"
        );
        entries.push(Entry {
            name: name.into(),
            before: Hash::from_hex(before).context("Huella original inválida")?,
            after: Hash::from_hex(after).context("Huella modificada inválida")?,
        });
    }
    ensure!(!entries.is_empty(), "Registro de respaldo vacío");
    Ok((level, entries))
}

pub fn region_name(name: &str) -> bool {
    let Some((x, z)) = name
        .strip_prefix("r.")
        .and_then(|s| s.strip_suffix(".mca"))
        .and_then(|s| s.split_once('.'))
    else {
        return false;
    };
    matches!((x.parse::<i32>(), z.parse::<i32>()), (Ok(x), Ok(z)) if format!("r.{x}.{z}.mca") == name)
}

fn new_backup_directory(world: &Path) -> Result<PathBuf> {
    let path = world.join(BACKUPS);
    match fs::create_dir(&path) {
        Ok(()) => (),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => (),
        Err(error) => return Err(error).context("No se pudo crear la carpeta de respaldos"),
    }
    let path = plain_dir(&path)?.join(format!("pegado-{}", unique_suffix()?));
    fs::create_dir(&path).with_context(|| format!("No se pudo crear {}", path.display()))?;
    Ok(path)
}

fn unique_suffix() -> Result<String> {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    Ok(format!("{nanos}-{}", std::process::id()))
}

pub fn plain_dir(path: &Path) -> Result<PathBuf> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("No se pudo abrir {}", path.display()))?;
    ensure!(
        metadata.is_dir() && !redirects(&metadata),
        "Directorio con enlace o redirección: {}",
        path.display()
    );
    Ok(path.to_owned())
}

pub fn plain_file(path: &Path) -> Result<Metadata> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("No se pudo abrir {}", path.display()))?;
    ensure!(
        metadata.is_file() && !redirects(&metadata),
        "Archivo con enlace o redirección: {}",
        path.display()
    );
    Ok(metadata)
}

fn redirects(metadata: &Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    metadata.file_type().is_symlink()
}

fn read_only(path: &Path) -> Result<File> {
    open_read(path, true)
}

fn open_read(path: &Path, share_delete: bool) -> Result<File> {
    plain_file(path)?;
    let mut options = File::options();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, FILE_SHARE_READ,
        };
        options
            .share_mode(FILE_SHARE_READ | if share_delete { FILE_SHARE_DELETE } else { 0 })
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    #[cfg(not(windows))]
    let _ = share_delete;
    let file = options
        .open(path)
        .with_context(|| format!("No se pudo leer {}", path.display()))?;
    ensure!(
        !redirects(&file.metadata()?),
        "El archivo cambió a un enlace: {}",
        path.display()
    );
    Ok(file)
}

fn pin_level(world: &Path, expected: Hash) -> Result<File> {
    let path = world.join("level.dat");
    let mut file = open_read(&path, false)?;
    ensure!(
        hash_reader(&mut file, &path)? == expected,
        "level.dat cambió. El registro pertenece a otro estado del mundo"
    );
    Ok(file)
}

pub fn hash_file(path: &Path) -> Result<Hash> {
    let mut file = read_only(path)?;
    hash_reader(&mut file, path)
}

fn hash_reader(file: &mut File, path: &Path) -> Result<Hash> {
    let mut hasher = blake3::Hasher::new();
    io::copy(file, &mut hasher)
        .with_context(|| format!("No se pudo verificar {}", path.display()))?;
    Ok(hasher.finalize())
}

#[cfg(windows)]
fn replace_region(stage: &Path, destination: &Path, displaced: &Path) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::ReplaceFileW;
    let wide = |path: &Path| {
        path.as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<u16>>()
    };
    let a = wide(destination);
    let b = wide(stage);
    let c = wide(displaced);
    // Windows captures the actual target as part of replacing the file.
    let replaced = unsafe {
        ReplaceFileW(
            a.as_ptr(),
            b.as_ptr(),
            c.as_ptr(),
            0,
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    if replaced == 0 {
        return Err(io::Error::last_os_error())
            .with_context(|| format!("No se pudo reemplazar {}", destination.display()));
    }
    Ok(())
}

#[cfg(not(windows))]
fn replace_region(stage: &Path, destination: &Path, _displaced: &Path) -> Result<()> {
    atomic_replace(stage, destination)
}

pub fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>> {
    bounded_read(&mut read_only(path)?, limit, path)
}

fn bounded_read(file: &mut File, limit: u64, path: &Path) -> Result<Vec<u8>> {
    let len = file.metadata()?.len();
    ensure!(
        len <= limit,
        "El archivo es demasiado grande para preparar el guardado: {}",
        path.display()
    );
    let mut bytes = Vec::with_capacity(len as usize);
    file.take(limit + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= limit,
        "El archivo creció durante la lectura: {}",
        path.display()
    );
    Ok(bytes)
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = File::options()
        .write(true)
        .create_new(true)
        .open(path)
        .with_context(|| format!("No se pudo preparar {}", path.display()))?;
    file.write_all(bytes)
        .with_context(|| format!("No se pudo escribir {}", path.display()))?;
    file.sync_all()
        .with_context(|| format!("No se pudo sincronizar {}", path.display()))
}

fn atomic_replace(from: &Path, to: &Path) -> Result<()> {
    #[cfg(windows)]
    let result = {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::{
            MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
        };
        let wide_from: Vec<u16> = from.as_os_str().encode_wide().chain(Some(0)).collect();
        let wide_to: Vec<u16> = to.as_os_str().encode_wide().chain(Some(0)).collect();
        // These nul-terminated buffers stay alive until MoveFileExW returns.
        let renamed = unsafe {
            MoveFileExW(
                wide_from.as_ptr(),
                wide_to.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        };
        if renamed == 0 {
            let error = io::Error::last_os_error();
            // Rust retries a share-delete replacement with FileRenameInfoEx
            // on Windows when a pinned handle prevents the older operation.
            if matches!(error.raw_os_error(), Some(5 | 32)) {
                fs::rename(from, to)
            } else {
                Err(error)
            }
        } else {
            Ok(())
        }
    };
    #[cfg(not(windows))]
    let result = fs::rename(from, to);
    result.with_context(|| {
        format!(
            "No se pudo sustituir {} con {}",
            to.display(),
            from.display()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Result<Self> {
            let root = std::env::temp_dir().canonicalize()?;
            let path = root.join(format!("blocko-transaction-{}", unique_suffix()?));
            fs::create_dir(&path)?;
            fs::create_dir(path.join("region"))?;
            fs::write(path.join("level.dat"), b"identidad de nivel")?;
            let fixture = Self(path);
            for (name, bytes) in originals() {
                fs::write(fixture.0.join("region").join(name), bytes)?;
            }
            Ok(fixture)
        }

        fn assert_originals(&self) -> Result<()> {
            for (name, bytes) in originals() {
                assert_eq!(fs::read(self.0.join("region").join(name))?, bytes);
            }
            Ok(())
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            if let (Ok(path), Ok(temp)) =
                (self.0.canonicalize(), std::env::temp_dir().canonicalize())
                && path.parent() == Some(temp.as_path())
                && path
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with("blocko-transaction-"))
            {
                let _ = fs::remove_dir_all(&path);
            }
        }
    }

    fn originals() -> [(&'static str, &'static [u8]); 2] {
        [
            ("r.-1.0.mca", b"region oeste"),
            ("r.0.0.mca", b"region este"),
        ]
    }

    fn edits() -> Vec<RegionEdit> {
        originals()
            .into_iter()
            .map(|(name, bytes)| RegionEdit {
                name: name.into(),
                before: blake3::hash(bytes),
                bytes: b"datos nuevos".to_vec(),
            })
            .collect()
    }

    fn level() -> Hash {
        blake3::hash(b"identidad de nivel")
    }

    #[test]
    fn commits_all_regions_and_retains_original_bytes_in_backup() -> Result<()> {
        let fixture = Fixture::new()?;
        let backup = commit(&fixture.0, level(), edits())?;
        assert!(backup.join(COMMITTED).is_file());
        for (name, bytes) in originals() {
            assert_eq!(fs::read(backup.join(name))?, bytes);
            assert_eq!(
                fs::read(fixture.0.join("region").join(name))?,
                b"datos nuevos"
            );
        }
        assert!(pending(&fixture.0)?.is_empty());
        Ok(())
    }

    #[test]
    fn restores_every_replaced_region_after_a_late_failure() -> Result<()> {
        let fixture = Fixture::new()?;
        let mut reached_second = false;
        let result = commit_with(&fixture.0, level(), edits(), |i| {
            if i == 1 {
                reached_second = true;
                assert_eq!(
                    fs::read(fixture.0.join("region/r.-1.0.mca"))?,
                    b"datos nuevos"
                );
                bail!("Fallo de escritura simulado");
            }
            Ok(())
        });
        assert!(reached_second);
        assert!(result.is_err());
        assert!(pending(&fixture.0)?.is_empty());
        fixture.assert_originals()
    }

    #[test]
    fn restores_an_interrupted_operation_from_its_durable_record() -> Result<()> {
        let fixture = Fixture::new()?;
        let interruption = std::panic::catch_unwind(|| {
            let _ = commit_with(&fixture.0, level(), edits(), |i| {
                assert_ne!(i, 1, "Interrupción simulada");
                Ok(())
            });
        });
        assert!(interruption.is_err());
        assert_eq!(pending(&fixture.0)?.len(), 1);
        recover(&fixture.0)?;
        fixture.assert_originals()?;
        assert!(pending(&fixture.0)?.is_empty());
        Ok(())
    }

    #[test]
    fn keeps_external_changes_and_still_restores_previous_regions() -> Result<()> {
        let fixture = Fixture::new()?;
        let destination = fixture.0.join("region/r.0.0.mca");
        let external = fixture.0.join("region/otra-escritura.tmp");
        let result = commit_with(&fixture.0, level(), edits(), |i| {
            if i == 1 {
                fs::write(&external, b"cambio externo")?;
                atomic_replace(&external, &destination)?;
            }
            Ok(())
        });
        assert!(result.is_err());
        assert_eq!(fs::read(&destination)?, b"cambio externo");
        assert_eq!(
            fs::read(fixture.0.join("region/r.-1.0.mca"))?,
            b"region oeste"
        );
        assert!(pending(&fixture.0)?.is_empty());
        Ok(())
    }

    #[test]
    fn refuses_to_recover_a_journal_when_level_identity_changed() -> Result<()> {
        let fixture = Fixture::new()?;
        let _ = std::panic::catch_unwind(|| {
            let _ = commit_with(&fixture.0, level(), edits(), |i| {
                assert_ne!(i, 1, "Interrupción simulada");
                Ok(())
            });
        });
        fs::write(fixture.0.join("level.dat"), b"otro nivel")?;
        assert!(recover(&fixture.0).is_err());
        assert_eq!(
            fs::read(fixture.0.join("region/r.-1.0.mca"))?,
            b"datos nuevos"
        );
        assert_eq!(pending(&fixture.0)?.len(), 1);
        Ok(())
    }
}
