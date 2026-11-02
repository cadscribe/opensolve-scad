//! Compares this interpreter with OpenSCAD, one file at a time, for a
//! script that runs both over a folder of models:
//!
//! `cargo run --release -p opensolve-scad --example scad_corpus -- build
//! [--lib DIR] [--stl OUT] FILE` builds the file once and writes a line of
//! JSON: whether it worked (or the error), milliseconds, volume,
//! triangles, closed, bounding box, the notes; with `--stl`, the solid as a
//! binary STL.
//!
//! `… -- compare A.stl B.stl [SAMPLES]` reads two STL files (A the
//! reference) and writes a line of JSON: each one's triangles, volume,
//! closedness and bounding box, and the volume of their symmetric
//! difference, |A−B| + |B−A|, by the kernel's booleans; when a boolean is
//! refused, estimated from points sampled in both boxes instead (`method`
//! says which).

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::Instant;

use opensolve_kernel::{FaceId, Facets, Op, Solid, read_stl};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("build") => build(&args[1..]),
        Some("compare") => compare(&args[1..]),
        _ => {
            eprintln!(
                "usage: scad_corpus build [--lib DIR] [--stl OUT] FILE | compare A.stl B.stl"
            );
            std::process::exit(2);
        }
    }
}

fn build(args: &[String]) {
    let mut libs = Vec::new();
    let mut stl = None;
    let mut file = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--lib" => libs.extend(it.next().map(PathBuf::from)),
            "--stl" => stl = it.next().map(PathBuf::from),
            _ => file = Some(PathBuf::from(a)),
        }
    }
    let Some(file) = file else {
        eprintln!("no file");
        std::process::exit(2);
    };
    if !libs.is_empty() {
        opensolve_scad::library::set_dirs(Some(libs));
    }
    let read = |p: &Path| std::fs::read(p).map_err(|e| e.to_string());
    let t = Instant::now();
    let out = opensolve_scad::build(&file, &read, &[]);
    let ms = t.elapsed().as_secs_f64() * 1000.0;
    let mut j = String::from("{");
    let _ = write!(j, "\"ms\":{ms:.1}");
    match out {
        Ok(o) => {
            let _ = write!(j, ",\"ok\":true");
            stats(&mut j, "", &o.solid);
            let _ = write!(j, ",\"closed_in_memory\":{}", o.solid.is_closed());
            let notes: Vec<String> = o.notes.iter().take(200).map(|n| quote(n)).collect();
            let _ = write!(
                j,
                ",\"note_count\":{},\"notes\":[{}]",
                o.notes.len(),
                notes.join(",")
            );
            if let Some(p) = stl
                && let Err(e) = std::fs::write(&p, o.solid.to_stl())
            {
                let _ = write!(j, ",\"stl_error\":{}", quote(&e.to_string()));
            }
        }
        Err(e) => {
            let _ = write!(j, ",\"ok\":false,\"error\":{}", quote(&e));
        }
    }
    j.push('}');
    println!("{j}");
}

fn compare(args: &[String]) {
    let (Some(a), Some(b)) = (args.first(), args.get(1)) else {
        eprintln!("compare needs two STL files");
        std::process::exit(2);
    };
    let samples: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(400_000);
    let load = |p: &str| -> Result<Vec<[[f64; 3]; 3]>, String> {
        let b = std::fs::read(p).map_err(|e| e.to_string())?;
        read_stl(&b).ok_or_else(|| format!("{p}: not an STL file"))
    };
    let (ta, tb) = match (load(a), load(b)) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(e), _) | (_, Err(e)) => {
            println!("{{\"error\":{}}}", quote(&e));
            return;
        }
    };
    let mut facets = Facets::default();
    let sa = Solid::from_triangles(&ta, &mut facets, |k| FaceId(k as u64));
    let sb = Solid::from_triangles(&tb, &mut facets, |k| FaceId(1 << 32 | k as u64));
    let mut j = String::from("{");
    let _ = write!(j, "\"a_raw_tris\":{},\"b_raw_tris\":{}", ta.len(), tb.len());
    stats(&mut j, "a_", &sa);
    stats(&mut j, "b_", &sb);
    let t = Instant::now();
    let booleans = (|| -> Result<(f64, f64), String> {
        let ab = sa
            .boolean(&sb, Op::Subtract, &mut facets)
            .map_err(|e| e.to_string())?;
        let ba = sb
            .boolean(&sa, Op::Subtract, &mut facets)
            .map_err(|e| e.to_string())?;
        Ok((ab.volume(), ba.volume()))
    })();
    match booleans {
        Ok((ab, ba)) => {
            let _ = write!(
                j,
                ",\"method\":\"boolean\",\"a_minus_b\":{ab:.6},\"b_minus_a\":{ba:.6},\"symdiff\":{:.6}",
                ab.abs() + ba.abs()
            );
        }
        Err(e) => {
            let (sym, ab, ba) = sampled(&ta, &tb, samples);
            let _ = write!(
                j,
                ",\"method\":\"sampled\",\"boolean_error\":{},\"samples\":{samples},\"a_minus_b\":{ab:.6},\"b_minus_a\":{ba:.6},\"symdiff\":{sym:.6}",
                quote(&e)
            );
        }
    }
    let _ = write!(
        j,
        ",\"compare_ms\":{:.1}",
        t.elapsed().as_secs_f64() * 1000.0
    );
    j.push('}');
    println!("{j}");
}

