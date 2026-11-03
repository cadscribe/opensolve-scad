//! OpenSCAD's shapes on the mesh kernel. Circles, spheres and cylinders are
//! cut into as many pieces as OpenSCAD cuts them, at the same places, so a
//! part comes out as OpenSCAD makes it. A 2D shape is a polygon with holes
//! until it is extruded, its booleans and offsets Clipper's, as in OpenSCAD
//! (see [`crate::flat`]).

use std::collections::{HashMap, HashSet};

use opensolve_kernel::{FaceId, Facets, Op, Solid};

use crate::eval::Fx;
use crate::flat::{self, Region};

pub type V3 = [f64; 3];
pub type V2 = [f64; 2];
/// An affine map: three rows of a 4×4 matrix.
pub type Matrix = [[f64; 4]; 3];

pub const IDENTITY: Matrix = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
];

#[derive(Clone, Debug)]
pub enum Geom {
    /// A 2D shape.
    D2(Region),
    D3(Solid),
}

/// Smaller than this, a radius makes three pieces (OpenSCAD's `GRID_FINE`).
const GRID_FINE: f64 = 0.000_000_953_674_316_406_25;

/// How many pieces OpenSCAD cuts a circle of radius `r` into.
pub fn fragments(r: f64, fn_: f64, fs: f64, fa: f64) -> usize {
    if r < GRID_FINE || !r.is_finite() {
        return 3;
    }
    if fn_ > 0.0 {
        return if fn_ >= 3.0 { fn_ as usize } else { 3 };
    }
    let (fa, fs) = (fa.max(0.01), fs.max(0.01));
    (360.0 / fa)
        .min(r * 2.0 * std::f64::consts::PI / fs)
        .max(5.0)
        .ceil() as usize
}

/// The sine of `x` degrees, exact at multiples of 30° as in OpenSCAD.
pub fn sin_deg(x: f64) -> f64 {
    crate::trig::sin(x)
}

/// The cosine of `x` degrees, exact at multiples of 30° as in OpenSCAD.
pub fn cos_deg(x: f64) -> f64 {
    crate::trig::cos(x)
}

fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn cross2(o: V2, a: V2, b: V2) -> f64 {
    (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
}

pub fn apply(m: &Matrix, p: V3) -> V3 {
    std::array::from_fn(|i| m[i][0] * p[0] + m[i][1] * p[1] + m[i][2] * p[2] + m[i][3])
}

/// `a` after `b`.
pub fn compose(a: &Matrix, b: &Matrix) -> Matrix {
    std::array::from_fn(|i| {
        std::array::from_fn(|j| {
            let t = if j == 3 { a[i][3] } else { 0.0 };
            (0..3).map(|k| a[i][k] * b[k][j]).sum::<f64>() + t
        })
    })
}

fn det(m: &Matrix) -> f64 {
    dot(
        [m[0][0], m[0][1], m[0][2]],
        cross([m[1][0], m[1][1], m[1][2]], [m[2][0], m[2][1], m[2][2]]),
    )
}

/// Builds shapes; their facets all go into one list.
#[derive(Debug, Default)]
pub struct Builder {
    pub facets: Facets,
    next_face: u64,
    /// Results of batches by what went in (see [`Builder::batch`]).
    made: HashMap<(u64, u64), Solid, Fx>,
    /// Their triangles, all told.
    made_size: usize,
}

/// Results kept, at most, in triangles: a few hundred megabytes.
const MOST_KEPT: usize = 4_000_000;

/// Two hashes of solids' corners and triangles in order, with `op`: as
/// good as their contents for a key.
fn content_key(list: &[&Solid], op: Op) -> (u64, u64) {
    let (mut a, mut b) = (op as u64, 0x9e37_79b9_7f4a_7c15u64 ^ op as u64);
    let mut add = |x: u64| {
        a = (a.rotate_left(5) ^ x).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95);
        b = (b ^ x).wrapping_mul(0x0000_0100_0000_01b3).rotate_left(23);
    };
    for s in list {
        add(s.points.len() as u64);
        for p in &s.points {
            for x in p {
                add(x.to_bits());
            }
        }
        add(s.tris.len() as u64);
        for t in &s.tris {
            add(u64::from(t[0]) | u64::from(t[1]) << 21 | u64::from(t[2]) << 42);
        }
    }
    (a, b)
}

impl Builder {
    /// [`Solid::batch`] of solids, kept: the same shapes made again (the
    /// cells of a grid, each the same before it is moved) are joined once,
    /// as OpenSCAD's cache of geometry does it.
    pub fn batch(&mut self, list: &[&Solid], op: Op) -> Option<Result<Solid, String>> {
        let key = content_key(list, op);
        if let Some(s) = self.made.get(&key) {
            return Some(Ok(s.clone()));
        }
        let made = Solid::batch(list, op, &mut self.facets)?.map_err(|e| e.to_string());
        if let Ok(s) = &made {
            if self.made_size + s.tris.len() > MOST_KEPT {
                self.made.clear();
                self.made_size = 0;
            }
            self.made_size += s.tris.len();
            self.made.insert(key, s.clone());
        }
        Some(made)
    }

    /// A solid from loose triangles, its faces named anew.
    fn solid(&mut self, tris: &[[V3; 3]]) -> Solid {
        let base = self.next_face + 1;
        self.next_face += tris.len() as u64 + 1;
        Solid::from_triangles(tris, &mut self.facets, |k| FaceId(base + k as u64))
    }

    /// Loose triangles turned inside out if they enclose a negative volume.
    fn outward(&mut self, mut tris: Vec<[V3; 3]>) -> Solid {
        let v: f64 = tris.iter().map(|[a, b, c]| dot(*a, cross(*b, *c))).sum();
        if v < 0.0 {
            for t in &mut tris {
                t.swap(1, 2);
            }
        }
        self.solid(&tris)
    }

    pub fn cube(&mut self, size: V3, center: bool) -> Option<Solid> {
        if size.iter().any(|s| *s <= 0.0 || !s.is_finite()) {
            return None;
        }
        let o = if center {
            size.map(|s| -s / 2.0)
        } else {
            [0.0; 3]
        };
        let p = |i: usize| -> V3 {
            std::array::from_fn(|k| o[k] + if i >> k & 1 == 1 { size[k] } else { 0.0 })
        };
        // Each face, counter-clockwise from outside.
        let quads = [
            [0, 2, 3, 1],
            [4, 5, 7, 6],
            [0, 1, 5, 4],
            [2, 6, 7, 3],
            [0, 4, 6, 2],
            [1, 3, 7, 5],
        ];
        let tris: Vec<[V3; 3]> = quads
            .iter()
            .flat_map(|q| [[p(q[0]), p(q[1]), p(q[2])], [p(q[0]), p(q[2]), p(q[3])]])
            .collect();
        Some(self.solid(&tris))
    }

