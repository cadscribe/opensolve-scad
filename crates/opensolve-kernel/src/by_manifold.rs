//! The booleans by Manifold, the C++ library OpenSCAD uses (see
//! `opensolve-manifold`). Each triangle carries its facet as Manifold's face
//! number, which every piece of it keeps.

use opensolve_manifold::Error as Refused;

use crate::{Error, Facets, Op, Solid, nearest_facet};

pub(crate) type Mesh = opensolve_manifold::Solid;

fn refused(e: Refused) -> Error {
    Error::Boolean(e.to_string())
}

/// Manifold's mesh of a solid, what is finer than the kernel's grid of
/// positions collapsed in results (a splinter between a corner on the grid
/// and one that a boolean computed next to it, say). Its corners are taken
/// as they are: parts
/// that touch along an edge with corners of their own are two parts. Parts
/// that share the corners of such an edge, which Manifold refuses, are
/// given corners of their own (and moved apart a hair).
pub(crate) fn make(s: &Solid) -> Result<Mesh, Error> {
    let of = |s: &Solid| {
        let points: Vec<f64> = s.points.iter().flatten().copied().collect();
        let tris: Vec<u32> = s.tris.iter().flatten().copied().collect();
        let faces: Vec<u64> = s.facets.iter().map(|&f| u64::from(f)).collect();
        Mesh::new(&points, &tris, &faces, crate::GRID)
    };
    match of(s) {
        Err(Refused::NotManifold) => match s.parted() {
            Some(p) => of(&p).map_err(refused),
            None => Err(refused(Refused::NotManifold)),
        },
        r => r.map_err(refused),
    }
}

fn kind(op: Op) -> opensolve_manifold::Op {
    match op {
        Op::Add => opensolve_manifold::Op::Add,
        Op::Subtract => opensolve_manifold::Op::Subtract,
        Op::Intersect => opensolve_manifold::Op::Intersect,
    }
}

/// The boolean; `None` if nothing is left.
pub(crate) fn boolean(a: &Mesh, b: &Mesh, op: Op) -> Result<Option<Mesh>, Error> {
    let m = a.boolean(b, kind(op)).map_err(refused)?;
    Ok((!m.is_empty()).then_some(m))
}

/// Manifold takes many solids at once ([`union`], [`intersection`]).
pub(crate) const BATCHES: bool = true;

/// A mesh of the caller's or one made here.
enum Item<'a> {
    Given(&'a Mesh),
    Made(Mesh),
}

impl Item<'_> {
    fn get(&self) -> &Mesh {
        match self {
            Item::Given(m) => m,
            Item::Made(m) => m,
        }
    }
}

/// The groups joined, each in rounds of pairs, the small ones first (as
/// Manifold's own batch does it), the pairs of all groups in a round on
/// threads of their own; then the groups' results put together without a
/// boolean, which they must be apart for. `None` if nothing is left.
pub(crate) fn union(groups: &[Vec<&Mesh>]) -> Result<Option<Mesh>, Error> {
    let mut levels: Vec<Vec<Item<'_>>> = groups
        .iter()
        .map(|g| g.iter().map(|m| Item::Given(m)).collect())
        .collect();
    rounds(&mut levels, Op::Add)?;
    let done: Vec<&Mesh> = levels.iter().flatten().map(Item::get).collect();
    let m = match done.as_slice() {
        [] => return Ok(None),
        [one] => (*one).clone(),
        _ => Mesh::compose(&done).map_err(refused)?,
    };
    Ok((!m.is_empty()).then_some(m))
}

/// What all hold, in rounds of pairs as [`union`]; `None` if nothing is.
pub(crate) fn intersection(list: &[&Mesh]) -> Result<Option<Mesh>, Error> {
    let mut levels = vec![list.iter().map(|m| Item::Given(m)).collect()];
    rounds(&mut levels, Op::Intersect)?;
    let m = match levels[0].pop() {
        Some(Item::Made(m)) => m,
        Some(Item::Given(m)) => m.clone(),
        None => return Ok(None),
    };
    Ok((!m.is_empty()).then_some(m))
}

/// Each list brought down to one by `op` in rounds of pairs, the small
/// ones first, the pairs of all lists in a round on threads of their own.
fn rounds(levels: &mut [Vec<Item<'_>>], op: Op) -> Result<(), Error> {
    loop {
        let mut jobs: Vec<(usize, usize)> = Vec::new();
        for (g, level) in levels.iter_mut().enumerate() {
            if level.len() > 1 {
                level.sort_by_key(|m| m.get().num_tri());
                jobs.extend((0..level.len() / 2).map(|k| (g, k)));
            }
        }
        if jobs.is_empty() {
            return Ok(());
        }
        let made = crate::parallel(jobs.len(), |j| {
            let (g, k) = jobs[j];
            let level = &levels[g];
            level[2 * k]
                .get()
                .boolean(level[2 * k + 1].get(), kind(op))
                .map_err(refused)
        });
        let mut made = made.into_iter();
        for level in levels.iter_mut().filter(|l| l.len() > 1) {
            let left = (level.len() % 2 == 1).then(|| level.pop()).flatten();
            let pairs = level.len() / 2;
            level.clear();
            for _ in 0..pairs {
                let m = made
                    .next()
                    .ok_or_else(|| Error::Boolean("a pair was lost".into()))??;
                level.push(Item::Made(m));
            }
            level.extend(left);
        }
    }
}

/// The corners, triangles and each triangle's facet of a result.
pub(crate) fn read(m: &Mesh, facets: &Facets) -> (Vec<[f64; 3]>, Vec<[u32; 3]>, Vec<u32>) {
    let mesh = m.mesh().unwrap_or_default();
    let points: Vec<[f64; 3]> = mesh
        .points
        .chunks_exact(3)
        .map(|p| [p[0], p[1], p[2]])
        .collect();
    let tris: Vec<[u32; 3]> = mesh
        .tris
        .chunks_exact(3)
        .map(|t| [t[0], t[1], t[2]])
        .collect();
    let found = mesh
        .faces
        .iter()
        .zip(&tris)
        .map(|(&f, t)| match u32::try_from(f) {
            Ok(f) if (f as usize) < facets.len() => f,
            _ => nearest_facet(facets, t.map(|i| points[i as usize])),
        })
        .collect();
    (points, tris, found)
}

/// Manifold's mesh keeps the facets it was given; the solid's are found
/// from them again after each boolean.
pub(crate) fn retag(m: Mesh, _: &[u32]) -> Mesh {
    m
}

/// Triangles of loops in the plane, by Manifold's triangulator.
pub(crate) fn triangulate_loops(loops: &[Vec<[f64; 2]>]) -> Option<Vec<[u32; 3]>> {
    opensolve_manifold::triangulate(loops).ok()
}
