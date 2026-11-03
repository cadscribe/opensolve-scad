//! The files the command line writes: meshes (STL, OFF, OBJ, 3MF, STEP),
//! 2D shapes (SVG, DXF) and OpenSCAD's CSG, a file OpenSCAD and this
//! program read again.

use std::fmt::Write as _;

use opensolve_kernel::{Facets, Solid};
use opensolve_scad::{Rgba, rgba_text};

/// The formats, by the name `--export-format` and the extension use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    AsciiStl,
    BinStl,
    Off,
    Obj,
    ThreeMf,
    Step,
    Csg,
    Svg,
    Dxf,
    Echo,
}

impl Format {
    pub fn named(name: &str) -> Option<Format> {
        Some(match name.to_ascii_lowercase().as_str() {
            "stl" | "asciistl" => Format::AsciiStl,
            "binstl" => Format::BinStl,
            "off" => Format::Off,
            "obj" => Format::Obj,
            "3mf" => Format::ThreeMf,
            "step" | "stp" => Format::Step,
            "csg" => Format::Csg,
            "svg" => Format::Svg,
            "dxf" => Format::Dxf,
            "echo" => Format::Echo,
            _ => return None,
        })
    }

    /// Whether the format holds a solid, a 2D shape, or either.
    pub fn dims(self) -> Dims {
        match self {
            Format::Svg | Format::Dxf => Dims::Two,
            Format::Csg | Format::Echo => Dims::Any,
            _ => Dims::Three,
        }
    }

