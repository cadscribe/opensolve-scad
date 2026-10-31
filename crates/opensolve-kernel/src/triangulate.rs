//! Ear clipping of a simple polygon.

fn cross(a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> f64 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}

fn inside(p: [f64; 2], a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> bool {
    cross(a, b, p) >= 0.0 && cross(b, c, p) >= 0.0 && cross(c, a, p) >= 0.0
}

/// Triangles of a counter-clockwise polygon, or `None` if it has no area or
/// is not simple.
pub fn triangulate(points: &[[f64; 2]]) -> Option<Vec<[u32; 3]>> {
    let n = points.len();
    let area: f64 = (0..n)
        .map(|i| cross([0.0, 0.0], points[i], points[(i + 1) % n]))
        .sum::<f64>()
        / 2.0;
    if n < 3 || area <= 1e-12 {
        return None;
    }
    let mut left: Vec<usize> = (0..n).collect();
    let mut out = Vec::with_capacity(n - 2);
    while left.len() > 3 {
        let m = left.len();
        let ear = (0..m).find(|&i| {
            let (a, b, c) = (left[(i + m - 1) % m], left[i], left[(i + 1) % m]);
            let (pa, pb, pc) = (points[a], points[b], points[c]);
            cross(pa, pb, pc) > 1e-14
                && !left
                    .iter()
                    .any(|&q| q != a && q != b && q != c && inside(points[q], pa, pb, pc))
        })?;
        let (a, b, c) = (left[(ear + m - 1) % m], left[ear], left[(ear + 1) % m]);
        out.push([a as u32, b as u32, c as u32]);
        left.remove(ear);
    }
    out.push([left[0] as u32, left[1] as u32, left[2] as u32]);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_concave_polygon_keeps_its_area() {
        let p = [[0.0, 0.0], [4.0, 0.0], [4.0, 4.0], [2.0, 1.0], [0.0, 4.0]];
        let t = triangulate(&p).unwrap();
        assert_eq!(t.len(), 3);
        let area: f64 = t
            .iter()
            .map(|t| cross(p[t[0] as usize], p[t[1] as usize], p[t[2] as usize]) / 2.0)
            .sum();
        assert!((area - 10.0).abs() < 1e-12);
    }

    #[test]
    fn clockwise_is_refused() {
        assert!(triangulate(&[[0.0, 0.0], [0.0, 1.0], [1.0, 0.0]]).is_none());
    }
}