    /// OpenSCAD's sphere: rings at the middle of even bands of latitude,
    /// each a circle of `n` pieces, closed by the top and bottom rings.
    pub fn sphere(&mut self, r: f64, n: usize) -> Option<Solid> {
        if r <= 0.0 || !r.is_finite() {
            return None;
        }
        let rings = n.div_ceil(2).max(1);
        let ring: Vec<Vec<V3>> = (0..rings)
            .map(|i| {
                let phi = 180.0 * (i as f64 + 0.5) / rings as f64;
                let (rr, z) = (r * sin_deg(phi), r * cos_deg(phi));
                circle(rr, n).into_iter().map(|p| [p[0], p[1], z]).collect()
            })
            .collect();
        let mut tris = Vec::new();
        fan(&mut tris, &ring[0], true);
        fan(&mut tris, &ring[rings - 1], false);
        for w in ring.windows(2) {
            band(&mut tris, &w[1], &w[0]);
        }
        Some(self.outward(tris))
    }

    pub fn cylinder(&mut self, h: f64, r1: f64, r2: f64, center: bool, n: usize) -> Option<Solid> {
        if h <= 0.0 || r1 < 0.0 || r2 < 0.0 || (r1 <= 0.0 && r2 <= 0.0) || !h.is_finite() {
            return None;
        }
        let (z1, z2) = if center {
            (-h / 2.0, h / 2.0)
        } else {
            (0.0, h)
        };
        let at = |r: f64, z: f64| -> Vec<V3> {
            circle(r, n).into_iter().map(|p| [p[0], p[1], z]).collect()
        };
        let (bottom, top) = (at(r1, z1), at(r2, z2));
        let mut tris = Vec::new();
        if r1 > 0.0 {
            fan(&mut tris, &bottom, false);
        }
        if r2 > 0.0 {
            fan(&mut tris, &top, true);
        }
        band(&mut tris, &bottom, &top);
        Some(self.outward(tris))
    }

    /// Faces listed clockwise seen from outside, as OpenSCAD wants them;
    /// a polyhedron the other way round is turned.
    pub fn polyhedron(&mut self, points: &[V3], faces: &[Vec<usize>]) -> Result<Solid, String> {
        let mut tris = Vec::new();
        for f in faces {
            if let Some(&bad) = f.iter().find(|&&i| i >= points.len()) {
                return Err(format!("polyhedron: point {bad} of a face does not exist"));
            }
            let mut face: Vec<V3> = f.iter().rev().map(|&i| points[i]).collect();
            face.dedup();
            for t in face_triangles(&face) {
                tris.push(t);
            }
        }
        let s = self.outward(tris);
        if s.is_empty() {
            return Err("polyhedron: it has no faces".into());
        }
        Ok(s)
    }

    /// A mesh read from a file, its faces counter-clockwise from outside;
    /// `None` if it has none.
    pub fn mesh(&mut self, faces: &[Vec<V3>]) -> Option<Solid> {
        let tris: Vec<[V3; 3]> = faces.iter().flat_map(|f| face_triangles(f)).collect();
        let s = self.outward(tris);
        (!s.is_empty()).then_some(s)
    }

    /// A polygon of paths, the even-odd rule deciding what is inside, as in
    /// OpenSCAD's `polygon` with several paths or a path that crosses
    /// itself; `None` if nothing is inside.
    pub fn polygon(&mut self, paths: &[Vec<V2>]) -> Option<Region> {
        let paths: Vec<Vec<V2>> = paths
            .iter()
            .map(|p| clean(p))
            .filter(|p| p.len() >= 3)
            .collect();
        let r = flat::even_odd(&paths);
        (!r.is_empty()).then_some(r)
    }

