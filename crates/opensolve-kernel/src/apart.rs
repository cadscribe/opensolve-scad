//! Which solids of a list may meet: their surfaces come near each other,
//! or one holds the other. Those that do not can be put together without a
//! boolean; a union of many parts apart (holes in a row, the ribs of a
//! pattern) is then no union at all.

use crate::{Solid, cross, dot, sub};

type P = [f64; 3];

/// A tree of boxes over a solid's triangles.
struct Bvh {
    /// Box and either the triangles `[start, start + len)` of `order` or,
    /// with `len` 0, the children at `start` and `start + 1`.
    nodes: Vec<Node>,
    order: Vec<u32>,
}

/// Triangles in a leaf, at most.
const LEAF: usize = 4;

fn tri(s: &Solid, t: u32) -> [P; 3] {
    s.tris[t as usize].map(|i| s.points[i as usize])
}

/// A tree's node: its box and either its triangles or its children.
type Node = (P, P, u32, u32);

/// Triangles beyond which a tree is built on several threads.
const BIG: usize = 1 << 16;

impl Bvh {
    /// The tree, split at the middle of the triangles' centres along the
    /// longest side of each box. Of a large solid, the subtrees below the
    /// first few splits are built on threads of their own: each from its
    /// own part of the triangles, so the tree is the one a single thread
    /// builds.
    fn new(s: &Solid) -> Bvh {
        let n = s.tris.len();
        let centre: Vec<P> = (0..n as u32)
            .map(|t| {
                let [a, p, c] = tri(s, t);
                std::array::from_fn(|k| (a[k] + p[k] + c[k]) / 3.0)
            })
            .collect();
        let mut order: Vec<u32> = (0..n as u32).collect();
        // Subtrees no larger than this are built apart; of a small solid,
        // none.
        let apart = if n >= BIG {
            n.div_ceil(4 * crate::threads())
        } else {
            0
        };
        let mut later: Vec<(usize, usize, usize)> = Vec::new();
        let mut nodes = Vec::with_capacity(2 * n / LEAF + 1);
        split(s, &centre, &mut order, 0, &mut nodes, apart, &mut later);
        let built = crate::parallel(later.len(), |k| {
            let (_, start, len) = later[k];
            let mut part = order[start..start + len].to_vec();
            let mut sub = Vec::with_capacity(2 * len / LEAF + 1);
            split(s, &centre, &mut part, start, &mut sub, 0, &mut Vec::new());
            (sub, part)
        });
        for (&(node, start, len), (sub, part)) in later.iter().zip(built) {
            order[start..start + len].copy_from_slice(&part);
            // The subtree's root takes the node kept for it, the rest go
            // after the nodes there are.
            let base = nodes.len() as u32 - 1;
            for (k, x) in sub.into_iter().enumerate() {
                let x = if x.3 == 0 {
                    (x.0, x.1, x.2 + base, 0)
                } else {
                    x
                };
                if k == 0 {
                    nodes[node] = x;
                } else {
                    nodes.push(x);
                }
            }
        }
        Bvh { nodes, order }
    }
}

/// The tree of the triangles `order` (those from `offset` on of the whole
/// order) into `nodes`, its root first. Subtrees of more than `LEAF` and at
/// most `apart` triangles (none for 0) are left to be built later: their
/// node, where their triangles begin in the whole order, and how many.
fn split(
    s: &Solid,
    centre: &[P],
    order: &mut [u32],
    offset: usize,
    nodes: &mut Vec<Node>,
    apart: usize,
    later: &mut Vec<(usize, usize, usize)>,
) {
    nodes.push(([0.0; 3], [0.0; 3], 0, 0));
    let mut stack = vec![(nodes.len() - 1, 0usize, order.len())];
    while let Some((node, start, len)) = stack.pop() {
        if len > LEAF && len <= apart {
            later.push((node, offset + start, len));
            continue;
        }
        let mut lo = [f64::MAX; 3];
        let mut hi = [f64::MIN; 3];
        for &t in &order[start..start + len] {
            for p in tri(s, t) {
                for k in 0..3 {
                    lo[k] = lo[k].min(p[k]);
                    hi[k] = hi[k].max(p[k]);
                }
            }
        }
        if len <= LEAF {
            nodes[node] = (lo, hi, (offset + start) as u32, len as u32);
            continue;
        }
        let axis = (0..3)
            .max_by(|&x, &y| (hi[x] - lo[x]).total_cmp(&(hi[y] - lo[y])))
            .unwrap_or(0);
        let half = len / 2;
        order[start..start + len].select_nth_unstable_by(half, |&x, &y| {
            centre[x as usize][axis].total_cmp(&centre[y as usize][axis])
        });
        let left = nodes.len();
        nodes.push(([0.0; 3], [0.0; 3], 0, 0));
        nodes.push(([0.0; 3], [0.0; 3], 0, 0));
        nodes[node] = (lo, hi, left as u32, 0);
        stack.push((left, start, half));
        stack.push((left + 1, start + half, len - half));
    }
}