    /// Whether the format keeps colours: then each colour is made apart.
    pub fn keeps_colours(self) -> bool {
        matches!(self, Format::ThreeMf | Format::Csg)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dims {
    Two,
    Three,
    Any,
}

/// The parts of a model: what each colour makes, and what none does.
#[derive(Debug)]
pub struct Parts<T> {
    pub plain: Option<T>,
    pub painted: Vec<(Rgba, T)>,
}

impl<T> Default for Parts<T> {
    fn default() -> Self {
        Parts {
            plain: None,
            painted: Vec::new(),
        }
    }
}

/// A solid as ASCII STL, as OpenSCAD writes it.
pub fn ascii_stl(solid: &Solid) -> Vec<u8> {
    let mut out = String::from("solid OpenSCAD_Model\n");
    for t in &solid.tris {
        let [a, b, c] = t.map(|i| solid.points[i as usize]);
        let n = normal(a, b, c);
        let _ = writeln!(out, "  facet normal {} {} {}", n[0], n[1], n[2]);
        out.push_str("    outer loop\n");
        for p in [a, b, c] {
            let _ = writeln!(out, "      vertex {} {} {}", p[0], p[1], p[2]);
        }
        out.push_str("    endloop\n  endfacet\n");
    }
    out.push_str("endsolid OpenSCAD_Model\n");
    out.into_bytes()
}

fn normal(a: [f64; 3], b: [f64; 3], c: [f64; 3]) -> [f64; 3] {
    let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
    let n = [
        u[1] * v[2] - u[2] * v[1],
        u[2] * v[0] - u[0] * v[2],
        u[0] * v[1] - u[1] * v[0],
    ];
    let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
    if l > 0.0 { n.map(|x| x / l) } else { [0.0; 3] }
}

/// A solid as OFF.
pub fn off(solid: &Solid) -> Vec<u8> {
    let mut out = format!("OFF\n{} {} 0\n", solid.points.len(), solid.tris.len());
    for p in &solid.points {
        let _ = writeln!(out, "{} {} {}", p[0], p[1], p[2]);
    }
    for t in &solid.tris {
        let _ = writeln!(out, "3 {} {} {}", t[0], t[1], t[2]);
    }
    out.into_bytes()
}

/// A solid as Wavefront OBJ.
pub fn obj(solid: &Solid) -> Vec<u8> {
    let mut out = String::from("# opensolve-scad\n");
    for p in &solid.points {
        let _ = writeln!(out, "v {} {} {}", p[0], p[1], p[2]);
    }
    for t in &solid.tris {
        let _ = writeln!(out, "f {} {} {}", t[0] + 1, t[1] + 1, t[2] + 1);
    }
    out.into_bytes()
}

/// A model of solids in colours as 3MF: one part per colour.
pub fn three_mf(name: &str, parts: &Parts<(Solid, Facets)>) -> Vec<u8> {
    let mut list: Vec<(&Solid, String, Option<Rgba>)> = Vec::new();
    for (i, (c, (s, _))) in parts.painted.iter().enumerate() {
        list.push((s, format!("{name} {}", i + 1), Some(*c)));
    }
    if let Some((s, _)) = &parts.plain {
        list.push((s, name.to_string(), None));
    }
    if list.len() == 1 && list[0].2.is_none() {
        return opensolve_kernel::three_mf(list[0].0, name);
    }
    let refs: Vec<(&Solid, &str, Option<Rgba>)> =
        list.iter().map(|(s, n, c)| (*s, n.as_str(), *c)).collect();
    opensolve_kernel::three_mf_painted(name, &refs)
}

/// What a part of the CSG file holds.
#[derive(Debug)]
pub enum Piece {
    Solid(Solid),
    Flat(Vec<Vec<[f64; 2]>>),
}

/// A model as OpenSCAD's CSG: each colour's part a `polyhedron` (or
/// `polygon`) in its `color()`, what has none without.
pub fn csg(parts: &Parts<Piece>) -> Vec<u8> {
    if parts.plain.is_none() && parts.painted.is_empty() {
        return b"group();\n".to_vec();
    }
    let mut out = String::from("group() {\n");
    for (c, p) in &parts.painted {
        let _ = writeln!(out, "color({}) {{", rgba_text(c));
        piece(&mut out, p);
        out.push_str("}\n");
    }
    if let Some(p) = &parts.plain {
        piece(&mut out, p);
    }
    out.push_str("}\n");
    out.into_bytes()
}

fn piece(out: &mut String, p: &Piece) {
    match p {
        Piece::Solid(s) => {
            out.push_str("polyhedron(points = [");
            for (i, q) in s.points.iter().enumerate() {
                let sep = if i == 0 { "" } else { ", " };
                let _ = write!(out, "{sep}[{}, {}, {}]", q[0], q[1], q[2]);
            }
            out.push_str("], faces = [");
            // OpenSCAD's faces turn clockwise seen from outside.
            for (i, t) in s.tris.iter().enumerate() {
                let sep = if i == 0 { "" } else { ", " };
                let _ = write!(out, "{sep}[{}, {}, {}]", t[0], t[2], t[1]);
            }
            out.push_str("], convexity = 1);\n");
        }
        Piece::Flat(loops) => {
            let mut points = String::new();
            let mut paths = String::new();
            let mut n = 0;
            for (k, l) in loops.iter().enumerate() {
                let _ = write!(paths, "{}[", if k == 0 { "" } else { ", " });
                for (i, q) in l.iter().enumerate() {
                    let sep = if n == 0 { "" } else { ", " };
                    let _ = write!(points, "{sep}[{}, {}]", q[0], q[1]);
                    let _ = write!(paths, "{}{n}", if i == 0 { "" } else { ", " });
                    n += 1;
                }
                paths.push(']');
            }
            let _ = writeln!(
                out,
                "polygon(points = [{points}], paths = [{paths}], convexity = 1);"
            );
        }
    }
}

/// A 2D shape as SVG, in millimetres, y up as in OpenSCAD.
pub fn svg(loops: &[Vec<[f64; 2]>]) -> Vec<u8> {
    let (mut lo, mut hi) = ([f64::MAX; 2], [f64::MIN; 2]);
    for p in loops.iter().flatten() {
        for k in 0..2 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    let (w, h) = (hi[0] - lo[0], hi[1] - lo[1]);
    let mut d = String::new();
    for l in loops {
        for (i, p) in l.iter().enumerate() {
            let _ = write!(d, "{}{},{} ", if i == 0 { "M" } else { "L" }, p[0], -p[1]);
        }
        d.push_str("z ");
    }
    format!(
        "<?xml version=\"1.0\" standalone=\"no\"?>\n\
         <svg width=\"{w}mm\" height=\"{h}mm\" viewBox=\"{} {} {w} {h}\" \
         xmlns=\"http://www.w3.org/2000/svg\" version=\"1.1\">\n\
         <title>OpenSCAD Model</title>\n\
         <path d=\"{}\" stroke=\"black\" fill=\"lightgray\" stroke-width=\"0.5\"/>\n</svg>\n",
        lo[0],
        -hi[1],
        d.trim_end()
    )
    .into_bytes()
}

/// A 2D shape as DXF (R12): its edges as lines.
pub fn dxf(loops: &[Vec<[f64; 2]>]) -> Vec<u8> {
    let mut out = String::from("  0\nSECTION\n  2\nENTITIES\n");
    for l in loops {
        for i in 0..l.len() {
            let (a, b) = (l[i], l[(i + 1) % l.len()]);
            let _ = write!(
                out,
                "  0\nLINE\n  8\n0\n 10\n{}\n 20\n{}\n 11\n{}\n 21\n{}\n",
                a[0], a[1], b[0], b[1]
            );
        }
    }
    out.push_str("  0\nENDSEC\n  0\nEOF\n");
    out.into_bytes()
}