    pub fn square(&mut self, size: V2, center: bool) -> Option<Region> {
        if size.iter().any(|s| *s <= 0.0 || !s.is_finite()) {
            return None;
        }
        let o = if center {
            size.map(|s| -s / 2.0)
        } else {
            [0.0; 2]
        };
        let (x0, y0, x1, y1) = (o[0], o[1], o[0] + size[0], o[1] + size[1]);
        self.polygon(&[vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]]])
    }

    pub fn circle(&mut self, r: f64, n: usize) -> Option<Region> {
        if r <= 0.0 || !r.is_finite() {
            return None;
        }
        self.polygon(&[circle(r, n)])
    }

    /// `a op b`.
    pub fn boolean(&mut self, a: &Solid, b: &Solid, op: Op) -> Result<Solid, String> {
        // Apart, a union is both as they are.
        if op == Op::Add
            && let (Some(p), Some(q)) = (a.bounds(), b.bounds())
            && (0..3).any(|i| p.1[i] < q.0[i] - 1e-9 || q.1[i] < p.0[i] - 1e-9)
        {
            return Ok(Solid::merged(&[a.clone(), b.clone()]));
        }
        a.boolean(b, op, &mut self.facets)
            .map_err(|e| e.to_string())
    }

    /// [`Builder::boolean`] of several pairs at once.
    pub fn booleans(&mut self, pairs: &[(&Solid, &Solid)], op: Op) -> Vec<Result<Solid, String>> {
        let mut out: Vec<Option<Result<Solid, String>>> = Vec::with_capacity(pairs.len());
        let mut todo: Vec<(Solid, Solid)> = Vec::new();
        let mut at = Vec::new();
        for (i, (a, b)) in pairs.iter().enumerate() {
            let b = (*b).clone();
            if op == Op::Add
                && let (Some(p), Some(q)) = (a.bounds(), b.bounds())
                && (0..3).any(|i| p.1[i] < q.0[i] - 1e-9 || q.1[i] < p.0[i] - 1e-9)
            {
                out.push(Some(Ok(Solid::merged(&[(*a).clone(), b]))));
                continue;
            }
            out.push(None);
            todo.push(((*a).clone(), b));
            at.push(i);
        }
        let refs: Vec<(&Solid, &Solid)> = todo.iter().map(|(a, b)| (a, b)).collect();
        let made = Solid::booleans(&refs, op, &mut self.facets);
        for (i, r) in at.into_iter().zip(made) {
            out[i] = Some(r.map_err(|e| e.to_string()));
        }
        out.into_iter()
            .map(|r| r.unwrap_or_else(|| Ok(Solid::default())))
            .collect()
    }

    /// The solid mapped by `m`; a mirror turns its triangles round.
    pub fn transformed(&mut self, s: &Solid, m: &Matrix) -> Solid {
        if let Some(moved) = s.mapped(&mut self.facets, m) {
            return moved;
        }
        let points = s.points.iter().map(|p| apply(m, *p)).collect();
        let flip = det(m) < 0.0;
        let tris = s
            .tris
            .iter()
            .map(|t| if flip { [t[0], t[2], t[1]] } else { *t })
            .collect();
        let faces: Vec<FaceId> = s.facets.iter().map(|&f| self.facets.get(f).face).collect();
        Solid::from_mesh(&mut self.facets, points, tris, &faces)
    }

    /// A 2D shape mapped by the 2D part of `m`.
    pub fn transformed_2d(&mut self, r: &Region, m: &Matrix) -> Region {
        flat::transformed(r, m)
    }

    /// The convex hull of points; `None` if they span no volume.
    pub fn hull(&mut self, points: &[V3]) -> Option<Solid> {
        let tris = hull3(points)?;
        Some(self.solid(&tris))
    }

    /// The convex hull of points in the plane.
    pub fn hull_2d(&mut self, points: &[V2]) -> Option<Region> {
        let h = hull2(points);
        self.polygon(&[h])
    }

    /// OpenSCAD's `linear_extrude`: the shape at `slices + 1` heights, each
    /// turned by its part of `twist` (clockwise seen from above, as OpenSCAD
    /// turns it) and scaled by its part of `scale`.
    pub fn linear_extrude(
        &mut self,
        shape: &Region,
        height: f64,
        center: bool,
        twist: f64,
        slices: usize,
        scale: V2,
    ) -> Option<Solid> {
        if height <= 0.0 || !height.is_finite() {
            return None;
        }
        let (loops, tris) = cap_indexed(&shape.parted())?;
        let z0 = if center { -height / 2.0 } else { 0.0 };
        let n = slices.max(1);
        let layer = |k: usize, p: V2| -> V3 {
            let t = k as f64 / n as f64;
            let a = -twist * t;
            let s = [1.0 + (scale[0] - 1.0) * t, 1.0 + (scale[1] - 1.0) * t];
            let (x, y) = (p[0] * s[0], p[1] * s[1]);
            let (c, si) = (cos_deg(a), sin_deg(a));
            [x * c - y * si, x * si + y * c, z0 + height * t]
        };
        // The corners of each layer, numbered as the caps number them.
        let corners: Vec<V2> = loops.concat();
        let m = corners.len() as u32;
        let points: Vec<V3> = (0..=n)
            .flat_map(|k| corners.iter().map(move |p| (k, *p)))
            .map(|(k, p)| layer(k, p))
            .collect();
        let at = |k: usize, i: u32| k as u32 * m + i;
        let mut faces: Vec<[u32; 3]> = Vec::with_capacity(2 * tris.len() + 2 * n * corners.len());
        for t in &tris {
            faces.push([t[0], t[2], t[1]]);
            faces.push(t.map(|i| at(n, i)));
        }
        // Twisted, a side is no plane: split as OpenSCAD splits it, by the
        // way it turns, the other way in holes.
        let turns_back = sin_deg(-twist / n as f64) >= 0.0;
        let mut first = 0u32;
        for ring in &loops {
            let hole = area(ring) < 0.0;
            let len = ring.len() as u32;
            for i in 0..len {
                let (a, b) = (first + i, first + (i + 1) % len);
                for k in 0..n {
                    let (a0, b0, a1, b1) = (at(k, a), at(k, b), at(k + 1, a), at(k + 1, b));
                    if turns_back != hole {
                        faces.push([a0, b0, b1]);
                        faces.push([a0, b1, a1]);
                    } else {
                        faces.push([a0, b0, a1]);
                        faces.push([a1, b0, b1]);
                    }
                }
            }
            first += len;
        }
        let base = self.next_face + 1;
        self.next_face += faces.len() as u64 + 1;
        Some(Solid::from_indexed(points, &faces, &mut self.facets, |k| {
            FaceId(base + k as u64)
        }))
    }

    /// OpenSCAD's `rotate_extrude`: the shape, in the XZ plane, turned about
    /// Z in `n` steps; a whole turn starts on −X, as OpenSCAD's does, and
    /// part of one on +X.
    pub fn rotate_extrude(
        &mut self,
        shape: &Region,
        angle: f64,
        n: usize,
    ) -> Result<Option<Solid>, String> {
        let Some((loops, tris)) = cap_indexed(&shape.parted()) else {
            return Ok(None);
        };
        let mut corners: Vec<V2> = loops.concat();
        let xs = corners.iter().map(|p| p[0]);
        let (lo, hi) = xs.fold((f64::MAX, f64::MIN), |(l, h), x| (l.min(x), h.max(x)));
        if lo < -1e-9 && hi > 1e-9 {
            return Err("rotate_extrude: the shape lies on both sides of the Y axis".into());
        }
        // All on the left: turned with its x as it is, as OpenSCAD's
        // `x · sin a, x · cos a` turns it, so the solid lies on the other
        // side of the axis (a half turn of the one of the mirror image).
        let mirrored = hi <= 1e-9;
        if mirrored {
            for p in &mut corners {
                p[0] = -p[0];
            }
        }
        let side = if mirrored { -1.0 } else { 1.0 };
        let whole = angle.abs() >= 360.0;
        let n = n.max(1);
        let at = |j: usize, p: V2| -> V3 {
            let a = if whole {
                -90.0 + (j % n) as f64 * 360.0 / n as f64
            } else {
                90.0 - j as f64 * angle / n as f64
            };
            let x = p[0].max(0.0) * side;
            [x * sin_deg(a), x * cos_deg(a), p[1]]
        };
        // Each corner at each step, numbered; one on the axis is one point
        // at all steps.
        let m = corners.len();
        let rings = if whole { n } else { n + 1 };
        let on_axis = |i: usize| corners[i][0] <= 0.0;
        let mut number = vec![0u32; rings * m];
        let mut points: Vec<V3> = Vec::with_capacity(rings * m);
        for i in (0..m).filter(|&i| on_axis(i)) {
            for j in 0..rings {
                number[j * m + i] = points.len() as u32;
            }
            points.push(at(0, corners[i]));
        }
        for j in 0..rings {
            for i in (0..m).filter(|&i| !on_axis(i)) {
                number[j * m + i] = points.len() as u32;
                points.push(at(j, corners[i]));
            }
        }
        let id = |j: usize, i: u32| number[(j % rings) * m + i as usize];
        let mut faces: Vec<[u32; 3]> = Vec::new();
        if !whole {
            for t in &tris {
                let t = if mirrored { [t[0], t[2], t[1]] } else { *t };
                faces.push([id(0, t[0]), id(0, t[2]), id(0, t[1])]);
                faces.push(t.map(|i| id(n, i)));
            }
        }
        let mut first = 0u32;
        for ring in &loops {
            let len = ring.len() as u32;
            for k in 0..len {
                let (mut a, mut b) = (first + k, first + (k + 1) % len);
                if mirrored {
                    std::mem::swap(&mut a, &mut b);
                }
                for j in 0..n {
                    let (a0, b0, a1, b1) = (id(j, a), id(j, b), id(j + 1, a), id(j + 1, b));
                    faces.push([a0, b0, b1]);
                    faces.push([a0, b1, a1]);
                }
            }
            first += len;
        }
        // Outward, whichever way the profile ran.
        let volume: f64 = faces
            .iter()
            .map(|t| {
                let [a, b, c] = t.map(|i| points[i as usize]);
                dot(a, cross(b, c))
            })
            .sum();
        if volume < 0.0 {
            for t in &mut faces {
                t.swap(1, 2);
            }
        }
        let base = self.next_face + 1;
        self.next_face += faces.len() as u64 + 1;
        Ok(Some(Solid::from_indexed(
            points,
            &faces,
            &mut self.facets,
            |k| FaceId(base + k as u64),
        )))
    }
}

