//! OpenSCAD's shapes on the mesh kernel. Circles, spheres and cylinders are
//! cut into as many pieces as OpenSCAD cuts them, at the same places, so a
//! part comes out as OpenSCAD makes it. A 2D shape is kept as a slab one
//! unit thick (z from 0 to 1): its booleans are the kernel's, and its top
//! is the shape.

use std::collections::{HashMap, HashSet};

use opensolve_kernel::{FaceId, Facets, Op, Solid};

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
    /// A slab from z 0 to 1, its top the shape.
    D2(Solid),
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
}

impl Builder {
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

    /// A slab of polygons, the even-odd rule deciding what is inside, as in
    /// OpenSCAD's `polygon` with several paths.
    pub fn polygon(&mut self, paths: &[Vec<V2>]) -> Result<Option<Solid>, String> {
        let mut out: Option<Solid> = None;
        for path in paths {
            let Some(s) = self.simple_polygon(path)? else {
                continue;
            };
            out = Some(match out {
                None => s,
                Some(o) => {
                    let a = self.boolean(&o, &s, Op::Subtract, true)?;
                    let b = self.boolean(&s, &o, Op::Subtract, true)?;
                    self.boolean(&a, &b, Op::Add, true)?
                }
            });
        }
        Ok(out.filter(|s| !s.is_empty()))
    }

    /// The slab of one simple polygon, either way round.
    fn simple_polygon(&mut self, path: &[V2]) -> Result<Option<Solid>, String> {
        let mut poly = clean(path);
        if poly.len() < 3 {
            return Ok(None);
        }
        if area(&poly) < 0.0 {
            poly.reverse();
        }
        if area(&poly).abs() < 1e-12 {
            return Ok(None);
        }
        let ears = ear_clip(&poly).ok_or("polygon: a path crosses itself")?;
        let tris: Vec<[V2; 3]> = ears.iter().map(|t| t.map(|i| poly[i])).collect();
        let edges: Vec<[V2; 2]> = (0..poly.len())
            .map(|i| [poly[i], poly[(i + 1) % poly.len()]])
            .collect();
        Ok(Some(self.slab(&tris, &edges)))
    }

    /// A slab from its top's triangles and the edges round them.
    fn slab(&mut self, tris: &[[V2; 3]], edges: &[[V2; 2]]) -> Solid {
        let mut soup = Vec::new();
        let at = |p: V2, z: f64| [p[0], p[1], z];
        for t in tris {
            soup.push([at(t[0], 0.0), at(t[2], 0.0), at(t[1], 0.0)]);
            soup.push([at(t[0], 1.0), at(t[1], 1.0), at(t[2], 1.0)]);
        }
        for [a, b] in edges {
            soup.push([at(*a, 0.0), at(*b, 0.0), at(*b, 1.0)]);
            soup.push([at(*a, 0.0), at(*b, 1.0), at(*a, 1.0)]);
        }
        self.solid(&soup)
    }

    pub fn square(&mut self, size: V2, center: bool) -> Option<Solid> {
        if size.iter().any(|s| *s <= 0.0 || !s.is_finite()) {
            return None;
        }
        let o = if center {
            size.map(|s| -s / 2.0)
        } else {
            [0.0; 2]
        };
        let (x0, y0, x1, y1) = (o[0], o[1], o[0] + size[0], o[1] + size[1]);
        self.simple_polygon(&[[x0, y0], [x1, y0], [x1, y1], [x0, y1]])
            .ok()
            .flatten()
    }

    pub fn circle(&mut self, r: f64, n: usize) -> Option<Solid> {
        if r <= 0.0 || !r.is_finite() {
            return None;
        }
        self.simple_polygon(&circle(r, n)).ok().flatten()
    }

