//! Solids as closed meshes whose triangles remember the face they came from
//! (ADR 0003), booleans on them, and the check and STL output for printing.
//!
//! Each planar piece of a solid is a [`Facet`]. A facet belongs to a [`FaceId`]:
//! a side of an extrusion that follows a circle is many facets and one face.
//! Booleans carry the facet index through as a value per triangle corner.

use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

mod apart;
// The booleans: Manifold's C++ where it builds, boolmesh in the browser.
#[cfg(target_arch = "wasm32")]
mod by_boolmesh;
#[cfg(not(target_arch = "wasm32"))]
mod by_manifold;
#[cfg(target_arch = "wasm32")]
use by_boolmesh as engine;
#[cfg(not(target_arch = "wasm32"))]
use by_manifold as engine;
mod export;
mod raster;
mod revolve;
mod stl;
mod triangulate;
mod zip;

pub use apart::meeting;
pub use export::{
    PrintObject, Rgba, encode, step, three_mf, three_mf_assembly, three_mf_objects,
    three_mf_painted, three_mf_print,
};
pub use raster::{Shot, render};
pub use revolve::{revolve, revolve_radius};
pub use stl::read_stl;

pub use triangulate::triangulate;

/// Triangles of loops in the plane: outlines counter-clockwise, holes
/// clockwise, any of them nested; corners numbered through all loops in
/// order. Manifold's triangulator (the sharpest ears first, robust on
/// touching and nearly straight corners); `None` where there is none (in
/// the browser) or it fails.
pub fn triangulate_loops(loops: &[Vec<[f64; 2]>]) -> Option<Vec<[u32; 3]>> {
    engine::triangulate_loops(loops)
}

/// The stable name of a face, chosen by the caller.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FaceId(pub u64);

/// A plane piece of a face.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Facet {
    pub face: FaceId,
    pub normal: [f64; 3],
    pub offset: f64,
}

/// The facets of everything built so far; solids index into it.
#[derive(Clone, Debug, Default)]
pub struct Facets {
    all: Vec<Facet>,
}

impl Facets {
    /// A new facet of `face`: the plane with `normal` through `on`.
    pub fn add(&mut self, face: FaceId, normal: [f64; 3], on: [f64; 3]) -> u32 {
        let offset = dot(normal, on);
        self.all.push(Facet {
            face,
            normal,
            offset,
        });
        self.all.len() as u32 - 1
    }

    pub fn get(&self, facet: u32) -> Facet {
        self.all[facet as usize]
    }

    pub fn len(&self) -> usize {
        self.all.len()
    }

    pub fn is_empty(&self) -> bool {
        self.all.is_empty()
    }

    /// Forgets the facets from `len` on, which no solid may still use.
    pub fn truncate(&mut self, len: usize) {
        self.all.truncate(len);
    }

    fn fits(&self, facet: u32, tri: [[f64; 3]; 3]) -> bool {
        let f = self.get(facet);
        tri.iter()
            .all(|p| (dot(f.normal, *p) - f.offset).abs() < 1e-6)
    }
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// The hash of rustc's own tables (FxHash), for maps keyed by numbers: far
/// quicker than the standard one, which guards against keys chosen to
/// collide.
#[derive(Clone, Copy, Debug, Default)]
struct Fx;

impl std::hash::BuildHasher for Fx {
    type Hasher = FxHasher;
    fn build_hasher(&self) -> FxHasher {
        FxHasher(0)
    }
}

#[derive(Debug)]
struct FxHasher(u64);

impl FxHasher {
    fn add(&mut self, x: u64) {
        self.0 = (self.0.rotate_left(5) ^ x).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95);
    }
}

impl std::hash::Hasher for FxHasher {
    fn write(&mut self, bytes: &[u8]) {
        for chunk in bytes.chunks(8) {
            let mut b = [0u8; 8];
            b[..chunk.len()].copy_from_slice(chunk);
            self.add(u64::from_le_bytes(b));
        }
    }

    fn write_u32(&mut self, i: u32) {
        self.add(u64::from(i));
    }

    fn write_u64(&mut self, i: u64) {
        self.add(i);
    }

    fn write_i64(&mut self, i: i64) {
        self.add(i as u64);
    }

    fn write_usize(&mut self, i: usize) {
        self.add(i as u64);
    }

    fn finish(&self) -> u64 {
        self.0
    }
}

/// A closed mesh; `facets[i]` names triangle `i`.
#[derive(Clone, Default)]
pub struct Solid {
    pub points: Vec<[f64; 3]>,
    pub tris: Vec<[u32; 3]>,
    pub facets: Vec<u32>,
    /// The boolean library's own mesh of the solid, kept: a boolean's
    /// result goes on to the next as the library made it.
    mesh: Option<Arc<engine::Mesh>>,
}

impl std::fmt::Debug for Solid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Solid")
            .field("points", &self.points.len())
            .field("tris", &self.tris.len())
            .finish_non_exhaustive()
    }
}

impl PartialEq for Solid {
    fn eq(&self, o: &Solid) -> bool {
        self.points == o.points && self.tris == o.tris && self.facets == o.facets
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Add,
    Subtract,
    Intersect,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// The profile has fewer than three distinct corners or no area.
    BadProfile,
    /// The mesh library refused, with its message.
    Boolean(String),
    /// A revolve's axis is not in the profile's plane.
    AxisOffPlane,
    /// A revolve's profile lies on both sides of its axis.
    CrossesAxis,
    /// The work was asked to stop (see [`Stop`]).
    Stopped,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::BadProfile => f.write_str("the profile has no area"),
            Error::Boolean(m) => write!(f, "boolean failed: {m}"),
            Error::AxisOffPlane => f.write_str("the axis is not in the sketch's plane"),
            Error::CrossesAxis => f.write_str("the profile crosses the axis"),
            Error::Stopped => f.write_str("stopped"),
        }
    }
}

impl std::error::Error for Error {}

/// Asks long work to stop: a flag that every clone shares, cheap to clone
/// and to look at. Set once, it stays set. The booleans of many solids
/// ([`Solid::batch`], [`Solid::booleans`]) look at the one of their thread
/// ([`stopping`]) between their rounds of pairs and before each pair, and
/// end with [`Error::Stopped`]; a single pair, in the mesh library, runs to
/// its end.
#[derive(Clone, Debug, Default)]
pub struct Stop(Arc<std::sync::atomic::AtomicBool>);

impl Stop {
    pub fn new() -> Stop {
        Stop::default()
    }

    /// Asks the work this is given to to stop.
    pub fn stop(&self) {
        self.0.store(true, std::sync::atomic::Ordering::Relaxed);
    }

    /// Whether it was asked to stop.
    pub fn is_stopped(&self) -> bool {
        self.0.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// The one of this thread, set by [`stopping`].
    fn current() -> Option<Stop> {
        STOP.with(|s| s.borrow().clone())
    }
}

thread_local! {
    /// What the work on this thread looks at (see [`stopping`]).
    static STOP: std::cell::RefCell<Option<Stop>> = const { std::cell::RefCell::new(None) };
}

/// Runs `f` with `stop` as what the kernel's long work on this thread looks
/// at; the one before is back after.
pub fn stopping<T>(stop: &Stop, f: impl FnOnce() -> T) -> T {
    struct Back(Option<Stop>);
    impl Drop for Back {
        fn drop(&mut self) {
            let before = self.0.take();
            STOP.with(|s| *s.borrow_mut() = before);
        }
    }
    let _back = Back(STOP.with(|s| s.borrow_mut().replace(stop.clone())));
    f()
}

/// Whether the work on this thread was asked to stop (see [`stopping`]).
pub fn stopped() -> bool {
    STOP.with(|s| s.borrow().as_ref().is_some_and(Stop::is_stopped))
}

/// One outline of a profile, counter-clockwise, with the face of each edge.
#[derive(Clone, Debug, PartialEq)]
pub struct Outline {
    pub points: Vec<[f64; 2]>,
    pub faces: Vec<FaceId>,
}

/// The prism over an outline from `z0` to `z1`, in the XY frame.
pub fn prism(
    facets: &mut Facets,
    outline: &Outline,
    z0: f64,
    z1: f64,
    bottom: FaceId,
    top: FaceId,
) -> Result<Solid, Error> {
    let n = outline.points.len();
    let caps = triangulate(&outline.points).ok_or(Error::BadProfile)?;
    if n != outline.faces.len() || z1 <= z0 {
        return Err(Error::BadProfile);
    }
    let mut s = Solid::default();
    for z in [z0, z1] {
        s.points
            .extend(outline.points.iter().map(|p| [p[0], p[1], z]));
    }
    let up = n as u32;
    let bottom_facet = facets.add(bottom, [0.0, 0.0, -1.0], [0.0, 0.0, z0]);
    let top_facet = facets.add(top, [0.0, 0.0, 1.0], [0.0, 0.0, z1]);
    for t in caps {
        s.push([t[0], t[2], t[1]], bottom_facet);
        s.push([t[0] + up, t[1] + up, t[2] + up], top_facet);
    }
    for i in 0..n {
        let j = (i + 1) % n;
        let (a, b) = (outline.points[i], outline.points[j]);
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let l = dx.hypot(dy);
        let f = facets.add(outline.faces[i], [dy / l, -dx / l, 0.0], [a[0], a[1], z0]);
        let (i, j) = (i as u32, j as u32);
        s.push([i, j, j + up], f);
        s.push([i, j + up, i + up], f);
    }
    Ok(s)
}

/// Where a 2D outline lies in the world.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub origin: [f64; 3],
    pub u: [f64; 3],
    pub v: [f64; 3],
}