/// OpenSCAD's circle of `n` pieces, the first corner on +X.
pub fn circle(r: f64, n: usize) -> Vec<V2> {
    (0..n)
        .map(|i| {
            let phi = 360.0 * i as f64 / n as f64;
            [r * cos_deg(phi), r * sin_deg(phi)]
        })
        .collect()
}

/// A convex ring closed by a strip zigzagging across it, seen from above
/// if `up`, as Manifold triangulates a convex polygon. A fan from one
/// corner would do, but its long triangles each reach over much of the
/// disc, and a boolean tests every one of them against all that lies
/// there.
fn fan(tris: &mut Vec<[V3; 3]>, ring: &[V3], up: bool) {
    let (mut i, mut k) = (0, ring.len().saturating_sub(1));
    let mut right = true;
    while i + 1 < k {
        let j = if right { i + 1 } else { k - 1 };
        let (a, b, c) = (ring[i], ring[j], ring[k]);
        tris.push(if up { [a, b, c] } else { [a, c, b] });
        if right {
            i = j;
        } else {
            k = j;
        }
        right = !right;
    }
}

/// The band between two rings of as many corners, `lower` below `upper`.
fn band(tris: &mut Vec<[V3; 3]>, lower: &[V3], upper: &[V3]) {
    let n = lower.len();
    for i in 0..n {
        let j = (i + 1) % n;
        tris.push([lower[i], lower[j], upper[j]]);
        tris.push([lower[i], upper[j], upper[i]]);
    }
}

/// The triangles of a face of a polyhedron, counter-clockwise: clipped
/// as a polygon in its plane, or a fan if it is not simple.
fn face_triangles(face: &[V3]) -> Vec<[V3; 3]> {
    if face.len() < 3 {
        return Vec::new();
    }
    // Newell's normal.
    let mut n = [0.0; 3];
    for i in 0..face.len() {
        let (a, b) = (face[i], face[(i + 1) % face.len()]);
        n[0] += (a[1] - b[1]) * (a[2] + b[2]);
        n[1] += (a[2] - b[2]) * (a[0] + b[0]);
        n[2] += (a[0] - b[0]) * (a[1] + b[1]);
    }
    let k = (0..3)
        .max_by(|&a, &b| n[a].abs().total_cmp(&n[b].abs()))
        .unwrap_or(2);
    let (u, v) = ((k + 1) % 3, (k + 2) % 3);
    let flat: Vec<V2> = face
        .iter()
        .map(|p| {
            if n[k] >= 0.0 {
                [p[u], p[v]]
            } else {
                [p[v], p[u]]
            }
        })
        .collect();
    match ear_clip(&flat) {
        Some(t) if face.len() > 3 => t.iter().map(|t| t.map(|i| face[i])).collect(),
        _ => (1..face.len() - 1)
            .map(|i| [face[0], face[i], face[i + 1]])
            .collect(),
    }
}

/// Without repeated corners and corners on a straight line.
fn clean(path: &[V2]) -> Vec<V2> {
    let mut p: Vec<V2> = Vec::with_capacity(path.len());
    for q in path {
        if p.last()
            .is_none_or(|l| (l[0] - q[0]).abs() + (l[1] - q[1]).abs() > 1e-12)
        {
            p.push(*q);
        }
    }
    while p.len() > 1 && {
        let (a, b) = (p[0], p[p.len() - 1]);
        (a[0] - b[0]).abs() + (a[1] - b[1]).abs() <= 1e-12
    } {
        p.pop();
    }
    loop {
        let n = p.len();
        if n < 3 {
            return p;
        }
        let straight = (0..n).find(|&i| {
            let (a, b, c) = (p[(i + n - 1) % n], p[i], p[(i + 1) % n]);
            let scale = (b[0] - a[0]).hypot(b[1] - a[1]) * (c[0] - b[0]).hypot(c[1] - b[1]);
            cross2(a, b, c).abs() <= 1e-12 * scale.max(1e-300)
        });
        match straight {
            Some(i) => {
                p.remove(i);
            }
            None => return p,
        }
    }
}

fn same(a: V2, b: V2) -> bool {
    (a[0] - b[0]).abs() + (a[1] - b[1]).abs() <= 1e-12
}

/// The signed area, positive counter-clockwise.
fn area(p: &[V2]) -> f64 {
    (0..p.len())
        .map(|i| cross2([0.0, 0.0], p[i], p[(i + 1) % p.len()]))
        .sum::<f64>()
        / 2.0
}

/// Ear clipping of a simple counter-clockwise polygon.
fn ear_clip(p: &[V2]) -> Option<Vec<[usize; 3]>> {
    let inside = |q: V2, a: V2, b: V2, c: V2| {
        cross2(a, b, q) >= 0.0 && cross2(b, c, q) >= 0.0 && cross2(c, a, q) >= 0.0
    };
    let mut left: Vec<usize> = (0..p.len()).collect();
    let mut out = Vec::with_capacity(p.len());
    while left.len() > 3 {
        let m = left.len();
        let ear = (0..m).find(|&i| {
            let (a, b, c) = (left[(i + m - 1) % m], left[i], left[(i + 1) % m]);
            cross2(p[a], p[b], p[c]) > 0.0
                && !left.iter().any(|&q| {
                    q != a
                        && q != b
                        && q != c
                        && !same(p[q], p[a])
                        && !same(p[q], p[b])
                        && !same(p[q], p[c])
                        && inside(p[q], p[a], p[b], p[c])
                })
        })?;
        let (a, b, c) = (left[(ear + m - 1) % m], left[ear], left[(ear + 1) % m]);
        out.push([a, b, c]);
        left.remove(ear);
    }
    if left.len() == 3 {
        out.push([left[0], left[1], left[2]]);
    }
    Some(out)
}

/// Loops, and triangles of their corners numbered through all of them.
type Cap = (Vec<Vec<V2>>, Vec<[u32; 3]>);

