//! Solids of revolution, and mirrored copies.

use std::collections::HashMap;

use super::{
    Error, FaceId, Facet, Facets, Frame, Outline, Solid, cross, dot, normalized, snap, sub,
};

/// Closer to the axis than this, a corner lies on it.
const ON_AXIS: f64 = 1e-6;

/// Turns `v` by `angle` radians about the unit axis `k` (Rodrigues).
fn turn(k: [f64; 3], angle: f64, v: [f64; 3]) -> [f64; 3] {
    let (s, c) = angle.sin_cos();
    let kv = cross(k, v);
    let d = dot(k, v) * (1.0 - c);
    std::array::from_fn(|i| v[i] * c + kv[i] * s + k[i] * d)
}

/// The largest distance of an outline from an axis: what the number of steps
/// round it depends on. `None` if the axis has no direction.
pub fn revolve_radius(
    outline: &Outline,
    frame: &Frame,
    origin: [f64; 3],
    axis: [f64; 3],
) -> Option<f64> {
    if dot(axis, axis) < 1e-18 {
        return None;
    }
    let k = normalized(axis);
    Some(
        outline
            .points
            .iter()
            .map(|p| {
                let d = sub(frame.at(*p, [0.0; 3]), origin);
                let along = dot(d, k);
                let r: [f64; 3] = std::array::from_fn(|i| d[i] - k[i] * along);
                dot(r, r).sqrt()
            })
            .fold(0.0, f64::max),
    )
}

/// The solid an outline sweeps turning about the axis through `origin`
/// along `axis`, from `from` to `to` radians, in `steps` even steps. A
/// whole turn (`to - from` of 2π or more) has no caps; otherwise the outline
/// at the start and at the end closes it, as faces `start` and `end`. The
/// axis must lie in the outline's plane and the outline on one side of it;
/// corners on the axis stay single, so a profile touching it closes there.
#[allow(clippy::too_many_arguments)]
pub fn revolve(
    facets: &mut Facets,
    outline: &Outline,
    frame: &Frame,
    origin: [f64; 3],
    axis: [f64; 3],
    from: f64,
    to: f64,
    steps: usize,
    start: FaceId,
    end: FaceId,
) -> Result<Solid, Error> {
    let n = outline.points.len();
    if n != outline.faces.len() || dot(axis, axis) < 1e-18 || to <= from || steps == 0 {
        return Err(Error::BadProfile);
    }
    let caps = super::triangulate(&outline.points).ok_or(Error::BadProfile)?;
    let k = normalized(axis);
    let normal = normalized(cross(frame.u, frame.v));
    // In the plane: the axis runs across no normal, and through the plane.
    if dot(k, normal).abs() > 1e-6 || dot(sub(origin, frame.origin), normal).abs() > 1e-6 {
        return Err(Error::AxisOffPlane);
    }
    let across = cross(normal, k);
    let world: Vec<[f64; 3]> = outline
        .points
        .iter()
        .map(|p| frame.at(*p, [0.0; 3]))
        .collect();
    let side: Vec<f64> = world.iter().map(|p| dot(sub(*p, origin), across)).collect();
    let (lo, hi) = side
        .iter()
        .fold((0.0f64, 0.0f64), |(l, h), s| (l.min(*s), h.max(*s)));
    if lo < -ON_AXIS && hi > ON_AXIS {
        return Err(Error::CrossesAxis);
    }
    if lo.abs().max(hi) <= ON_AXIS {
        return Err(Error::BadProfile);
    }
    let whole = to - from >= std::f64::consts::TAU - 1e-9;
    let rings = if whole { steps } else { steps + 1 };
    let angle = |r: usize| from + (to - from) * r as f64 / steps as f64;
    // Each corner: one point on the axis, else one per ring.
    let mut s = Solid::default();
    let mut first: Vec<u32> = Vec::with_capacity(n);
    let on: Vec<bool> = side.iter().map(|x| x.abs() <= ON_AXIS).collect();
    for (i, p) in world.iter().enumerate() {
        first.push(s.points.len() as u32);
        let d = sub(*p, origin);
        if on[i] {
            let along = dot(d, k);
            s.points
                .push(std::array::from_fn(|j| snap(origin[j] + k[j] * along)));
        } else {
            for r in 0..rings {
                let q = turn(k, angle(r), d);
                s.points
                    .push(std::array::from_fn(|j| snap(q[j] + origin[j])));
            }
        }
    }
    let at = |i: usize, r: usize| -> u32 {
        if on[i] {
            first[i]
        } else {
            first[i] + (r % rings) as u32
        }
    };
    // Turning the right way round the axis moves the outline along its
    // normal, as an extrusion does; the other way, everything flips.
    let far = (0..n).max_by(|&a, &b| side[a].abs().total_cmp(&side[b].abs()));
    let sweep = cross(k, sub(world[far.unwrap_or(0)], origin));
    let flip = dot(sweep, normal) < 0.0;
    let oriented = |t: [u32; 3]| if flip { [t[0], t[2], t[1]] } else { t };
    for i in 0..n {
        let j = (i + 1) % n;
        if on[i] && on[j] {
            continue;
        }
        for r in 0..steps {
            let (a, b, c, d) = (at(i, r), at(j, r), at(j, r + 1), at(i, r + 1));
            let tris: Vec<[u32; 3]> = if on[i] {
                vec![oriented([a, b, c])]
            } else if on[j] {
                vec![oriented([a, b, d])]
            } else {
                vec![oriented([a, b, c]), oriented([a, c, d])]
            };
            // A quad between two rings is flat: one facet for both halves.
            let mut facet: Option<u32> = None;
            for t in tris {
                let fits = facet.filter(|&f| facets.fits(f, t.map(|x| s.points[x as usize])));
                let f = match fits {
                    Some(f) => f,
                    None => match plane_of(facets, &s, outline.faces[i], t) {
                        Some(f) => f,
                        None => continue,
                    },
                };
                facet = Some(f);
                s.push(t, f);
            }
        }
    }
    if !whole {
        // The start faces back along the turn, the end along it.
        for (r, face, reversed) in [(0, start, !flip), (steps, end, flip)] {
            let a = angle(r);
            let n_cap = turn(k, a, normal).map(|x| if reversed { -x } else { x });
            let f = facets.add(face, n_cap, s.points[at(0, r) as usize]);
            for t in &caps {
                let t = t.map(|i| at(i as usize, r));
                s.push(if reversed { [t[0], t[2], t[1]] } else { t }, f);
            }
        }
    }
    Ok(s)
}