/// World positions are kept on this grid, in millimetres. Values that are
/// meant to be equal can differ in their last bits after solving and rotating;
/// the booleans take that literally and leave a sheet of no thickness.
const GRID: f64 = 1e-7;

fn snap(x: f64) -> f64 {
    (x / GRID).round() * GRID
}

impl Frame {
    fn at(&self, p: [f64; 2], along: [f64; 3]) -> [f64; 3] {
        std::array::from_fn(|i| {
            snap(self.origin[i] + p[0] * self.u[i] + p[1] * self.v[i] + along[i])
        })
    }
}

fn normalized(n: [f64; 3]) -> [f64; 3] {
    let l = dot(n, n).sqrt().max(1e-300);
    n.map(|x| x / l)
}

/// The prism swept by an outline from `from` to `to`, offsets from its frame.
pub fn extrude(
    facets: &mut Facets,
    outline: &Outline,
    frame: &Frame,
    from: [f64; 3],
    to: [f64; 3],
    bottom: FaceId,
    top: FaceId,
) -> Result<Solid, Error> {
    let n = outline.points.len();
    if n != outline.faces.len() {
        return Err(Error::BadProfile);
    }
    let caps = triangulate(&outline.points).ok_or(Error::BadProfile)?;
    let sweep = sub(to, from);
    // Seen from `to`, a counter-clockwise outline must stay counter-clockwise.
    let normal = cross(frame.u, frame.v);
    let flip = dot(normal, sweep) < 0.0;
    if dot(normal, sweep).abs() < 1e-12 {
        return Err(Error::BadProfile);
    }
    let mut s = Solid::default();
    for along in [from, to] {
        s.points
            .extend(outline.points.iter().map(|p| frame.at(*p, along)));
    }
    let up = n as u32;
    let oriented = |t: [u32; 3]| if flip { [t[0], t[2], t[1]] } else { t };
    let plain = normalized(normal);
    let (down_n, up_n) = if flip {
        (plain, plain.map(|x| -x))
    } else {
        (plain.map(|x| -x), plain)
    };
    let bottom_facet = facets.add(bottom, down_n, s.points[0]);
    let top_facet = facets.add(top, up_n, s.points[n]);
    for t in caps {
        s.push(oriented([t[0], t[2], t[1]]), bottom_facet);
        s.push(oriented([t[0] + up, t[1] + up, t[2] + up]), top_facet);
    }
    for i in 0..n {
        let j = (i + 1) % n;
        let (a, b, c) = (i as u32, j as u32, j as u32 + up);
        let edge = sub(s.points[j], s.points[i]);
        let sign = if flip { -1.0 } else { 1.0 };
        let side = normalized(cross(edge, sweep)).map(|x| x * sign);
        let f = facets.add(outline.faces[i], side, s.points[i]);
        s.push(oriented([a, b, c]), f);
        s.push(oriented([a, c, a + up]), f);
    }
    Ok(s)
}

/// An affine map (three rows of a 4×4 matrix) that does not flatten, and
/// what moves planes by it.
pub(crate) struct Affine {
    m: [[f64; 4]; 3],
    /// Normals go by the inverse transpose: the cofactors over the
    /// determinant.
    cof: [[f64; 3]; 3],
    det: f64,
}

impl Affine {
    pub(crate) fn new(m: &[[f64; 4]; 3]) -> Option<Affine> {
        let row = |i: usize| [m[i][0], m[i][1], m[i][2]];
        let det = dot(row(0), cross(row(1), row(2)));
        if !det.is_finite() || det.abs() < 1e-18 {
            return None;
        }
        let cof = [
            cross(row(1), row(2)),
            cross(row(2), row(0)),
            cross(row(0), row(1)),
        ];
        Some(Affine { m: *m, cof, det })
    }

    fn at(&self, p: [f64; 3]) -> [f64; 3] {
        let m = &self.m;
        std::array::from_fn(|i| m[i][0] * p[0] + m[i][1] * p[1] + m[i][2] * p[2] + m[i][3])
    }

    /// A new facet: facet `f` moved, of the same face.
    pub(crate) fn facet(&self, facets: &mut Facets, f: u32) -> u32 {
        let x = facets.get(f);
        let n: [f64; 3] = std::array::from_fn(|i| dot(self.cof[i], x.normal) / self.det);
        let on = self.at(x.normal.map(|k| k * x.offset));
        facets.add(x.face, normalized(n), on)
    }
}

/// An edge where two facets meet at an angle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Crease {
    pub a: [f64; 3],
    pub b: [f64; 3],
    /// The facet whose triangle runs from `a` to `b`, and the one beside it.
    pub left: u32,
    pub right: u32,
}

impl Solid {
    fn push(&mut self, t: [u32; 3], facet: u32) {
        self.tris.push(t);
        self.facets.push(facet);
    }

    pub fn is_empty(&self) -> bool {
        self.tris.is_empty()
    }

    /// A copy moved by `delta`, on facets of its own named by `rename`.
    pub fn translated(
        &self,
        facets: &mut Facets,
        delta: [f64; 3],
        rename: impl Fn(FaceId) -> FaceId,
    ) -> Solid {
        let mut map: HashMap<u32, u32, Fx> = HashMap::default();
        let copied: Vec<u32> = self
            .facets
            .iter()
            .map(|&f| {
                *map.entry(f).or_insert_with(|| {
                    let old = facets.get(f);
                    let shifted = Facet {
                        face: rename(old.face),
                        normal: old.normal,
                        offset: old.offset + dot(old.normal, delta),
                    };
                    facets.all.push(shifted);
                    facets.all.len() as u32 - 1
                })
            })
            .collect();
        Solid {
            points: self
                .points
                .iter()
                .map(|p| std::array::from_fn(|i| snap(p[i] + delta[i])))
                .collect(),
            tris: self.tris.clone(),
            facets: copied,
            mesh: None,
        }
    }

    /// A copy turned by `angle` radians about the axis through `origin` along
    /// `axis`, on facets of its own named by `rename`.
    pub fn rotated(
        &self,
        facets: &mut Facets,
        origin: [f64; 3],
        axis: [f64; 3],
        angle: f64,
        rename: impl Fn(FaceId) -> FaceId,
    ) -> Solid {
        let k = {
            let l = dot(axis, axis).sqrt().max(1e-300);
            axis.map(|x| x / l)
        };
        let (s, c) = angle.sin_cos();
        // Rodrigues: v cos + (k × v) sin + k (k · v)(1 − cos).
        let turn = |v: [f64; 3]| -> [f64; 3] {
            let kv = cross(k, v);
            let d = dot(k, v) * (1.0 - c);
            std::array::from_fn(|i| v[i] * c + kv[i] * s + k[i] * d)
        };
        let mut map: HashMap<u32, u32, Fx> = HashMap::default();
        let points: Vec<[f64; 3]> = self
            .points
            .iter()
            .map(|p| {
                let q = turn(sub(*p, origin));
                std::array::from_fn(|i| snap(q[i] + origin[i]))
            })
            .collect();
        let copied: Vec<u32> = self
            .facets
            .iter()
            .map(|&f| {
                *map.entry(f).or_insert_with(|| {
                    let old = facets.get(f);
                    let normal = turn(old.normal);
                    let on = turn(sub(old.normal.map(|x| x * old.offset), origin));
                    let on: [f64; 3] = std::array::from_fn(|i| on[i] + origin[i]);
                    facets.add(rename(old.face), normal, on)
                })
            })
            .collect();
        Solid {
            points,
            tris: self.tris.clone(),
            facets: copied,
            mesh: None,
        }
    }

    /// Only the triangles whose facet `keep` keeps: to show, not to cut.
    pub fn only(&self, keep: impl Fn(u32) -> bool) -> Solid {
        let (tris, facets) = self
            .tris
            .iter()
            .zip(&self.facets)
            .filter(|&(_, &f)| keep(f))
            .map(|(t, &f)| (*t, f))
            .unzip();
        Solid {
            points: self.points.clone(),
            tris,
            facets,
            mesh: None,
        }
    }