/// The caps of a 2D shape: its loops, cleaned, and triangles of their
/// corners, numbered through all loops in order. Manifold's triangles, as
/// OpenSCAD triangulates; where there is none (in the browser), each
/// outline with the holes it holds bridged into one loop and clipped.
/// `None` if it has none.
fn cap_indexed(r: &Region) -> Option<Cap> {
    let loops: Vec<Vec<V2>> = r
        .loops
        .iter()
        .map(|l| clean(l))
        .filter(|l| l.len() >= 3 && area(l).abs() > 1e-18)
        .collect();
    if let Some(tris) = opensolve_kernel::triangulate_loops(&loops) {
        return (!tris.is_empty()).then_some((loops, tris));
    }
    let (outers, holes): (Vec<&Vec<V2>>, Vec<&Vec<V2>>) = loops.iter().partition(|l| area(l) > 0.0);
    let mut held: Vec<Vec<Vec<V2>>> = vec![Vec::new(); outers.len()];
    for h in holes {
        // A point just inside the hole: right of its first edge.
        let (a, b) = (h[0], h[1]);
        let len = (b[0] - a[0]).hypot(b[1] - a[1]).max(1e-300);
        let off = len * 1e-4;
        let q = [
            (a[0] + b[0]) / 2.0 + (b[1] - a[1]) / len * off,
            (a[1] + b[1]) / 2.0 - (b[0] - a[0]) / len * off,
        ];
        let around = (0..outers.len())
            .filter(|&o| crate::region::contains(outers[o], q))
            .min_by(|&x, &y| area(outers[x]).total_cmp(&area(outers[y])));
        if let Some(o) = around {
            held[o].push(h.clone());
        }
    }
    // The bridged loops repeat corners: back to their numbers by place.
    let mut number: HashMap<[u64; 2], u32, Fx> = HashMap::default();
    for (i, p) in loops.iter().flatten().enumerate() {
        number.entry(p.map(f64::to_bits)).or_insert(i as u32);
    }
    let mut tris = Vec::new();
    for (o, outer) in outers.iter().enumerate() {
        let poly = crate::region::bridge(outer, &held[o]);
        for t in ear_clip(&poly)? {
            tris.push(t.map(|i| number.get(&poly[i].map(f64::to_bits)).copied().unwrap_or(0)));
        }
    }
    (!tris.is_empty()).then_some((loops, tris))
}

/// The convex hull of points in the plane, counter-clockwise (Andrew).
fn hull2(points: &[V2]) -> Vec<V2> {
    let mut p = points.to_vec();
    p.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    p.dedup();
    if p.len() < 3 {
        return p;
    }
    let mut lower: Vec<V2> = Vec::new();
    for q in &p {
        while lower.len() >= 2 && cross2(lower[lower.len() - 2], lower[lower.len() - 1], *q) <= 0.0
        {
            lower.pop();
        }
        lower.push(*q);
    }
    let mut upper: Vec<V2> = Vec::new();
    for q in p.iter().rev() {
        while upper.len() >= 2 && cross2(upper[upper.len() - 2], upper[upper.len() - 1], *q) <= 0.0
        {
            upper.pop();
        }
        upper.push(*q);
    }
    lower.pop();
    upper.pop();
    lower.extend(upper);
    lower
}

/// The triangles of the convex hull of points, outward; `None` if they
/// lie in a plane. Quickhull: each face keeps the points above it, the
/// farthest of them is added next, and the points of the faces it sees go
/// to the new faces or, inside, away; the faces by their edges in a map
/// kept up to date.
fn hull3(points: &[V3]) -> Option<Vec<[V3; 3]>> {
    // One point per place.
    let mut seen: HashSet<[i64; 3], Fx> = HashSet::default();
    let p: Vec<V3> = points
        .iter()
        .copied()
        .filter(|q| seen.insert(q.map(|x| (x * 1e7).round() as i64)))
        .collect();
    if p.len() < 4 {
        return None;
    }
    let size = p
        .iter()
        .flat_map(|q| q.iter())
        .fold(0.0f64, |m, x| m.max(x.abs()))
        .max(1.0);
    let eps = 1e-10 * size;
    // A first tetrahedron of points far apart.
    let a = 0;
    let b = (0..p.len()).max_by(|&i, &j| {
        let d = |k: usize| dot(sub(p[k], p[a]), sub(p[k], p[a]));
        d(i).total_cmp(&d(j))
    })?;
    let line = sub(p[b], p[a]);
    let c = (0..p.len()).max_by(|&i, &j| {
        let d = |k: usize| {
            let x = cross(line, sub(p[k], p[a]));
            dot(x, x)
        };
        d(i).total_cmp(&d(j))
    })?;
    let normal = cross(line, sub(p[c], p[a]));
    if dot(normal, normal).sqrt() < eps * size {
        return None;
    }
    let d = (0..p.len()).max_by(|&i, &j| {
        let h = |k: usize| dot(normal, sub(p[k], p[a])).abs();
        h(i).total_cmp(&h(j))
    })?;
    if dot(normal, sub(p[d], p[a])).abs() / dot(normal, normal).sqrt() < eps {
        return None;
    }
    struct Face {
        n: V3,
        v: [usize; 3],
        above: Vec<usize>,
        alive: bool,
    }
    let plane = |v: [usize; 3]| {
        let n = cross(sub(p[v[1]], p[v[0]]), sub(p[v[2]], p[v[0]]));
        let l = dot(n, n).sqrt().max(1e-300);
        Face {
            n: n.map(|x| x / l),
            v,
            above: Vec::new(),
            alive: true,
        }
    };
    let height = |f: &Face, q: V3| dot(f.n, sub(q, p[f.v[0]]));
    let mut faces: Vec<Face> = Vec::new();
    for v in [[a, b, c], [a, c, d], [a, d, b], [b, d, c]] {
        let mut f = plane(v);
        // Outward: the fourth point below.
        let other = [a, b, c, d]
            .into_iter()
            .find(|i| !v.contains(i))
            .unwrap_or(a);
        if height(&f, p[other]) > 0.0 {
            f = plane([v[0], v[2], v[1]]);
        }
        faces.push(f);
    }
    // Each point to the face it is farthest above.
    let place = |faces: &mut Vec<Face>, among: &[usize], q: usize| {
        let mut best = None;
        let mut most = eps;
        for &k in among {
            let h = height(&faces[k], p[q]);
            if h > most {
                most = h;
                best = Some(k);
            }
        }
        if let Some(k) = best {
            faces[k].above.push(q);
        }
    };
    let first: Vec<usize> = (0..4).collect();
    for q in 0..p.len() {
        if ![a, b, c, d].contains(&q) {
            place(&mut faces, &first, q);
        }
    }
    let mut by_edge: HashMap<(usize, usize), usize, Fx> = HashMap::default();
    for (k, f) in faces.iter().enumerate() {
        for e in 0..3 {
            by_edge.insert((f.v[e], f.v[(e + 1) % 3]), k);
        }
    }
    let mut open: Vec<usize> = (0..4).filter(|&k| !faces[k].above.is_empty()).collect();
    let mut visible: Vec<usize> = Vec::new();
    let mut mark: Vec<bool> = vec![false; 4];
    while let Some(top) = open.pop() {
        if !faces[top].alive || faces[top].above.is_empty() {
            continue;
        }
        // The point farthest above the face.
        let eye = *faces[top]
            .above
            .iter()
            .max_by(|&&x, &&y| height(&faces[top], p[x]).total_cmp(&height(&faces[top], p[y])))?;
        // The faces it sees, connected to this one: their border is one
        // loop.
        visible.clear();
        visible.push(top);
        mark.resize(faces.len(), false);
        mark[top] = true;
        let mut i = 0;
        while i < visible.len() {
            let f = faces[visible[i]].v;
            i += 1;
            for e in 0..3 {
                if let Some(&n) = by_edge.get(&(f[(e + 1) % 3], f[e]))
                    && !mark[n]
                    && height(&faces[n], p[eye]) > eps
                {
                    mark[n] = true;
                    visible.push(n);
                }
            }
        }
        let mut horizon = Vec::new();
        let mut orphans = Vec::new();
        for &k in &visible {
            let f = faces[k].v;
            for e in 0..3 {
                let (x, y) = (f[e], f[(e + 1) % 3]);
                if by_edge.get(&(y, x)).is_none_or(|&n| !mark[n]) {
                    horizon.push((x, y));
                }
            }
            faces[k].alive = false;
            orphans.append(&mut faces[k].above);
        }
        for &k in &visible {
            let f = faces[k].v;
            for e in 0..3 {
                by_edge.remove(&(f[e], f[(e + 1) % 3]));
            }
            mark[k] = false;
        }
        let start = faces.len();
        for (x, y) in horizon {
            let k = faces.len();
            faces.push(plane([x, y, eye]));
            for (u, w) in [(x, y), (y, eye), (eye, x)] {
                by_edge.insert((u, w), k);
            }
        }
        let new: Vec<usize> = (start..faces.len()).collect();
        for q in orphans {
            if q != eye {
                place(&mut faces, &new, q);
            }
        }
        open.extend(new.into_iter().filter(|&k| !faces[k].above.is_empty()));
    }
    Some(
        faces
            .iter()
            .filter(|f| f.alive)
            .map(|f| f.v.map(|i| p[i]))
            .collect(),
    )
}