fn boxes_meet(a: (P, P), b: (P, P), eps: f64) -> bool {
    (0..3).all(|k| a.0[k] <= b.1[k] + eps && b.0[k] <= a.1[k] + eps)
}

/// Whether two triangles are apart by more than `eps` along some axis
/// (their normals, the crosses of their edges, and, for triangles in one
/// plane, the normals of their edges in it): if not, they may meet.
fn apart(a: &[P; 3], b: &[P; 3], eps: f64) -> bool {
    let ea = [sub(a[1], a[0]), sub(a[2], a[1]), sub(a[0], a[2])];
    let eb = [sub(b[1], b[0]), sub(b[2], b[1]), sub(b[0], b[2])];
    let separates = |axis: P| {
        let l = dot(axis, axis).sqrt();
        if l < 1e-30 {
            return false;
        }
        let span = |t: &[P; 3]| {
            let d = t.map(|p| dot(axis, p));
            (d[0].min(d[1]).min(d[2]), d[0].max(d[1]).max(d[2]))
        };
        let ((a0, a1), (b0, b1)) = (span(a), span(b));
        a1 + eps * l < b0 || b1 + eps * l < a0
    };
    let (na, nb) = (cross(ea[0], ea[1]), cross(eb[0], eb[1]));
    if separates(na) || separates(nb) {
        return true;
    }
    ea.iter()
        .any(|x| eb.iter().any(|y| separates(cross(*x, *y))))
        || ea.iter().any(|e| separates(cross(na, *e)))
        || eb.iter().any(|e| separates(cross(nb, *e)))
}

/// Whether the surfaces of two solids come within `eps` of each other.
fn surfaces_meet(a: &Solid, ta: &Bvh, b: &Solid, tb: &Bvh, eps: f64) -> bool {
    let mut stack = vec![(0usize, 0usize)];
    while let Some((x, y)) = stack.pop() {
        let (nx, ny) = (ta.nodes[x], tb.nodes[y]);
        if !boxes_meet((nx.0, nx.1), (ny.0, ny.1), eps) {
            continue;
        }
        match (nx.3 > 0, ny.3 > 0) {
            (true, true) => {
                for &i in &ta.order[nx.2 as usize..(nx.2 + nx.3) as usize] {
                    let p = tri(a, i);
                    for &j in &tb.order[ny.2 as usize..(ny.2 + ny.3) as usize] {
                        if !apart(&p, &tri(b, j), eps) {
                            return true;
                        }
                    }
                }
            }
            // The larger box is split first.
            (false, leaf_y) => {
                let size = |n: (P, P, u32, u32)| (0..3).map(|k| n.1[k] - n.0[k]).sum::<f64>();
                if leaf_y || size(nx) >= size(ny) {
                    stack.push((nx.2 as usize, y));
                    stack.push((nx.2 as usize + 1, y));
                } else {
                    stack.push((x, ny.2 as usize));
                    stack.push((x, ny.2 as usize + 1));
                }
            }
            (true, false) => {
                stack.push((x, ny.2 as usize));
                stack.push((x, ny.2 as usize + 1));
            }
        }
    }
    false
}

