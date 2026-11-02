//! Builds OpenSCAD files and says what came out and how long it took:
//! `cargo run --release -p opensolve-scad --example scad -- [--lib DIR]
//! [--bundled] [-D name=value] FILE…`. `--bundled` searches only the
//! BOSL2 that comes with the source.

use std::path::{Path, PathBuf};
use std::time::Instant;

fn main() {
    let mut files = Vec::new();
    let mut libs: Vec<PathBuf> = Vec::new();
    let mut overrides = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--lib" => libs.extend(args.next().map(PathBuf::from)),
            "--bundled" => libs.push(Path::new(env!("CARGO_MANIFEST_DIR")).join("library")),
            "-D" => {
                if let Some((n, v)) = args.next().as_deref().and_then(|d| d.split_once('=')) {
                    overrides.push((n.to_string(), v.to_string()));
                }
            }
            _ => files.push(PathBuf::from(a)),
        }
    }
    if !libs.is_empty() {
        opensolve_scad::library::set_dirs(Some(libs));
    }
    let read = |p: &Path| std::fs::read(p).map_err(|e| e.to_string());
    for f in &files {
        for round in 0..2 {
            let t = Instant::now();
            let out = opensolve_scad::build(f, &read, &overrides);
            let ms = t.elapsed().as_secs_f64() * 1000.0;
            match out {
                Ok(o) => {
                    println!(
                        "{} [{round}]: {ms:.0} ms, {} triangles, volume {:.3}, closed {}",
                        f.display(),
                        o.solid.tris.len(),
                        o.solid.volume(),
                        o.solid.is_closed()
                    );
                    if round == 0 {
                        for n in o.notes.iter().take(30) {
                            println!("  {n}");
                        }
                    }
                }
                Err(e) => {
                    println!("{} [{round}]: {ms:.0} ms, error: {e}", f.display());
                    if round == 0 {
                        let d = opensolve_scad::diagnose(f, &read, &overrides);
                        for n in d.notes.iter().take(30) {
                            println!("  {}:{}: {}", n.line, n.col, n.message);
                        }
                    }
                }
            }
        }
    }
}
