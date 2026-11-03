//! Polygons whose paths cross themselves or each other, as OpenSCAD fills
//! them: by the even-odd rule. The paths are cut where they cross, what
//! is covered an even number of times drops out, and the edges left are
//! chained into loops with the inside on their left; the loops are then
//! triangulated together with their holes. No booleans: the pieces of a
//! self-crossing gear outline would only touch along their edges.

// Points of the paths are compared as they are: the same or not.
#![allow(clippy::float_cmp)]

use std::collections::HashMap;

pub type V2 = [f64; 2];

fn cross(o: V2, a: V2, b: V2) -> f64 {
    (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
}

/// The loops that bound the even-odd fill of `paths`, the inside on the
/// left of each: outlines counter-clockwise, holes clockwise.
pub fn even_odd_loops(paths: &[Vec<V2>]) -> Vec<Vec<V2>> {
    // The segments of all paths, each from a point to the next.
    let mut segs: Vec<(V2, V2)> = Vec::new();
    for p in paths {
        let n = p.len();
        if n < 2 {
            continue;
        }
        for i in 0..n {
            let (a, b) = (p[i], p[(i + 1) % n]);
            if a != b {
                segs.push((a, b));
            }
        }
    }
    let scale = segs
        .iter()
        .flat_map(|(a, b)| [a[0].abs(), a[1].abs(), b[0].abs(), b[1].abs()])
        .fold(1.0f64, f64::max);
    let eps = 1e-12 * scale;
    // Where each segment is cut: by the parameter along it.
    let mut cuts: Vec<Vec<f64>> = vec![vec![0.0, 1.0]; segs.len()];
    let mut order: Vec<usize> = (0..segs.len()).collect();
    let lo = |s: &(V2, V2)| s.0[0].min(s.1[0]);
    let hi = |s: &(V2, V2)| s.0[0].max(s.1[0]);
    order.sort_by(|&i, &j| lo(&segs[i]).total_cmp(&lo(&segs[j])));
    for (k, &i) in order.iter().enumerate() {
        let (a, b) = segs[i];
        let (ylo, yhi) = (a[1].min(b[1]), a[1].max(b[1]));
        for &j in &order[k + 1..] {
            if lo(&segs[j]) > hi(&segs[i]) + eps {
                break;
            }
            let (c, d) = segs[j];
            if c[1].max(d[1]) < ylo - eps || c[1].min(d[1]) > yhi + eps {
                continue;
            }
            for (t, u) in meet(a, b, c, d, eps) {
                cuts[i].push(t);
                cuts[j].push(u);
            }
        }
    }
    // The pieces, as edges between numbered points; an edge covered an
    // even number of times is no border.
    let key = |p: V2| [(p[0] / eps).round() as i64, (p[1] / eps).round() as i64];
    let snap = 1e3; // points closer than 1000 eps are one
    let mut ids: HashMap<[i64; 2], usize> = HashMap::new();
    let mut points: Vec<V2> = Vec::new();
    let mut id_of = |p: V2| -> usize {
        let k = key(p).map(|x| (x as f64 / snap).round() as i64);
        *ids.entry(k).or_insert_with(|| {
            points.push(p);
            points.len() - 1
        })
    };
    let mut count: HashMap<(usize, usize), usize> = HashMap::new();
    for (i, (a, b)) in segs.iter().enumerate() {
        let mut ts = std::mem::take(&mut cuts[i]);
        ts.sort_by(f64::total_cmp);
        ts.dedup_by(|x, y| (*x - *y).abs() < 1e-12);
        let at = |t: f64| [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
        let ids: Vec<usize> = ts.iter().map(|&t| id_of(at(t))).collect();
        for w in ids.windows(2) {
            if w[0] != w[1] {
                let e = (w[0].min(w[1]), w[0].max(w[1]));
                *count.entry(e).or_default() += 1;
            }
        }
    }
    let mut edges: Vec<(usize, usize)> = count
        .into_iter()
        .filter(|(_, n)| n % 2 == 1)
        .map(|(e, _)| e)
        .collect();
    // In a fixed order, so that a file builds the same each time.
    edges.sort_unstable();
    // Each border edge the way round with the inside on its left: a point
    // just left of its middle is inside if a ray from it crosses an odd
    // number of borders.
    let inside = |q: V2| {
        let mut odd = false;
        for &(i, j) in &edges {
            let (a, b) = (points[i], points[j]);
            if (a[1] > q[1]) != (b[1] > q[1]) {
                let x = a[0] + (q[1] - a[1]) / (b[1] - a[1]) * (b[0] - a[0]);
                if x > q[0] {
                    odd = !odd;
                }
            }
        }
        odd
    };
    let mut directed: Vec<(usize, usize)> = Vec::with_capacity(edges.len());
    for &(i, j) in &edges {
        let (a, b) = (points[i], points[j]);
        let m = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
        let len = (b[0] - a[0]).hypot(b[1] - a[1]);
        let off = (len * 1e-6).max(eps * 10.0);
        let left = [
            m[0] - (b[1] - a[1]) / len * off,
            m[1] + (b[0] - a[0]) / len * off,
        ];
        directed.push(if inside(left) { (i, j) } else { (j, i) });
    }
    // Chained into loops, at each point turning as far left as can be:
    // regions that touch at a point stay loops of their own.
    let mut out_of: HashMap<usize, Vec<usize>> = HashMap::new();
    for (k, &(i, _)) in directed.iter().enumerate() {
        out_of.entry(i).or_default().push(k);
    }
    let angle = |k: usize| {
        let (i, j) = directed[k];
        let (a, b) = (points[i], points[j]);
        (b[1] - a[1]).atan2(b[0] - a[0])
    };
    let mut used = vec![false; directed.len()];
    let mut loops = Vec::new();
    for start in 0..directed.len() {
        if used[start] {
            continue;
        }
        let mut ring = Vec::new();
        let mut k = start;
        loop {
            used[k] = true;
            let (i, j) = directed[k];
            ring.push(points[i]);
            if j == directed[start].0 {
                break;
            }
            // The way back, and the first way out clockwise from it.
            let back = angle(k) + std::f64::consts::PI;
            let next = out_of.get(&j).and_then(|ks| {
                ks.iter().copied().filter(|&n| !used[n]).min_by(|&x, &y| {
                    let turn = |n: usize| (back - angle(n)).rem_euclid(std::f64::consts::TAU);
                    turn(x).total_cmp(&turn(y))
                })
            });
            match next {
                Some(n) => k = n,
                None => break,
            }
        }
        if ring.len() >= 3 {
            loops.push(ring);
        }
    }
    apart(&mut loops, scale);
    loops
}

/// Loops that touch at a point (the tips of a pentagram, the small loops
/// of a gear's undercut) moved apart there by a hair, each into its own
/// inside: a mesh of them must not have edges of four faces.
fn apart(loops: &mut [Vec<V2>], scale: f64) {
    let key = |p: V2| p.map(f64::to_bits);
    let mut seen: HashMap<[u64; 2], usize> = HashMap::new();
    let hair = (1e-9 * scale).max(1e-5);
    for l in loops.iter_mut() {
        let n = l.len();
        let moved: Vec<Option<V2>> = (0..n)
            .map(|i| {
                let count = seen.entry(key(l[i])).or_default();
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
}

/// Where segment `a`–`b` and segment `c`–`d` meet, as the parameters
/// along each: a crossing, an end on the other, or the ends of an overlap.
fn meet(a: V2, b: V2, c: V2, d: V2, eps: f64) -> Vec<(f64, f64)> {
    let r = [b[0] - a[0], b[1] - a[1]];
    let s = [d[0] - c[0], d[1] - c[1]];
    let den = r[0] * s[1] - r[1] * s[0];
    let w = [c[0] - a[0], c[1] - a[1]];
    let rr = r[0] * r[0] + r[1] * r[1];
    let ss = s[0] * s[0] + s[1] * s[1];
    let param =
        |p: V2, o: V2, v: [f64; 2], vv: f64| ((p[0] - o[0]) * v[0] + (p[1] - o[1]) * v[1]) / vv;
    let near_line = |p: V2, o: V2, v: [f64; 2], vv: f64| {
        let t = param(p, o, v, vv);
        let q = [o[0] + v[0] * t, o[1] + v[1] * t];
        (p[0] - q[0]).hypot(p[1] - q[1]) <= eps * 1e3
    };
    let mut out = Vec::new();
    let tol = 1e-12;
    if den.abs() > 1e-15 * (rr.sqrt() * ss.sqrt()).max(1e-300) {
        let t = (w[0] * s[1] - w[1] * s[0]) / den;
        let u = (w[0] * r[1] - w[1] * r[0]) / den;
        if (-tol..=1.0 + tol).contains(&t) && (-tol..=1.0 + tol).contains(&u) {
            out.push((t.clamp(0.0, 1.0), u.clamp(0.0, 1.0)));
        }
        return out;
    }
    // Parallel: only if on one line, at the ends of what they share.
    if !near_line(c, a, r, rr) {
        return out;
    }
    for p in [c, d] {
        let t = param(p, a, r, rr);
        if (-tol..=1.0 + tol).contains(&t) {
            out.push((t.clamp(0.0, 1.0), param(p, c, s, ss).clamp(0.0, 1.0)));
        }
    }
    for p in [a, b] {
        let u = param(p, c, s, ss);
        if (-tol..=1.0 + tol).contains(&u) {
            out.push((param(p, a, r, rr).clamp(0.0, 1.0), u.clamp(0.0, 1.0)));
        }
    }
    out
}

/// Whether `q` lies inside loop `p`, by the even-odd rule.
pub fn contains(p: &[V2], q: V2) -> bool {
    let mut odd = false;
    for i in 0..p.len() {
        let (a, b) = (p[i], p[(i + 1) % p.len()]);
        if (a[1] > q[1]) != (b[1] > q[1]) {
            let x = a[0] + (q[1] - a[1]) / (b[1] - a[1]) * (b[0] - a[0]);
            if x > q[0] {
                odd = !odd;
            }
        }
    }
    odd
}

/// Holes joined to their outline by a cut from each hole's rightmost
/// point to a point of the outline it sees: one loop to ear-clip
/// (Eberly's way). The outline counter-clockwise, the holes clockwise.
pub fn bridge(outer: &[V2], holes: &[Vec<V2>]) -> Vec<V2> {
    let mut poly = outer.to_vec();
    let mut holes: Vec<&Vec<V2>> = holes.iter().filter(|h| h.len() >= 3).collect();
    let right = |h: &[V2]| {
        (0..h.len())
            .max_by(|&i, &j| {
                h[i][0]
                    .total_cmp(&h[j][0])
                    .then(h[j][1].total_cmp(&h[i][1]))
            })
            .unwrap_or(0)
    };
    holes.sort_by(|a, b| b[right(b)][0].total_cmp(&a[right(a)][0]));
    for h in holes {
        let mi = right(h);
        let m = h[mi];
        // The nearest edge of the outline a ray to +x meets.
        let n = poly.len();
        let mut best: Option<(f64, usize)> = None;
        for i in 0..n {
            let (a, b) = (poly[i], poly[(i + 1) % n]);
            if (a[1] > m[1]) == (b[1] > m[1]) && a[1] != m[1] {
                continue;
            }
            if a[1] == b[1] {
                continue;
            }
            let x = a[0] + (m[1] - a[1]) / (b[1] - a[1]) * (b[0] - a[0]);
            if x >= m[0] && best.is_none_or(|(bx, _)| x < bx) {
                best = Some((x, i));
            }
        }
        let Some((x, i)) = best else { continue };
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        let ip = [x, m[1]];
        // The end of that edge further right, or a corner inside the
        // triangle it makes that is closer in angle.
        let mut pi = if a[0] > b[0] { i } else { (i + 1) % n };
        let p = poly[pi];
        let mut best_angle = f64::MAX;
        for (k, q) in poly.iter().enumerate() {
            if k == pi || *q == m {
                continue;
            }
            let reflex = cross(poly[(k + n - 1) % n], *q, poly[(k + 1) % n]) < 0.0;
            let tri_in = {
                let (t0, t1, t2) = (m, ip, p);
                let s1 = cross(t0, t1, *q);
                let s2 = cross(t1, t2, *q);
                let s3 = cross(t2, t0, *q);
                (s1 >= 0.0 && s2 >= 0.0 && s3 >= 0.0) || (s1 <= 0.0 && s2 <= 0.0 && s3 <= 0.0)
            };
            if reflex && tri_in && q[0] >= m[0] {
                let ang = (q[1] - m[1]).abs().atan2(q[0] - m[0]);
                if ang < best_angle {
                    best_angle = ang;
                    pi = k;
                }
            }
        }
        let mut merged = Vec::with_capacity(poly.len() + h.len() + 2);
        merged.extend_from_slice(&poly[..=pi]);
        for k in 0..=h.len() {
            merged.push(h[(mi + k) % h.len()]);
        }
        merged.push(poly[pi]);
        merged.extend_from_slice(&poly[pi + 1..]);
        poly = merged;
    }
    poly
}
