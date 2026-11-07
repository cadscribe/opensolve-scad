//! The booleans by Manifold, the C++ library OpenSCAD uses (see
//! `opensolve-manifold`). Each triangle carries its facet as Manifold's face
//! number, which every piece of it keeps.

use opensolve_manifold::Error as Refused;

use std::collections::HashMap;

use crate::{Affine, Error, Facets, Fx, Op, Solid, Stop, nearest_facet};

pub(crate) type Mesh = opensolve_manifold::Solid;

fn refused(e: Refused) -> Error {
    Error::Boolean(e.to_string())
}

/// Manifold's mesh of a solid. Its corners are taken as they are: parts
/// that touch along an edge with corners of their own are two parts. Parts
/// that share the corners of such an edge, which Manifold refuses, are
/// given corners of their own (and moved apart a hair).
pub(crate) fn make(s: &Solid) -> Result<Mesh, Error> {
    let of = |s: &Solid| {
        let points: Vec<f64> = s.points.iter().flatten().copied().collect();
        let tris: Vec<u32> = s.tris.iter().flatten().copied().collect();
        let faces: Vec<u64> = s.facets.iter().map(|&f| u64::from(f)).collect();
        Mesh::new(&points, &tris, &faces)
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

/// The solid moved by Manifold: the map composed with those before it and
/// applied to the corners of the meshes it was made of at once.
pub(crate) fn transformed(m: &Mesh, map: &[[f64; 4]; 3]) -> Option<Mesh> {
    m.transformed(map).ok()
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

/// Each group joined, in rounds of pairs, the small ones first (as
/// Manifold's own batch does it), the pairs of all groups in a round on
/// threads of their own; the groups' results, those not empty.
pub(crate) fn union(groups: &[Vec<&Mesh>]) -> Result<Vec<Mesh>, Error> {
    let mut levels: Vec<Vec<Item<'_>>> = groups
        .iter()
        .map(|g| g.iter().map(|m| Item::Given(m)).collect())
        .collect();
    rounds(&mut levels, Op::Add)?;
    Ok(levels
        .into_iter()
        .flatten()
        .map(|m| match m {
            Item::Given(m) => m.clone(),
            Item::Made(m) => m,
        })
        .filter(|m| !m.is_empty())
        .collect())
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
///
/// Asked to stop ([`crate::stopping`]), it ends between rounds and leaves
/// out the pairs of a round not begun: [`Error::Stopped`].
fn rounds(levels: &mut [Vec<Item<'_>>], op: Op) -> Result<(), Error> {
    let stop = Stop::current();
    let halted = || stop.as_ref().is_some_and(Stop::is_stopped);
    loop {
        if halted() {
            return Err(Error::Stopped);
        }
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
            if halted() {
                return Err(Error::Stopped);
            }
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

/// The corners, triangles and each triangle's facet of a result. A
/// triangle's number is the facet it had in the mesh it comes from; where
/// that mesh was moved since ([`transformed`]), the facet moved as well,
/// once for each facet and run.
pub(crate) fn read(m: &Mesh, facets: &mut Facets) -> (Vec<[f64; 3]>, Vec<[u32; 3]>, Vec<u32>) {
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
    const SAME: [[f64; 4]; 3] = [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
    ];
    // Each run's map, unless it is none; the run of each triangle.
    let maps: Vec<Option<Affine>> = (0..mesh.run_start.len().saturating_sub(1))
        .map(|r| {
            mesh.run_transform
                .get(r)
                .filter(|t| **t != SAME)
                .and_then(Affine::new)
        })
        .collect();
    let mut run_of = vec![usize::MAX; tris.len()];
    for (r, w) in mesh.run_start.windows(2).enumerate() {
        let (lo, hi) = (w[0] as usize, (w[1] as usize).min(tris.len()));
        run_of[lo.min(hi)..hi].fill(r);
    }
    let mut moved: HashMap<(u32, usize), u32, Fx> = HashMap::default();
    let known = facets.len();
    let found = mesh
        .faces
        .iter()
        .zip(&tris)
        .zip(run_of)
        .map(|((&f, t), r)| match u32::try_from(f) {
            Ok(f) if (f as usize) < known && facets.contains(f) => {
                match maps.get(r).and_then(Option::as_ref) {
                    None => f,
                    Some(a) => *moved.entry((f, r)).or_insert_with(|| a.facet(facets, f)),
                }
            }
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