    /// The bounding box: lowest and highest corner.
    pub fn bounds(&self) -> Option<([f64; 3], [f64; 3])> {
        let first = *self.points.first()?;
        Some(self.points.iter().fold((first, first), |(lo, hi), p| {
            (
                std::array::from_fn(|i| lo[i].min(p[i])),
                std::array::from_fn(|i| hi[i].max(p[i])),
            )
        }))
    }

    /// A copy mapped by the affine map `m` (three rows of a 4×4 matrix), a
    /// mirror turned round, on facets of its own: each facet's plane mapped,
    /// so that the triangles of a plane stay one facet (and the boolean
    /// library joins them up again). The corners stay on the kernel's grid:
    /// corners meant to meet (a roof on a wall cut to its slope) then meet,
    /// where the last bits of a turn would part them and the booleans take
    /// that literally. `None` if the map flattens it.
    pub fn mapped(&self, facets: &mut Facets, m: &[[f64; 4]; 3]) -> Option<Solid> {
        let a = Affine::new(m)?;
        let mut moved: HashMap<u32, u32, Fx> = HashMap::default();
        let facets_of = self
            .facets
            .iter()
            .map(|&f| *moved.entry(f).or_insert_with(|| a.facet(facets, f)))
            .collect();
        let flip = a.det < 0.0;
        let tris = self
            .tris
            .iter()
            .map(|t| if flip { [t[0], t[2], t[1]] } else { *t })
            .collect();
        // A boolean's result the library moves itself, as OpenSCAD has it
        // moved: its corners where the map puts them, the maps before
        // composed with this one. Snapped, the corners a boolean computed on
        // a slanted face bend it (and the library then cuts each of its
        // triangles on its own); read in again, a result with parts that
        // nearly touch can lose one of them in the next boolean.
        if let Some(mesh) = self.mesh.as_ref().and_then(|x| engine::transformed(x, m)) {
            return Some(Solid {
                points: self.points.iter().map(|p| a.at(*p)).collect(),
                tris,
                facets: facets_of,
                mesh: Some(Arc::new(mesh)),
            });
        }
        Some(Solid {
            points: self.points.iter().map(|p| a.at(*p).map(snap)).collect(),
            tris,
            facets: facets_of,
            mesh: None,
        })
    }

    /// A solid from its triangles, each with the face it belongs to; every
    /// triangle gets a facet of its own plane, its normal by its winding.
    /// The caller sees to it that the mesh is closed.
    pub fn from_mesh(
        facets: &mut Facets,
        points: Vec<[f64; 3]>,
        tris: Vec<[u32; 3]>,
        faces: &[FaceId],
    ) -> Solid {
        let tags = tris
            .iter()
            .zip(faces)
            .map(|(t, &face)| {
                let [a, b, c] = t.map(|i| points[i as usize]);
                let n = cross(sub(b, a), sub(c, a));
                let l = dot(n, n).sqrt().max(1e-300);
                facets.add(face, n.map(|x| x / l), a)
            })
            .collect();
        Solid {
            points: points.into_iter().map(|p| p.map(snap)).collect(),
            tris,
            facets: tags,
            mesh: None,
        }
    }

    /// The solid with its facets, read from `from`, added to `into` anew:
    /// a solid kept from another build, in this one's facets.
    pub fn rehome(&self, from: &Facets, into: &mut Facets) -> Solid {
        let mut moved: HashMap<u32, u32, Fx> = HashMap::default();
        let facets = self
            .facets
            .iter()
            .map(|&f| {
                *moved.entry(f).or_insert_with(|| {
                    into.all.push(from.get(f));
                    into.all.len() as u32 - 1
                })
            })
            .collect();
        Solid {
            points: self.points.clone(),
            tris: self.tris.clone(),
            facets,
            mesh: None,
        }
    }

    /// Several solids that do not touch, as one. Cutting a batch at once is
    /// much faster than one after the other.
    pub fn merged(parts: &[Solid]) -> Solid {
        let mut s = Solid::default();
        for p in parts {
            let base = s.points.len() as u32;
            s.points.extend_from_slice(&p.points);
            s.tris.extend(p.tris.iter().map(|t| t.map(|i| i + base)));
            s.facets.extend_from_slice(&p.facets);
        }
        s
    }

    /// The solid ready for booleans: the library's mesh of it made now and
    /// kept, so that many booleans with it make it once.
    pub fn prepared(&self) -> Result<Solid, Error> {
        Ok(Solid {
            mesh: Some(self.manifold()?),
            ..self.clone()
        })
    }

    fn manifold(&self) -> Result<Arc<engine::Mesh>, Error> {
        if let Some(m) = &self.mesh {
            return Ok(m.clone());
        }
        engine::make(self).map(Arc::new)
    }

    /// The solid with the parts that meet along an edge moved apart, each
    /// shrunk towards its middle by a hair (a ten-millionth of a micrometre
    /// for a part of 10 mm); `None` if no edge has more than two
    /// triangles once corners at one place are one.
    pub(crate) fn parted(&self) -> Option<Solid> {
        let n = self.points.len();
        let mut weld: HashMap<[u64; 3], u32> = HashMap::with_capacity(n);
        let at: Vec<u32> = self
            .points
            .iter()
            .map(|p| {
                let k = weld.len() as u32;
                *weld.entry(p.map(f64::to_bits)).or_insert(k)
            })
            .collect();
        let key = |a: u32, b: u32| if a < b { (a, b) } else { (b, a) };
        let mut uses: HashMap<(u32, u32), u32> = HashMap::with_capacity(self.tris.len() * 3 / 2);
        for t in &self.tris {
            for k in 0..3 {
                *uses
                    .entry(key(at[t[k] as usize], at[t[(k + 1) % 3] as usize]))
                    .or_default() += 1;
            }
        }
        if uses.values().all(|&u| u <= 2) {
            return None;
        }
        // The parts: triangles joined across an edge of exactly two, by
        // the solid's own corners.
        let mut own: HashMap<(u32, u32), u32> = HashMap::with_capacity(self.tris.len() * 3 / 2);
        for t in &self.tris {
            for k in 0..3 {
                *own.entry(key(t[k], t[(k + 1) % 3])).or_default() += 1;
            }
        }
        let mut parent: Vec<usize> = (0..self.tris.len()).collect();
        fn root(p: &mut [usize], mut i: usize) -> usize {
            while p[i] != i {
                p[i] = p[p[i]];
                i = p[i];
            }
            i
        }
        let mut first: HashMap<(u32, u32), usize> = HashMap::new();
        for (i, t) in self.tris.iter().enumerate() {
            for k in 0..3 {
                let e = key(t[k], t[(k + 1) % 3]);
                let welded = key(at[t[k] as usize], at[t[(k + 1) % 3] as usize]);
                if own[&e] != 2 || uses[&welded] != 2 {
                    continue;
                }
                match first.get(&e) {
                    Some(&j) => {
                        let (a, b) = (root(&mut parent, i), root(&mut parent, j));
                        parent[a] = b;
                    }
                    None => {
                        first.insert(e, i);
                    }
                }
            }
        }
        // A corner goes with the part of its first triangle; one shared by
        // parts is copied.
        let mut out = Solid {
            points: Vec::with_capacity(n),
            tris: Vec::with_capacity(self.tris.len()),
            facets: self.facets.clone(),
            mesh: None,
        };
        let mut copy: HashMap<(u32, usize), u32> = HashMap::with_capacity(n);
        let mut part_of_point: Vec<usize> = Vec::with_capacity(n);
        for i in 0..self.tris.len() {
            let part = root(&mut parent, i);
            let t = self.tris[i].map(|v| {
                *copy.entry((v, part)).or_insert_with(|| {
                    out.points.push(self.points[v as usize]);
                    part_of_point.push(part);
                    out.points.len() as u32 - 1
                })
            });
            out.tris.push(t);
        }
        let mut boxes: HashMap<usize, ([f64; 3], [f64; 3])> = HashMap::new();
        for (p, &part) in out.points.iter().zip(&part_of_point) {
            let b = boxes.entry(part).or_insert((*p, *p));
            for (k, x) in p.iter().enumerate() {
                b.0[k] = b.0[k].min(*x);
                b.1[k] = b.1[k].max(*x);
            }
        }
        for (p, part) in out.points.iter_mut().zip(&part_of_point) {
            let (lo, hi) = boxes[part];
            for k in 0..3 {
                let mid = (lo[k] + hi[k]) / 2.0;
                p[k] = mid + (p[k] - mid) * (1.0 - 1e-10);
            }
        }
        Some(out)
    }