/// The surface of a solid in convex pieces, each its corners: triangles of
/// one plane joined across their edges while what they make stays convex.
/// What a Minkowski sum takes piece by piece: a hull for each.
fn convex_patches(s: &Solid) -> Vec<Vec<V3>> {
    let tri = |t: usize| s.tris[t].map(|i| s.points[i as usize]);
    let planes: Vec<(V3, f64)> = (0..s.tris.len())
        .map(|t| {
            let [a, b, c] = tri(t);
            let n = cross(sub(b, a), sub(c, a));
            let l = dot(n, n).sqrt().max(1e-300);
            let n = n.map(|x| x / l);
            (n, dot(n, a))
        })
        .collect();
    let same = |x: usize, y: usize| {
        let ((n, d), (m, e)) = (planes[x], planes[y]);
        dot(n, m) > 1.0 - 1e-9 && (d - e).abs() < 1e-7
    };
    let key = |p: V3| p.map(f64::to_bits);
    let mut by_edge: HashMap<([u64; 3], [u64; 3]), usize, Fx> = HashMap::default();
    for t in 0..s.tris.len() {
        let c = tri(t);
        for e in 0..3 {
            by_edge.insert((key(c[e]), key(c[(e + 1) % 3])), t);
        }
    }
    let mut done = vec![false; s.tris.len()];
    let mut out = Vec::new();
    for start in 0..s.tris.len() {
        if done[start] {
            continue;
        }
        done[start] = true;
        let (n, _) = planes[start];
        // In the plane: the coordinates but the one the normal is most along.
        let k = (0..3)
            .max_by(|&a, &b| n[a].abs().total_cmp(&n[b].abs()))
            .unwrap_or(2);
        let (u, v) = ((k + 1) % 3, (k + 2) % 3);
        let flat = |p: V3| [p[u], p[v]];
        let mut corners: Vec<V3> = tri(start).to_vec();
        let mut area = cross2(flat(corners[0]), flat(corners[1]), flat(corners[2])).abs() / 2.0;
        let mut stack = vec![start];
        while let Some(t) = stack.pop() {
            let c = tri(t);
            for e in 0..3 {
                let Some(&m) = by_edge.get(&(key(c[(e + 1) % 3]), key(c[e]))) else {
                    continue;
                };
                if done[m] || !same(start, m) {
                    continue;
                }
                let d = tri(m);
                let more = cross2(flat(d[0]), flat(d[1]), flat(d[2])).abs() / 2.0;
                let mut with = corners.clone();
                with.extend(d.iter().filter(|p| !corners.contains(p)));
                let pts: Vec<V2> = with.iter().map(|p| flat(*p)).collect();
                let hull = area_of(&hull2(&pts)).abs();
                if (hull - (area + more)).abs() <= 1e-9 * hull.max(1e-12) + 1e-12 {
                    done[m] = true;
                    corners = with;
                    area += more;
                    stack.push(m);
                }
            }
        }
        out.push(corners);
    }
    out
}

/// The most convex pieces a `minkowski` of shapes that are not convex is
/// put together from: each is one boolean more.
const MOST_PIECES: usize = 20_000;

impl Builder {
    /// The solid's corners, each once.
    fn corners(s: &Solid) -> Vec<V3> {
        let mut seen = HashSet::new();
        s.points
            .iter()
            .copied()
            .filter(|p| seen.insert(p.map(f64::to_bits)))
            .collect()
    }

    /// Whether a solid is convex: its hull is no larger.
    fn is_convex(s: &Solid) -> bool {
        let Some(tris) = hull3(&s.points) else {
            return false;
        };
        let hull: f64 = tris
            .iter()
            .map(|[a, b, c]| dot(*a, cross(*b, *c)))
            .sum::<f64>()
            / 6.0;
        let v = s.volume();
        // Corners lie on the kernel's grid: a little more is still convex.
        (hull - v).abs() <= 1e-7 * hull.abs().max(1.0)
    }

    /// Several solids as one, joined as [`Builder::union_many`] joins
    /// them; what fails to join is put together as it is.
    pub fn union_all(&mut self, list: Vec<Solid>) -> Result<Option<Solid>, String> {
        Ok(self.union_many(list).0)
    }

