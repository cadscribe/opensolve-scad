//! The letters of a text as closed outlines: glyph outlines of a TrueType
//! or OpenType font, in cap heights. A text one unit high has capitals one
//! unit tall; it starts at the origin, on the baseline.
//!
//! Fonts are given as the bytes of a font file and the number of a face in
//! it (0 unless the file is a collection); which file a font name means is
//! the caller's business. OpenSCAD's `text()` (opensolve-scad) and
//! OpenSolve's sketches, as SolveSpace draws text, both use it: OpenSCAD
//! sets letters kerned and cuts every curve into so many chords; SolveSpace
//! does not kern and cuts curves to a tolerance.

use ttf_parser::{Face, GlyphId, OutlineBuilder};

/// How the curves of a letter are cut into chords.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Chords {
    /// Each chord within this distance of its curve, in cap heights.
    Within(f64),
    /// Every curve in so many chords, as OpenSCAD's `text` cuts them.
    Steps(usize),
}

/// Whether `data` holds a font with face `index` that can be read.
pub fn is_font(data: &[u8], index: u32) -> bool {
    Face::parse(data, index).is_ok()
}

/// The height of a capital in font units: the top of an "A".
fn cap_height(face: &Face) -> f64 {
    face.glyph_index('A')
        .and_then(|g| face.glyph_bounding_box(g))
        .map_or(f64::from(face.units_per_em()), |b| f64::from(b.y_max))
}

/// A font's cap height in its em, the size its glyphs are designed at:
/// what an OpenSCAD text sized by the em is scaled by. 0.7 for a font that
/// cannot be read.
pub fn cap_per_em(data: &[u8], index: u32) -> f64 {
    Face::parse(data, index).map_or(0.7, |f| cap_height(&f) / f64::from(f.units_per_em()))
}

/// The glyphs of a text with where each starts, in font units; with
/// `kern`, pairs moved closer or apart as the font's `kern` table says (as
/// OpenSCAD lays out text; SolveSpace does not).
fn glyphs(face: &Face, text: &str, kern: bool) -> (Vec<(GlyphId, f64)>, f64) {
    let mut x = 0.0;
    let mut out: Vec<(GlyphId, f64)> = Vec::new();
    for ch in text.chars() {
        let g = face.glyph_index(ch).unwrap_or(GlyphId(0));
        if kern && let Some(&(prev, _)) = out.last() {
            x += kerning(face, prev, g);
        }
        out.push((g, x));
        x += f64::from(face.glyph_hor_advance(g).unwrap_or(0));
    }
    (out, x)
}

/// How much closer (negative) or apart a pair of glyphs is set.
fn kerning(face: &Face, left: GlyphId, right: GlyphId) -> f64 {
    face.tables().kern.map_or(0.0, |k| {
        k.subtables
            .into_iter()
            .filter(|t| t.horizontal && !t.variable && !t.has_cross_stream)
            .find_map(|t| t.glyphs_kerning(left, right))
            .map_or(0.0, f64::from)
    })
}

/// How long a text is, in its cap heights, kerned or not. Never zero, so
/// that the box of an empty text still has a size; 1 for a font that
/// cannot be read.
pub fn width(text: &str, data: &[u8], index: u32, kern: bool) -> f64 {
    Face::parse(data, index).map_or(1.0, |face| {
        let w = glyphs(&face, text, kern).1 / cap_height(&face);
        if w > 1e-9 { w } else { 1.0 }
    })
}

/// Collects one glyph's contours as polylines.
struct Pen {
    /// From font units to cap heights, and the glyph's start.
    scale: f64,
    dx: f64,
    /// Largest deviation of a chord, in font units.
    tol: f64,
    /// Or so many chords for every curve, as OpenSCAD cuts them.
    steps: Option<usize>,
    at: [f64; 2],
    contour: Vec<[f64; 2]>,
    done: Vec<Vec<[f64; 2]>>,
}

impl Pen {
    fn put(&mut self, p: [f64; 2]) {
        self.contour
            .push([(self.dx + p[0]) * self.scale, p[1] * self.scale]);
        self.at = p;
    }

    /// `n` chords along a curve, `n` from the size of its bend.
    fn chords(&mut self, bend: f64, n_for: f64, curve: impl Fn(f64) -> [f64; 2]) {
        let n = self
            .steps
            .unwrap_or_else(|| ((bend * n_for / self.tol).sqrt().ceil() as usize).clamp(1, 64));
        for i in 1..=n {
            self.put(curve(i as f64 / n as f64));
        }
    }
}

fn len(x: f64, y: f64) -> f64 {
    x.hypot(y)
}

impl OutlineBuilder for Pen {
    fn move_to(&mut self, x: f32, y: f32) {
        self.close();
        self.put([f64::from(x), f64::from(y)]);
    }

