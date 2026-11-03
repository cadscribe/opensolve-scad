//! 2D shapes as OpenSCAD has them: polygons with holes, their booleans,
//! offsets and Minkowski sums by Clipper2 (clipper2-rust, a port of the
//! library OpenSCAD uses), on integer coordinates scaled as OpenSCAD scales
//! them (`ClipperUtils.cc`). A shape stays a polygon until it is extruded.

use std::collections::HashMap;

use clipper2_rust::{
    ClipType, Clipper64, ClipperOffset, EndType, FillRule, JoinType, Path64, Paths64, Point64,
    PolyTree64, is_positive, simplify_path,
};

pub type V2 = [f64; 2];

/// A 2D shape: closed loops that do not cross, outlines counter-clockwise
/// and holes clockwise (as Clipper leaves them).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Region {
    pub loops: Vec<Vec<V2>>,
}

/// OpenSCAD's precision for Clipper: 8 decimals, as bits
/// (`scaleBitsFromPrecision`), so a unit is 2^-27 mm.
const SCALE: f64 = (1u64 << 27) as f64;

/// How far a corner may be from the line of its neighbours and still be
/// dropped, in Clipper's units: OpenSCAD's (from Clipper 1).
const SIMPLIFY: f64 = 1.1415;

fn to_path(l: &[V2]) -> Path64 {
    l.iter()
        .map(|p| Point64::new((p[0] * SCALE).round() as i64, (p[1] * SCALE).round() as i64))
        .collect()
}

fn to_paths(r: &Region) -> Paths64 {
    r.loops.iter().map(|l| to_path(l)).collect()
}

/// The loops of a tree, depth first, each simplified as OpenSCAD's
/// `toPolygon2d` does it.
fn from_tree(tree: &PolyTree64) -> Region {
    let mut loops = Vec::new();
    let mut stack: Vec<usize> = tree.root().children().iter().rev().copied().collect();
    while let Some(k) = stack.pop() {
        let node = &tree.nodes[k];
        let path = simplify_path(node.polygon(), SIMPLIFY, true);
        if path.len() >= 3 {
            loops.push(
                path.iter()
                    .map(|p| [p.x as f64 / SCALE, p.y as f64 / SCALE])
                    .collect(),
            );
        }
        stack.extend(node.children().iter().rev().copied());
    }
    Region { loops }
}

fn clipper() -> Clipper64 {
    let mut c = Clipper64::new();
    c.set_preserve_collinear(false);
    c
}

/// Paths filled by the even-odd rule, each taken counter-clockwise (as
/// OpenSCAD's `sanitize` takes a polygon of its own): `polygon`, `circle`,
/// `square`, the letters of `text`, an imported drawing.
pub fn even_odd(paths: &[Vec<V2>]) -> Region {
    let p: Paths64 = paths
        .iter()
        .filter(|l| l.len() >= 3)
        .map(|l| {
            let mut p = to_path(l);
            if !is_positive(&p) {
                p.reverse();
            }
            p
        })
        .collect();
    let mut c = clipper();
    c.add_subject(&p);
    let mut tree = PolyTree64::new();
    c.execute_tree(
        ClipType::Union,
        FillRule::EvenOdd,
        &mut tree,
        &mut Paths64::new(),
    );
    from_tree(&tree)
}

/// Paths filled by the non-zero rule, each the way it runs: the shadow of
/// faces, where faces that overlap count once.
pub fn non_zero(paths: &[Vec<V2>]) -> Region {
    let p: Paths64 = paths
        .iter()
        .filter(|l| l.len() >= 3)
        .map(|l| to_path(l))
        .collect();
    let mut c = clipper();
    c.add_subject(&p);
    let mut tree = PolyTree64::new();
    c.execute_tree(
        ClipType::Union,
        FillRule::NonZero,
        &mut tree,
        &mut Paths64::new(),
    );
    from_tree(&tree)
}