    /// Several solids as one. Those apart from all others (their surfaces
    /// do not meet, neither holds the other) are only put together; those
    /// that meet are joined in pairs, the pairs again and so on, the pairs
    /// of a round at once. A pair that fails to join is put together as it
    /// is, its error returned.
    pub fn union_many(&mut self, list: Vec<Solid>) -> (Option<Solid>, Vec<String>) {
        if list.len() <= 1 {
            return (list.into_iter().next(), Vec::new());
        }
        // All at once where the kernel's library can, as OpenSCAD does.
        let refs: Vec<&Solid> = list.iter().collect();
        if let Some(Ok(s)) = self.batch(&refs, Op::Add) {
            return ((!s.is_empty()).then_some(s), Vec::new());
        }
        let meeting = {
            let refs: Vec<&Solid> = list.iter().collect();
            opensolve_kernel::meeting(&refs)
        };
        let mut parent: Vec<usize> = (0..list.len()).collect();
        fn root(p: &mut [usize], mut i: usize) -> usize {
            while p[i] != i {
                p[i] = p[p[i]];
                i = p[i];
            }
            i
        }
        for (a, b) in meeting {
            let (a, b) = (root(&mut parent, a), root(&mut parent, b));
            parent[a.max(b)] = a.min(b);
        }
        let mut groups: Vec<Vec<Solid>> = Vec::new();
        let mut group_of: HashMap<usize, usize> = HashMap::new();
        for (i, s) in list.into_iter().enumerate() {
            let r = root(&mut parent, i);
            let g = *group_of.entry(r).or_insert_with(|| {
                groups.push(Vec::new());
                groups.len() - 1
            });
            groups[g].push(s);
        }
        let mut errors = Vec::new();
        while groups.iter().any(|g| g.len() > 1) {
            // Each group's list of the next round: a pair's result, by the
            // pair's number, or one left over, in their order.
            let mut jobs: Vec<(Solid, Solid)> = Vec::new();
            let mut slots: Vec<Vec<Result<usize, Solid>>> = Vec::with_capacity(groups.len());
            for mut list in groups {
                // The small ones first, as Manifold batches them: a large
                // mesh is gone through once, at the end, not every round.
                list.sort_by_key(|s| s.tris.len());
                let mut next = Vec::with_capacity(list.len().div_ceil(2));
                let mut it = list.into_iter();
                while let Some(a) = it.next() {
                    match it.next() {
                        Some(b) => {
                            next.push(Ok(jobs.len()));
                            jobs.push((a, b));
                        }
                        None => next.push(Err(a)),
                    }
                }
                slots.push(next);
            }
            let pairs: Vec<(&Solid, &Solid)> = jobs.iter().map(|(a, b)| (a, b)).collect();
            let mut made: Vec<Option<Solid>> = self
                .booleans(&pairs, Op::Add)
                .into_iter()
                .zip(&jobs)
                .map(|(r, (a, b))| {
                    Some(r.unwrap_or_else(|e| {
                        errors.push(e);
                        Solid::merged(&[a.clone(), b.clone()])
                    }))
                })
                .collect();
            groups = slots
                .into_iter()
                .map(|g| {
                    g.into_iter()
                        .map(|s| match s {
                            Ok(k) => made[k].take().unwrap_or_default(),
                            Err(s) => s,
                        })
                        .collect()
                })
                .collect();
        }
        let mut parts: Vec<Solid> = groups.into_iter().flatten().collect();
        let out = if parts.len() == 1 {
            parts.pop()
        } else {
            Some(Solid::merged(&parts))
        };
        (out, errors)
    }

    /// The Minkowski sum of two solids: the hull of the sums of their
    /// corners when both are convex; else the union of such hulls of each
    /// convex piece of one's faces (see `convex_patches`) with the other
    /// (or its pieces), with the inside of both, joined in rounds of pairs
    /// at once.
    pub fn minkowski(&mut self, a: &Solid, b: &Solid) -> Result<Option<Solid>, String> {
        let (ca, cb) = (Self::is_convex(a), Self::is_convex(b));
        let (pa, pb) = (Self::corners(a), Self::corners(b));
        let sums = |p: &[V3], q: &[V3]| -> Vec<V3> {
            p.iter()
                .flat_map(|x| {
                    q.iter()
                        .map(move |y| [x[0] + y[0], x[1] + y[1], x[2] + y[2]])
                })
                .collect()
        };
        if ca && cb {
            return Ok(self.hull(&sums(&pa, &pb)));
        }
        // The one that is not convex goes by convex pieces of its faces.
        let (big, small, cs) = if ca { (b, a, ca) } else { (a, b, cb) };
        let big_pieces = convex_patches(big);
        let small_pieces = if cs {
            vec![Self::corners(small)]
        } else {
            convex_patches(small)
        };
        if big_pieces.len() * small_pieces.len() > MOST_PIECES {
            return Err(format!(
                "minkowski: {} triangles with {} pieces are too many",
                big_pieces.len(),
                small_pieces.len()
            ));
        }
        let mut parts = Vec::new();
        for p in &big_pieces {
            for q in &small_pieces {
                if let Some(h) = self.hull(&sums(p, q)) {
                    parts.push(h);
                }
            }
        }
        // The insides: each moved by a corner of the other.
        let shift = |m: &mut Self, s: &Solid, by: V3| {
            m.transformed(
                s,
                &[
                    [1.0, 0.0, 0.0, by[0]],
                    [0.0, 1.0, 0.0, by[1]],
                    [0.0, 0.0, 1.0, by[2]],
                ],
            )
        };
        if let (Some(&x), Some(&y)) = (pb.first(), pa.first()) {
            parts.push(shift(self, big, x));
            parts.push(shift(self, small, y));
        }
        self.union_all(parts)
    }

    /// The Minkowski sum of two 2D shapes, as OpenSCAD's (see
    /// [`flat::minkowski`]).
    pub fn minkowski_2d(&mut self, a: &Region, b: &Region) -> Option<Region> {
        let r = flat::minkowski(&[a, b]);
        (!r.is_empty()).then_some(r)
    }
}

/// The area of a polygon, either way round.
fn area_of(p: &[V2]) -> f64 {
    area(p).abs()
}

impl Builder {
    /// OpenSCAD's `projection()`: the shadow of a solid on the xy plane,
    /// the union of what its faces that look up cover, by the outlines of
    /// the patches of them that hang together; with `cut`, its section at
    /// z = 0.
    pub fn projection(&mut self, s: &Solid, cut: bool) -> Result<Option<Region>, String> {
        if cut {
            return self.section(s);
        }
        let up: Vec<usize> = (0..s.tris.len())
            .filter(|&k| {
                let [a, b, c] = s.tris[k].map(|i| s.points[i as usize]);
                cross(sub(b, a), sub(c, a))[2] > 1e-12
            })
            .collect();
        // Patches of faces that share edges.
        let mut by_edge: HashMap<(u32, u32), usize> = HashMap::new();
        for (j, &k) in up.iter().enumerate() {
            let t = s.tris[k];
            for e in 0..3 {
                by_edge.insert((t[e], t[(e + 1) % 3]), j);
            }
        }
        let mut patch = vec![usize::MAX; up.len()];
        let mut patches = 0;
        for start in 0..up.len() {
            if patch[start] != usize::MAX {
                continue;
            }
            patch[start] = patches;
            let mut stack = vec![start];
            while let Some(j) = stack.pop() {
                let t = s.tris[up[j]];
                for e in 0..3 {
                    if let Some(&n) = by_edge.get(&(t[(e + 1) % 3], t[e]))
                        && patch[n] == usize::MAX
                    {
                        patch[n] = patches;
                        stack.push(n);
                    }
                }
            }
            patches += 1;
        }
        let flat = |i: u32| {
            let p = s.points[i as usize];
            [p[0], p[1]]
        };
        // Each outline the way round its faces see it: by the non-zero rule,
        // what any face covers, also where a patch folds over itself.
        let mut paths: Vec<Vec<V2>> = Vec::new();
        for p in 0..patches {
            // The outline of a patch: its edges not shared within it.
            let edges: Vec<(u32, u32)> = (0..up.len())
                .filter(|&j| patch[j] == p)
                .flat_map(|j| {
                    let t = s.tris[up[j]];
                    (0..3).map(move |e| (t[e], t[(e + 1) % 3]))
                })
                .filter(|&(a, b)| by_edge.get(&(b, a)).is_none_or(|&n| patch[n] != p))
                .collect();
            paths.extend(
                chain(&edges)
                    .iter()
                    .map(|l| l.iter().map(|&i| flat(i)).collect::<Vec<V2>>()),
            );
        }
        let r = flat::non_zero(&paths);
        Ok((!r.is_empty()).then_some(r))
    }