/// A new facet of `face` in the plane of triangle `t`; `None` if it has no
/// area.
fn plane_of(facets: &mut Facets, s: &Solid, face: FaceId, t: [u32; 3]) -> Option<u32> {
    let [a, b, c] = t.map(|i| s.points[i as usize]);
    let m = cross(sub(b, a), sub(c, a));
    if dot(m, m) < 1e-24 {
        return None;
    }
    Some(facets.add(face, normalized(m), a))
}

impl Solid {
    /// A copy reflected in the plane through `origin` with `normal`, on facets
    /// of its own named by `rename`.
    pub fn mirrored(
        &self,
        facets: &mut Facets,
        origin: [f64; 3],
        normal: [f64; 3],
        rename: impl Fn(FaceId) -> FaceId,
    ) -> Solid {
        let m = normalized(normal);
        let reflect = |v: [f64; 3]| -> [f64; 3] {
            let d = 2.0 * dot(v, m);
            std::array::from_fn(|i| v[i] - d * m[i])
        };
        let mut map: HashMap<u32, u32> = HashMap::new();
        let copied: Vec<u32> = self
            .facets
            .iter()
            .map(|&f| {
                *map.entry(f).or_insert_with(|| {
                    let old = facets.get(f);
                    let on = old.normal.map(|x| x * old.offset);
                    let on = reflect(sub(on, origin));
                    let on: [f64; 3] = std::array::from_fn(|i| on[i] + origin[i]);
                    let normal = reflect(old.normal);
                    facets.push(Facet {
                        face: rename(old.face),
                        normal,
                        offset: dot(normal, on),
                    })
                })
            })
            .collect();
        Solid {
            points: self
                .points
                .iter()
                .map(|p| {
                    let q = reflect(sub(*p, origin));
                    std::array::from_fn(|i| snap(q[i] + origin[i]))
                })
                .collect(),
            // A reflection turns the winding round.
            tris: self.tris.iter().map(|t| [t[0], t[2], t[1]]).collect(),
            facets: copied,
            mesh: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Op;
    use std::f64::consts::{FRAC_PI_2, PI, TAU};

    fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Outline {
        Outline {
            points: vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]],
            faces: (0..4).map(FaceId).collect(),
        }
    }

    const XY: Frame = Frame {
        origin: [0.0; 3],
        u: [1.0, 0.0, 0.0],
        v: [0.0, 1.0, 0.0],
    };

    /// Closed, every triangle in its facet's plane and facing out of it.
    fn sound(s: &Solid, f: &Facets) {
        assert!(s.is_closed());
        for (t, &facet) in s.tris.iter().zip(&s.facets) {
            let [a, b, c] = t.map(|i| s.points[i as usize]);
            assert!(f.fits(facet, [a, b, c]), "{t:?}");
            let n = cross(sub(b, a), sub(c, a));
            assert!(dot(n, n) > 1e-20, "a triangle without area");
            assert!(dot(n, f.get(facet).normal) > 0.0, "{t:?} faces inward");
        }
        assert_eq!(s.sheet_triangles(f), 0);
    }

    fn turned(o: &Outline, axis: [f64; 3], from: f64, to: f64) -> (Solid, Facets) {
        let mut f = Facets::default();
        let s = revolve(
            &mut f,
            o,
            &XY,
            [0.0; 3],
            axis,
            from,
            to,
            120,
            FaceId(90),
            FaceId(91),
        );
        (s.unwrap(), f)
    }

