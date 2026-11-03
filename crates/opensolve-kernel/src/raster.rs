//! Pictures of a solid without a GPU: flat-shaded faces under a z-buffer,
//! with the edges where faces meet at an angle, as the app's 3D view draws
//! them. For previews from the command line and for assistants.

use crate::{Facets, Solid, cross, dot};

/// Two faces meeting flatter than this share no drawn edge.
const EDGE_COS: f64 = 0.985;

/// Pixels per output pixel along each side, smoothed down at the end.
const SAMPLES: usize = 3;

impl Solid {
    /// The edges where two faces meet at an angle, in the order of the
    /// triangle that closes each. The half-edges are put in buckets by
    /// the lower of their two corners rather than in a map: for millions
    /// of triangles (render_baskets has 2.4 million) the map's lookups were
    /// most of the time.
    pub fn feature_edges(&self, facets: &Facets) -> Vec<([f64; 3], [f64; 3])> {
        let half = |h: u32| {
            let t = self.tris[h as usize / 3];
            let k = h as usize % 3;
            (t[k], t[(k + 1) % 3])
        };
        let halves = 3 * self.tris.len() as u32;
        // Each half-edge in the bucket of its lower corner, with the other
        // one, the buckets in the order of the half-edges (three per
        // triangle).
        let mut start = vec![0u32; self.points.len() + 1];
        for h in 0..halves {
            let (a, b) = half(h);
            start[a.min(b) as usize + 1] += 1;
        }
        for v in 1..start.len() {
            start[v] += start[v - 1];
        }
        let mut at = start.clone();
        let mut bucket = vec![(0u32, 0u32); halves as usize];
        for h in 0..halves {
            let (a, b) = half(h);
            let lo = a.min(b) as usize;
            bucket[at[lo] as usize] = (a.max(b), h);
            at[lo] += 1;
        }
        // In a bucket, each half-edge closes the open one that runs the
        // other way along its edge, if there is one, else stays open.
        let mut crease = vec![false; halves as usize];
        let mut open: Vec<u32> = Vec::new();
        for w in start.windows(2) {
            open.clear();
            for &(_, h) in &bucket[w[0] as usize..w[1] as usize] {
                let (a, b) = half(h);
                match open.iter().position(|&o| half(o) == (b, a)) {
                    Some(i) => {
                        let other = open.remove(i);
                        let f = facets.get(self.facets[h as usize / 3]);
                        let g = facets.get(self.facets[other as usize / 3]);
                        crease[h as usize] = f.face != g.face && dot(f.normal, g.normal) < EDGE_COS;
                    }
                    None => {
                        open.retain(|&o| half(o) != (a, b));
                        open.push(h);
                    }
                }
            }
        }
        (0..halves)
            .filter(|&h| crease[h as usize])
            .map(|h| {
                let (a, b) = half(h);
                (self.points[a as usize], self.points[b as usize])
            })
            .collect()
    }
}

/// How a picture is taken.
#[derive(Clone, Copy, Debug)]
pub struct Shot {
    /// Towards the viewer.
    pub toward: [f64; 3],
    /// Up on the picture; made square to `toward`.
    pub up: [f64; 3],
    pub width: usize,
    pub height: usize,
    pub part: [u8; 3],
    pub edge: [u8; 3],
    /// `None`: transparent.
    pub background: Option<[u8; 3]>,
    /// A close-up: this point in the middle, this many millimetres across
    /// the shorter side. `None`: the whole part.
    pub around: Option<([f64; 3], f64)>,
}

impl Default for Shot {
    /// From the front right, above, as the app's Iso view.
    fn default() -> Shot {
        Shot {
            toward: [0.563, -0.604, 0.565],
            up: [0.0, 0.0, 1.0],
            width: 800,
            height: 600,
            part: [139, 160, 196],
            edge: [36, 44, 62],
            background: Some([244, 244, 246]),
            around: None,
        }
    }
}

fn unit(a: [f64; 3]) -> [f64; 3] {
    let l = dot(a, a).sqrt().max(1e-300);
    a.map(|x| x / l)
}