    /// The section of a solid at z = 0.
    fn section(&mut self, s: &Solid) -> Result<Option<Region>, String> {
        // Where an edge crosses the plane, the same point from either
        // triangle: from its lower end.
        let at = |a: V3, b: V3| -> V2 {
            let (a, b) = if (a[2], a[0], a[1]) < (b[2], b[0], b[1]) {
                (a, b)
            } else {
                (b, a)
            };
            let t = -a[2] / (b[2] - a[2]);
            [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
        };
        let key = |p: V2| p.map(|x| (x * 1e7).round() as i64);
        let mut ids: HashMap<[i64; 2], u32> = HashMap::new();
        let mut points: Vec<V2> = Vec::new();
        let mut id = |p: V2| -> u32 {
            *ids.entry(key(p)).or_insert_with(|| {
                points.push(p);
                points.len() as u32 - 1
            })
        };
        let mut edges = Vec::new();
        for t in &s.tris {
            let v = t.map(|i| s.points[i as usize]);
            let above: Vec<bool> = v.iter().map(|p| p[2] >= 0.0).collect();
            let crossing: Vec<V2> = (0..3)
                .filter(|&e| above[e] != above[(e + 1) % 3])
                .map(|e| at(v[e], v[(e + 1) % 3]))
                .collect();
            if crossing.len() != 2 {
                continue;
            }
            // The shape on the left: the outward normal on the right.
            let n = cross(sub(v[1], v[0]), sub(v[2], v[0]));
            let d = [-n[1], n[0]];
            let (p, q) = (crossing[0], crossing[1]);
            let along = (q[0] - p[0]) * d[0] + (q[1] - p[1]) * d[1];
            let (p, q) = if along >= 0.0 { (p, q) } else { (q, p) };
            let (i, j) = (id(p), id(q));
            if i != j {
                edges.push((i, j));
            }
        }
        let paths: Vec<Vec<V2>> = chain(&edges)
            .iter()
            .map(|l| l.iter().map(|&i| points[i as usize]).collect())
            .collect();
        Ok(self.polygon(&paths))
    }
}

/// Directed edges chained into loops, each edge once.
fn chain(edges: &[(u32, u32)]) -> Vec<Vec<u32>> {
    let mut from: HashMap<u32, Vec<u32>> = HashMap::new();
    for &(a, b) in edges.iter().rev() {
        from.entry(a).or_default().push(b);
    }
    let mut loops = Vec::new();
    for &(start, _) in edges {
        while from.get(&start).is_some_and(|v| !v.is_empty()) {
            let mut ring = vec![start];
            let mut at = start;
            while let Some(next) = from.get_mut(&at).and_then(Vec::pop) {
                if next == start {
                    break;
                }
                ring.push(next);
                at = next;
            }
            if ring.len() >= 3 {
                loops.push(ring);
            }
        }
    }
    loops
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fragments_as_openscad_counts_them() {
        // $fn wins; else $fa and $fs, at least 5.
        assert_eq!(fragments(10.0, 0.0, 2.0, 12.0), 30);
        assert_eq!(fragments(1.0, 0.0, 2.0, 12.0), 5);
        assert_eq!(fragments(5.0, 0.0, 2.0, 12.0), 16);
        assert_eq!(fragments(5.0, 7.0, 2.0, 12.0), 7);
        assert_eq!(fragments(5.0, 2.0, 2.0, 12.0), 3);
        assert_eq!(fragments(0.0, 0.0, 2.0, 12.0), 3);
    }

    #[test]
    fn degrees_are_exact_where_openscad_is() {
        let exact = |a: f64, b: f64| (a - b).abs() < f64::MIN_POSITIVE;
        assert!(exact(sin_deg(30.0), 0.5));
        assert!(exact(cos_deg(90.0), 0.0));
        assert!(exact(sin_deg(180.0), 0.0));
        assert!(exact(cos_deg(-120.0), -0.5));
        assert!((sin_deg(10.0) - 10f64.to_radians().sin()).abs() < 1e-15);
    }

    #[test]
    fn a_cylinder_has_the_volume_of_its_polygon() {
        let mut b = Builder::default();
        let s = b.cylinder(10.0, 5.0, 5.0, false, 12).unwrap();
        assert!(s.is_closed());
        let polygon = 12.0 / 2.0 * 25.0 * (std::f64::consts::TAU / 12.0).sin();
        // Corners are kept on the kernel's grid of 0.1 µm.
        assert!((s.volume() - polygon * 10.0).abs() < 1e-4);
    }

    #[test]
    fn a_sphere_is_closed() {
        let mut b = Builder::default();
        for n in [3, 5, 8, 30] {
            let s = b.sphere(3.0, n).unwrap();
            assert!(s.is_closed(), "{n}");
            assert!(s.volume() > 0.0);
        }
    }

    #[test]
    fn the_hull_of_a_cube_and_a_point() {
        let mut p: Vec<V3> = (0..8)
            .map(|i| std::array::from_fn(|k| f64::from((i >> k) & 1)))
            .collect();
        p.push([0.5, 0.5, 0.5]);
        p.push([0.5, 0.5, 2.0]);
        let mut b = Builder::default();
        let s = b.hull(&p).unwrap();
        assert!(s.is_closed());
        assert!(
            (s.volume() - (1.0 + 1.0 / 3.0)).abs() < 1e-9,
            "{}",
            s.volume()
        );
    }

    #[test]
    fn polygon_paths_cut_holes() {
        let mut b = Builder::default();
        let outer = vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]];
        let hole = vec![[2.0, 2.0], [2.0, 4.0], [4.0, 4.0], [4.0, 2.0]];
        let r = b.polygon(&[outer, hole]).unwrap();
        assert!((r.area() - 96.0).abs() < 1e-6, "{}", r.area());
        assert_eq!(r.loops.len(), 2);
        let (loops, tris) = cap_indexed(&r).unwrap();
        let corners: Vec<V2> = loops.concat();
        let a: f64 = tris
            .iter()
            .map(|t| {
                let [p, q, w] = t.map(|i| corners[i as usize]);
                cross2(p, q, w) / 2.0
            })
            .sum();
        assert!((a - 96.0).abs() < 1e-6);
        let s = b
            .linear_extrude(&r, 2.0, false, 0.0, 1, [1.0, 1.0])
            .unwrap();
        assert!(s.is_closed());
        assert!((s.volume() - 192.0).abs() < 1e-6, "{}", s.volume());
    }

    #[test]
    fn the_shadow_of_a_cylinder_is_its_circle() {
        let mut b = Builder::default();
        let c = b.cylinder(10.0, 5.0, 5.0, false, 12).unwrap();
        let r = b.projection(&c, false).unwrap().unwrap();
        let polygon = 12.0 / 2.0 * 25.0 * (std::f64::consts::TAU / 12.0).sin();
        assert!((r.area() - polygon).abs() < 1e-4, "{}", r.area());
        let r = b.projection(&c, true).unwrap();
        assert!(r.is_none());
    }
}
