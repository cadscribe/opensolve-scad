//! Outlines with their holes made one loop, for ear clipping where there is
//! no other triangulator (in the browser; see `geom::cap`).

// Points of the paths are compared as they are: the same or not.
#![allow(clippy::float_cmp)]

pub type V2 = [f64; 2];

fn cross(o: V2, a: V2, b: V2) -> f64 {
    (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
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
