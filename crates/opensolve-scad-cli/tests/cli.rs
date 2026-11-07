//! The command line as scripts call OpenSCAD's: GoSTL's colour passes
//! (`-o x.csg`, then `-D` modules that replace `color`), formats, exit
//! codes, echoes on stderr.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// A folder of its own for each test.
fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("opensolve-scad-cli-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn run(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_opensolve-scad"))
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn ok(o: &Output) {
    assert!(o.status.success(), "{}", stderr(o));
}

/// Points and triangles of an OFF file.
fn read_off(text: &str) -> (Vec<[f64; 3]>, Vec<[usize; 3]>) {
    let mut lines = text.lines().skip(1);
    let counts: Vec<usize> = lines
        .next()
        .unwrap()
        .split_whitespace()
        .map(|x| x.parse().unwrap())
        .collect();
    let num = |l: &str| -> Vec<f64> { l.split_whitespace().map(|x| x.parse().unwrap()).collect() };
    let points = (0..counts[0])
        .map(|_| {
            let v = num(lines.next().unwrap());
            [v[0], v[1], v[2]]
        })
        .collect();
    let tris = (0..counts[1])
        .map(|_| {
            let v = num(lines.next().unwrap());
            [v[1] as usize, v[2] as usize, v[3] as usize]
        })
        .collect();
    (points, tris)
}

fn volume(off: &str) -> f64 {
    let (p, t) = read_off(off);
    t.iter()
        .map(|&[a, b, c]| {
            let (a, b, c) = (p[a], p[b], p[c]);
            a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0])
                + a[2] * (b[0] * c[1] - b[1] * c[0])
        })
        .sum::<f64>()
        / 6.0
}

fn off_of(dir: &Path, args: &[&str]) -> String {
    let mut all = vec!["--export-format", "off", "-o", "-"];
    all.extend_from_slice(args);
    let o = run(dir, &all);
    ok(&o);
    String::from_utf8(o.stdout).unwrap()
}

