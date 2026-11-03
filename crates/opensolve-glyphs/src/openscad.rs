//! A text laid out as OpenSCAD 2026 lays it out with FreeType and
//! HarfBuzz, to the micrometre: what its `text()` makes.

/// A text laid out as OpenSCAD lays it out, in units of its `size`: the
/// outlines of its letters, each glyph at its pen position, and what
/// `halign` and `valign` go by.
#[derive(Clone, Debug)]
pub struct Laid {
    pub outlines: Vec<Vec<[f64; 2]>>,
    /// The advances of all glyphs, `spacing` included.
    pub advance: f64,
    /// The highest and lowest ink (pixel boxes, as FreeType fits them to
    /// its grid), `None` without ink.
    pub ink: Option<(f64, f64)>,
}

/// OpenSCAD asks FreeType for an em of 10⁵ 26.6 units at 100 dpi (so a
/// size is 1/0.72 of an em) and divides what it gets by 10⁵.
const FT_SCALE: f64 = 1e5;
/// That em in 26.6 units: FreeType's `FT_REQUEST_HEIGHT`, 10⁵ points at
/// 100 dpi, rounded down as it rounds.
const FT_PPEM: i64 = (100_000 * 100 + 36) / 72;

/// FreeType's `FT_DivFix`: `a / b` in 16.16, rounded.
fn div_fix(a: i64, b: i64) -> i64 {
    (a * 65536 + b / 2) / b
}

/// FreeType's `FT_MulFix`: `a · b / 2¹⁶`, rounded half away from zero.
fn mul_fix(a: i64, b: i64) -> i64 {
    let p = (a * b).abs();
    let r = (p + 0x8000) >> 16;
    if (a < 0) != (b < 0) { -r } else { r }
}

/// The glyph outlines of a face as FreeType hands them to OpenSCAD: every
/// point in integer 26.6 units of the em above, the curves then cut into
/// `steps` chords as OpenSCAD's `DrawingCallback` cuts them.
struct FtPen {
    /// The 16.16 scale from font units to 26.6 units.
    scale: i64,
    /// The glyph's start in 26.6 units (advances as OpenSCAD sums them,
    /// divided by its 10⁵), and its shift in font units.
    at: [f64; 2],
    shift: f64,
    steps: usize,
    pen: [f64; 2],
    contour: Vec<[f64; 2]>,
    done: Vec<Vec<[f64; 2]>>,
    /// The box of all points, control points too, in 26.6 units.
    lo: [i64; 2],
    hi: [i64; 2],
}

impl FtPen {
    /// A point of the font, in units of the size, relative to the glyph.
    fn q(&mut self, x: f32, y: f32) -> [f64; 2] {
        // A point of the font scaled; one between two off-curve points
        // (half a unit, as ttf-parser gives it) halfway between theirs, as
        // FreeType puts it after scaling them.
        let scaled = |u: f64| {
            let lo = u.floor();
            let a = mul_fix(lo as i64, self.scale);
            if u - lo < 0.25 {
                a
            } else {
                (a + mul_fix(lo as i64 + 1, self.scale)) / 2
            }
        };
        let v = [scaled(f64::from(x) + self.shift), scaled(f64::from(y))];
        for (k, x) in v.iter().enumerate() {
            self.lo[k] = self.lo[k].min(*x);
            self.hi[k] = self.hi[k].max(*x);
        }
        [v[0] as f64 / FT_SCALE, v[1] as f64 / FT_SCALE]
    }

    fn put(&mut self, p: [f64; 2]) {
        self.contour.push([p[0] + self.at[0], p[1] + self.at[1]]);
        self.pen = p;
    }

    fn close(&mut self) {
        let c = std::mem::take(&mut self.contour);
        if c.len() >= 3 {
            self.done.push(c);
        }
    }
}

impl ttf_parser::OutlineBuilder for FtPen {
    fn move_to(&mut self, x: f32, y: f32) {
        self.close();
        let p = self.q(x, y);
        self.put(p);
    }