/// Volume, triangles, closedness and box of a solid, as JSON fields.
fn stats(j: &mut String, prefix: &str, s: &Solid) {
    let _ = write!(
        j,
        ",\"{prefix}volume\":{:.6},\"{prefix}tris\":{},\"{prefix}closed\":{}",
        s.volume(),
        s.tris.len(),
        s.is_closed()
    );
    if let Some((lo, hi)) = s.bounds() {
        let _ = write!(
            j,
            ",\"{prefix}bbox\":[[{:.5},{:.5},{:.5}],[{:.5},{:.5},{:.5}]]",
            lo[0], lo[1], lo[2], hi[0], hi[1], hi[2]
        );
    }
}

fn quote(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            c if (c as u32) < 0x20 => {
                let _ = write!(o, "\\u{:04x}", c as u32);
            }
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

/// Triangles binned by their extent in x and y, for counting the crossings
/// of a ray up the z axis.
struct Grid {
    lo: [f64; 2],
    cell: [f64; 2],
    n: usize,
    bins: Vec<Vec<u32>>,
    tris: Vec<[[f64; 3]; 3]>,
}

impl Grid {
    fn new(tris: &[[[f64; 3]; 3]], lo: [f64; 2], hi: [f64; 2]) -> Grid {
        let n = ((tris.len() as f64).sqrt() as usize).clamp(8, 512);
        let cell = [
            ((hi[0] - lo[0]) / n as f64).max(1e-9),
            ((hi[1] - lo[1]) / n as f64).max(1e-9),
        ];
        let mut g = Grid {
            lo,
            cell,
            n,
            bins: vec![Vec::new(); n * n],
            tris: tris.to_vec(),
        };
        for (i, t) in tris.iter().enumerate() {
            let (mut x0, mut x1, mut y0, mut y1) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
            for p in t {
                x0 = x0.min(p[0]);
                x1 = x1.max(p[0]);
                y0 = y0.min(p[1]);
                y1 = y1.max(p[1]);
            }
            let (cx0, cy0) = (g.at(x0, 0), g.at(y0, 1));
            let (cx1, cy1) = (g.at(x1, 0), g.at(y1, 1));
            for cy in cy0..=cy1 {
                for cx in cx0..=cx1 {
                    g.bins[cy * n + cx].push(i as u32);
                }
            }
        }
        g
    }

    fn at(&self, v: f64, axis: usize) -> usize {
        (((v - self.lo[axis]) / self.cell[axis]).floor().max(0.0) as usize).min(self.n - 1)
    }

    /// Whether the point is inside: an odd number of triangles above it.
    fn inside(&self, p: [f64; 3]) -> bool {
        let bin = &self.bins[self.at(p[1], 1) * self.n + self.at(p[0], 0)];
        let mut crossings = 0u32;
        for &i in bin {
            let [a, b, c] = self.tris[i as usize];
            let d = (b[0] - a[0]) * (c[1] - a[1]) - (c[0] - a[0]) * (b[1] - a[1]);
            if d.abs() < 1e-300 {
                continue;
            }
            let u = ((b[0] - p[0]) * (c[1] - p[1]) - (c[0] - p[0]) * (b[1] - p[1])) / d;
            let v = ((c[0] - p[0]) * (a[1] - p[1]) - (a[0] - p[0]) * (c[1] - p[1])) / d;
            let w = 1.0 - u - v;
            if u < 0.0 || v < 0.0 || w < 0.0 {
                continue;
            }
            if u * a[2] + v * b[2] + w * c[2] > p[2] {
                crossings += 1;
            }
        }
        crossings % 2 == 1
    }
}

/// The symmetric difference and both one-sided differences, estimated
/// from `n` points spread through the box of both meshes.
fn sampled(a: &[[[f64; 3]; 3]], b: &[[[f64; 3]; 3]], n: usize) -> (f64, f64, f64) {
    let mut lo = [f64::MAX; 3];
    let mut hi = [f64::MIN; 3];
    for p in a.iter().chain(b).flatten() {
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    if lo[0] > hi[0] {
        return (0.0, 0.0, 0.0);
    }
    let ga = Grid::new(a, [lo[0], lo[1]], [hi[0], hi[1]]);
    let gb = Grid::new(b, [lo[0], lo[1]], [hi[0], hi[1]]);
    // A fixed sequence, so that a run repeats.
    let mut s: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut next = || {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        (s >> 11) as f64 / (1u64 << 53) as f64
    };
    let (mut only_a, mut only_b) = (0usize, 0usize);
    for _ in 0..n {
        let p = [0, 1, 2].map(|k| lo[k] + next() * (hi[k] - lo[k]));
        match (ga.inside(p), gb.inside(p)) {
            (true, false) => only_a += 1,
            (false, true) => only_b += 1,
            _ => {}
        }
    }
    let box_volume = (hi[0] - lo[0]) * (hi[1] - lo[1]) * (hi[2] - lo[2]);
    let ab = only_a as f64 / n as f64 * box_volume;
    let ba = only_b as f64 / n as f64 * box_volume;
    (ab + ba, ab, ba)
}