    /// `a op b`; with `flat`, of two slabs: what is cut away or kept by `b`
    /// is made taller first, so that no faces lie on each other.
    pub fn boolean(&mut self, a: &Solid, b: &Solid, op: Op, flat: bool) -> Result<Solid, String> {
        let b = if flat && op != Op::Add {
            let tall = [
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 3.0, -1.0],
            ];
            self.transformed(b, &tall)
        } else {
            b.clone()
        };
        // Apart, a union is both as they are.
        if op == Op::Add
            && let (Some(p), Some(q)) = (a.bounds(), b.bounds())
            && (0..3).any(|i| p.1[i] < q.0[i] - 1e-9 || q.1[i] < p.0[i] - 1e-9)
        {
            return Ok(Solid::merged(&[a.clone(), b]));
        }
        a.boolean(&b, op, &mut self.facets)
            .map_err(|e| e.to_string())
    }

    /// The solid mapped by `m`; a mirror turns its triangles round.
    pub fn transformed(&mut self, s: &Solid, m: &Matrix) -> Solid {
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

    /// A slab mapped by the 2D part of `m`; z stays.
    pub fn transformed_2d(&mut self, s: &Solid, m: &Matrix) -> Solid {
        let flat = [
            [m[0][0], m[0][1], 0.0, m[0][3]],
            [m[1][0], m[1][1], 0.0, m[1][3]],
            [0.0, 0.0, 1.0, 0.0],
        ];
        self.transformed(s, &flat)
    }

    /// The convex hull of points; `None` if they span no volume.
    pub fn hull(&mut self, points: &[V3]) -> Option<Solid> {
        let tris = hull3(points)?;
        Some(self.solid(&tris))
    }

    /// The convex hull of points in the plane, as a slab.
    pub fn hull_2d(&mut self, points: &[V2]) -> Option<Solid> {
        let h = hull2(points);
        self.simple_polygon(&h).ok().flatten()
    }

    /// OpenSCAD's `linear_extrude`: the shape at `slices + 1` heights, each
    /// turned by its part of `twist` (clockwise seen from above, as OpenSCAD
    /// turns it) and scaled by its part of `scale`.
    pub fn linear_extrude(
        &mut self,
        slab: &Solid,
        height: f64,
        center: bool,
        twist: f64,
        slices: usize,
        scale: V2,
    ) -> Option<Solid> {
        if height <= 0.0 || !height.is_finite() {
            return None;
        }
        let (tris, loops) = region(slab);
        if tris.is_empty() {
            return None;
        }
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
        let mut soup = Vec::new();
        for t in &tris {
            soup.push([layer(0, t[0]), layer(0, t[2]), layer(0, t[1])]);
            soup.push([layer(n, t[0]), layer(n, t[1]), layer(n, t[2])]);
        }
        // Twisted, a side is no plane: split as OpenSCAD splits it, by the
        // way it turns, the other way in holes.
        let turns_back = sin_deg(-twist / n as f64) >= 0.0;
        for (a, b, ring) in edges_of(&loops) {
            let hole = area(ring) < 0.0;
            for k in 0..n {
                let (a0, b0, a1, b1) = (layer(k, a), layer(k, b), layer(k + 1, a), layer(k + 1, b));
                if turns_back != hole {
                    soup.push([a0, b0, b1]);
                    soup.push([a0, b1, a1]);
                } else {
                    soup.push([a0, b0, a1]);
                    soup.push([a1, b0, b1]);
                }
            }
        }
        Some(self.solid(&soup))
    }

    /// OpenSCAD's `rotate_extrude`: the shape, in the XZ plane, turned about
    /// Z in `n` steps; a whole turn starts on −X, as OpenSCAD's does, and
    /// part of one on +X.
    pub fn rotate_extrude(
        &mut self,
        slab: &Solid,
        angle: f64,
        n: usize,
    ) -> Result<Option<Solid>, String> {
        let (mut tris, mut loops) = region(slab);
        if tris.is_empty() {
            return Ok(None);
        }
        let xs = tris.iter().flatten().map(|p| p[0]);
        let (lo, hi) = xs.fold((f64::MAX, f64::MIN), |(l, h), x| (l.min(x), h.max(x)));
        if lo < -1e-9 && hi > 1e-9 {
            return Err("rotate_extrude: the shape lies on both sides of the Y axis".into());
        }
        if hi <= 1e-9 {
            // All on the left: as OpenSCAD does, the mirror image.
            for t in &mut tris {
                *t = [t[0], t[2], t[1]].map(|p| [-p[0], p[1]]);
            }
            for l in &mut loops {
                l.reverse();
                for p in l.iter_mut() {
                    p[0] = -p[0];
                }
            }
        }
        let whole = angle.abs() >= 360.0;
        let n = n.max(1);
        let at = |j: usize, p: V2| -> V3 {
            let a = if whole {
                -90.0 + (j % n) as f64 * 360.0 / n as f64
            } else {
                90.0 - j as f64 * angle / n as f64
            };
            let x = p[0].max(0.0);
            [x * sin_deg(a), x * cos_deg(a), p[1]]
        };
        let mut soup = Vec::new();
        if !whole {
            for t in &tris {
                soup.push([at(0, t[0]), at(0, t[2]), at(0, t[1])]);
                soup.push([at(n, t[0]), at(n, t[1]), at(n, t[2])]);
            }
        }
        for (a, b, _) in edges_of(&loops) {
            for j in 0..n {
                let (a0, b0, a1, b1) = (at(j, a), at(j, b), at(j + 1, a), at(j + 1, b));
                soup.push([a0, b0, b1]);
                soup.push([a0, b1, a1]);
            }
        }
        Ok(Some(self.outward(soup)))
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

/// A ring closed by a fan from its first corner, seen from above if `up`.
fn fan(tris: &mut Vec<[V3; 3]>, ring: &[V3], up: bool) {
    for i in 1..ring.len().saturating_sub(1) {
        let (a, b, c) = (ring[0], ring[i], ring[i + 1]);
        tris.push(if up { [a, b, c] } else { [a, c, b] });
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

/// A slab's top: its triangles and the outlines round them, outer ones
/// counter-clockwise and holes clockwise.
pub fn region(slab: &Solid) -> (Vec<[V2; 3]>, Vec<Vec<V2>>) {
    let top: Vec<[u32; 3]> = slab
        .tris
        .iter()
        .copied()
        .filter(|t| {
            let [a, b, c] = t.map(|i| slab.points[i as usize]);
            [a, b, c].iter().all(|p| (p[2] - 1.0).abs() < 1e-6)
                && cross(sub(b, a), sub(c, a))[2] > 0.0
        })
        .collect();
    let mut edges: HashSet<(u32, u32)> = HashSet::new();
    for t in &top {
        for k in 0..3 {
            edges.insert((t[k], t[(k + 1) % 3]));
        }
    }
    let flat = |i: u32| {
        let p = slab.points[i as usize];
        [p[0], p[1]]
    };
    let tris = top.iter().map(|t| t.map(flat)).collect();
    let mut open: Vec<(u32, u32)> = edges
        .iter()
        .filter(|(a, b)| !edges.contains(&(*b, *a)))
        .copied()
        .collect();
    open.sort_unstable();
    // Chained into loops, each edge once.
    let mut from: HashMap<u32, Vec<u32>> = HashMap::new();
    for &(a, b) in open.iter().rev() {
        from.entry(a).or_default().push(b);
    }
    let mut loops = Vec::new();
    for &(start, _) in &open {
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
            loops.push(ring.into_iter().map(flat).collect());
        }
    }
    (tris, loops)
}

/// The edges of loops, each from a corner to the next, with its loop.
fn edges_of(loops: &[Vec<V2>]) -> impl Iterator<Item = (V2, V2, &Vec<V2>)> {
    loops
        .iter()
        .flat_map(|l| (0..l.len()).map(move |i| (l[i], l[(i + 1) % l.len()], l)))
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

/// The triangles of the convex hull of points, outward, by adding one
/// point after the other; `None` if they lie in a plane.
fn hull3(points: &[V3]) -> Option<Vec<[V3; 3]>> {
    // One point per place.
    let mut seen = HashSet::new();
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
    let plane = |f: [usize; 3]| {
        let n = cross(sub(p[f[1]], p[f[0]]), sub(p[f[2]], p[f[0]]));
        let l = dot(n, n).sqrt().max(1e-300);
        (n.map(|x| x / l), f)
    };
    let height = |f: &(V3, [usize; 3]), q: V3| dot(f.0, sub(q, p[f.1[0]]));
    if dot(normal, sub(p[d], p[a])).abs() / dot(normal, normal).sqrt() < eps {
        return None;
    }
    let mut faces: Vec<(V3, [usize; 3])> = Vec::new();
    for f in [[a, b, c], [a, c, d], [a, d, b], [b, d, c]] {
        let mut x = plane(f);
        // Outward: the fourth point below.
        let other = [a, b, c, d]
            .into_iter()
            .find(|i| !f.contains(i))
            .unwrap_or(a);
        if height(&x, p[other]) > 0.0 {
            x = plane([f[0], f[2], f[1]]);
        }
        faces.push(x);
    }
    // The points farthest out first: what lies near the hull's faces then
    // seldom makes faces that a later point takes away again.
    let mid: V3 = std::array::from_fn(|k| (p[a][k] + p[b][k] + p[c][k] + p[d][k]) / 4.0);
    let mut order: Vec<usize> = (0..p.len()).filter(|i| ![a, b, c, d].contains(i)).collect();
    let far = |i: usize| dot(sub(p[i], mid), sub(p[i], mid));
    order.sort_by(|&i, &j| far(j).total_cmp(&far(i)));
    for i in order {
        let q = p[i];
        let heights: Vec<f64> = faces.iter().map(|f| height(f, q)).collect();
        let Some(top) = (0..faces.len()).max_by(|&x, &y| heights[x].total_cmp(&heights[y])) else {
            break;
        };
        if heights[top] <= eps {
            continue;
        }
        // The faces the point sees, connected to the one it sees best:
        // their border is one loop.
        let mut by_edge: HashMap<(usize, usize), usize> = HashMap::with_capacity(faces.len() * 3);
        for (k, f) in faces.iter().enumerate() {
            for e in 0..3 {
                by_edge.insert((f.1[e], f.1[(e + 1) % 3]), k);
            }
        }
        let mut visible = vec![false; faces.len()];
        visible[top] = true;
        let mut stack = vec![top];
        while let Some(k) = stack.pop() {
            let f = faces[k].1;
            for e in 0..3 {
                if let Some(&n) = by_edge.get(&(f[(e + 1) % 3], f[e]))
                    && !visible[n]
                    && heights[n] > eps
                {
                    visible[n] = true;
                    stack.push(n);
                }
            }
        }
        let mut next: Vec<(V3, [usize; 3])> = Vec::with_capacity(faces.len() + 8);
        let mut cone = Vec::new();
        for (k, f) in faces.iter().enumerate() {
            if !visible[k] {
                next.push(*f);
                continue;
            }
            for e in 0..3 {
                let (x, y) = (f.1[e], f.1[(e + 1) % 3]);
                if by_edge.get(&(y, x)).is_none_or(|&n| !visible[n]) {
                    cone.push(plane([x, y, i]));
                }
            }
        }
        next.extend(cone);
        faces = next;
    }
    Some(faces.iter().map(|f| f.1.map(|i| p[i])).collect())
}

/// How `offset` joins the offset edges at a corner that grows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Join {
    /// An arc of so many pieces a whole turn (`r`).
    Round(usize),
    /// The edges lengthened until they meet (`delta`).
    Miter,
    /// The corner cut off (`delta`, `chamfer = true`).
    Chamfer,
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

    /// Several solids as one, joined in pairs and the pairs again.
    pub fn union_all(&mut self, mut list: Vec<Solid>, flat: bool) -> Result<Option<Solid>, String> {
        while list.len() > 1 {
            let mut next = Vec::with_capacity(list.len().div_ceil(2));
            let mut it = list.into_iter();
            while let Some(a) = it.next() {
                next.push(match it.next() {
                    Some(b) => self.boolean(&a, &b, Op::Add, flat)?,
                    None => a,
                });
            }
            list = next;
        }
        Ok(list.pop())
    }

    /// The Minkowski sum of two solids: the hull of the sums of their
    /// corners when both are convex; else the union of such hulls of each
    /// triangle of one with the other (or its triangles), with the inside
    /// of both, which takes a boolean for each.
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
        // The one that is not convex goes by triangles.
        let (big, small, cs) = if ca { (b, a, ca) } else { (a, b, cb) };
        let pieces = |s: &Solid| -> Vec<Vec<V3>> {
            s.tris
                .iter()
                .map(|t| t.iter().map(|&i| s.points[i as usize]).collect())
                .collect()
        };
        let big_pieces = pieces(big);
        let small_pieces = if cs {
            vec![Self::corners(small)]
        } else {
            pieces(small)
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
        self.union_all(parts, false)
    }

    /// The Minkowski sum of two slabs, as [`Builder::minkowski`] in the
    /// plane: of their tops' triangles where they are not convex.
    pub fn minkowski_2d(&mut self, a: &Solid, b: &Solid) -> Result<Option<Solid>, String> {
        let flat = |s: &Solid| -> (Vec<[V2; 3]>, Vec<V2>, bool) {
            let (tris, _) = region(s);
            let mut seen = HashSet::new();
            let pts: Vec<V2> = tris
                .iter()
                .flatten()
                .copied()
                .filter(|p| seen.insert(p.map(f64::to_bits)))
                .collect();
            let area: f64 = tris.iter().map(|t| cross2(t[0], t[1], t[2]) / 2.0).sum();
            let hull = area_of(&hull2(&pts));
            let convex = (hull - area).abs() <= 1e-9 * hull.max(1.0);
            (tris, pts, convex)
        };
        let (ta, pa, ca) = flat(a);
        let (tb, pb, cb) = flat(b);
        let sums = |p: &[V2], q: &[V2]| -> Vec<V2> {
            p.iter()
                .flat_map(|x| q.iter().map(move |y| [x[0] + y[0], x[1] + y[1]]))
                .collect()
        };
        if ca && cb {
            return Ok(self.hull_2d(&sums(&pa, &pb)));
        }
        let pieces = |t: &[[V2; 3]], p: &[V2], convex: bool| -> Vec<Vec<V2>> {
            if convex {
                vec![p.to_vec()]
            } else {
                t.iter().map(|t| t.to_vec()).collect()
            }
        };
        let (pa_pieces, pb_pieces) = (pieces(&ta, &pa, ca), pieces(&tb, &pb, cb));
        if pa_pieces.len() * pb_pieces.len() > MOST_PIECES {
            return Err(format!(
                "minkowski: {} with {} pieces are too many",
                pa_pieces.len(),
                pb_pieces.len()
            ));
        }
        let mut parts = Vec::new();
        for p in &pa_pieces {
            for q in &pb_pieces {
                parts.extend(self.hull_2d(&sums(p, q)));
            }
        }
        let shift = |m: &mut Self, s: &Solid, by: V2| {
            m.transformed_2d(
                s,
                &[
                    [1.0, 0.0, 0.0, by[0]],
                    [0.0, 1.0, 0.0, by[1]],
                    [0.0, 0.0, 1.0, 0.0],
                ],
            )
        };
        if let (Some(&x), Some(&y)) = (pb.first(), pa.first()) {
            parts.push(shift(self, a, x));
            parts.push(shift(self, b, y));
        }
        self.union_all(parts, true)
    }

    /// OpenSCAD's `offset`: the outlines of a slab moved out by `d` (in
    /// for a negative one), the corners that grow joined by `join`. Out,
    /// the shape with a strip along each edge and a piece at each corner
    /// that grows; in, what is left of a box round it so grown, cut away.
    pub fn offset(&mut self, slab: &Solid, d: f64, join: Join) -> Result<Option<Solid>, String> {
        if d == 0.0 || !d.is_finite() {
            return Ok(Some(slab.clone()));
        }
        if d > 0.0 {
            return self.grow(slab, d, join);
        }
        let Some((lo, hi)) = slab.bounds() else {
            return Ok(None);
        };
        let m = 2.0 * d.abs() + 1.0;
        let Some(boxed) = self.square([hi[0] - lo[0] + 2.0 * m, hi[1] - lo[1] + 2.0 * m], false)
        else {
            return Ok(None);
        };
        let boxed = self.transformed_2d(
            &boxed,
            &[
                [1.0, 0.0, 0.0, lo[0] - m],
                [0.0, 1.0, 0.0, lo[1] - m],
                [0.0, 0.0, 1.0, 0.0],
            ],
        );
        let outside = self.boolean(&boxed, slab, Op::Subtract, true)?;
        let Some(grown) = self.grow(&outside, -d, join)? else {
            return Ok(Some(slab.clone()));
        };
        let left = self.boolean(&boxed, &grown, Op::Subtract, true)?;
        Ok(Some(left).filter(|s| !s.is_empty()))
    }

    fn grow(&mut self, slab: &Solid, d: f64, join: Join) -> Result<Option<Solid>, String> {
        let (_, loops) = region(slab);
        // Mostly each outline moved out is a simple loop again: the shape
        // is these loops, without a boolean of many small pieces.
        let rings: Option<Vec<Option<Vec<V2>>>> = loops
            .iter()
            .filter(|l| l.len() >= 3)
            .map(|l| offset_ring(l, d, join))
            .collect();
        if let Some(rings) = rings {
            let kept: Vec<Vec<V2>> = rings.into_iter().flatten().collect();
            if !rings_cross(&kept) {
                return self.polygon(&kept);
            }
        }
        self.grow_by_pieces(&loops, slab, d, join)
    }

    /// [`Builder::grow`] as the shape with a strip along each edge and a
    /// piece at each corner that grows, joined: when the outlines moved
    /// out cross themselves or each other.
    fn grow_by_pieces(
        &mut self,
        loops: &[Vec<V2>],
        slab: &Solid,
        d: f64,
        join: Join,
    ) -> Result<Option<Solid>, String> {
        let mut pieces: Vec<Vec<V2>> = Vec::new();
        let add = |p: V2, n: V2, k: f64| [p[0] + n[0] * k, p[1] + n[1] * k];
        for l in loops {
            let n = l.len();
            if n < 3 {
                continue;
            }
            let normal = |i: usize| edge_normal(l[i], l[(i + 1) % n]);
            // Each strip reaches a little into the shape and past its
            // ends: pieces that overlap join well, pieces that only touch
            // do not always.
            let eps = 1e-3 * d;
            for i in 0..n {
                let (a, b) = (l[i], l[(i + 1) % n]);
                let nm = normal(i);
                let e = [-nm[1], nm[0]];
                let (a, b) = (add(a, e, -eps), add(b, e, eps));
                pieces.push(vec![
                    add(a, nm, -eps),
                    add(b, nm, -eps),
                    add(b, nm, d),
                    add(a, nm, d),
                ]);
                let b = l[(i + 1) % n];
                let n2 = normal((i + 1) % n);
                if let Some(mut corner) = join_points(b, nm, n2, d, join) {
                    corner.insert(0, b);
                    pieces.push(corner);
                }
            }
        }
        let mut parts = vec![slab.clone()];
        for p in pieces {
            if let Ok(Some(s)) = self.simple_polygon(&p) {
                parts.push(s);
            }
        }
        self.union_all(parts, true)
    }
}

/// The outward normal of an edge from `a` to `b` of an outline with the
/// shape on its left.
fn edge_normal(a: V2, b: V2) -> V2 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let len = dx.hypot(dy).max(1e-300);
    [dy / len, -dx / len]
}

/// The points that join, at corner `b`, the edge before (normal `n1`) and
/// the edge after (normal `n2`) moved out by `d`, from the end of the one
/// to the start of the other; `None` where the corner does not grow.
fn join_points(b: V2, n1: V2, n2: V2, d: f64, join: Join) -> Option<Vec<V2>> {
    let add = |p: V2, n: V2, k: f64| [p[0] + n[0] * k, p[1] + n[1] * k];
    let turn = n1[0] * n2[1] - n1[1] * n2[0];
    if turn <= 1e-12 {
        return None;
    }
    let (p1, p2) = (add(b, n1, d), add(b, n2, d));
    Some(match join {
        // Squared off at `d` from the corner, as Clipper's square join
        // does it.
        Join::Chamfer => {
            let u = [n1[0] + n2[0], n1[1] + n2[1]];
            let ul = u[0].hypot(u[1]).max(1e-300);
            let u = [u[0] / ul, u[1] / ul];
            let e1 = [-n1[1], n1[0]];
            let along = e1[0] * u[0] + e1[1] * u[1];
            let s = if along.abs() < 1e-12 {
                0.0
            } else {
                d * (1.0 - (n1[0] * u[0] + n1[1] * u[1])) / along
            };
            let e2 = [-n2[1], n2[0]];
            vec![p1, add(p1, e1, s), add(p2, e2, -s), p2]
        }
        Join::Miter => {
            let k = d / (1.0 + n1[0] * n2[0] + n1[1] * n2[1]);
            vec![
                p1,
                [b[0] + (n1[0] + n2[0]) * k, b[1] + (n1[1] + n2[1]) * k],
                p2,
            ]
        }
        Join::Round(steps) => {
            let a1 = n1[1].atan2(n1[0]);
            let mut a2 = n2[1].atan2(n2[0]);
            while a2 < a1 {
                a2 += std::f64::consts::TAU;
            }
            let k = ((a2 - a1) / std::f64::consts::TAU * steps.max(3) as f64)
                .ceil()
                .max(1.0) as usize;
            (0..=k)
                .map(|j| {
                    let t = a1 + (a2 - a1) * j as f64 / k as f64;
                    [b[0] + d * t.cos(), b[1] + d * t.sin()]
                })
                .collect()
        }
    })
}

/// Where the lines through `p` along `u` and through `q` along `v` meet,
/// as the parameters along each.
fn meet(p: V2, u: V2, q: V2, v: V2) -> Option<(f64, f64)> {
    let den = u[0] * v[1] - u[1] * v[0];
    if den.abs() < 1e-14 {
        return None;
    }
    let w = [q[0] - p[0], q[1] - p[1]];
    Some((
        (w[0] * v[1] - w[1] * v[0]) / den,
        (w[0] * u[1] - w[1] * u[0]) / den,
    ))
}

/// An outline (shape on its left) moved out by `d`, its corners that grow
/// joined by `join`, those that shrink cut where the moved edges meet:
/// `Some(None)` if it closes up to nothing, `None` if it is no simple loop
/// (an edge shorter than what a corner takes of it).
fn offset_ring(l: &[V2], d: f64, join: Join) -> Option<Option<Vec<V2>>> {
    let n = l.len();
    let normals: Vec<V2> = (0..n).map(|i| edge_normal(l[i], l[(i + 1) % n])).collect();
    let add = |p: V2, nm: V2, k: f64| [p[0] + nm[0] * k, p[1] + nm[1] * k];
    // Each moved edge as a part of its line, 0 at its start corner and 1
    // at its end corner, and the points of each corner.
    let mut part = vec![(0.0f64, 1.0f64); n];
    let mut corners: Vec<Vec<V2>> = Vec::with_capacity(n);
    for i in 0..n {
        let j = (i + 1) % n;
        let b = l[j];
        if let Some(pts) = join_points(b, normals[i], normals[j], d, join) {
            corners.push(pts);
            continue;
        }
        let pi = add(l[i], normals[i], d);
        let ui = [b[0] - l[i][0], b[1] - l[i][1]];
        let turn = normals[i][0] * normals[j][1] - normals[i][1] * normals[j][0];
        if turn.abs() <= 1e-12 {
            corners.push(vec![add(b, normals[i], d)]);
            continue;
        }
        // A corner that shrinks: the moved edges meet inside.
        let pj = add(b, normals[j], d);
        let next = l[(j + 1) % n];
        let uj = [next[0] - b[0], next[1] - b[1]];
        let (s, t) = meet(pi, ui, pj, uj)?;
        part[i].1 = s;
        part[j].0 = t;
        corners.push(vec![[pi[0] + ui[0] * s, pi[1] + ui[1] * s]]);
    }
    // An edge shorter than its corners take: no simple loop.
    if part.iter().any(|(s, e)| s > e) {
        return None;
    }
    let ring = clean(&corners.concat());
    if ring.len() < 3 || ring_crosses(&ring) {
        return None;
    }
    // Turned round: a hole that closed up, or nothing left.
    if (area(&ring) > 0.0) != (area(l) > 0.0) {
        return Some(None);
    }
    Some(Some(ring))
}

/// Whether two segments cross or touch, but not at a shared end.
fn segments_cross(a: V2, b: V2, c: V2, e: V2) -> bool {
    let d1 = cross2(a, b, c);
    let d2 = cross2(a, b, e);
    let d3 = cross2(c, e, a);
    let d4 = cross2(c, e, b);
    ((d1 > 0.0 && d2 < 0.0) || (d1 < 0.0 && d2 > 0.0))
        && ((d3 > 0.0 && d4 < 0.0) || (d3 < 0.0 && d4 > 0.0))
}

/// Whether a loop crosses itself.
fn ring_crosses(r: &[V2]) -> bool {
    let n = r.len();
    (0..n).any(|i| {
        let (a, b) = (r[i], r[(i + 1) % n]);
        (i + 2..n).any(|j| {
            let k = (j + 1) % n;
            k != i && segments_cross(a, b, r[j], r[k])
        })
    })
}

/// Whether loops cross each other.
fn rings_cross(rings: &[Vec<V2>]) -> bool {
    rings.iter().enumerate().any(|(x, p)| {
        rings[x + 1..].iter().any(|q| {
            (0..p.len()).any(|i| {
                (0..q.len())
                    .any(|j| segments_cross(p[i], p[(i + 1) % p.len()], q[j], q[(j + 1) % q.len()]))
            })
        })
    })
}

/// The area of a polygon, either way round.
fn area_of(p: &[V2]) -> f64 {
    area(p).abs()
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
        let s = b.polygon(&[outer, hole]).unwrap().unwrap();
        assert!((s.volume() - 96.0).abs() < 1e-6, "{}", s.volume());
        let (tris, loops) = region(&s);
        let a: f64 = tris.iter().map(|t| cross2(t[0], t[1], t[2]) / 2.0).sum();
        assert!((a - 96.0).abs() < 1e-6);
        assert_eq!(loops.len(), 2);
        assert!((loops.iter().map(|l| area(l)).sum::<f64>() - 96.0).abs() < 1e-6);
    }

    #[test]
    fn slabs_unite_cut_and_intersect() {
        let mut b = Builder::default();
        let a = b.square([10.0, 10.0], false).unwrap();
        let c = b.circle(5.0, 16).unwrap();
        let c = b.transformed_2d(&c, &[[1.0, 0.0, 0.0, 10.0], [0.0, 1.0, 0.0, 5.0], [0.0; 4]]);
        let disc = 8.0 * 25.0 * (std::f64::consts::TAU / 16.0).sin();
        let u = b.boolean(&a, &c, Op::Add, true).unwrap();
        let d = b.boolean(&a, &c, Op::Subtract, true).unwrap();
        let i = b.boolean(&a, &c, Op::Intersect, true).unwrap();
        assert!(
            (u.volume() - (100.0 + disc / 2.0)).abs() < 1e-6,
            "{}",
            u.volume()
        );
        assert!(
            (d.volume() - (100.0 - disc / 2.0)).abs() < 1e-6,
            "{}",
            d.volume()
        );
        assert!((i.volume() - disc / 2.0).abs() < 1e-6, "{}", i.volume());
        for s in [&u, &d, &i] {
            let (lo, hi) = s.bounds().unwrap();
            assert!(lo[2].abs() < 1e-9 && (hi[2] - 1.0).abs() < 1e-9);
        }
    }
}