    fn line_to(&mut self, x: f32, y: f32) {
        self.put([f64::from(x), f64::from(y)]);
    }

    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let (p0, p1, p2) = (
            self.at,
            [f64::from(x1), f64::from(y1)],
            [f64::from(x), f64::from(y)],
        );
        // A chord over 1/n of a quadratic is off by |p0 - 2 p1 + p2| / 4n².
        let bend = len(p0[0] - 2.0 * p1[0] + p2[0], p0[1] - 2.0 * p1[1] + p2[1]);
        self.chords(bend, 0.25, |t| {
            let s = 1.0 - t;
            std::array::from_fn(|k| s * s * p0[k] + 2.0 * s * t * p1[k] + t * t * p2[k])
        });
    }

    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let (p0, p1, p2, p3) = (
            self.at,
            [f64::from(x1), f64::from(y1)],
            [f64::from(x2), f64::from(y2)],
            [f64::from(x), f64::from(y)],
        );
        // For a cubic, at most 3/4 of the larger second difference over n².
        let bend = len(p0[0] - 2.0 * p1[0] + p2[0], p0[1] - 2.0 * p1[1] + p2[1]).max(len(
            p1[0] - 2.0 * p2[0] + p3[0],
            p1[1] - 2.0 * p2[1] + p3[1],
        ));
        self.chords(bend, 0.75, |t| {
            let s = 1.0 - t;
            std::array::from_fn(|k| {
                s * s * s * p0[k]
                    + 3.0 * s * s * t * p1[k]
                    + 3.0 * s * t * t * p2[k]
                    + t * t * t * p3[k]
            })
        });
    }

    fn close(&mut self) {
        let mut c = std::mem::take(&mut self.contour);
        let (first, last) = (c.first().copied(), c.last().copied());
        if let (true, Some(a), Some(b)) = (c.len() > 1, first, last)
            && len(a[0] - b[0], a[1] - b[1]) < 1e-12
        {
            c.pop();
        }
        if c.len() >= 3 {
            self.done.push(c);
        }
    }
}

/// The closed outlines of a text's letters, in cap heights, the last point
/// of each not repeated; kerned or not. Holes, as in "o", are outlines
/// inside others, turning the other way. Nothing for a font that cannot be
/// read.
pub fn outlines(
    text: &str,
    data: &[u8],
    index: u32,
    chords: Chords,
    kern: bool,
) -> Vec<Vec<[f64; 2]>> {
    let Ok(face) = Face::parse(data, index) else {
        return Vec::new();
    };
    let cap = cap_height(&face);
    let (tol, steps) = match chords {
        Chords::Within(tol) => (tol, None),
        Chords::Steps(n) => (1e-3, Some(n.max(1))),
    };
    let mut pen = Pen {
        scale: 1.0 / cap,
        dx: 0.0,
        tol: (tol * cap).max(1e-3),
        steps,
        at: [0.0; 2],
        contour: Vec::new(),
        done: Vec::new(),
    };
    for (g, x) in glyphs(&face, text, kern).0 {
        // Where SolveSpace puts a glyph: its start, moved by the side
        // bearing less the left of its box (nothing, for most fonts).
        let bearing = face.glyph_hor_side_bearing(g).unwrap_or(0);
        let left = face.glyph_bounding_box(g).map_or(0, |b| b.x_min);
        pen.dx = x + f64::from(bearing) - f64::from(left);
        face.outline_glyph(g, &mut pen);
        pen.close();
    }
    pen.done
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Liberation Sans, which opensolve-scad carries (SIL Open Font
    /// License).
    const SANS: &[u8] = include_bytes!("../../../assets/fonts/LiberationSans-Regular.ttf");

    fn area(p: &[[f64; 2]]) -> f64 {
        (0..p.len())
            .map(|i| {
                let (a, b) = (p[i], p[(i + 1) % p.len()]);
                a[0] * b[1] - a[1] * b[0]
            })
            .sum::<f64>()
            / 2.0
    }

    #[test]
    fn every_curve_in_so_many_chords() {
        let points = |steps| -> usize {
            outlines("O", SANS, 0, Chords::Steps(steps), true)
                .iter()
                .map(Vec::len)
                .sum()
        };
        let (few, more) = (points(2), points(8));
        assert!(few >= 8 && more > 3 * few, "{few} {more}");
    }

    #[test]
    fn a_capital_is_one_unit_tall() {
        let o = outlines("H", SANS, 0, Chords::Within(1e-3), false);
        assert_eq!(o.len(), 1);
        let top = o[0].iter().map(|p| p[1]).fold(f64::MIN, f64::max);
        let bottom = o[0].iter().map(|p| p[1]).fold(f64::MAX, f64::min);
        assert!(
            (top - 1.0).abs() < 0.02 && bottom.abs() < 1e-9,
            "{bottom} {top}"
        );
    }

    #[test]
    fn an_o_has_a_hole_turning_the_other_way() {
        let o = outlines("o", SANS, 0, Chords::Within(1e-3), false);
        assert_eq!(o.len(), 2);
        let (a, b) = (area(&o[0]), area(&o[1]));
        assert!(a * b < 0.0, "{a} {b}");
        assert!(o[0].len() > 16);
    }

    #[test]
    fn a_finer_tolerance_gives_more_chords() {
        let coarse = outlines("S", SANS, 0, Chords::Within(0.05), false)[0].len();
        let fine = outlines("S", SANS, 0, Chords::Within(0.0005), false)[0].len();
        assert!(fine > coarse * 2, "{coarse} {fine}");
    }

    #[test]
    fn widths_kerned_and_not() {
        assert!(width("11", SANS, 0, false) > 1.7 * width("1", SANS, 0, false) - 1e-9);
        assert!((width("", SANS, 0, true) - 1.0).abs() < 1e-12);
        // Liberation Sans kerns "AV" closer.
        assert!(width("AV", SANS, 0, true) < width("AV", SANS, 0, false));
    }

    #[test]
    fn what_is_no_font_gives_defaults() {
        assert!(!is_font(b"not a font", 0));
        assert!(is_font(SANS, 0));
        assert!((cap_per_em(b"", 0) - 0.7).abs() < 1e-12);
        assert!(outlines("A", b"", 0, Chords::Steps(4), true).is_empty());
        let c = cap_per_em(SANS, 0);
        assert!(c > 0.6 && c < 0.8, "{c}");
    }
}