/// The boolean of shapes, as OpenSCAD's `ClipperUtils::apply`: the first
/// and the others, by the non-zero rule; an intersection in pairs.
pub fn boolean(shapes: &[&Region], op: opensolve_kernel::Op) -> Region {
    use opensolve_kernel::Op;
    let Some((first, rest)) = shapes.split_first() else {
        return Region::default();
    };
    if rest.is_empty() {
        return (*first).clone();
    }
    let kind = match op {
        Op::Add => ClipType::Union,
        Op::Subtract => ClipType::Difference,
        Op::Intersect => ClipType::Intersection,
    };
    let mut tree = PolyTree64::new();
    if op == Op::Intersect {
        let mut source = to_paths(first);
        for (i, s) in rest.iter().enumerate() {
            let mut c = clipper();
            c.add_subject(&source);
            c.add_clip(&to_paths(s));
            c.execute_tree(kind, FillRule::NonZero, &mut tree, &mut Paths64::new());
            if i + 1 < rest.len() {
                source = clipper2_rust::poly_tree_to_paths64(&tree);
            }
        }
        return from_tree(&tree);
    }
    let mut c = clipper();
    c.add_subject(&to_paths(first));
    for s in rest {
        c.add_clip(&to_paths(s));
    }
    c.execute_tree(kind, FillRule::NonZero, &mut tree, &mut Paths64::new());
    from_tree(&tree)
}

/// How `offset` joins the moved edges at a corner that grows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Join {
    /// An arc, within so far of the true circle (`r`).
    Round(f64),
    /// The edges lengthened until they meet (`delta`).
    Miter,
    /// The corner cut off (`delta`, `chamfer = true`).
    Chamfer,
}

/// OpenSCAD's `offset`: the outlines moved out by `delta` (in for a
/// negative one), by Clipper's offset with OpenSCAD's settings.
pub fn offset(r: &Region, delta: f64, join: Join) -> Region {
    let (kind, miter, arc) = match join {
        Join::Round(tolerance) => (JoinType::Round, 2.0, tolerance * SCALE),
        // OpenSCAD's miter limit, which it does not let a file set.
        Join::Miter => (JoinType::Miter, 2.0, 1.0),
        Join::Chamfer => (JoinType::Square, 2.0, 1.0),
    };
    let mut co = ClipperOffset::new(miter, arc, false, false);
    co.add_paths(&to_paths(r), kind, EndType::Polygon);
    let mut tree = PolyTree64::new();
    co.execute_tree(delta * SCALE, &mut tree);
    from_tree(&tree)
}

/// The Minkowski sum of shapes, as OpenSCAD's `applyMinkowski`: each
/// outline of one swept along each of the other, as quads, the insides
/// filled, all joined.
pub fn minkowski(shapes: &[&Region]) -> Region {
    let Some((first, rest)) = shapes.split_first() else {
        return Region::default();
    };
    let mut lhs = to_paths(first);
    let mut terms = Paths64::new();
    for (i, s) in rest.iter().enumerate() {
        let rhs = to_paths(s);
        terms.clear();
        for b in &rhs {
            for a in &lhs {
                outline_sum(a, b, &mut terms);
            }
        }
        fill_insides(&lhs, &rhs, &mut terms);
        fill_insides(&rhs, &lhs, &mut terms);
        if i + 1 < rest.len() {
            let mut c = clipper();
            c.add_subject(&terms);
            lhs.clear();
            c.execute(ClipType::Union, FillRule::NonZero, &mut lhs, None);
        }
    }
    if rest.is_empty() {
        return (*first).clone();
    }
    let mut c = clipper();
    c.add_subject(&terms);
    let mut tree = PolyTree64::new();
    c.execute_tree(
        ClipType::Union,
        FillRule::NonZero,
        &mut tree,
        &mut Paths64::new(),
    );
    from_tree(&tree)
}