    /// A copy scaled by `1 + k` about the middle of its box, on facets
    /// moved as well: what a boolean the library panics on is tried with
    /// again.
    fn nudged(&self, k: f64, facets: &mut Facets) -> Solid {
        let Some((lo, hi)) = self.bounds() else {
            return self.clone();
        };
        let mid: [f64; 3] = std::array::from_fn(|i| (lo[i] + hi[i]) / 2.0);
        let at = |p: [f64; 3]| -> [f64; 3] {
            std::array::from_fn(|i| mid[i] + (p[i] - mid[i]) * (1.0 + k))
        };
        let mut moved: HashMap<u32, u32, Fx> = HashMap::default();
        let facets_of = self
            .facets
            .iter()
            .map(|&f| {
                *moved.entry(f).or_insert_with(|| {
                    let x = facets.get(f);
                    let on = at(x.normal.map(|n| n * x.offset));
                    facets.add(x.face, x.normal, on)
                })
            })
            .collect();
        Solid {
            points: self.points.iter().map(|p| at(*p)).collect(),
            tris: self.tris.clone(),
            facets: facets_of,
            mesh: None,
        }
    }

    /// The boolean of two solids. What a cut leaves of the cutter faces the
    /// other way: such triangles get a facet turned round, of the same face.
    ///
    /// Where the library panics (on a result with an edge of more than two
    /// triangles, as where two cutters meet along an edge), the boolean is
    /// tried again with the other solid a millionth larger, then smaller,
    /// then this one larger: a sliver of that width is the difference.
    pub fn boolean(&self, other: &Solid, op: Op, facets: &mut Facets) -> Result<Solid, Error> {
        Solid::booleans(&[(self, other)], op, facets)
            .pop()
            .unwrap_or_else(|| Ok(Solid::default()))
    }

    /// The booleans of several pairs, as [`Solid::boolean`]: the library's
    /// work for each on a thread of its own where the machine has threads,
    /// what joins many shapes in a tree of pairs.
    pub fn booleans(
        pairs: &[(&Solid, &Solid)],
        op: Op,
        facets: &mut Facets,
    ) -> Vec<Result<Solid, Error>> {
        let mut out: Vec<Option<Result<Solid, Error>>> = pairs
            .iter()
            .map(|(a, b)| {
                (a.is_empty() || b.is_empty()).then(|| {
                    Ok(match op {
                        Op::Add if a.is_empty() => (*b).clone(),
                        Op::Add | Op::Subtract => (*a).clone(),
                        Op::Intersect => Solid::default(),
                    })
                })
            })
            .collect();
        let todo: Vec<usize> = (0..pairs.len()).filter(|&i| out[i].is_none()).collect();
        let stop = Stop::current();
        let halted = || stop.as_ref().is_some_and(Stop::is_stopped);
        let first = parallel(todo.len(), |k| {
            if halted() {
                return Err(Error::Stopped);
            }
            let (a, b) = pairs[todo[k]];
            run_boolean(a, b, op)
        });
        for (&i, mut result) in todo.iter().zip(first) {
            let (a, b) = pairs[i];
            for (which, k) in [(1, 1e-6), (1, -1e-6), (0, 1e-6)] {
                if result.is_ok() || halted() {
                    break;
                }
                result = if which == 1 {
                    run_boolean(a, &b.nudged(k, facets), op)
                } else {
                    run_boolean(&a.nudged(k, facets), b, op)
                };
            }
            out[i] = Some(match result {
                // Stopped: what was made is not read back.
                _ if halted() => Err(Error::Stopped),
                Err(e) => Err(e),
                Ok(Some(m)) => Ok(Solid::from_manifold(m, facets)),
                Ok(None) => Ok(Solid::default()),
            });
        }
        out.into_iter()
            .map(|r| r.unwrap_or_else(|| Ok(Solid::default())))
            .collect()
    }

    /// Several solids by `op` at once, as OpenSCAD does it: all joined, the
    /// first with the others cut away, or what all hold. The library sorts
    /// out which meet (those apart are only put together) and joins the
    /// small ones first. `None` where the library has no such batch (in the
    /// browser): the caller then goes in pairs ([`Solid::booleans`]).
    pub fn batch(list: &[&Solid], op: Op, facets: &mut Facets) -> Option<Result<Solid, Error>> {
        let (first, rest) = list.split_first()?;
        let mut keep: Vec<&Solid> = Vec::with_capacity(list.len());
        match op {
            Op::Add => keep.extend(list.iter().filter(|s| !s.is_empty())),
            Op::Subtract if first.is_empty() => return Some(Ok(Solid::default())),
            Op::Subtract => {
                keep.push(first);
                keep.extend(rest.iter().filter(|s| !s.is_empty()));
            }
            Op::Intersect if list.iter().any(|s| s.is_empty()) => {
                return Some(Ok(Solid::default()));
            }
            Op::Intersect => keep.extend(list),
        }
        match keep.len() {
            0 => return Some(Ok(Solid::default())),
            1 => return Some(Ok(keep[0].clone())),
            _ => {}
        }
        if !engine::BATCHES {
            return None;
        }
        let made = parallel(keep.len(), |i| {
            catch_unwind(AssertUnwindSafe(|| keep[i].manifold()))
                .unwrap_or_else(|_| Err(Error::Boolean("the mesh library panicked".into())))
        });
        let meshes = match made.into_iter().collect::<Result<Vec<_>, _>>() {
            Ok(m) => m,
            Err(e) => return Some(Err(e)),
        };
        if stopped() {
            return Some(Err(Error::Stopped));
        }
        let refs: Vec<&engine::Mesh> = meshes.iter().map(|m| &**m).collect();
        // Those that meet joined, each such group put together with the
        // others as it is: a boolean of solids apart is wasted work.
        let groups = |from: usize| -> Vec<Vec<&engine::Mesh>> {
            let part = &keep[from..];
            let mut parent: Vec<usize> = (0..part.len()).collect();
            fn root(p: &mut [usize], mut i: usize) -> usize {
                while p[i] != i {
                    p[i] = p[p[i]];
                    i = p[i];
                }
                i
            }
            for (a, b) in meeting(part) {
                let (a, b) = (root(&mut parent, a), root(&mut parent, b));
                parent[a.max(b)] = a.min(b);
            }
            let mut out: Vec<Vec<&engine::Mesh>> = Vec::new();
            let mut slot: HashMap<usize, usize, Fx> = HashMap::default();
            for i in 0..part.len() {
                let r = root(&mut parent, i);
                let k = *slot.entry(r).or_insert_with(|| {
                    out.push(Vec::new());
                    out.len() - 1
                });
                out[k].push(refs[from + i]);
            }
            out
        };
        let caught = |e| Error::Boolean(format!("the mesh library panicked ({e:?})"));
        let joined = |from: usize| {
            catch_unwind(AssertUnwindSafe(|| engine::union(&groups(from))))
                .unwrap_or_else(|e| Err(caught(e)))
        };
        // The groups' results, apart from each other, as one solid. They
        // are put together here, not in the library: a mesh of them read
        // in again came out with parts of a face moved.
        let together = |parts: Vec<engine::Mesh>, facets: &mut Facets| {
            if stopped() {
                return Err(Error::Stopped);
            }
            let solids: Vec<Solid> = parts
                .into_iter()
                .map(|m| Solid::from_manifold(m, facets))
                .collect();
            Ok(match solids.len() {
                1 => solids.into_iter().next().unwrap_or_default(),
                _ => Solid::merged(&solids),
            })
        };
        // A result read back as a solid, unless the work was stopped.
        let read = |m: Option<engine::Mesh>, facets: &mut Facets| {
            if stopped() {
                return Err(Error::Stopped);
            }
            Ok(m.map_or_else(Solid::default, |m| Solid::from_manifold(m, facets)))
        };
        let done = match op {
            Op::Add => joined(0).and_then(|parts| together(parts, facets)),
            Op::Intersect => catch_unwind(AssertUnwindSafe(|| engine::intersection(&refs)))
                .unwrap_or_else(|e| Err(caught(e)))
                .and_then(|m| read(m, facets)),
            Op::Subtract => joined(1).and_then(|mut parts| {
                let cut = match parts.len() {
                    0 => return Ok((*keep[0]).clone()),
                    1 => Arc::new(parts.remove(0)),
                    _ => together(parts, facets)?.manifold()?,
                };
                if stopped() {
                    return Err(Error::Stopped);
                }
                catch_unwind(AssertUnwindSafe(|| engine::boolean(refs[0], &cut, op)))
                    .unwrap_or_else(|e| Err(caught(e)))
                    .and_then(|m| read(m, facets))
            }),
        };
        Some(done)
    }

