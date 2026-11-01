//! Real files: a box with a lid, a bracket that includes a library, and one
//! of modules, recursion and comprehensions that uses another.

use std::path::{Path, PathBuf};

use opensolve_scad::{Choices, Kind, Output, build, files, parameters};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn read(p: &Path) -> Result<Vec<u8>, String> {
    std::fs::read(p).map_err(|e| e.to_string())
}

fn built(name: &str, overrides: &[(&str, &str)]) -> Output {
    let o: Vec<(String, String)> = overrides
        .iter()
        .map(|(n, v)| (n.to_string(), v.to_string()))
        .collect();
    let out = build(&fixture(name), &read, &o).unwrap_or_else(|e| panic!("{name}: {e}"));
    assert!(out.solid.is_closed(), "{name} is not closed");
    out
}

/// The area of the hull of four circles of 32 pieces in the corners.
fn rounded(w: f64, d: f64, r: f64) -> f64 {
    let ngon = 16.0 * r * r * (std::f64::consts::TAU / 32.0).sin();
    (w - 2.0 * r) * (d - 2.0 * r) + 2.0 * r * (w - 2.0 * r) + 2.0 * r * (d - 2.0 * r) + ngon
}

#[test]
fn the_box_and_its_parameters() {
    let p = parameters(&std::fs::read_to_string(fixture("box_with_lid.scad")).unwrap()).unwrap();
    let names: Vec<&str> = p.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "width",
            "depth",
            "height",
            "wall",
            "radius",
            "lid",
            "gap",
            "lid_height"
        ]
    );
    assert_eq!(p[0].description, "Inside width");
    assert_eq!((p[0].group.as_str(), p[5].group.as_str()), ("Box", "Lid"));
    assert_eq!(p[5].kind, Kind::Bool);
    assert!(matches!(&p[6].choices, Choices::List(c) if c.len() == 4));
    let box_only = rounded(64.0, 44.0, 4.0) * 27.0 - rounded(60.0, 40.0, 2.0) * 25.0;
    let out = built("box_with_lid.scad", &[("lid", "false")]);
    let v = out.solid.volume();
    assert!(
        (v - box_only).abs() < 1e-3 * box_only,
        "{v} is not {box_only}"
    );
    assert!(
        out.notes
            .contains(&"ECHO: \"box 60 x 40 x 25\"".to_string()),
        "{:?}",
        out.notes
    );
    // With the lid, and wider.
    let both = built("box_with_lid.scad", &[]).solid.volume();
    assert!(both > v + rounded(64.0, 44.0, 4.0) * 2.0);
    let wide = built("box_with_lid.scad", &[("lid", "false"), ("width", "80")]);
    let (lo, hi) = wide.solid.bounds().unwrap();
    assert!((hi[0] - lo[0] - 84.0).abs() < 1e-6);
}

#[test]
fn the_bracket_with_its_library() {
    let out = built("bracket.scad", &[]);
    let (lo, hi) = out.solid.bounds().unwrap();
    for (a, b) in lo.iter().chain(&hi).zip([0.0, 0.0, 0.0, 50.0, 30.0, 50.0]) {
        assert!((a - b).abs() < 1e-6, "{lo:?} {hi:?}");
    }
    let v = out.solid.volume();
    // Two plates less their overlap, less four holes, and a gusset.
    let plates = 2.0 * 50.0 * 30.0 * 4.0 - 4.0 * 30.0 * 4.0;
    assert!(v < plates + 300.0 && v > plates - 4.0 * 600.0, "{v}");
    let more = built("bracket.scad", &[("holes", "4")]).solid.volume();
    assert!(more < v);
    let mut f: Vec<String> = files(&fixture("bracket.scad"), &read)
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    f.sort();
    assert_eq!(f, ["bracket.scad", "fasteners.scad"]);
}

#[test]
fn modules_recursion_and_comprehensions() {
    let out = built("modules.scad", &[]);
    assert!(
        out.notes
            .contains(&"ECHO: teeth = 12, r = 12, points = 48".to_string()),
        "{:?}",
        out.notes
    );
    // Four parts apart: the wheel, the shaft, the tree, the plate.
    let (lo, hi) = out.solid.bounds().unwrap();
    assert!(
        lo[0] < -39.0 && hi[0] > 45.0 && hi[1] > 49.0,
        "{lo:?} {hi:?}"
    );
    // The plate of the used file, not the one it makes itself.
    assert!(hi[2] < 50.0);
    let more = built("modules.scad", &[("teeth", "20")]).solid.volume();
    assert!(more > out.solid.volume());
}