    #[test]
    fn a_rectangle_off_the_axis_is_a_ring() {
        let (s, f) = turned(&rect(10.0, 0.0, 12.0, 3.0), [0.0, 1.0, 0.0], 0.0, TAU);
        sound(&s, &f);
        let exact = PI * (144.0 - 100.0) * 3.0;
        assert!(
            s.volume() < exact && s.volume() > 0.998 * exact,
            "{}",
            s.volume()
        );
        // Four sides, no caps.
        assert_eq!(s.faces(&f), (0..4).map(FaceId).collect::<Vec<_>>());
    }

    #[test]
    fn a_rectangle_on_the_axis_is_a_closed_cylinder() {
        for axis in [[0.0, 1.0, 0.0], [0.0, -1.0, 0.0]] {
            let (s, f) = turned(&rect(0.0, 0.0, 5.0, 4.0), axis, 0.0, TAU);
            sound(&s, &f);
            let exact = PI * 25.0 * 4.0;
            assert!(s.volume() > 0.998 * exact && s.volume() < exact);
            // The side on the axis leaves no face.
            assert!(!s.faces(&f).contains(&FaceId(3)));
        }
    }

    #[test]
    fn a_part_turn_has_caps_and_either_side_works() {
        for (o, axis) in [
            (rect(0.0, 0.0, 5.0, 4.0), [0.0, 1.0, 0.0]),
            (rect(-5.0, 0.0, 0.0, 4.0), [0.0, 1.0, 0.0]),
            (rect(2.0, 0.0, 5.0, 4.0), [0.0, -1.0, 0.0]),
        ] {
            let (s, f) = turned(&o, axis, -0.25, FRAC_PI_2 - 0.25);
            sound(&s, &f);
            let r0 = o.points[0][0].abs().min(o.points[1][0].abs());
            let exact = PI * (25.0 - r0 * r0) * 4.0 / 4.0;
            assert!(
                s.volume() > 0.998 * exact && s.volume() < exact + 1e-9,
                "{} {exact}",
                s.volume()
            );
            let faces = s.faces(&f);
            assert!(faces.contains(&FaceId(90)) && faces.contains(&FaceId(91)));
        }
    }

    #[test]
    fn a_revolved_solid_takes_booleans() {
        let (ring, mut f) = turned(&rect(0.0, 0.0, 5.0, 4.0), [0.0, 1.0, 0.0], 0.0, TAU);
        let block = crate::prism(
            &mut f,
            &rect(-1.0, -1.0, 1.0, 1.0),
            -1.0,
            1.0,
            FaceId(7),
            FaceId(8),
        );
        let r = ring.boolean(&block.unwrap(), Op::Subtract, &mut f).unwrap();
        assert!(r.is_closed());
        assert!(r.volume() < ring.volume() - 3.9);
    }

    #[test]
    fn profiles_across_or_beside_the_plane_are_refused() {
        let mut f = Facets::default();
        let o = rect(-1.0, 0.0, 1.0, 1.0);
        let r = revolve(
            &mut f,
            &o,
            &XY,
            [0.0; 3],
            [0.0, 1.0, 0.0],
            0.0,
            TAU,
            8,
            FaceId(0),
            FaceId(1),
        );
        assert_eq!(r.unwrap_err(), Error::CrossesAxis);
        let o = rect(1.0, 0.0, 2.0, 1.0);
        let r = revolve(
            &mut f,
            &o,
            &XY,
            [0.0, 0.0, 1.0],
            [0.0, 1.0, 0.0],
            0.0,
            TAU,
            8,
            FaceId(0),
            FaceId(1),
        );
        assert_eq!(r.unwrap_err(), Error::AxisOffPlane);
    }

    #[test]
    fn a_mirrored_copy_is_sound_and_joins_its_original() {
        let mut f = Facets::default();
        let a = crate::prism(
            &mut f,
            &rect(0.0, 0.0, 2.0, 3.0),
            0.0,
            1.0,
            FaceId(7),
            FaceId(8),
        )
        .unwrap();
        let b = a.mirrored(&mut f, [0.0; 3], [2.0, 0.0, 0.0], |x| FaceId(x.0 + 100));
        sound(&b, &f);
        assert!((b.volume() - 6.0).abs() < 1e-9);
        assert!((b.bounds().unwrap().0[0] + 2.0).abs() < 1e-9);
        let both = a.boolean(&b, Op::Add, &mut f).unwrap();
        assert!(both.is_closed());
        assert!((both.volume() - 12.0).abs() < 1e-9);
        // A slanted plane.
        let c = a.mirrored(&mut f, [5.0, 0.0, 0.0], [1.0, 1.0, 0.5], |x| x);
        sound(&c, &f);
        assert!((c.volume() - 6.0).abs() < 1e-6);
    }
}
