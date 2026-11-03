//! Meshes from other programs: STL in, as a solid whose flat faces are faces.

use std::collections::HashMap;

use crate::{FaceId, Facets, Fx, Solid, cross, dot, snap, sub};

/// The triangles of an STL file, binary or text; `None` when it is neither.
#[must_use]
pub fn read_stl(b: &[u8]) -> Option<Vec<[[f64; 3]; 3]>> {
    let n = u32::from_le_bytes(b.get(80..84)?.try_into().ok()?) as usize;
    if b.len() == 84 + 50 * n {
        return Some(
            b[84..]
                .chunks_exact(50)
                .map(|r| {
                    std::array::from_fn(|k| {
                        let at = 12 + 12 * k;
                        std::array::from_fn(|i| {
                            let o = at + 4 * i;
                            f64::from(f32::from_le_bytes([r[o], r[o + 1], r[o + 2], r[o + 3]]))
                        })
                    })
                })
                .collect(),
        );
    }
    let t = std::str::from_utf8(b).ok()?;
    if !t.trim_start().starts_with("solid") {
        return None;
    }
    let vs: Vec<[f64; 3]> = t
        .lines()
        .filter_map(|l| l.trim().strip_prefix("vertex "))
        .filter_map(|l| {
            let p: Vec<f64> = l
                .split_whitespace()
                .filter_map(|x| x.parse().ok())
                .collect();
            (p.len() == 3).then(|| [p[0], p[1], p[2]])
        })
        .collect();
    Some(vs.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect())
}

impl Solid {
    /// A solid from loose triangles: corners at one place become one vertex,
    /// on the kernel's grid, triangles of one plane one face, named `face(k)`
    /// for the k-th plane. Triangles without area are left out.
    pub fn from_triangles(
        tris: &[[[f64; 3]; 3]],
        facets: &mut Facets,
        face: impl Fn(usize) -> FaceId,
    ) -> Solid {
        Solid::welded(tris, facets, face, snap)
    }

    /// [`Solid::from_triangles`] with the corners where they are, not on
    /// the kernel's grid: four corners of a plane stay in one plane, as the
    /// boolean library has to see them to join its triangles up again
    /// (OpenSCAD's shapes, which are not snapped either).
    pub fn from_triangles_exact(
        tris: &[[[f64; 3]; 3]],
        facets: &mut Facets,
        face: impl Fn(usize) -> FaceId,
    ) -> Solid {
        Solid::welded(tris, facets, face, |x| x)
    }

    fn welded(
        tris: &[[[f64; 3]; 3]],
        facets: &mut Facets,
        face: impl Fn(usize) -> FaceId,
        place: fn(f64) -> f64,
    ) -> Solid {
        // Welded on a grid finer than a print and coarser than f32 noise.
        let key = |p: [f64; 3]| p.map(|x| (x * 1e5).round() as i64);
        let mut at: HashMap<[i64; 3], u32, Fx> = HashMap::default();
        let mut planes: HashMap<([i64; 3], i64), u32, Fx> = HashMap::default();
        let mut s = Solid::default();
        for t in tris {
            let ids = t.map(|p| {
                *at.entry(key(p)).or_insert_with(|| {
                    s.points.push(p.map(place));
                    s.points.len() as u32 - 1
                })
            });
            if ids[0] == ids[1] || ids[1] == ids[2] || ids[0] == ids[2] {
                continue;
            }
            let n = cross(sub(t[1], t[0]), sub(t[2], t[0]));
            let l = dot(n, n).sqrt();
            if l < 1e-12 {
                continue;
            }
            let n = n.map(|x| x / l);
            let plane = (
                n.map(|x| (x * 1e4).round() as i64),
                (dot(n, t[0]) * 1e3).round() as i64,
            );
            let k = planes.len();
            let facet = *planes
                .entry(plane)
                .or_insert_with(|| facets.add(face(k), n, t[0]));
            s.tris.push(ids);
            s.facets.push(facet);
        }
        s
    }

    /// A solid from its corners and triangles, as
    /// [`Solid::from_triangles_exact`] makes one from loose triangles, but
    /// no two corners at one place may be meant as one. Triangles of one plane are one
    /// face, named `face(k)` for the k-th plane; those without area are
    /// left out.
    pub fn from_indexed(
        points: Vec<[f64; 3]>,
        tris: &[[u32; 3]],
        facets: &mut Facets,
        face: impl Fn(usize) -> FaceId,
    ) -> Solid {
        let mut planes: HashMap<([i64; 3], i64), u32, Fx> = HashMap::default();
        let mut s = Solid {
            points,
            ..Solid::default()
        };
        s.tris.reserve(tris.len());
        s.facets.reserve(tris.len());
        for &t in tris {
            if t[0] == t[1] || t[1] == t[2] || t[0] == t[2] {
                continue;
            }
            let [a, b, c] = t.map(|i| s.points[i as usize]);
            let n = cross(sub(b, a), sub(c, a));
            let l = dot(n, n).sqrt();
            if l < 1e-12 {
                continue;
            }
            let n = n.map(|x| x / l);
            let plane = (
                n.map(|x| (x * 1e4).round() as i64),
                (dot(n, a) * 1e3).round() as i64,
            );
            let k = planes.len();
            let facet = *planes
                .entry(plane)
                .or_insert_with(|| facets.add(face(k), n, a));
            s.tris.push(t);
            s.facets.push(facet);
        }
        s
    }

