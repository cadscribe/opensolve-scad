//! The booleans by boolmesh 0.2.1 (a Rust port of Manifold), where
//! Manifold's C++ cannot go: in the browser. Its meshes carry each corner's
//! facet as a value, which corners that share a vertex can blend; those are
//! repaired from the planes the facets lie in.

use std::collections::HashMap;
use std::ops::{Add, Mul};

use boolmesh::prelude::{Manifold, OpType, compute_boolean};

use crate::{Error, Facets, Op, Solid, dot, nearest_facet};

pub(crate) type Mesh = Manifold<Tag>;

/// Value carried through a boolean: the facet, as a number.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Tag(f64);

impl Add for Tag {
    type Output = Tag;
    fn add(self, o: Tag) -> Tag {
        Tag(self.0 + o.0)
    }
}

impl Mul<boolmesh::Real> for Tag {
    type Output = Tag;
    fn mul(self, k: boolmesh::Real) -> Tag {
        Tag(self.0 * k)
    }
}

/// The library's mesh of a solid. It welds corners by their place: parts
/// that touch along an edge would make an edge of four triangles, which it
/// cannot take (it panics). Such parts are moved apart a hair.
pub(crate) fn make(s: &Solid) -> Result<Mesh, Error> {
    let parted;
    let me = match s.parted() {
        Some(p) => {
            parted = p;
            &parted
        }
        None => s,
    };
    let pos: Vec<f64> = me.points.iter().flatten().copied().collect();
    let idx: Vec<usize> = me.tris.iter().flatten().map(|&i| i as usize).collect();
    let vals: Vec<Tag> = me
        .facets
        .iter()
        .flat_map(|&f| std::iter::repeat_n(Tag(f64::from(f)), 3))
        .collect();
    Manifold::new_with_values(&pos, &idx, &vals).map_err(Error::Boolean)
}

/// The boolean; `None` if nothing is left (the library has no empty mesh).
pub(crate) fn boolean(a: &Mesh, b: &Mesh, op: Op) -> Result<Option<Mesh>, Error> {
    let kind = match op {
        Op::Add => OpType::Add,
        Op::Subtract => OpType::Subtract,
        Op::Intersect => OpType::Intersect,
    };
    match compute_boolean(a, b, kind) {
        Ok(m) => Ok(Some(m)),
        Err(e) if e.contains("empty") => Ok(None),
        Err(e) => Err(Error::Boolean(e)),
    }
}

/// The library takes two solids at a time: a batch goes in pairs.
pub(crate) const BATCHES: bool = false;

pub(crate) fn union(_: &[Vec<&Mesh>]) -> Result<Vec<Mesh>, Error> {
    Err(Error::Boolean("no batch booleans".into()))
}

pub(crate) fn intersection(_: &[&Mesh]) -> Result<Option<Mesh>, Error> {
    Err(Error::Boolean("no batch booleans".into()))
}

/// The corners, triangles and each triangle's facet of a result.
pub(crate) fn read(m: &Mesh, facets: &Facets) -> (Vec<[f64; 3]>, Vec<[u32; 3]>, Vec<u32>) {
    let points: Vec<[f64; 3]> = m.ps.iter().map(|p| [p.x, p.y, p.z]).collect();
    let tris: Vec<[u32; 3]> =
        m.hs.chunks(3)
            .map(|h| [h[0].tail, h[1].tail, h[2].tail].map(|i| i as u32))
            .collect();
    let at = |t: [u32; 3]| t.map(|i| points[i as usize]);
    // Corners that share a vertex can blend the tags of two faces. A tag
    // on a clean triangle has held in every test, so only the blended
    // ones are repaired, by the plane their tag has to lie in.
    let whole = |v: &Tag| (v.0 - v.0.round()).abs() < 1e-6 && v.0 >= 0.0;
    let mut out: Vec<Option<u32>> = m
        .vals
        .chunks(3)
        .zip(&tris)
        .map(|(c, t)| {
            let clean = c.iter().all(|v| whole(v) && (v.0 - c[0].0).abs() < 1e-6);
            if clean {
                return Some(c[0].0.round() as u32);
            }
            c.iter()
                .filter(|v| whole(v))
                .map(|v| v.0.round() as u32)
                .find(|&f| (f as usize) < facets.all.len() && facets.fits(f, at(*t)))
        })
        .collect();
    // What is left takes the facet of a neighbour in the same plane.
    let mut edges: HashMap<(u32, u32), usize> = HashMap::new();
    for (i, t) in tris.iter().enumerate() {
        for k in 0..3 {
            edges.insert((t[k], t[(k + 1) % 3]), i);
        }
    }
    // Pass after pass while one finds more: a triangle none fits stops
    // nothing.
    loop {
        let mut found_any = false;
        for i in 0..tris.len() {
            if out[i].is_some() {
                continue;
            }
            let t = tris[i];
            let found = (0..3)
                .filter_map(|k| edges.get(&(t[(k + 1) % 3], t[k])).and_then(|&n| out[n]))
                .find(|&f| facets.fits(f, at(t)));
            out[i] = found;
            found_any |= found.is_some();
        }
        if !found_any {
            break;
        }
    }
    // The rest: the nearest of the facets its corners and neighbours
    // name, else of all.
    let near: Vec<Option<u32>> = (0..tris.len())
        .map(|i| {
            if out[i].is_some() {
                return None;
            }
            let t = tris[i];
            let mut names: Vec<u32> = m.vals[3 * i..3 * i + 3]
                .iter()
                .filter(|v| whole(v))
                .map(|v| v.0.round() as u32)
                .filter(|&f| (f as usize) < facets.all.len())
                .collect();
            names.extend(
                (0..3).filter_map(|k| edges.get(&(t[(k + 1) % 3], t[k])).and_then(|&n| out[n])),
            );
            let off = |f: u32| {
                let x = facets.get(f);
                at(t)
                    .iter()
                    .map(|p| (dot(x.normal, *p) - x.offset).abs())
                    .sum::<f64>()
            };
            names
                .into_iter()
                .min_by(|&a, &b| off(a).total_cmp(&off(b)))
                .or_else(|| Some(nearest_facet(facets, at(t))))
        })
        .collect();
    let found = out
        .iter()
        .zip(&near)
        .map(|(f, n)| f.or(*n).unwrap_or(0))
        .collect();
    (points, tris, found)
}

/// The mesh with its triangles' facets as the solid has them: the next
/// boolean starts from the repaired names.
pub(crate) fn retag(mut m: Mesh, facets_of: &[u32]) -> Mesh {
    for (c, f) in m.vals.chunks_mut(3).zip(facets_of) {
        c.fill(Tag(f64::from(*f)));
    }
    m
}

/// No triangulator of loops with holes here: the caller has its own.
pub(crate) fn triangulate_loops(_: &[Vec<[f64; 2]>]) -> Option<Vec<[u32; 3]>> {
    None
}