    fn line_to(&mut self, x: f32, y: f32) {
        let p = self.q(x, y);
        self.put(p);
    }

    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let (p0, c, p) = (self.pen, self.q(x1, y1), self.q(x, y));
        let n = self.steps;
        for i in 1..=n {
            let a = i as f64 * (1.0 / n as f64);
            let (s, t) = ((1.0 - a).powi(2), a.powi(2));
            let m = 2.0 * (1.0 - a) * a;
            self.put(std::array::from_fn(|k| p0[k] * s + c[k] * m + p[k] * t));
        }
        self.pen = p;
    }

    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let (p0, c1, c2, p) = (self.pen, self.q(x1, y1), self.q(x2, y2), self.q(x, y));
        let n = self.steps;
        for i in 1..=n {
            let a = i as f64 * (1.0 / n as f64);
            let b = 1.0 - a;
            self.put(std::array::from_fn(|k| {
                p0[k] * b.powi(3)
                    + c1[k] * 3.0 * b.powi(2) * a
                    + c2[k] * 3.0 * b * a.powi(2)
                    + p[k] * a.powi(3)
            }));
        }
        self.pen = p;
    }

    fn close(&mut self) {
        FtPen::close(self);
    }
}

/// `text` as OpenSCAD 2026 lays it out with FreeType and HarfBuzz: the
/// outlines at FreeType's hinted size (a TrueType font whose head asks for
/// whole pixels per em gets them: an em of 2170 pixels, not 2170.14), its
/// points on FreeType's 26.6 grid; the advances (kerned by the font's
/// `kern` table) at the size unrounded, as HarfBuzz takes them, each times
/// `spacing`; and the ink's height from boxes fitted to whole pixels.
pub fn lay_out(text: &str, data: &[u8], index: u32, steps: usize, spacing: f64) -> Option<Laid> {
    let face = ttf_parser::Face::parse(data, index).ok()?;
    let upem = i64::from(face.units_per_em());
    let whole_pixels = face.tables().glyf.is_some()
        && face
            .raw_face()
            .table(ttf_parser::Tag::from_bytes(b"head"))
            .filter(|h| h.len() >= 18)
            .is_some_and(|h| u16::from_be_bytes([h[16], h[17]]) & 8 != 0);
    let hinted = if whole_pixels {
        ((FT_PPEM + 32) >> 6) << 6
    } else {
        FT_PPEM
    };
    let outline_scale = div_fix(hinted, upem);
    // HarfBuzz's scale: FreeType's unrounded one, in 26.6 units per em.
    let advance_scale = div_fix(FT_PPEM, upem);
    let hb_scale = (advance_scale * upem + 0x8000) >> 16;
    let hb_mult = (hb_scale << 16) / upem;
    let kern = |l: ttf_parser::GlyphId, r: ttf_parser::GlyphId| -> i64 {
        face.tables().kern.map_or(0, |k| {
            k.subtables
                .into_iter()
                .filter(|t| t.horizontal && !t.variable && !t.has_cross_stream)
                .find_map(|t| t.glyphs_kerning(l, r))
                .map_or(0, |v| (i64::from(v) * hb_mult + 0x8000) >> 16)
        })
    };
    let mut pen = FtPen {
        scale: outline_scale,
        at: [0.0; 2],
        shift: 0.0,
        steps: steps.max(1),
        pen: [0.0; 2],
        contour: Vec::new(),
        done: Vec::new(),
        lo: [0; 2],
        hi: [0; 2],
    };
    let (mut advance, mut ink): (f64, Option<(f64, f64)>) = (0.0, None);
    let mut prev = None;
    for ch in text.chars() {
        let g = face.glyph_index(ch).unwrap_or(ttf_parser::GlyphId(0));
        // HarfBuzz puts a kerning on the glyph before the pair.
        if let Some(p) = prev {
            advance += kern(p, g) as f64 / FT_SCALE * spacing;
        }
        prev = Some(g);
        pen.at = [advance, 0.0];
        // FreeType puts a glyph's left side bearing point at its origin.
        let bearing = face.glyph_hor_side_bearing(g).unwrap_or(0);
        let left = face.glyph_bounding_box(g).map_or(0, |b| b.x_min);
        pen.shift = f64::from(bearing) - f64::from(left);
        pen.lo = [i64::MAX; 2];
        pen.hi = [i64::MIN; 2];
        face.outline_glyph(g, &mut pen);
        pen.close();
        if pen.hi[0] > pen.lo[0] && pen.hi[1] > pen.lo[1] {
            // FT_GLYPH_BBOX_GRIDFIT: the box out to whole pixels.
            let top = ((pen.hi[1] + 63) & !63) as f64 / FT_SCALE;
            let bottom = (pen.lo[1] & !63) as f64 / FT_SCALE;
            ink = Some(ink.map_or((top, bottom), |(t, b)| (t.max(top), b.min(bottom))));
        }
        let a = mul_fix(
            i64::from(face.glyph_hor_advance(g).unwrap_or(0)),
            advance_scale,
        );
        advance += a as f64 / FT_SCALE * spacing;
    }
    Some(Laid {
        outlines: pen.done,
        advance,
        ink,
    })
}