/// The pairs of `solids` that may meet: their boxes overlap and their
/// surfaces come within a hair of each other, or one holds a corner of
/// the other. Those of no pair are apart from each other.
pub fn meeting(solids: &[&Solid]) -> Vec<(usize, usize)> {
    let boxes: Vec<Option<(P, P)>> = solids.iter().map(|s| s.bounds()).collect();
    let size = boxes
        .iter()
        .flatten()
        .flat_map(|(lo, hi)| lo.iter().chain(hi.iter()))
        .fold(1.0f64, |m, x| m.max(x.abs()));
    let eps = 1e-9 * size;
    // Candidates by their boxes, swept along x.
    let mut by_x: Vec<usize> = (0..solids.len()).filter(|&i| boxes[i].is_some()).collect();
    by_x.sort_by(|&i, &j| {
        let (a, b) = (
            boxes[i].map_or(0.0, |b| b.0[0]),
            boxes[j].map_or(0.0, |b| b.0[0]),
        );
        a.total_cmp(&b)
    });
    let mut pairs = Vec::new();
    for (k, &i) in by_x.iter().enumerate() {
        let Some(bi) = boxes[i] else { continue };
        for &j in &by_x[k + 1..] {
            let Some(bj) = boxes[j] else { continue };
            if bj.0[0] > bi.1[0] + eps {
                break;
            }
            if boxes_meet(bi, bj, eps) {
                pairs.push((i.min(j), i.max(j)));
            }
        }
    }
    if pairs.is_empty() {
        return pairs;
    }
    // The trees of those in a pair, then the pairs, on threads of their own.
    let mut used: Vec<usize> = pairs.iter().flat_map(|&(i, j)| [i, j]).collect();
    used.sort_unstable();
    used.dedup();
    let built = crate::parallel(used.len(), |k| Bvh::new(solids[used[k]]));
    let mut trees: Vec<Option<Bvh>> = (0..solids.len()).map(|_| None).collect();
    for (k, t) in used.into_iter().zip(built) {
        trees[k] = Some(t);
    }
    // Only a solid whose box holds the other's can hold the other.
    let holds =
        |a: (P, P), b: (P, P)| (0..3).all(|k| a.0[k] <= b.0[k] + eps && b.1[k] <= a.1[k] + eps);
    let meets = crate::parallel(pairs.len(), |k| {
        let (i, j) = pairs[k];
        let (Some(ti), Some(tj), Some(bi), Some(bj)) = (&trees[i], &trees[j], boxes[i], boxes[j])
        else {
            return false;
        };
        let (a, b) = (solids[i], solids[j]);
        surfaces_meet(a, ti, b, tj, eps)
            || holds(bi, bj) && b.points.first().is_some_and(|p| a.contains(*p))
            || holds(bj, bi) && a.points.first().is_some_and(|p| b.contains(*p))
    });
    pairs
        .into_iter()
        .zip(meets)
        .filter_map(|(p, m)| m.then_some(p))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FaceId, Facets};

    fn cube(f: &mut Facets, o: P, s: f64) -> Solid {
        let p = |i: usize| -> P {
            std::array::from_fn(|k| o[k] + if i >> k & 1 == 1 { s } else { 0.0 })
        };
        let quads = [
            [0, 2, 3, 1],
            [4, 5, 7, 6],
            [0, 1, 5, 4],
            [2, 6, 7, 3],
            [0, 4, 6, 2],
            [1, 3, 7, 5],
        ];
        let tris: Vec<[P; 3]> = quads
            .iter()
            .flat_map(|q| [[p(q[0]), p(q[1]), p(q[2])], [p(q[0]), p(q[2]), p(q[3])]])
            .collect();
        Solid::from_triangles(&tris, f, |k| FaceId(k as u64))
    }

    #[test]
    fn a_large_tree_built_on_threads_is_the_one_built_on_one() {
        let mut f = Facets::default();
        let cubes: Vec<Solid> = (0..6000)
            .map(|i| {
                let o = [
                    (i % 20) as f64 * 0.7,
                    (i / 20 % 20) as f64 * 0.7,
                    (i / 400) as f64,
                ];
                cube(&mut f, o, 1.0)
            })
            .collect();
        let s = Solid::merged(&cubes);
        assert!(s.tris.len() >= BIG);
        let leaves = |nodes: &[Node], order: &[u32]| {
            let mut out: Vec<(Vec<u32>, [u64; 6])> = nodes
                .iter()
                .filter(|n| n.3 > 0)
                .map(|n| {
                    let mut t = order[n.2 as usize..(n.2 + n.3) as usize].to_vec();
                    t.sort_unstable();
                    let b = |k: usize| if k < 3 { n.0[k] } else { n.1[k - 3] };
                    (t, std::array::from_fn::<u64, 6, _>(|k| b(k).to_bits()))
                })
                .collect();
            out.sort_unstable();
            out
        };
        let tree = Bvh::new(&s);
        let centre: Vec<P> = (0..s.tris.len() as u32)
            .map(|t| {
                let [a, p, c] = tri(&s, t);
                std::array::from_fn(|k| (a[k] + p[k] + c[k]) / 3.0)
            })
            .collect();
        let mut order: Vec<u32> = (0..s.tris.len() as u32).collect();
        let mut nodes = Vec::new();
        split(&s, &centre, &mut order, 0, &mut nodes, 0, &mut Vec::new());
        assert_eq!(tree.nodes.len(), nodes.len());
        assert_eq!(tree.order, order);
        assert_eq!(leaves(&tree.nodes, &tree.order), leaves(&nodes, &order));
        // Every node's children lie in it.
        for n in tree.nodes.iter().filter(|n| n.3 == 0) {
            for c in [&tree.nodes[n.2 as usize], &tree.nodes[n.2 as usize + 1]] {
                assert!((0..3).all(|k| n.0[k] <= c.0[k] && c.1[k] <= n.1[k]));
            }
        }
    }

    #[test]
    fn apart_touching_overlapping_and_inside() {
        let mut f = Facets::default();
        let solids = [
            cube(&mut f, [0.0; 3], 1.0),
            // Apart, though the boxes overlap on x and y.
            cube(&mut f, [0.5, 0.5, 2.0], 1.0),
            // Apart beside it, in the planes of its top and bottom.
            cube(&mut f, [0.0, 1.5, 0.0], 1.0),
            // Touching the first on a face.
            cube(&mut f, [1.0, 0.0, 0.0], 1.0),
            // Inside a large one, apart from its surface.
            cube(&mut f, [10.0, 10.0, 10.0], 10.0),
            cube(&mut f, [14.0, 14.0, 14.0], 1.0),
        ];
        let refs: Vec<&Solid> = solids.iter().collect();
        let mut m = meeting(&refs);
        m.sort_unstable();
        assert_eq!(m, vec![(0, 3), (4, 5)]);
    }
}