    /// The solid placed elsewhere: scaled by `scale` (below zero mirrored),
    /// turned by the unit quaternion `rotation` (w, x, y, z) and moved by
    /// `offset`. Its facets come from `from` and go into `to`, renamed.
    pub fn placed(
        &self,
        from: &Facets,
        to: &mut Facets,
        rotation: [f64; 4],
        scale: f64,
        offset: [f64; 3],
        rename: impl Fn(FaceId) -> FaceId,
    ) -> Solid {
        let [w, x, y, z] = {
            let l = rotation.iter().map(|v| v * v).sum::<f64>().sqrt();
            if l < 1e-12 {
                [1.0, 0.0, 0.0, 0.0]
            } else {
                rotation.map(|v| v / l)
            }
        };
        let m = [
            [
                1.0 - 2.0 * (y * y + z * z),
                2.0 * (x * y - w * z),
                2.0 * (x * z + w * y),
            ],
            [
                2.0 * (x * y + w * z),
                1.0 - 2.0 * (x * x + z * z),
                2.0 * (y * z - w * x),
            ],
            [
                2.0 * (x * z - w * y),
                2.0 * (y * z + w * x),
                1.0 - 2.0 * (x * x + y * y),
            ],
        ];
        let turn = |v: [f64; 3]| -> [f64; 3] { std::array::from_fn(|i| dot(m[i], v)) };
        let place = |p: [f64; 3]| -> [f64; 3] {
            let q = turn(p.map(|c| c * scale));
            std::array::from_fn(|i| q[i] + offset[i])
        };
        let mirrored = scale < 0.0;
        let mut map: HashMap<u32, u32> = HashMap::new();
        let facets = self
            .facets
            .iter()
            .map(|&f| {
                *map.entry(f).or_insert_with(|| {
                    let old = from.get(f);
                    let n = turn(old.normal).map(|c| if mirrored { -c } else { c });
                    let on = place(old.normal.map(|c| c * old.offset));
                    to.add(rename(old.face), n, on)
                })
            })
            .collect();
        Solid {
            points: self.points.iter().map(|&p| place(p).map(snap)).collect(),
            tris: self
                .tris
                .iter()
                .map(|&[a, b, c]| if mirrored { [a, c, b] } else { [a, b, c] })
                .collect(),
            facets,
            mesh: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ball_stl() -> Vec<u8> {
        let mut f = Facets::default();
        let s = Solid::sphere(&mut f, [0.0; 3], 5.0, 16, FaceId(1));
        s.to_stl()
    }

    #[test]
    fn an_stl_comes_back_closed_with_its_volume() {
        let bytes = ball_stl();
        let tris = read_stl(&bytes).expect("stl");
        let mut f = Facets::default();
        let s = Solid::from_triangles(&tris, &mut f, |k| FaceId(k as u64));
        assert!(s.is_closed());
        let mut g = Facets::default();
        let back = Solid::sphere(&mut g, [0.0; 3], 5.0, 16, FaceId(1));
        assert!((s.volume() - back.volume()).abs() < 1e-3);
        let text = "solid t\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex 1 0 0\nvertex 0 1 0\nendloop\nendfacet\nendsolid t\n";
        assert_eq!(read_stl(text.as_bytes()).map(|t| t.len()), Some(1));
        assert!(read_stl(b"nothing").is_none());
    }

    #[test]
    fn a_placed_solid_turns_moves_and_mirrors() {
        let mut f = Facets::default();
        let s = Solid::sphere(&mut f, [1.0, 0.0, 0.0], 1.0, 12, FaceId(1));
        let v = s.volume();
        let mut to = Facets::default();
        // A quarter turn about z, then 10 up.
        let h = std::f64::consts::FRAC_1_SQRT_2;
        let p = s.placed(&f, &mut to, [h, 0.0, 0.0, h], 1.0, [0.0, 0.0, 10.0], |x| x);
        let (lo, hi) = p.bounds().expect("points");
        assert!((lo[1] - 0.0).abs() < 1e-6 && (hi[1] - 2.0).abs() < 1e-6);
        assert!((lo[2] - 9.0).abs() < 1e-6);
        assert!((p.volume() - v).abs() < 1e-6);
        // Mirrored and doubled: eight times the volume, still outward.
        let m = s.placed(&f, &mut to, [1.0, 0.0, 0.0, 0.0], -2.0, [0.0; 3], |x| x);
        assert!((m.volume() - 8.0 * v).abs() < 1e-4);
        assert!(m.is_closed());
    }
}
