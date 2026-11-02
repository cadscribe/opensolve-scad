//! Shapes as OpenSCAD makes them: volumes against those of the polygons
//! OpenSCAD cuts circles into.

use std::f64::consts::TAU;
use std::path::Path;

use opensolve_scad::{Output, build};

fn solid(text: &str) -> Output {
    let read = |_: &Path| Ok(text.as_bytes().to_vec());
    let out = build(Path::new("main.scad"), &read, &[]).unwrap_or_else(|e| panic!("{e}"));
    assert!(out.solid.is_closed(), "not closed: {text}");
    out
}

fn volume(text: &str) -> f64 {
    solid(text).solid.volume()
}

/// The area of OpenSCAD's circle of `n` pieces.
fn ngon(r: f64, n: usize) -> f64 {
    n as f64 / 2.0 * r * r * (TAU / n as f64).sin()
}

fn near(a: f64, b: f64) {
    // Corners lie on the kernel's grid of 0.1 µm.
    assert!((a - b).abs() < 1e-4 * b.abs().max(1.0), "{a} is not {b}");
}

#[test]
fn cubes() {
    near(volume("cube(2);"), 8.0);
    near(volume("cube([1, 2, 3], center = true);"), 6.0);
    let s = solid("cube([2, 4, 6], center = true);").solid;
    let (lo, hi) = s.bounds().unwrap();
    assert_eq!((lo, hi), ([-1.0, -2.0, -3.0], [1.0, 2.0, 3.0]));
}

#[test]
fn cylinders_by_fn_and_by_fa_and_fs() {
    near(
        volume("cylinder(h = 10, r = 5, $fn = 8);"),
        ngon(5.0, 8) * 10.0,
    );
    near(
        volume("cylinder(10, d = 10, $fn = 6);"),
        ngon(5.0, 6) * 10.0,
    );
    // $fa 12 and $fs 2: a radius of 10 makes 30 pieces, of 1 five.
    near(volume("cylinder(h = 1, r = 10);"), ngon(10.0, 30));
    near(volume("cylinder(h = 1, r = 1);"), ngon(1.0, 5));
    near(
        volume("$fs = 0.5; $fa = 6; cylinder(h = 1, r = 3);"),
        ngon(3.0, 38),
    );
    // A frustum, and a cone.
    let (a, b) = (ngon(4.0, 12), ngon(2.0, 12));
    near(
        volume("cylinder(h = 3, r1 = 4, r2 = 2, $fn = 12);"),
        3.0 / 3.0 * (a + b + (a * b).sqrt()),
    );
    near(
        volume("cylinder(h = 3, d1 = 8, d2 = 0, center = true, $fn = 12);"),
        a,
    );
}

#[test]
fn spheres_in_rings() {
    for n in [5usize, 8, 16, 30] {
        let r = 3.0;
        let rings = n.div_ceil(2);
        let ring = |i: usize| {
            let phi = (180.0 * (i as f64 + 0.5) / rings as f64).to_radians();
            (r * phi.sin(), r * phi.cos())
        };
        let mut v = 0.0;
        for i in 0..rings - 1 {
            let ((r0, z0), (r1, z1)) = (ring(i), ring(i + 1));
            let (a, b) = (ngon(r0, n), ngon(r1, n));
            v += (z0 - z1) / 3.0 * (a + b + (a * b).sqrt());
        }
        near(volume(&format!("sphere(r = 3, $fn = {n});")), v);
    }
}

#[test]
fn booleans() {
    let hole = ngon(2.0, 16) * 10.0;
    near(
        volume(
            "difference() { cube(10); translate([5, 5, -1]) cylinder(h = 12, r = 2, $fn = 16); }",
        ),
        1000.0 - hole,
    );
    near(
        volume("union() { cube(10); translate([5, 0, 0]) cube(10); }"),
        1500.0,
    );
    near(
        volume("intersection() { cube(10); translate([5, 5, 5]) cube(10); }"),
        125.0,
    );
    // Several cut at once, and separate parts kept.
    near(
        volume(
            "difference() { cube([30, 10, 2]); for (x = [5:10:25]) translate([x, 5, -1]) cylinder(h = 4, r = 2, $fn = 16); }",
        ),
        600.0 - 3.0 * ngon(2.0, 16) * 2.0,
    );
    near(volume("cube(1); translate([5, 0, 0]) cube(2);"), 9.0);
}

