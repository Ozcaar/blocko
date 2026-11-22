mod anvil_write;
mod editor;
mod flight;
mod java;
mod java_paste;
mod paste;
mod placement;
mod preview_mask;
mod preview_visual;
mod selection;
mod streaming;
mod structure;
mod terrain;
mod transaction;
mod ui;
mod world;
mod world_write;

use std::{ffi::OsStr, path::PathBuf, process::ExitCode};

use anyhow::{Context, Result, ensure};
use java::JavaWorld;
use world::ChunkPos;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    let first = args.next();
    if first.as_deref() == Some(OsStr::new("--inspeccionar")) {
        let path = PathBuf::from(args.next().context("Indica la carpeta del mundo")?);
        ensure!(args.next().is_none(), "Uso: blocko --inspeccionar CARPETA");
        let world = JavaWorld::open(&path)?;
        let [x, y, z] = world.spawn;
        let chunk = world
            .load_chunk(ChunkPos::from_block(x, z))?
            .context("El chunk inicial no está disponible")?;
        let mesh = terrain::mesh_chunk(&chunk, [None; 4]);
        println!(
            "{} · inicio ({x}, {y}, {z}) · {} secciones · {} vértices",
            world.name,
            chunk.sections.len(),
            mesh.map_or(0, |mesh| mesh.count_vertices())
        );
        return Ok(());
    }

    ensure!(args.next().is_none(), "Uso: blocko [CARPETA | --demo]");
    let demo = first.as_deref() == Some(OsStr::new("--demo"));
    editor::run(first.filter(|_| !demo).map(PathBuf::from), demo);
    Ok(())
}