/// The quads that `poly` sweeps along the closed `path` (Clipper's
/// Minkowski sum without its union, as OpenSCAD has it).
fn outline_sum(poly: &Path64, path: &Path64, quads: &mut Paths64) {
    let (n, m) = (poly.len(), path.len());
    if n == 0 || m == 0 {
        return;
    }
    let moved: Vec<Path64> = path
        .iter()
        .map(|q| {
            poly.iter()
                .map(|p| Point64::new(q.x + p.x, q.y + p.y))
                .collect()
        })
        .collect();
    for i in 0..m {
        for j in 0..n {
            let mut quad = vec![
                moved[i % m][j % n],
                moved[(i + 1) % m][j % n],
                moved[(i + 1) % m][(j + 1) % n],
                moved[i % m][(j + 1) % n],
            ];
            if !is_positive(&quad) {
                quad.reverse();
            }
            quads.push(quad);
        }
    }
}

/// `a` moved to a corner of each outline of `b`: what the sweeps leave
/// open inside.
fn fill_insides(a: &Paths64, b: &Paths64, target: &mut Paths64) {
    for path in b {
        if path.is_empty() || !is_positive(path) {
            continue;
        }
        let d = path[0];
        for p in a {
            target.push(
                p.iter()
                    .map(|q| Point64::new(q.x + d.x, q.y + d.y))
                    .collect(),
            );
        }
    }
}

/// The shape mapped by the 2D part of `m` (rows x and y of an affine map);
/// a mirror keeps its outlines counter-clockwise. Nothing if it is
/// flattened, as in OpenSCAD.
pub fn transformed(r: &Region, m: &[[f64; 4]; 3]) -> Region {
    let det = m[0][0] * m[1][1] - m[0][1] * m[1][0];
    if det == 0.0 || !det.is_finite() {
        return Region::default();
    }
    let loops = r
        .loops
        .iter()
        .map(|l| {
            let mut l: Vec<V2> = l
                .iter()
                .map(|p| {
                    [
                        m[0][0] * p[0] + m[0][1] * p[1] + m[0][3],
                        m[1][0] * p[0] + m[1][1] * p[1] + m[1][3],
                    ]
                })
                .collect();
            if det < 0.0 {
                l.reverse();
            }
            l
        })
        .collect();
    Region { loops }
}

impl Region {
    pub fn is_empty(&self) -> bool {
        self.loops.is_empty()
    }

    /// The shape with loops that touch at a corner (the tips of a
    /// pentagram, the small loops of a gear's undercut) moved apart there
    /// by a hair, each into its own inside: a mesh of them must not have
    /// edges of four faces.
    pub fn parted(&self) -> Region {
        let scale = self
            .loops
            .iter()
            .flatten()
            .fold(1.0f64, |m, p| m.max(p[0].abs()).max(p[1].abs()));
        let hair = (1e-9 * scale).max(1e-5);
        let mut seen: HashMap<[u64; 2], usize> = HashMap::new();
        let mut loops = self.loops.clone();
        for l in &mut loops {
            let n = l.len();
            let moved: Vec<Option<V2>> = (0..n)
                .map(|i| {
                    let count = seen.entry(l[i].map(f64::to_bits)).or_default();
                    *count += 1;
                    if *count == 1 {
                        return None;
                    }
                    let (p, v, q) = (l[(i + n - 1) % n], l[i], l[(i + 1) % n]);
                    let unit = |a: V2, b: V2| {
                        let len = (b[0] - a[0]).hypot(b[1] - a[1]).max(1e-300);
                        [(b[0] - a[0]) / len, (b[1] - a[1]) / len]
                    };
                    let (d1, d2) = (unit(p, v), unit(v, q));
                    // Left of both edges: the inside.
                    let w = [-d1[1] - d2[1], d1[0] + d2[0]];
                    let len = w[0].hypot(w[1]);
                    if len < 1e-12 {
                        return None;
                    }
                    Some([v[0] + w[0] / len * hair, v[1] + w[1] / len * hair])
                })
                .collect();
            for (i, m) in moved.into_iter().enumerate() {
                if let Some(m) = m {
                    l[i] = m;
                }
            }
        }
        Region { loops }
    }