#[test]
fn transformations_keep_or_scale_volumes() {
    near(volume("rotate([30, 45, 60]) cube(2);"), 8.0);
    near(volume("rotate(45, [1, 1, 0]) cube(2);"), 8.0);
    near(volume("mirror([1, 1, 0]) cube(2);"), 8.0);
    near(volume("scale([1, 2, 3]) cube(1);"), 6.0);
    near(
        volume("multmatrix([[1, 0.5, 0, 3], [0, 1, 0, 0], [0, 0, 2, 0]]) cube(1);"),
        2.0,
    );
    let s = solid("resize([20, 0, 0], auto = true) cube([10, 5, 2]);").solid;
    let (lo, hi) = s.bounds().unwrap();
    near(hi[0] - lo[0], 20.0);
    near(hi[1] - lo[1], 10.0);
    // rotate([0, 0, 90]) takes +X to +Y, exactly.
    let s = solid("rotate([0, 0, 90]) translate([5, 0, 0]) cube(1);").solid;
    let (lo, hi) = s.bounds().unwrap();
    assert_eq!((lo, hi), ([-1.0, 5.0, 0.0], [0.0, 6.0, 1.0]));
}

#[test]
fn hulls() {
    near(
        volume("hull() { cube(1); translate([4, 0, 0]) cube(1); }"),
        5.0,
    );
    // Two balls and the tube between them: more than both, less than a box.
    let ball = volume("sphere(1, $fn = 8);");
    let v = volume("hull() { sphere(1, $fn = 8); translate([5, 0, 0]) sphere(1, $fn = 8); }");
    assert!(v > 2.0 * ball && v < 7.0 * 2.0 * 2.0, "{v}");
    // 2D: a stadium.
    let a = ngon(1.0, 12);
    near(
        volume(
            "linear_extrude(1) hull() { circle(1, $fn = 12); translate([4, 0]) circle(1, $fn = 12); }",
        ),
        a + 4.0 * 2.0,
    );
}

#[test]
fn linear_extrusions() {
    near(volume("linear_extrude(height = 5) square([2, 3]);"), 30.0);
    near(
        volume("linear_extrude(5, center = true) circle(r = 2, $fn = 10);"),
        ngon(2.0, 10) * 5.0,
    );
    // Scaled: a frustum.
    near(
        volume("linear_extrude(height = 10, scale = 0.5) square(10, center = true);"),
        10.0 / 3.0 * (100.0 + 25.0 + 50.0),
    );
    // Twisted: closed, its top turned clockwise, its sides split as
    // OpenSCAD splits them: a little less inside.
    let out = solid(
        "linear_extrude(height = 10, twist = 90, slices = 20) square([10, 2], center = true);",
    );
    let v = out.solid.volume();
    assert!(v < 200.0 && v > 185.0, "{v}");
    let top: Vec<[f64; 3]> = out
        .solid
        .points
        .iter()
        .copied()
        .filter(|p| (p[2] - 10.0).abs() < 1e-9)
        .collect();
    assert!(
        top.iter()
            .all(|p| p[0].abs() < 1.0 + 1e-6 && p[1].abs() < 5.0 + 1e-6),
        "{top:?}"
    );
    // Without slices: as many as the twist takes of the circle's pieces.
    let s = solid("linear_extrude(height = 10, twist = 180, $fn = 8) square(4, center = true);");
    let heights: std::collections::BTreeSet<i64> = s
        .solid
        .points
        .iter()
        .map(|p| (p[2] * 1000.0).round() as i64)
        .collect();
    assert_eq!(heights.len(), 5);
}

#[test]
fn polygons_with_holes() {
    let text = "linear_extrude(2) polygon(points = [[0, 0], [10, 0], [10, 10], [0, 10], [2, 2], [4, 2], [4, 4], [2, 4]], paths = [[0, 1, 2, 3], [4, 5, 6, 7]]);";
    near(volume(text), 2.0 * 96.0);
    near(
        volume(
            "linear_extrude(1) difference() { square(10); translate([5, 5]) circle(2, $fn = 8); }",
        ),
        100.0 - ngon(2.0, 8),
    );
    near(
        volume("linear_extrude(1) polygon([[0, 0], [4, 0], [4, 4], [2, 1], [0, 4]]);"),
        10.0,
    );
}

