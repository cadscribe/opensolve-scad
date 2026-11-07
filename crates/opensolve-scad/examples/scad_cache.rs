//! Measures a cache of results across runs (`opensolve_scad::Cache`) on a
//! file, and checks that it makes the same as without:
//!
//! `cargo run --release -p opensolve-scad --example scad_cache -- [--lib
//! DIR] [--vary] [-D NAME=VALUE]... FILE [--then -D NAME=VALUE]...`
//!
//! Runs the file without a cache, then with one: a first run (the cache
//! empty), the same again, and each `--then` set of `-D` (a parameter
//! changed) after the runs before it, each also without the cache to
//! compare. `--vary` adds a step of its own: the first number among the
//! file's Customizer parameters changed by a tenth (at least 1). Writes a
//! line for each: milliseconds with and without the cache, whether the
//! shapes, colours and notes are the same, and the cache's numbers.

use std::path::{Path, PathBuf};
use std::time::Instant;

use opensolve_scad::{Cache, Made, Run, Shape, make};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut libs = Vec::new();
    let mut file = None;
    let mut sets: Vec<Vec<String>> = vec![Vec::new()];
    let mut vary = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--vary" => vary = true,
            "--lib" => libs.extend(it.next().map(PathBuf::from)),
            "-D" => sets.last_mut().unwrap().extend(it.next().cloned()),
            "--then" => {
                let mut next = sets.last().cloned().unwrap_or_default();
                next.clear();
                sets.push(next);
            }
            _ => file = Some(PathBuf::from(a)),
        }
    }
    let Some(file) = file else {
        eprintln!("usage: scad_cache [--lib DIR] [-D N=V]... FILE [--then -D N=V]...");
        std::process::exit(2);
    };
    if !libs.is_empty() {
        opensolve_scad::library::set_dirs(Some(libs));
    }
    if vary {
        let text = std::fs::read_to_string(&file).unwrap_or_default();
        let first = opensolve_scad::parameters(&text)
            .unwrap_or_default()
            .into_iter()
            .find_map(|p| Some((p.name, p.value.trim().parse::<f64>().ok()?)));
        if let Some((name, v)) = first {
            let next = v + (v.abs() / 10.0).max(1.0);
            sets.push(vec![format!("{name}={next}")]);
        }
    }
    let cache = Cache::new();
    // The first set is the start; each `--then` replaces the values it
    // names, as a parameter changed in the Customizer.
    let mut defines: Vec<String> = Vec::new();
    let mut steps: Vec<(String, Vec<String>)> = Vec::new();
    for (i, set) in sets.iter().enumerate() {
        for d in set {
            let name = d.split('=').next().unwrap_or_default();
            defines.retain(|x| x.split('=').next() != Some(name));
            defines.push(d.clone());
        }
        if i == 0 {
            steps.push(("first".into(), defines.clone()));
            steps.push(("again".into(), defines.clone()));
        } else {
            steps.push((set.join(" "), defines.clone()));
        }
    }
    // Once for the files to be read and parsed, which a document open in
    // OpenSolve has been already.
    let _ = run(&file, &steps[0].1, None);
    let mut all_same = true;
    for (name, defs) in &steps {
        let (plain, t0) = run(&file, defs, None);
        let (cached, t1) = run(&file, defs, Some(&cache));
        let mut same = same(&plain, &cached);
        // A file that does not build the same each time without a cache
        // either (the boolean library's threads) is not held against it.
        let mut stable = true;
        if same.is_err() {
            let (again, _) = run(&file, defs, None);
            stable = self::same(&plain, &again).is_ok();
            if !stable {
                same = Ok(());
            }
        }
        all_same &= same.is_ok();
        println!(
            "{{\"step\":{name:?},\"ms_without\":{:.1},\"ms_with\":{:.1},\"same\":{},\"stable\":{stable},\"diff\":{:?},\"cache\":\"{}\"}}",
            t0 * 1000.0,
            t1 * 1000.0,
            same.is_ok(),
            same.err().unwrap_or_default(),
            cache.stats()
        );
    }
    if !all_same {
        std::process::exit(1);
    }
}

fn run(file: &Path, defines: &[String], cache: Option<&Cache>) -> (Result<Made, String>, f64) {
    let read = |p: &Path| std::fs::read(p).map_err(|e| e.to_string());
    let r = Run {
        defines: defines.to_vec(),
        cache: cache.cloned(),
        ..Run::default()
    };
    let t = Instant::now();
    let made = make(file, &read, &r).map_err(|e| e.to_string());
    (made, t.elapsed().as_secs_f64())
}