const MODEL: &str = "\
echo(\"hello\", 1/3);
w = 10;
color(\"red\") cube(w);
translate([20, 0, 0]) color([0, 0, 1]) cube(4);
translate([0, 20, 0]) cube(2);
";

#[test]
fn writes_stl_and_echoes_to_stderr() {
    let d = dir("stl");
    std::fs::write(d.join("m.scad"), MODEL).unwrap();
    let o = run(&d, &["-o", "m.stl", "m.scad"]);
    ok(&o);
    assert!(stderr(&o).contains("ECHO: \"hello\", 0.333333"));
    let stl = std::fs::read_to_string(d.join("m.stl")).unwrap();
    assert!(stl.starts_with("solid OpenSCAD_Model"));
    assert_eq!(stl.matches("facet normal").count(), 36);
    // Quiet: nothing on stderr.
    let o = run(&d, &["-q", "-o", "m.stl", "m.scad"]);
    ok(&o);
    assert!(stderr(&o).is_empty(), "{}", stderr(&o));
    // Binary STL: 84 bytes and 50 per triangle.
    ok(&run(
        &d,
        &["--export-format", "binstl", "-o", "b.stl", "m.scad"],
    ));
    assert_eq!(
        std::fs::metadata(d.join("b.stl")).unwrap().len(),
        84 + 50 * 36
    );
}

#[test]
fn defines_set_variables_after_the_files_own() {
    let d = dir("defines");
    std::fs::write(d.join("m.scad"), MODEL).unwrap();
    let v = volume(&off_of(&d, &["m.scad"]));
    assert!((v - (1000.0 + 64.0 + 8.0)).abs() < 1e-6, "{v}");
    let v = volume(&off_of(&d, &["-D", "w=5", "m.scad"]));
    assert!((v - (125.0 + 64.0 + 8.0)).abs() < 1e-6, "{v}");
}

#[test]
fn gostl_colour_passes_on_the_csg() {
    let d = dir("gostl");
    std::fs::write(d.join("m.scad"), MODEL).unwrap();
    ok(&run(&d, &["-o", "m.csg", "m.scad"]));
    let csg = std::fs::read_to_string(d.join("m.csg")).unwrap();
    assert!(csg.contains("color([1, 0, 0, 1])") && csg.contains("color([0, 0, 1, 1])"));

    // The colours, by a `color` that echoes them.
    let o = run(
        &d,
        &[
            "-D",
            "module color(c, alpha) { echo(GOSTL_COLOR=str(c)); }",
            "-o",
            "c.stl",
            "m.csg",
        ],
    );
    let echoes: Vec<&str> = std::str::from_utf8(&o.stderr)
        .unwrap()
        .lines()
        .filter(|l| l.starts_with("ECHO: GOSTL_COLOR = "))
        .collect();
    assert_eq!(
        echoes,
        [
            "ECHO: GOSTL_COLOR = \"[1, 0, 0, 1]\"",
            "ECHO: GOSTL_COLOR = \"[0, 0, 1, 1]\""
        ]
    );

    // One colour: what it paints, and what has none.
    let filter = "module color(c, alpha) { if ($colored) { children(); } else { \
                  $colored = true; if (str(c) == \"[1, 0, 0, 1]\") children(); } }";
    let red = off_of(&d, &["-D", "$colored = false;", "-D", filter, "m.csg"]);
    assert!((volume(&red) - (1000.0 + 8.0)).abs() < 1e-6);

    // What has no colour.
    let plain = off_of(&d, &["-D", "module color(c, alpha) { }", "m.csg"]);
    assert!((volume(&plain) - 8.0).abs() < 1e-6);

    // Nothing at all: an error, and no file.
    let o = run(
        &d,
        &["-D", "module color(c, alpha) { }", "-o", "n.stl", "e.csg"],
    );
    assert!(!o.status.success());
    std::fs::write(d.join("e.scad"), "color(\"red\") cube();").unwrap();
    let o = run(
        &d,
        &["-D", "module color(c, alpha) { }", "-o", "n.stl", "e.scad"],
    );
    assert!(!o.status.success());
    assert!(stderr(&o).contains("Current top level object is empty."));
    assert!(!d.join("n.stl").exists());
}

#[test]
fn three_mf_keeps_the_colours() {
    let d = dir("3mf");
    std::fs::write(d.join("m.scad"), MODEL).unwrap();
    let o = run(&d, &["-o", "m.3mf", "m.scad"]);
    ok(&o);
    // The colour passes do not echo again.
    assert_eq!(stderr(&o).matches("ECHO:").count(), 1);
    let model = String::from_utf8_lossy(&std::fs::read(d.join("m.3mf")).unwrap()).into_owned();
    assert!(model.contains("displaycolor=\"#FF0000FF\""));
    assert!(model.contains("displaycolor=\"#0000FFFF\""));
    assert_eq!(model.matches("<mesh>").count(), 3);
}

#[test]
fn the_cache_tells_what_it_did() {
    let d = dir("cache");
    std::fs::write(d.join("m.scad"), MODEL).unwrap();
    let o = run(&d, &["--cache-stats", "-o", "m.3mf", "m.scad"]);
    ok(&o);
    let line = stderr(&o)
        .lines()
        .find(|l| l.starts_with("cache: "))
        .map(str::to_string)
        .unwrap_or_default();
    // The whole and the three colours: four runs.
    assert!(line.contains("4 runs"), "{line}");
    assert!(line.contains(" hits, "), "{line}");
    let o = run(&d, &["-o", "m.stl", "m.scad"]);
    ok(&o);
    assert!(!stderr(&o).contains("cache: "));
}

#[test]
fn a_colour_cut_by_a_shape_of_none() {
    let d = dir("cut");
    std::fs::write(
        d.join("m.scad"),
        "difference() { color(\"red\") cube(10); translate([5, 5, 5]) cube(10); }",
    )
    .unwrap();
    ok(&run(&d, &["-o", "m.csg", "m.scad"]));
    let csg = std::fs::read_to_string(d.join("m.csg")).unwrap();
    // One part, red, cut: only its colour, no part without one.
    assert_eq!(csg.matches("polyhedron").count(), 1);
    let v = volume(&off_of(&d, &["m.csg"]));
    assert!((v - 875.0).abs() < 1e-6, "{v}");
}

#[test]
fn flat_shapes_and_empty_files() {
    let d = dir("flat");
    std::fs::write(d.join("f.scad"), "square(5);").unwrap();
    let o = run(&d, &["-o", "f.stl", "f.scad"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).contains("Current top level object is not a 3D object."));
    ok(&run(&d, &["-o", "f.svg", "f.scad"]));
    assert!(
        std::fs::read_to_string(d.join("f.svg"))
            .unwrap()
            .contains("<path")
    );
    ok(&run(&d, &["-o", "f.dxf", "f.scad"]));
    // A CSG of a 2D shape reads again.
    ok(&run(&d, &["-o", "f.csg", "f.scad"]));
    ok(&run(&d, &["-o", "g.svg", "f.csg"]));
    std::fs::write(d.join("c.scad"), "cube(1);").unwrap();
    let o = run(&d, &["-o", "c.svg", "c.scad"]);
    assert!(stderr(&o).contains("Current top level object is not a 2D object."));

    std::fs::write(d.join("e.scad"), "x = 1;").unwrap();
    let o = run(&d, &["-o", "e.stl", "e.scad"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).contains("Current top level object is empty."));
}

#[test]
fn errors_warnings_and_usage() {
    let d = dir("errors");
    std::fs::write(d.join("bad.scad"), "cube(;").unwrap();
    let o = run(&d, &["-o", "b.stl", "bad.scad"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(
        stderr(&o).starts_with("ERROR: bad.scad, line 1"),
        "{}",
        stderr(&o)
    );

    std::fs::write(d.join("w.scad"), "nothing_here(); cube(1);").unwrap();
    let o = run(&d, &["-o", "w.stl", "w.scad"]);
    ok(&o);
    assert!(stderr(&o).contains("WARNING:"));
    let o = run(&d, &["--hardwarnings", "-o", "w2.stl", "w.scad"]);
    assert_eq!(o.status.code(), Some(1));

    assert_eq!(run(&d, &["w.scad"]).status.code(), Some(1));
    assert_eq!(run(&d, &["-o", "x.xyz", "w.scad"]).status.code(), Some(1));
    assert_eq!(
        run(&d, &["-o", "x.stl", "missing.scad"]).status.code(),
        Some(1)
    );
    assert!(run(&d, &["--help"]).status.success());
}

#[test]
fn the_version_is_the_crates() {
    let d = dir("version");
    for flag in ["--version", "-v"] {
        let o = run(&d, &[flag]);
        ok(&o);
        let line = format!("opensolve-scad {}\n", opensolve_scad::VERSION);
        assert_eq!(String::from_utf8_lossy(&o.stdout), line);
    }
    assert_eq!(opensolve_scad::VERSION, env!("CARGO_PKG_VERSION"));
    assert_eq!(opensolve_kernel::VERSION, opensolve_scad::VERSION);
}

#[test]
fn more_outputs_and_dependencies() {
    let d = dir("deps");
    std::fs::write(d.join("lib.scad"), "module part() cube(3);").unwrap();
    std::fs::write(d.join("m.scad"), "use <lib.scad>\npart();").unwrap();
    ok(&run(
        &d,
        &[
            "-o", "m.obj", "-o", "m.off", "-o", "m.step", "-d", "m.deps", "m.scad",
        ],
    ));
    assert!(
        std::fs::read_to_string(d.join("m.obj"))
            .unwrap()
            .contains("\nf ")
    );
    assert!((volume(&std::fs::read_to_string(d.join("m.off")).unwrap()) - 27.0).abs() < 1e-9);
    assert!(
        std::fs::read_to_string(d.join("m.step"))
            .unwrap()
            .starts_with("ISO-10303-21;")
    );
    let deps = std::fs::read_to_string(d.join("m.deps")).unwrap();
    assert!(deps.starts_with("m.obj m.off m.step:") && deps.contains("lib.scad"));
}

#[test]
fn gostl_extrudes_a_flat_file_by_an_include_in_a_module() {
    let d = dir("wrap");
    std::fs::write(d.join("flat.scad"), "square(5);").unwrap();
    let wrapper = format!(
        "module _gostl_2d_content() {{\n    include <{}>\n}}\n\
         linear_extrude(height = 1) _gostl_2d_content();\n",
        d.join("flat.scad").display()
    );
    std::fs::write(d.join("wrap.scad"), wrapper).unwrap();
    let v = volume(&off_of(&d, &["wrap.scad"]));
    assert!((v - 25.0).abs() < 1e-9, "{v}");
}