    fn from_manifold(m: engine::Mesh, facets: &mut Facets) -> Solid {
        let (points, tris, found) = engine::read(&m, facets);
        let at = |t: [u32; 3]| t.map(|i| points[i as usize]);
        let mut turned: HashMap<u32, u32, Fx> = HashMap::default();
        // A splinter's winding is noise: it goes with the rest of its facet,
        // turned if that is (below), else as it is.
        let splinter: Vec<bool> = tris
            .iter()
            .map(|t| {
                let [p, q, r] = at(*t);
                let n = cross(sub(q, p), sub(r, p));
                let longest = [sub(q, p), sub(r, q), sub(p, r)]
                    .map(|e| dot(e, e))
                    .into_iter()
                    .fold(0.0, f64::max);
                dot(n, n) < 1e-12 * longest * longest
            })
            .collect();
        let mut facets_of: Vec<u32> = found
            .iter()
            .zip(&tris)
            .zip(&splinter)
            .map(|((&f, t), &splinter)| {
                let [p, q, r] = at(*t);
                let n = cross(sub(q, p), sub(r, p));
                let old = facets.get(f);
                if splinter || dot(n, old.normal) >= 0.0 {
                    return f;
                }
                *turned.entry(f).or_insert_with(|| {
                    let on = old.normal.map(|x| x * old.offset);
                    facets.add(old.face, old.normal.map(|x| -x), on)
                })
            })
            .collect();
        for (f, _) in facets_of.iter_mut().zip(&splinter).filter(|(_, s)| **s) {
            if let Some(&t) = turned.get(f) {
                *f = t;
            }
        }
        // One plane, one way round: one face, under the older name. A cut
        // that ends on a face (a fillet's end on a floor) must not rename
        // the floor.
        let plane = |f: u32| {
            let x = facets.get(f);
            let r = |v: f64| (v * 1e6).round() as i64;
            (x.normal.map(r), r(x.offset))
        };
        // Each facet once (many triangles share one), the oldest first: the
        // first of a plane is its oldest.
        let mut distinct = facets_of.clone();
        distinct.sort_unstable();
        distinct.dedup();
        let mut oldest: HashMap<([i64; 3], i64), u32, Fx> = HashMap::default();
        let mut renamed: HashMap<u32, u32, Fx> =
            HashMap::with_capacity_and_hasher(distinct.len(), Fx);
        for &f in &distinct {
            renamed.insert(f, *oldest.entry(plane(f)).or_insert(f));
        }
        let facets_of: Vec<u32> = facets_of.iter().map(|&f| renamed[&f]).collect();
        let m = engine::retag(m, &facets_of);
        Solid {
            points,
            tris,
            facets: facets_of,
            mesh: Some(Arc::new(m)),
        }
    }

    /// Triangles covered by a triangle of the opposite side in the same plane:
    /// a sheet of no thickness, closed and without volume.
    pub fn sheet_triangles(&self, facets: &Facets) -> usize {
        self.sheets(facets).len()
    }

    /// Fins: edges where two faces fold nearly back to back, wider than
    /// `wider` (how far the narrower of the two triangles reaches from the
    /// edge). Booleans meeting at a graze leave them; a printed part with no
    /// knife edge has none.
    pub fn fins(&self, facets: &Facets, wider: f64) -> Vec<([f64; 3], [f64; 3], f64)> {
        let to_line = |p: [f64; 3], a: [f64; 3], b: [f64; 3]| {
            let d = sub(b, a);
            let t = (dot(sub(p, a), d) / dot(d, d).max(1e-300)).clamp(0.0, 1.0);
            let q = sub(p, a);
            let r = [q[0] - d[0] * t, q[1] - d[1] * t, q[2] - d[2] * t];
            dot(r, r).sqrt()
        };
        let mut out = Vec::new();
        for c in self.creases(facets) {
            let (f, g) = (facets.get(c.left), facets.get(c.right));
            let long = dot(sub(c.b, c.a), sub(c.b, c.a)).sqrt();
            if dot(f.normal, g.normal) >= -0.7 || long <= 1e-4 {
                continue;
            }
            let at = |p: [f64; 3], q: [f64; 3]| (0..3).all(|i| (p[i] - q[i]).abs() < 1e-12);
            let on = |p: [f64; 3]| at(p, c.a) || at(p, c.b);
            let width = self
                .tris
                .iter()
                .map(|t| t.map(|i| self.points[i as usize]))
                .filter(|t| t.iter().filter(|p| on(**p)).count() == 2)
                .flat_map(|t| t.into_iter().filter(|p| !on(*p)))
                .map(|p| to_line(p, c.a, c.b))
                .fold(f64::MAX, f64::min);
            if width > wider && width < f64::MAX {
                out.push((c.a, c.b, width));
            }
        }
        out
    }

    /// Which triangles those are.
    pub fn sheets(&self, facets: &Facets) -> Vec<usize> {
        let key = |f: &Facet, sign: f64| {
            let round = |x: f64| (x * 1e5).round() as i64;
            (f.normal.map(|x| round(x * sign)), round(f.offset * sign))
        };
        let mut planes: HashMap<([i64; 3], i64), Vec<usize>> = HashMap::new();
        for (t, &i) in self.facets.iter().enumerate() {
            planes.entry(key(&facets.get(i), 1.0)).or_default().push(t);
        }
        let corners = |t: usize| self.tris[t].map(|i| self.points[i as usize]);
        (0..self.tris.len())
            .filter(|&t| {
                let f = facets.get(self.facets[t]);
                let [a, b, c] = corners(t);
                // A splinter of no area has no side to speak of.
                let n = cross(sub(b, a), sub(c, a));
                if dot(n, n) < 4e-18 {
                    return false;
                }
                let mid = std::array::from_fn(|i| (a[i] + b[i] + c[i]) / 3.0);
                planes
                    .get(&key(&f, -1.0))
                    .is_some_and(|list| list.iter().any(|&o| covers(corners(o), mid, f.normal)))
            })
            .collect()
    }

    /// Whether `p` lies inside: a ray's crossings, counted on a direction no
    /// model edge is likely to run along.
    pub fn contains(&self, p: [f64; 3]) -> bool {
        let dir = [0.577_215_664_9, 0.312_345_678_9, 0.759_134_183_3];
        let mut inside = false;
        for t in &self.tris {
            let [a, b, c] = t.map(|i| self.points[i as usize]);
            let (e, f) = (sub(b, a), sub(c, a));
            let q = cross(dir, f);
            let det = dot(e, q);
            if det.abs() < 1e-14 {
                continue;
            }
            let s = sub(p, a);
            let u = dot(s, q) / det;
            let r = cross(s, e);
            let v = dot(dir, r) / det;
            let d = dot(f, r) / det;
            if u >= 0.0 && v >= 0.0 && u + v < 1.0 && d > 0.0 {
                inside = !inside;
            }
        }
        inside
    }

    /// The edges where two facets meet that are not one plane: each once,
    /// with the facet on either side (`left` runs `a` to `b`).
    pub fn creases(&self, facets: &Facets) -> Vec<Crease> {
        let mut out = Vec::new();
        let mut seen: HashMap<(u32, u32), u32> = HashMap::new();
        for (t, &facet) in self.tris.iter().zip(&self.facets) {
            for k in 0..3 {
                let (a, b) = (t[k], t[(k + 1) % 3]);
                match seen.remove(&(b, a)) {
                    Some(other) => {
                        let (f, g) = (facets.get(facet), facets.get(other));
                        if dot(f.normal, g.normal) < 1.0 - 1e-9 {
                            out.push(Crease {
                                a: self.points[a as usize],
                                b: self.points[b as usize],
                                left: facet,
                                right: other,
                            });
                        }
                    }
                    None => {
                        seen.insert((a, b), facet);
                    }
                }
            }
        }
        out
    }

    /// A sphere of facets of one face, `n` around.
    pub fn sphere(
        facets: &mut Facets,
        center: [f64; 3],
        radius: f64,
        n: usize,
        face: FaceId,
    ) -> Solid {
        let (rings, around) = (n.max(4) / 2, n.max(4));
        let mut s = Solid::default();
        let at = |i: usize, j: usize| -> [f64; 3] {
            let th = std::f64::consts::PI * i as f64 / rings as f64;
            let ph = std::f64::consts::TAU * j as f64 / around as f64;
            [
                center[0] + radius * th.sin() * ph.cos(),
                center[1] + radius * th.sin() * ph.sin(),
                center[2] + radius * th.cos(),
            ]
        };
        s.points.push(at(0, 0));
        for i in 1..rings {
            for j in 0..around {
                s.points.push(at(i, j));
            }
        }
        s.points.push(at(rings, 0));
        let bottom = s.points.len() as u32 - 1;
        let idx = |i: usize, j: usize| (1 + (i - 1) * around + j % around) as u32;
        let mut tri = |s: &mut Solid, t: [u32; 3]| {
            let [a, b, c] = t.map(|k| s.points[k as usize]);
            let n = cross(sub(b, a), sub(c, a));
            let l = dot(n, n).sqrt().max(1e-300);
            let f = facets.add(face, n.map(|x| x / l), a);
            s.push(t, f);
        };
        for j in 0..around {
            tri(&mut s, [0, idx(1, j), idx(1, j + 1)]);
            tri(&mut s, [bottom, idx(rings - 1, j + 1), idx(rings - 1, j)]);
        }
        for i in 1..rings - 1 {
            for j in 0..around {
                let (a, b, c, d) = (idx(i, j), idx(i, j + 1), idx(i + 1, j), idx(i + 1, j + 1));
                tri(&mut s, [a, c, d]);
                tri(&mut s, [a, d, b]);
            }
        }
        s
    }