/// Whether two results are the same: shapes (corners, triangles, the plane
/// of each triangle's facet, which triangles share a face), colours, notes.
fn same(a: &Result<Made, String>, b: &Result<Made, String>) -> Result<(), String> {
    let (a, b) = match (a, b) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(x), Err(y)) if x == y => return Ok(()),
        (x, y) => return Err(format!("{:?} / {:?}", x.as_ref().err(), y.as_ref().err())),
    };
    if a.notes != b.notes {
        let at = a.notes.iter().zip(&b.notes).position(|(x, y)| x != y);
        return Err(format!(
            "notes: {} / {}, the first apart: {:?} / {:?}",
            a.notes.len(),
            b.notes.len(),
            at.and_then(|i| a.notes.get(i)),
            at.and_then(|i| b.notes.get(i))
        ));
    }
    if a.colours != b.colours {
        return Err("colours".into());
    }
    same_shape(&a.shape, &b.shape)
}

fn same_shape(a: &Shape, b: &Shape) -> Result<(), String> {
    match (a, b) {
        (Shape::Empty, Shape::Empty) => Ok(()),
        (Shape::Flat(x), Shape::Flat(y)) if x == y => Ok(()),
        (Shape::Solid(s, f), Shape::Solid(t, g)) => {
            if s.tris.len() != t.tris.len() {
                return Err(format!("triangles: {} / {}", s.tris.len(), t.tris.len()));
            }
            if surface(s, f) != surface(t, g) {
                let flat = |x: Vec<Vec<[u64; 13]>>, planes: bool| {
                    let mut v: Vec<Vec<u64>> = x
                        .into_iter()
                        .flatten()
                        .map(|r| r[..if planes { 13 } else { 9 }].to_vec())
                        .collect();
                    v.sort_unstable();
                    v
                };
                let what = if flat(surface(s, f), false) != flat(surface(t, g), false) {
                    "the surface"
                } else if flat(surface(s, f), true) != flat(surface(t, g), true) {
                    // How far apart: the planes of a face's facets are the
                    // same within rounding, and which names it is order.
                    let mut most = 0.0f64;
                    let mut x: Vec<Vec<u64>> = surface(s, f)
                        .into_iter()
                        .flatten()
                        .map(|r| r.to_vec())
                        .collect();
                    let mut y: Vec<Vec<u64>> = surface(t, g)
                        .into_iter()
                        .flatten()
                        .map(|r| r.to_vec())
                        .collect();
                    x.sort_unstable_by(|a, b| a[..9].cmp(&b[..9]));
                    y.sort_unstable_by(|a, b| a[..9].cmp(&b[..9]));
                    for (p, q) in x.iter().zip(&y) {
                        for k in 9..13 {
                            most = most.max((f64::from_bits(p[k]) - f64::from_bits(q[k])).abs());
                        }
                    }
                    return Err(format!(
                        "the planes of the facets, at most {most:e} apart, {} triangles",
                        s.tris.len()
                    ));
                } else {
                    let count = |x: &opensolve_kernel::Facets, so: &opensolve_kernel::Solid| {
                        so.facets
                            .iter()
                            .map(|&i| x.get(i).face)
                            .collect::<std::collections::HashSet<_>>()
                            .len()
                    };
                    return Err(format!(
                        "the faces: {} / {} of {} triangles",
                        count(f, s),
                        count(g, t),
                        s.tris.len()
                    ));
                };
                return Err(format!("{what}, {} triangles", s.tris.len()));
            }
            Ok(())
        }
        _ => Err("different kinds of shape".into()),
    }
}

/// A solid's triangles, each by its corners from its least one and its
/// facet's plane, in groups by face, all sorted: the same for the same
/// surface cut into the same faces, in whichever order the boolean
/// library's threads gave the triangles.
fn surface(s: &opensolve_kernel::Solid, f: &opensolve_kernel::Facets) -> Vec<Vec<[u64; 13]>> {
    let mut faces: std::collections::HashMap<u64, Vec<[u64; 13]>> = Default::default();
    for (t, &facet) in s.tris.iter().zip(&s.facets) {
        let c = t.map(|i| s.points[i as usize].map(f64::to_bits));
        let k = (0..3).min_by_key(|&k| c[k]).unwrap_or(0);
        let x = f.get(facet);
        let mut row = [0u64; 13];
        for (j, p) in [c[k], c[(k + 1) % 3], c[(k + 2) % 3]].iter().enumerate() {
            row[3 * j..3 * j + 3].copy_from_slice(p);
        }
        row[9] = x.normal[0].to_bits();
        row[10] = x.normal[1].to_bits();
        row[11] = x.normal[2].to_bits();
        row[12] = x.offset.to_bits();
        faces.entry(x.face.0).or_default().push(row);
    }
    let mut out: Vec<Vec<[u64; 13]>> = faces
        .into_values()
        .map(|mut v| {
            v.sort_unstable();
            v
        })
        .collect();
    out.sort_unstable();
    out
}