/// The solid as RGBA pixels, row by row from the top, filling the picture
/// with a margin.
#[must_use]
pub fn render(solid: &Solid, facets: &Facets, shot: &Shot) -> Vec<u8> {
    let (w, h) = (shot.width.max(1) * SAMPLES, shot.height.max(1) * SAMPLES);
    let b = unit(shot.toward);
    let r = {
        let r = cross(shot.up, b);
        if dot(r, r) < 1e-12 {
            unit(cross([0.0, 1.0, 0.0], b))
        } else {
            unit(r)
        }
    };
    let u = cross(b, r);
    // Fitted on what is seen: the outline fills the picture, centred.
    if solid.points.is_empty() {
        return background(shot);
    }
    let (mut lo, mut hi) = ([f64::MAX; 2], [f64::MIN; 2]);
    for p in &solid.points {
        let q = [dot(*p, r), dot(*p, u)];
        for i in 0..2 {
            lo[i] = lo[i].min(q[i]);
            hi[i] = hi[i].max(q[i]);
        }
    }
    let span = [(hi[0] - lo[0]).max(1e-9), (hi[1] - lo[1]).max(1e-9)];
    let (scale, mid) = match shot.around {
        Some((at, across)) => (w.min(h) as f64 / across.max(1e-9), [dot(at, r), dot(at, u)]),
        None => (
            0.9 * (w as f64 / span[0]).min(h as f64 / span[1]),
            [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0],
        ),
    };
    let (cx, cy) = (w as f64 / 2.0, h as f64 / 2.0);
    let project = |p: [f64; 3]| {
        [
            cx + (dot(p, r) - mid[0]) * scale,
            cy - (dot(p, u) - mid[1]) * scale,
            dot(p, b),
        ]
    };
    let light = unit(std::array::from_fn(|i| {
        0.35 * u[i] + 0.9 * b[i] - 0.25 * r[i]
    }));
    let mut depth = vec![f64::NEG_INFINITY; w * h];
    let mut color = vec![[0u8; 3]; w * h];
    let pts: Vec<[f64; 3]> = solid.points.iter().map(|&p| project(p)).collect();
    for (t, &facet) in solid.tris.iter().zip(&solid.facets) {
        let [a, bb, c] = t.map(|i| pts[i as usize]);
        let area = (bb[0] - a[0]) * (c[1] - a[1]) - (bb[1] - a[1]) * (c[0] - a[0]);
        if area.abs() < 1e-12 {
            continue;
        }
        let n = facets.get(facet).normal;
        let lit = 0.38 + 0.62 * dot(n, light).max(0.0);
        let shade = shot.part.map(|x| (f64::from(x) * lit).min(255.0) as u8);
        let x0 = a[0].min(bb[0]).min(c[0]).floor().max(0.0) as usize;
        let x1 = (a[0].max(bb[0]).max(c[0]).ceil() as usize).min(w - 1);
        let y0 = a[1].min(bb[1]).min(c[1]).floor().max(0.0) as usize;
        let y1 = (a[1].max(bb[1]).max(c[1]).ceil() as usize).min(h - 1);
        for y in y0..=y1 {
            for x in x0..=x1 {
                let (px, py) = (x as f64 + 0.5, y as f64 + 0.5);
                let w0 = ((bb[0] - px) * (c[1] - py) - (bb[1] - py) * (c[0] - px)) / area;
                let w1 = ((c[0] - px) * (a[1] - py) - (c[1] - py) * (a[0] - px)) / area;
                let w2 = 1.0 - w0 - w1;
                if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                    continue;
                }
                let z = w0 * a[2] + w1 * bb[2] + w2 * c[2];
                let k = y * w + x;
                if z > depth[k] {
                    depth[k] = z;
                    color[k] = shade;
                }
            }
        }
    }
    // Edges over the faces they bound, hidden behind nearer ones. A face
    // seen at a slant comes a long way nearer within a pixel or two.
    let bias = 8.0 / scale;
    let thick = SAMPLES as isize / 2;
    for (p, q) in solid.feature_edges(facets) {
        let (p, q) = (project(p), project(q));
        let steps = ((q[0] - p[0]).hypot(q[1] - p[1]) * 2.0).ceil().max(1.0) as usize;
        for s in 0..=steps {
            let t = s as f64 / steps as f64;
            let at: [f64; 3] = std::array::from_fn(|i| p[i] + (q[i] - p[i]) * t);
            for dy in -thick..=thick {
                for dx in -thick..=thick {
                    let (x, y) = (at[0] as isize + dx, at[1] as isize + dy);
                    if x < 0 || y < 0 || x >= w as isize || y >= h as isize {
                        continue;
                    }
                    let k = y as usize * w + x as usize;
                    if at[2] + bias >= depth[k] {
                        color[k] = shot.edge;
                        depth[k] = depth[k].max(at[2]);
                    }
                }
            }
        }
    }
    // Down to the picture's size, coverage into alpha.
    let mut out = Vec::with_capacity(shot.width * shot.height * 4);
    for y in 0..shot.height {
        for x in 0..shot.width {
            let (mut sum, mut hit) = ([0u32; 3], 0u32);
            for sy in 0..SAMPLES {
                for sx in 0..SAMPLES {
                    let k = (y * SAMPLES + sy) * w + x * SAMPLES + sx;
                    if depth[k] > f64::NEG_INFINITY {
                        hit += 1;
                        for i in 0..3 {
                            sum[i] += u32::from(color[k][i]);
                        }
                    }
                }
            }
            let all = (SAMPLES * SAMPLES) as u32;
            let mean = |i: usize| sum[i].checked_div(hit).unwrap_or(0);
            match shot.background {
                Some(bg) => {
                    for (i, &g) in bg.iter().enumerate() {
                        let v = (mean(i) * hit + u32::from(g) * (all - hit)) / all;
                        out.push(v as u8);
                    }
                    out.push(255);
                }
                None => {
                    out.extend((0..3).map(|i| mean(i) as u8));
                    out.push((hit * 255 / all) as u8);
                }
            }
        }
    }
    out
}