#[test]
fn rotate_extrusions() {
    // A ring of a rectangle: the outer prism less the inner one.
    let ring = 4.0 * (ngon(7.0, 16) - ngon(5.0, 16));
    near(
        volume("rotate_extrude($fn = 16) translate([5, 0]) square([2, 4]);"),
        ring,
    );
    // A quarter, in four steps.
    let wedge = |r: f64| 4.0 / 2.0 * r * r * (std::f64::consts::FRAC_PI_2 / 4.0).sin();
    near(
        volume("rotate_extrude(angle = 90, $fn = 16) translate([5, 0]) square([2, 4]);"),
        4.0 * (wedge(7.0) - wedge(5.0)),
    );
    // Touching the axis: a cone.
    near(
        volume("rotate_extrude($fn = 12) polygon([[0, 0], [3, 0], [0, 4]]);"),
        ngon(3.0, 12) * 4.0 / 3.0,
    );
}

#[test]
fn modifiers() {
    near(
        volume("cube(1); *cube(5); %translate([9, 0, 0]) cube(5);"),
        1.0,
    );
    near(
        volume("cube(1); !translate([9, 0, 0]) cube(2); cube(3);"),
        8.0,
    );
    near(
        volume("#cube(2); color(\"red\") translate([5, 0, 0]) cube(1);"),
        9.0,
    );
}

#[test]
fn intersection_for() {
    near(
        volume(
            "intersection_for (a = [0, 45]) rotate([0, 0, a]) cube([10, 10, 1], center = true);",
        ),
        // An octagon of inner radius 5.
        8.0 * 25.0 * (std::f64::consts::PI / 8.0).tan(),
    );
}

#[test]
fn polyhedrons_either_way_round() {
    let points = "[[0, 0, 0], [1, 0, 0], [0, 1, 0], [0, 0, 1]]";
    // OpenSCAD's order: clockwise seen from outside.
    near(
        volume(&format!(
            "polyhedron({points}, [[0, 1, 2], [0, 3, 1], [0, 2, 3], [1, 3, 2]]);"
        )),
        1.0 / 6.0,
    );
    near(
        volume(&format!(
            "polyhedron({points}, [[0, 2, 1], [0, 1, 3], [0, 3, 2], [1, 2, 3]]);"
        )),
        1.0 / 6.0,
    );
    // Faces of four corners.
    let cube = "polyhedron([[0,0,0],[1,0,0],[1,1,0],[0,1,0],[0,0,1],[1,0,1],[1,1,1],[0,1,1]], \
                [[0,1,2,3],[4,5,1,0],[7,6,5,4],[5,6,2,1],[6,7,3,2],[7,4,0,3]]);";
    near(volume(cube), 1.0);
}

#[test]
fn not_supported_is_said() {
    let out = solid("cube(1); surface(\"a.png\"); text(\"a\");");
    assert!(
        out.notes
            .iter()
            .any(|n| n.contains("not supported: surface"))
    );
    assert!(out.notes.iter().any(|n| n.contains("not supported: text")));
}

/// Minkowski sums and offsets, their volumes as OpenSCAD makes them.
#[test]
fn minkowski_sums() {
    // Convex: the hull of the sums.
    near(
        volume("minkowski() { cube([20, 30, 10], center = true); sphere(2, $fn = 16); }"),
        volume(
            "hull() for (x = [-10, 10], y = [-15, 15], z = [-5, 5]) translate([x, y, z]) sphere(2, $fn = 16);",
        ),
    );
    // Not convex: an L and a sphere, a frame and a cube, in 2D an L and a
    // circle.
    near(
        volume(
            "minkowski() { union() { cube([20, 5, 5]); cube([5, 20, 5]); } sphere(1, $fn = 8); }",
        ),
        1694.762,
    );
    near(
        volume(
            "minkowski() { difference() { cube(20, center = true); \
             cube([10, 10, 30], center = true); } cube(2, center = true); }",
        ),
        9240.0,
    );
    near(
        volume(
            "linear_extrude(2) minkowski() { polygon([[0, 0], [20, 0], [20, 5], [5, 5], \
             [5, 20], [0, 20]]); circle(1, $fn = 8); }",
        ),
        515.071,
    );
}

