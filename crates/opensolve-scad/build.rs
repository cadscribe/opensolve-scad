//! Lists the files of the bundled BOSL2 (`library/BOSL2`) for `include!`:
//! the app carries them and writes them out where OpenSCAD files look for
//! libraries (ADR 0014).

use std::fmt::Write as _;
use std::path::Path;

fn main() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("library/BOSL2");
    println!("cargo:rerun-if-changed={}", dir.display());
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .map(|r| {
            r.filter_map(Result::ok)
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.ends_with(".scad") || n == "LICENSE")
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    let mut out = String::from("pub const FILES: &[(&str, &str)] = &[\n");
    for n in &names {
        let path = dir.join(n).display().to_string();
        let _ = writeln!(out, "    ({n:?}, include_str!({path:?})),");
    }
    out.push_str("];\n");
    let target = Path::new(&std::env::var("OUT_DIR").expect("OUT_DIR")).join("bundled.rs");
    std::fs::write(target, out).expect("bundled.rs");
}