fn background(shot: &Shot) -> Vec<u8> {
    let px = match shot.background {
        Some([r, g, b]) => [r, g, b, 255],
        None => [0; 4],
    };
    px.repeat(shot.width * shot.height)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FaceId, Outline, prism};

    fn block(facets: &mut Facets) -> Solid {
        let outline = Outline {
            points: vec![[0.0, 0.0], [20.0, 0.0], [20.0, 10.0], [0.0, 10.0]],
            faces: (1..=4).map(FaceId).collect(),
        };
        prism(facets, &outline, 0.0, 5.0, FaceId(5), FaceId(6)).expect("prism")
    }

    #[test]
    fn a_block_has_twelve_edges_and_shows_in_the_middle() {
        let mut f = Facets::default();
        let s = block(&mut f);
        assert_eq!(s.feature_edges(&f).len(), 12);
        let shot = Shot {
            width: 64,
            height: 48,
            background: None,
            ..Shot::default()
        };
        let px = render(&s, &f, &shot);
        assert_eq!(px.len(), 64 * 48 * 4);
        let alpha = |x: usize, y: usize| px[(y * 64 + x) * 4 + 3];
        assert_eq!(alpha(32, 24), 255, "the part covers the middle");
        assert_eq!(alpha(0, 0), 0, "the corner is empty");
        // Faces facing other ways are lit differently.
        let shades: std::collections::HashSet<[u8; 3]> = px
            .chunks(4)
            .filter(|p| p[3] == 255)
            .map(|p| [p[0], p[1], p[2]])
            .collect();
        assert!(shades.len() >= 3, "{}", shades.len());
        let edge = Shot::default().edge;
        let lines = px.chunks(4).filter(|p| p[..3] == edge).count();
        assert!(lines > 40, "{lines} edge pixels");
    }

    /// The edges as a map of open half-edges finds them, in its order.
    fn by_map(s: &Solid, facets: &Facets) -> Vec<([f64; 3], [f64; 3])> {
        let mut out = Vec::new();
        let mut seen: std::collections::HashMap<(u32, u32), u32> = Default::default();
        for (t, &facet) in s.tris.iter().zip(&s.facets) {
            for k in 0..3 {
                let (a, b) = (t[k], t[(k + 1) % 3]);
                match seen.remove(&(b, a)) {
                    Some(other) => {
                        let (f, g) = (facets.get(facet), facets.get(other));
                        if f.face != g.face && dot(f.normal, g.normal) < EDGE_COS {
                            out.push((s.points[a as usize], s.points[b as usize]));
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

    #[test]
    fn edges_come_as_a_map_of_half_edges_finds_them() {
        let mut f = Facets::default();
        let a = block(&mut f);
        // Two blocks sharing the corners of an edge (four triangles on it)
        // and a stray triangle.
        let b = a
            .mapped(
                &mut f,
                &[
                    [1.0, 0.0, 0.0, 20.0],
                    [0.0, 1.0, 0.0, 10.0],
                    [0.0, 0.0, 1.0, 0.0],
                ],
            )
            .expect("moved");
        let mut s = Solid::merged(&[a, b]);
        let n = s.points.len() as u32;
        s.points.push([40.0, 40.0, 40.0]);
        s.tris.push([0, 1, n]);
        s.facets.push(s.facets[0]);
        let mut weld: std::collections::HashMap<[u64; 3], u32> = Default::default();
        let ids: Vec<u32> = s
            .points
            .iter()
            .enumerate()
            .map(|(i, p)| *weld.entry(p.map(f64::to_bits)).or_insert(i as u32))
            .collect();
        for t in &mut s.tris {
            *t = t.map(|i| ids[i as usize]);
        }
        let want = by_map(&s, &f);
        assert!(want.len() > 12);
        assert_eq!(s.feature_edges(&f), want);
    }

    #[test]
    fn nothing_gives_the_background() {
        let shot = Shot {
            width: 4,
            height: 3,
            ..Shot::default()
        };
        let px = render(&Solid::default(), &Facets::default(), &shot);
        assert_eq!(px.len(), 4 * 3 * 4);
        assert_eq!(&px[..4], &[244, 244, 246, 255]);
    }
}