#[test]
fn projections() {
    let parts = "union() { cube([20, 10, 5]); translate([5, 5, 0]) rotate([30, 20, 10]) \
                 cube([10, 10, 10]); translate([30, 0, 0]) difference() { \
                 cylinder(r = 8, h = 4, $fn = 24); cylinder(r = 3, h = 10, center = true, \
                 $fn = 12); } }";
    // As OpenSCAD makes them: the shadow, and the section.
    near(
        volume(&format!("linear_extrude(1) projection() {parts}")),
        427.176,
    );
    near(
        volume(&format!(
            "linear_extrude(1) projection(cut = true) translate([0, 0, -2]) {{ {parts} \
             sphere(4, $fn = 12); }}"
        )),
        425.515,
    );
}

#[test]
fn polygons_that_cross_themselves_are_filled_even_odd() {
    // As OpenSCAD fills them: a pentagram without its middle, a bow tie,
    // a bar across a frame; closed, the points where parts touch apart.
    for (text, v) in [
        (
            "polygon([for (i = [0:4]) 50 * [cos(90 + 144 * i), sin(90 + 144 * i)]]);",
            1939.192,
        ),
        (
            "polygon([[0, 0], [10, 0], [10, 10], [5, -5], [0, 10]]);",
            41.667,
        ),
        (
            "polygon(points = [[0, 0], [20, 0], [20, 20], [0, 20], [5, 5], [15, 5], [15, 15], \
             [5, 15], [8, -3], [12, -3], [12, 25], [8, 25]], paths = [[0, 1, 2, 3], \
             [4, 5, 6, 7], [8, 9, 10, 11]]);",
            332.0,
        ),
    ] {
        let v2 = volume(&format!("linear_extrude(1) {text}"));
        assert!((v2 - v).abs() < 2e-3, "{v2} is not {v}: {text}");
    }
}

#[test]
fn nothing_made_is_a_child_too() {
    // From nothing nothing is cut; an `if` that makes nothing is no child.
    near(
        volume(
            "difference() { if (false) cube(1); cube(2); sphere(1); }\n\
             translate([5, 0, 0]) intersection() { if (false) cube(1); cube(2); \
             translate([1, 0, 0]) cube(2); }\n\
             translate([10, 0, 0]) difference() { group() {} cube(3); }",
        ),
        11.693,
    );
}

#[test]
fn offsets() {
    let l = "polygon([[0, 0], [20, 0], [20, 20], [11, 20], [11, 5], [9, 5], [9, 20], [0, 20]])";
    let ring = "difference() { square(20); translate([5, 5]) square(10); }";
    for (text, v) in [
        ("offset(r = 2, $fn = 16) square(10);".to_string(), 192.246),
        (
            "offset(delta = 2, chamfer = true) square(10);".into(),
            193.255,
        ),
        ("offset(r = -2, $fn = 16) square(10);".into(), 36.0),
        ("offset(delta = -3) circle(10, $fn = 30);".into(), 152.094),
        ("offset(delta = 3) circle(10, $fn = 30);".into(), 528.397),
        (format!("offset(delta = 1.5) {ring}"), 480.0),
        (format!("offset(r = -1.5, $fn = 20) {ring}"), 122.047),
        // The slot closes: the outlines moved out cross.
        (format!("offset(delta = 2) {l};"), 576.0),
        (format!("offset(r = 2, $fn = 16) {l};"), 572.021),
    ] {
        near(volume(&format!("linear_extrude(1) {text}")), v);
    }
}

#[test]
fn too_many_pieces_are_capped_and_said() {
    let out = solid("cylinder(h = 1, r = 5, $fn = 100000);");
    assert!(
        out.notes.iter().any(|n| n.contains("too many")),
        "{:?}",
        out.notes
    );
    near(out.solid.volume(), ngon(5.0, 1000));
}