    /// `true` if every edge has exactly one partner running the other way.
    pub fn is_closed(&self) -> bool {
        // Each half-edge in the bucket of its lower corner, as its other
        // corner and which way it runs; an edge of one corner (both ends
        // the same) is its own partner.
        let corners = self
            .tris
            .iter()
            .flatten()
            .max()
            .map_or(0, |&m| m as usize + 1);
        let mut start = vec![0u32; corners + 1];
        let halves = || {
            self.tris
                .iter()
                .flat_map(|t| (0..3).map(move |k| (t[k], t[(k + 1) % 3])))
        };
        for (a, b) in halves() {
            start[a.min(b) as usize + 1] += 1;
        }
        for v in 1..start.len() {
            start[v] += start[v - 1];
        }
        let mut at = start.clone();
        let mut bucket = vec![0u64; 3 * self.tris.len()];
        for (a, b) in halves() {
            let lo = a.min(b) as usize;
            bucket[at[lo] as usize] = u64::from(a.max(b)) << 1 | u64::from(a > b);
            at[lo] += 1;
        }
        start.windows(2).enumerate().all(|(lo, w)| {
            let edges = &mut bucket[w[0] as usize..w[1] as usize];
            edges.sort_unstable();
            // Each edge once each way: in order, pairs of the other corner
            // running up then down, but the edge of one corner alone.
            let mut i = 0;
            while i < edges.len() {
                let hi = edges[i] >> 1;
                if hi == lo as u64 {
                    if edges.get(i + 1).is_some_and(|&e| e >> 1 == hi) {
                        return false;
                    }
                    i += 1;
                    continue;
                }
                let pair = edges.get(i + 1).is_some_and(|&e| e == edges[i] | 1);
                if edges[i] & 1 == 1 || !pair || edges.get(i + 2).is_some_and(|&e| e >> 1 == hi) {
                    return false;
                }
                i += 2;
            }
            true
        })
    }

    pub fn volume(&self) -> f64 {
        self.tris
            .iter()
            .map(|t| {
                let [a, b, c] = t.map(|i| self.points[i as usize]);
                dot(a, cross(b, c)) / 6.0
            })
            .sum()
    }

    /// Binary STL.
    pub fn to_stl(&self) -> Vec<u8> {
        let mut out = vec![0u8; 80];
        out.extend_from_slice(&(self.tris.len() as u32).to_le_bytes());
        for t in &self.tris {
            let [a, b, c] = t.map(|i| self.points[i as usize]);
            let n = cross(sub(b, a), sub(c, a));
            let l = dot(n, n).sqrt().max(1e-300);
            for v in [n.map(|x| x / l), a, b, c] {
                for x in v {
                    out.extend_from_slice(&(x as f32).to_le_bytes());
                }
            }
            out.extend_from_slice(&[0, 0]);
        }
        out
    }

    pub fn faces(&self, facets: &Facets) -> Vec<FaceId> {
        let mut f: Vec<FaceId> = self.facets.iter().map(|&i| facets.get(i).face).collect();
        f.sort_unstable();
        f.dedup();
        f
    }
}

/// Is `p`, in the triangle's plane, inside it? `normal` is the plane's, seen from
/// the side the triangle's winding runs against.
/// The library's boolean of two solids, `None` if nothing is left, its
/// panic caught: a library can panic on an index, and a failed feature must
/// not take the app with it.
fn run_boolean(a: &Solid, b: &Solid, op: Op) -> Result<Option<engine::Mesh>, Error> {
    catch_unwind(AssertUnwindSafe(|| {
        let (a, b) = (a.manifold()?, b.manifold()?);
        engine::boolean(&a, &b, op)
    }))
    .map_err(|_| Error::Boolean("the mesh library panicked".to_string()))?
}

/// How many threads the machine has (one in the browser).
pub(crate) fn threads() -> usize {
    std::thread::available_parallelism().map_or(1, std::num::NonZero::get)
}

/// `f` of `0..n`, on as many threads as the machine has (in the browser
/// one after the other).
#[cfg(not(target_arch = "wasm32"))]
pub fn parallel<T: Send>(n: usize, f: impl Fn(usize) -> T + Sync) -> Vec<T> {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let threads = std::thread::available_parallelism()
        .map_or(1, std::num::NonZero::get)
        .min(n);
    if threads <= 1 {
        return (0..n).map(f).collect();
    }
    let next = AtomicUsize::new(0);
    let done: Vec<Mutex<Option<T>>> = (0..n).map(|_| Mutex::new(None)).collect();
    let work = || {
        loop {
            let i = next.fetch_add(1, Ordering::Relaxed);
            if i >= n {
                break;
            }
            let r = f(i);
            if let Ok(mut d) = done[i].lock() {
                *d = Some(r);
            }
        }
    };
    std::thread::scope(|s| {
        for _ in 1..threads {
            // The library recurses: room as on the evaluation's thread.
            let _ = std::thread::Builder::new()
                .stack_size(64 << 20)
                .spawn_scoped(s, work);
        }
        work();
    });
    done.into_iter()
        .enumerate()
        .map(|(i, d)| d.into_inner().ok().flatten().unwrap_or_else(|| f(i)))
        .collect()
}

#[cfg(target_arch = "wasm32")]
pub fn parallel<T>(n: usize, f: impl Fn(usize) -> T) -> Vec<T> {
    (0..n).map(f).collect()
}

fn covers(t: [[f64; 3]; 3], p: [f64; 3], normal: [f64; 3]) -> bool {
    (0..3).all(|k| {
        let (a, b) = (t[k], t[(k + 1) % 3]);
        dot(cross(sub(b, a), sub(p, a)), normal) <= 1e-9
    })
}