    /// The lowest and highest corner.
    pub fn bounds(&self) -> Option<(V2, V2)> {
        let first = *self.loops.iter().flatten().next()?;
        Some(
            self.loops
                .iter()
                .flatten()
                .fold((first, first), |(lo, hi), p| {
                    (
                        [lo[0].min(p[0]), lo[1].min(p[1])],
                        [hi[0].max(p[0]), hi[1].max(p[1])],
                    )
                }),
        )
    }

    #[cfg(test)]
    pub fn area(&self) -> f64 {
        self.loops
            .iter()
            .map(|l| {
                (0..l.len())
                    .map(|i| {
                        let (a, b) = (l[i], l[(i + 1) % l.len()]);
                        a[0] * b[1] - a[1] * b[0]
                    })
                    .sum::<f64>()
                    / 2.0
            })
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opensolve_kernel::Op;

    fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<V2> {
        vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]]
    }

    #[test]
    fn crossing_paths_fill_by_even_odd() {
        // A hole given either way round is a hole.
        let r = even_odd(&[rect(0.0, 0.0, 10.0, 10.0), rect(2.0, 2.0, 4.0, 4.0)]);
        assert!((r.area() - 96.0).abs() < 1e-9);
        assert_eq!(r.loops.len(), 2);
        let mut hole = rect(2.0, 2.0, 4.0, 4.0);
        hole.reverse();
        let r = even_odd(&[rect(0.0, 0.0, 10.0, 10.0), hole]);
        assert!((r.area() - 96.0).abs() < 1e-9);
    }

    #[test]
    fn booleans_of_two_squares() {
        let a = even_odd(&[rect(0.0, 0.0, 10.0, 10.0)]);
        let b = even_odd(&[rect(5.0, 5.0, 15.0, 15.0)]);
        let area = |op| boolean(&[&a, &b], op).area();
        assert!((area(Op::Add) - 175.0).abs() < 1e-9);
        assert!((area(Op::Subtract) - 75.0).abs() < 1e-9);
        assert!((area(Op::Intersect) - 25.0).abs() < 1e-9);
        // Squares that only touch are one.
        let c = even_odd(&[rect(10.0, 0.0, 20.0, 10.0)]);
        assert_eq!(boolean(&[&a, &c], Op::Add).loops.len(), 1);
    }

    #[test]
    fn offsets_grow_and_shrink() {
        let a = even_odd(&[rect(0.0, 0.0, 10.0, 10.0)]);
        assert!((offset(&a, 1.0, Join::Miter).area() - 144.0).abs() < 1e-9);
        assert!((offset(&a, -1.0, Join::Miter).area() - 64.0).abs() < 1e-9);
        let round = offset(&a, 1.0, Join::Round(0.01)).area();
        assert!(round > 143.0 && round < 144.0, "{round}");
        let chamfer = offset(&a, 1.0, Join::Chamfer).area();
        assert!(chamfer > 140.0 && chamfer < 144.0, "{chamfer}");
    }

    #[test]
    fn minkowski_of_two_squares() {
        let a = even_odd(&[rect(0.0, 0.0, 10.0, 10.0)]);
        let b = even_odd(&[rect(-1.0, -1.0, 1.0, 1.0)]);
        assert!((minkowski(&[&a, &b]).area() - 144.0).abs() < 1e-9);
    }

    #[test]
    fn a_mirror_keeps_outlines_counter_clockwise() {
        let a = even_odd(&[rect(0.0, 0.0, 10.0, 10.0), rect(2.0, 2.0, 4.0, 4.0)]);
        let m = transformed(
            &a,
            &[
                [-1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
            ],
        );
        assert!((m.area() - 96.0).abs() < 1e-9);
    }
}
