//! Solids as closed meshes whose triangles remember the face they came from
//! (ADR 0003), booleans on them, and the check and STL output for printing.
//!
//! Each planar piece of a solid is a [`Facet`]. A facet belongs to a [`FaceId`]:
//! a side of an extrusion that follows a circle is many facets and one face.
//! Booleans carry the facet index through as a value per triangle corner.

use std::collections::HashMap;
use std::ops::{Add, Mul};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use boolmesh::prelude::{Manifold, OpType, compute_boolean};

mod export;
mod raster;
mod revolve;
mod stl;
mod triangulate;
mod zip;

pub use export::{encode, step, three_mf, three_mf_objects};
pub use raster::{Shot, render};
pub use revolve::{revolve, revolve_radius};
pub use stl::read_stl;

pub use triangulate::triangulate;

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

/// A closed mesh; `facets[i]` names triangle `i`.
#[derive(Clone, Default)]
pub struct Solid {
    pub points: Vec<[f64; 3]>,
    pub tris: Vec<[u32; 3]>,
    pub facets: Vec<u32>,
    /// The library's own mesh of a boolean's result. Reading a mesh in welds
    /// vertices by position, which turns two parts touching along an edge
    /// into a mesh it refuses, so results go on as they are.
    mesh: Option<Arc<Manifold<Tag>>>,
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
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::BadProfile => f.write_str("the profile has no area"),
            Error::Boolean(m) => write!(f, "boolean failed: {m}"),
            Error::AxisOffPlane => f.write_str("the axis is not in the sketch's plane"),
            Error::CrossesAxis => f.write_str("the profile crosses the axis"),
        }
    }
}

impl std::error::Error for Error {}

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

/// An edge where two facets meet at an angle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Crease {
    pub a: [f64; 3],
    pub b: [f64; 3],
    /// The facet whose triangle runs from `a` to `b`, and the one beside it.
    pub left: u32,
    pub right: u32,
}

/// Value carried through a boolean: the facet, as a number.
#[derive(Clone, Debug, Default, PartialEq)]
struct Tag(f64);

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
        let mut map: HashMap<u32, u32> = HashMap::new();
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
        let mut map: HashMap<u32, u32> = HashMap::new();
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

    fn manifold(&self) -> Result<Arc<Manifold<Tag>>, Error> {
        if let Some(m) = &self.mesh {
            return Ok(m.clone());
        }
        let pos: Vec<f64> = self.points.iter().flatten().copied().collect();
        let idx: Vec<usize> = self.tris.iter().flatten().map(|&i| i as usize).collect();
        let vals: Vec<Tag> = self
            .facets
            .iter()
            .flat_map(|&f| std::iter::repeat_n(Tag(f64::from(f)), 3))
            .collect();
        Manifold::new_with_values(&pos, &idx, &vals)
            .map(Arc::new)
            .map_err(Error::Boolean)
    }

    /// The boolean of two solids. What a cut leaves of the cutter faces the
    /// other way: such triangles get a facet turned round, of the same face.
    pub fn boolean(&self, other: &Solid, op: Op, facets: &mut Facets) -> Result<Solid, Error> {
        if self.is_empty() || other.is_empty() {
            return Ok(match op {
                Op::Add if self.is_empty() => other.clone(),
                Op::Add | Op::Subtract => self.clone(),
                Op::Intersect => Solid::default(),
            });
        }
        let kind = match op {
            Op::Add => OpType::Add,
            Op::Subtract => OpType::Subtract,
            Op::Intersect => OpType::Intersect,
        };
        // The library can panic on an index; a failed feature must not take
        // the app with it.
        let result = catch_unwind(AssertUnwindSafe(
            || -> Result<Result<Manifold<Tag>, String>, Error> {
                let (a, b) = (self.manifold()?, other.manifold()?);
                Ok(compute_boolean(&a, &b, kind))
            },
        ))
        .map_err(|_| Error::Boolean("the mesh library panicked".to_string()))??;
        match result {
            Ok(m) => Ok(Solid::from_manifold(m, facets)),
            // Nothing is left: the library has no empty mesh.
            Err(e) if e.contains("empty") => Ok(Solid::default()),
            Err(e) => Err(Error::Boolean(e)),
        }
    }

    fn from_manifold(mut m: Manifold<Tag>, facets: &mut Facets) -> Solid {
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
        for _ in 0..tris.len() {
            let mut open = false;
            for i in 0..tris.len() {
                if out[i].is_some() {
                    continue;
                }
                let t = tris[i];
                let found = (0..3)
                    .filter_map(|k| edges.get(&(t[(k + 1) % 3], t[k])).and_then(|&n| out[n]))
                    .find(|&f| facets.fits(f, at(t)));
                out[i] = found;
                open |= found.is_none();
            }
            if !open {
                break;
            }
        }
        let mut turned: HashMap<u32, u32> = HashMap::new();
        let facets_of: Vec<u32> = out
            .iter()
            .zip(&tris)
            .map(|(f, t)| {
                let f = f.unwrap_or_else(|| nearest_facet(facets, at(*t)));
                let [p, q, r] = at(*t);
                let n = cross(sub(q, p), sub(r, p));
                let old = facets.get(f);
                // A splinter's winding is noise: it keeps its facet.
                let longest = [sub(q, p), sub(r, q), sub(p, r)]
                    .map(|e| dot(e, e))
                    .into_iter()
                    .fold(0.0, f64::max);
                if dot(n, old.normal) >= 0.0 || dot(n, n) < 1e-12 * longest * longest {
                    return f;
                }
                *turned.entry(f).or_insert_with(|| {
                    let on = old.normal.map(|x| x * old.offset);
                    facets.add(old.face, old.normal.map(|x| -x), on)
                })
            })
            .collect();
        // One plane, one way round: one face, under the older name. A cut
        // that ends on a face (a fillet's end on a floor) must not rename
        // the floor.
        let plane = |f: u32| {
            let x = facets.get(f);
            let r = |v: f64| (v * 1e6).round() as i64;
            (x.normal.map(r), r(x.offset))
        };
        let mut oldest: HashMap<([i64; 3], i64), u32> = HashMap::new();
        for &f in &facets_of {
            oldest
                .entry(plane(f))
                .and_modify(|o| *o = (*o).min(f))
                .or_insert(f);
        }
        let facets_of: Vec<u32> = facets_of.iter().map(|&f| oldest[&plane(f)]).collect();
        // The next boolean starts from the repaired names.
        for (c, f) in m.vals.chunks_mut(3).zip(&facets_of) {
            c.fill(Tag(f64::from(*f)));
        }
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
        let mut e: HashMap<(u32, u32), u32> = HashMap::new();
        for t in &self.tris {
            for k in 0..3 {
                *e.entry((t[k], t[(k + 1) % 3])).or_default() += 1;
            }
        }
        e.iter()
            .all(|(&(a, b), &n)| n == 1 && e.get(&(b, a)) == Some(&1))
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
    fn stl_has_the_size_the_header_promises() {
        let mut f = Facets::default();
        let a = block(&mut f, rect(0.0, 0.0, 1.0, 1.0), 0.0, 1.0, 0);
        assert_eq!(a.to_stl().len(), 84 + 50 * a.tris.len());
    }
}