/// The last resort for a triangle no tag reached: the plane it lies in best.
fn nearest_facet(facets: &Facets, tri: [[f64; 3]; 3]) -> u32 {
    let off = |f: &Facet| {
        tri.iter()
            .map(|p| (dot(f.normal, *p) - f.offset).abs())
            .sum::<f64>()
    };
    (0..facets.all.len())
        .min_by(|&a, &b| off(&facets.all[a]).total_cmp(&off(&facets.all[b])))
        .unwrap_or(0) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outline(points: Vec<[f64; 2]>, base: u64) -> Outline {
        let faces = (0..points.len() as u64).map(|i| FaceId(base + i)).collect();
        Outline { points, faces }
    }

    fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<[f64; 2]> {
        vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]]
    }

    fn circle(x: f64, y: f64, r: f64, n: usize) -> Vec<[f64; 2]> {
        (0..n)
            .map(|i| {
                let a = std::f64::consts::TAU * i as f64 / n as f64;
                [x + r * a.cos(), y + r * a.sin()]
            })
            .collect()
    }

    fn block(f: &mut Facets, p: Vec<[f64; 2]>, z0: f64, z1: f64, base: u64) -> Solid {
        prism(
            f,
            &outline(p, base),
            z0,
            z1,
            FaceId(base + 1000),
            FaceId(base + 1001),
        )
        .unwrap()
    }

    #[test]
    fn closed_is_each_edge_once_each_way() {
        // As it was defined: a map of the half-edges.
        fn by_map(s: &Solid) -> bool {
            let mut e: HashMap<(u32, u32), u32> = HashMap::new();
            for t in &s.tris {
                for k in 0..3 {
                    *e.entry((t[k], t[(k + 1) % 3])).or_default() += 1;
                }
            }
            e.iter()
                .all(|(&(a, b), &n)| n == 1 && e.get(&(b, a)) == Some(&1))
        }
        let mut f = Facets::default();
        let cube = block(&mut f, rect(0.0, 0.0, 1.0, 1.0), 0.0, 1.0, 0);
        let with = |tris: Vec<[u32; 3]>| Solid {
            tris,
            facets: Vec::new(),
            ..cube.clone()
        };
        let mut cases = vec![
            cube.tris.clone(),
            Vec::new(),
            cube.tris[1..].to_vec(),
            [cube.tris.clone(), cube.tris.clone()].concat(),
            [cube.tris.clone(), vec![[0, 0, 0]]].concat(),
            [cube.tris.clone(), vec![[0, 0, 0], [0, 0, 0]]].concat(),
            [cube.tris.clone(), vec![[0, 1, 1], [1, 0, 0]]].concat(),
            vec![[0, 1, 2], [0, 2, 1]],
            vec![[0, 1, 2], [0, 1, 2]],
            vec![[0, 1, 2], [2, 1, 0], [0, 1, 2], [2, 1, 0]],
        ];
        // Turned round: the same answer.
        let turned: Vec<Vec<[u32; 3]>> = cases
            .iter()
            .map(|c| c.iter().map(|t| [t[0], t[2], t[1]]).collect())
            .collect();
        cases.extend(turned);
        for (i, tris) in cases.into_iter().enumerate() {
            let s = with(tris);
            assert_eq!(s.is_closed(), by_map(&s), "case {i}");
        }
        assert!(cube.is_closed());
    }

    #[test]
    fn a_through_cut_is_closed_and_keeps_its_faces() {
        let mut f = Facets::default();
        let a = block(&mut f, rect(0.0, 0.0, 10.0, 10.0), 0.0, 10.0, 0);
        let c = block(&mut f, rect(3.0, 3.0, 7.0, 7.0), 0.0, 10.0, 2000);
        let r = a.boolean(&c, Op::Subtract, &mut f).unwrap();
        assert!(r.is_closed());
        assert!((r.volume() - 840.0).abs() < 1e-9);
        let faces = r.faces(&f);
        assert!(faces.contains(&FaceId(1000)) && faces.contains(&FaceId(1001)));
        assert!(faces.contains(&FaceId(2000)));
        // The cutter's caps lie in the block's caps and leave nothing behind.
        assert!(!faces.contains(&FaceId(3000)) && !faces.contains(&FaceId(3001)));
    }

    #[test]
    fn cutting_everything_away_is_an_empty_solid() {
        let mut f = Facets::default();
        let a = block(&mut f, rect(0.0, 0.0, 10.0, 10.0), 0.0, 10.0, 0);
        let b = block(&mut f, rect(0.0, 0.0, 10.0, 10.0), 0.0, 10.0, 2000);
        assert!(a.boolean(&b, Op::Subtract, &mut f).unwrap().is_empty());
    }

    #[test]
    fn a_batch_of_holes_cut_at_once_is_closed() {
        let mut f = Facets::default();
        let a = block(&mut f, rect(0.0, 0.0, 50.0, 50.0), 0.0, 3.0, 0);
        let holes: Vec<Solid> = (0..25)
            .map(|i| {
                let (x, y) = (5.0 + 10.0 * (i % 5) as f64, 5.0 + 10.0 * (i / 5) as f64);
                block(&mut f, circle(x, y, 2.0, 32), 0.0, 3.0, 2000 + 2000 * i)
            })
            .collect();
        let r = a
            .boolean(&Solid::merged(&holes), Op::Subtract, &mut f)
            .unwrap();
        assert!(r.is_closed());
        assert!(r.volume() > 0.0 && r.volume() < 7500.0);
    }

    #[test]
    fn a_batch_joins_what_meets_and_cuts_all_at_once() {
        let mut f = Facets::default();
        // Two blocks that overlap, one apart, one inside another.
        let parts = [
            block(&mut f, rect(0.0, 0.0, 10.0, 10.0), 0.0, 10.0, 0),
            block(&mut f, rect(5.0, 0.0, 15.0, 10.0), 0.0, 10.0, 2000),
            block(&mut f, rect(20.0, 0.0, 30.0, 10.0), 0.0, 10.0, 4000),
            block(&mut f, rect(1.0, 1.0, 2.0, 2.0), 1.0, 2.0, 6000),
        ];
        let refs: Vec<&Solid> = parts.iter().collect();
        if let Some(u) = Solid::batch(&refs, Op::Add, &mut f) {
            let u = u.unwrap();
            assert!(u.is_closed());
            assert!((u.volume() - 2500.0).abs() < 1e-6, "{}", u.volume());
        }
        let holes: Vec<Solid> = (0..4)
            .map(|i| {
                let x = 2.0 + 7.0 * i as f64;
                block(
                    &mut f,
                    rect(x, 4.0, x + 2.0, 6.0),
                    -1.0,
                    11.0,
                    8000 + 2000 * i,
                )
            })
            .collect();
        let mut all = vec![&parts[0]];
        all.extend(&holes);
        if let Some(d) = Solid::batch(&all, Op::Subtract, &mut f) {
            let d = d.unwrap();
            assert!(d.is_closed());
            // A hole through the block, one through its edge, two beside it.
            assert!(
                (d.volume() - (1000.0 - 40.0 - 20.0)).abs() < 1e-6,
                "{}",
                d.volume()
            );
            assert!(d.facets.iter().all(|&k| (k as usize) < f.len()));
        }
    }

    #[test]
    fn a_mirrored_copy_keeps_one_facet_a_plane() {
        let mut f = Facets::default();
        let a = block(&mut f, rect(0.0, 0.0, 10.0, 5.0), 0.0, 2.0, 0);
        let m = [
            [-1.0, 0.0, 0.0, 3.0],
            [0.0, 2.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 1.0],
        ];
        let b = a.mapped(&mut f, &m).unwrap();
        assert!(b.is_closed());
        assert!((b.volume() - 200.0).abs() < 1e-9, "{}", b.volume());
        let mut planes: Vec<u32> = b.facets.clone();
        planes.sort_unstable();
        planes.dedup();
        assert_eq!(planes.len(), 6);
        // Each triangle lies in its facet's plane, its normal outward.
        for (t, &k) in b.tris.iter().zip(&b.facets) {
            let [p, q, r] = t.map(|i| b.points[i as usize]);
            assert!(f.fits(k, [p, q, r]));
            assert!(dot(cross(sub(q, p), sub(r, p)), f.get(k).normal) > 0.0);
        }
        assert!(
            a.mapped(&mut f, &[[1.0, 0.0, 0.0, 0.0], [0.0; 4], [0.0; 4]])
                .is_none()
        );
    }

    /// A boolean's result moved (by the library, where it moves it), cut
    /// again: each triangle on a facet whose plane it lies in, its normal
    /// outward, the moved copy's facets not the unmoved one's.
    #[test]
    fn a_moved_result_keeps_its_facets_through_the_next_cut() {
        let mut f = Facets::default();
        let a = block(&mut f, rect(0.0, 0.0, 10.0, 10.0), 0.0, 10.0, 0);
        let c = block(&mut f, circle(5.0, 5.0, 2.0, 24), 0.0, 10.0, 2000);
        let holed = a.boolean(&c, Op::Subtract, &mut f).unwrap();
        // Turned about z, mirrored in x, moved.
        let (s, k) = 1.2f64.sin_cos();
        let m = [[-k, s, 0.0, 30.0], [s, k, 0.0, 1.0], [0.0, 0.0, 1.0, 2.0]];
        let moved = holed.mapped(&mut f, &m).unwrap();
        let cut = block(&mut f, rect(-50.0, -50.0, 50.0, 50.0), 4.0, 5.0, 4000);
        let r = moved.boolean(&cut, Op::Subtract, &mut f).unwrap();
        assert!(r.is_closed());
        let want = holed.volume() * 0.9;
        assert!((r.volume() - want).abs() < 1e-6, "{} {}", r.volume(), want);
        for (t, &k) in r.tris.iter().zip(&r.facets) {
            let [p, q, w] = t.map(|i| r.points[i as usize]);
            assert!(f.fits(k, [p, q, w]), "{:?}", f.get(k));
            assert!(dot(cross(sub(q, p), sub(w, p)), f.get(k).normal) > 0.0);
        }
    }

    /// Rotated random cuts blend corner tags; every triangle must still end
    /// on a facet whose plane it lies in.
    #[test]
    fn faces_survive_many_rotated_cuts() {
        let mut f = Facets::default();
        let mut a = block(&mut f, rect(0.0, 0.0, 100.0, 100.0), 0.0, 10.0, 0);
        let mut s = 12345u64;
        let mut next = move || {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            (s % 10000) as f64 / 100.0
        };
        for i in 0..120u64 {
            let (x, y, an) = (next(), next(), next() * 0.03);
            let (sn, cs) = an.sin_cos();
            let r = rect(x, y, x + next() / 4.0 + 1.0, y + next() / 8.0 + 1.0);
            let p = r
                .iter()
                .map(|q| [cs * q[0] - sn * q[1], sn * q[0] + cs * q[1]])
                .collect();
            let c = block(
                &mut f,
                p,
                if next() > 50.0 { 2.0 } else { 0.0 },
                10.0,
                2000 + i * 2000,
            );
            a = a.boolean(&c, Op::Subtract, &mut f).unwrap();
            assert!(a.is_closed(), "cut {i}");
        }
        for (t, &facet) in a.tris.iter().zip(&a.facets) {
            let tri = t.map(|i| a.points[i as usize]);
            assert!(f.fits(facet, tri), "{tri:?}");
        }
    }

    fn oblique_frame() -> Frame {
        Frame {
            origin: [1.0, 2.0, 3.0],
            u: [0.0, 1.0, 0.0],
            v: [0.0, 0.0, 1.0],
        }
    }

    #[test]
    fn extruding_in_either_direction_gives_a_closed_positive_solid() {
        for to in [[5.0, 0.0, 0.0], [-5.0, 0.0, 0.0]] {
            let mut f = Facets::default();
            let o = outline(rect(0.0, 0.0, 2.0, 3.0), 0);
            let s = extrude(
                &mut f,
                &o,
                &oblique_frame(),
                [0.0; 3],
                to,
                FaceId(90),
                FaceId(91),
            )
            .unwrap();
            assert!(s.is_closed());
            assert!((s.volume() - 30.0).abs() < 1e-9, "{}", s.volume());
            for (t, &facet) in s.tris.iter().zip(&s.facets) {
                assert!(f.fits(facet, t.map(|i| s.points[i as usize])));
                let [a, b, c] = t.map(|i| s.points[i as usize]);
                let n = cross(sub(b, a), sub(c, a));
                assert!(dot(n, f.get(facet).normal) > 0.0, "normal of {t:?}");
            }
        }
    }

    #[test]
    fn a_translated_copy_keeps_its_planes() {
        let mut f = Facets::default();
        let a = block(&mut f, rect(0.0, 0.0, 1.0, 1.0), 0.0, 1.0, 0);
        let b = a.translated(&mut f, [3.0, 0.0, 0.0], |x| FaceId(x.0 + 5000));
        for (t, &facet) in b.tris.iter().zip(&b.facets) {
            assert!(f.fits(facet, t.map(|i| b.points[i as usize])));
        }
        let r = a.boolean(&b, Op::Add, &mut f).unwrap();
        assert!(r.is_closed());
        assert!((r.volume() - 2.0).abs() < 1e-9);
    }

    /// Two blocks left touching along an edge are one mesh the library made
    /// and would refuse to read back.
    #[test]
    fn parts_touching_along_an_edge_can_be_cut_again() {
        let mut f = Facets::default();
        let a = block(&mut f, rect(0.0, 0.0, 10.0, 10.0), 0.0, 5.0, 0);
        let c1 = block(&mut f, rect(5.0, 0.0, 10.0, 5.0), 0.0, 5.0, 2000);
        let c2 = block(&mut f, rect(0.0, 5.0, 5.0, 10.0), 0.0, 5.0, 4000);
        let r = a.boolean(&c1, Op::Subtract, &mut f).unwrap();
        let r = r.boolean(&c2, Op::Subtract, &mut f).unwrap();
        assert!((r.volume() - 250.0).abs() < 1e-9);
        let hole = block(&mut f, rect(1.0, 1.0, 2.0, 2.0), 0.0, 5.0, 6000);
        let r = r.boolean(&hole, Op::Subtract, &mut f).unwrap();
        assert!((r.volume() - 245.0).abs() < 1e-9);
    }

    /// Heights meant to be equal land on the same grid value, so a cut ending
    /// in a face ends exactly there.
    #[test]
    fn heights_a_hair_apart_become_equal() {
        let mut f = Facets::default();
        let frame = Frame {
            origin: [0.0; 3],
            u: [1.0, 0.0, 0.0],
            v: [0.0, 1.0, 0.0],
        };
        let top = |f: &mut Facets, z: f64, base| {
            let o = outline(rect(0.0, 0.0, 1.0, 1.0), base);
            let s = extrude(f, &o, &frame, [0.0; 3], [0.0, 0.0, z], FaceId(9), FaceId(8)).unwrap();
            s.points[4][2]
        };
        let (a, b) = (top(&mut f, 2.0, 0), top(&mut f, 2.0 + 1e-13, 2000));
        assert_eq!(a.to_bits(), b.to_bits());
    }

    #[test]
    fn a_turned_copy_keeps_its_volume_and_planes() {
        let mut f = Facets::default();
        let a = block(&mut f, rect(10.0, 0.0, 12.0, 2.0), 0.0, 3.0, 0);
        let b = a.rotated(
            &mut f,
            [0.0; 3],
            [0.0, 0.0, 1.0],
            std::f64::consts::FRAC_PI_2,
            |x| FaceId(x.0 + 9000),
        );
        assert!((b.volume() - a.volume()).abs() < 1e-9);
        let (lo, hi) = b.bounds().unwrap();
        assert!(
            (lo[0] + 2.0).abs() < 1e-6 && (hi[1] - 12.0).abs() < 1e-6,
            "{lo:?} {hi:?}"
        );
        for (t, &facet) in b.tris.iter().zip(&b.facets) {
            assert!(f.fits(facet, t.map(|i| b.points[i as usize])));
        }
    }

    #[test]
    fn inside_creases_and_a_sphere() {
        let mut f = Facets::default();
        let a = block(&mut f, rect(0.0, 0.0, 10.0, 10.0), 0.0, 10.0, 0);
        assert!(a.contains([5.0, 5.0, 5.0]) && !a.contains([15.0, 5.0, 5.0]));
        assert_eq!(a.creases(&f).len(), 12);
        let s = Solid::sphere(&mut f, [0.0; 3], 2.0, 32, FaceId(7));
        assert!(s.is_closed());
        let exact = 4.0 / 3.0 * std::f64::consts::PI * 8.0;
        assert!(
            s.volume() > 0.95 * exact && s.volume() < exact,
            "{}",
            s.volume()
        );
    }

    #[test]
    fn parts_that_meet_along_an_edge_are_cut_and_cut_from() {
        // Two blocks that share only an edge: welded, an edge of four
        // triangles, on which the library panics as input and as result.
        let mut f = Facets::default();
        let a = block(&mut f, rect(0.0, 0.0, 10.0, 10.0), 0.0, 10.0, 0);
        let b = block(&mut f, rect(10.0, 0.0, 20.0, 10.0), 10.0, 20.0, 2000);
        let both = Solid::merged(&[a.clone(), b.clone()]);
        let big = block(&mut f, rect(-5.0, -5.0, 25.0, 15.0), -5.0, 25.0, 4000);
        let r = big.boolean(&both, Op::Subtract, &mut f).unwrap();
        assert!(r.is_closed());
        assert!(
            (r.volume() - (30.0 * 20.0 * 30.0 - 2000.0)).abs() < 1e-2,
            "{}",
            r.volume()
        );
        // As a cut away, one after the other.
        let r = big
            .boolean(&a, Op::Subtract, &mut f)
            .unwrap()
            .boolean(&b, Op::Subtract, &mut f)
            .unwrap();
        assert!(
            (r.volume() - (30.0 * 20.0 * 30.0 - 2000.0)).abs() < 1e-2,
            "{}",
            r.volume()
        );
        // Their union, read again (as after a move), cut from.
        let u = a.boolean(&b, Op::Add, &mut f).unwrap();
        let mut g = Facets::default();
        let u = u.rehome(&f, &mut g);
        let big = big.rehome(&f, &mut g);
        let r = big.boolean(&u, Op::Subtract, &mut g).unwrap();
        assert!(
            (r.volume() - (30.0 * 20.0 * 30.0 - 2000.0)).abs() < 1e-2,
            "{}",
            r.volume()
        );
    }

    #[test]
    fn stl_has_the_size_the_header_promises() {
        let mut f = Facets::default();
        let a = block(&mut f, rect(0.0, 0.0, 1.0, 1.0), 0.0, 1.0, 0);
        assert_eq!(a.to_stl().len(), 84 + 50 * a.tris.len());
    }

    #[test]
    fn a_batch_asked_to_stop_ends_with_stopped() {
        let mut f = Facets::default();
        let balls: Vec<Solid> = (0..64)
            .map(|i| Solid::sphere(&mut f, [f64::from(i) * 0.5, 0.0, 0.0], 1.0, 16, FaceId(1)))
            .collect();
        let refs: Vec<&Solid> = balls.iter().collect();
        let stop = Stop::new();
        stop.stop();
        let made = stopping(&stop, || Solid::batch(&refs, Op::Add, &mut f));
        if engine::BATCHES {
            assert_eq!(made.map(|r| r.err()), Some(Some(Error::Stopped)));
        }
        let pairs: Vec<(&Solid, &Solid)> = refs.windows(2).map(|w| (w[0], w[1])).collect();
        let made = stopping(&stop, || Solid::booleans(&pairs, Op::Add, &mut f));
        assert!(made.iter().all(|r| r == &Err(Error::Stopped)));
        // Outside, the stop of the thread is the one before: none.
        assert!(!stopped());
        assert!(Solid::batch(&refs, Op::Add, &mut f).is_none_or(|r| r.is_ok()));
    }
}
