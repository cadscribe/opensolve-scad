//! What `import()` reads, as OpenSCAD reads it: meshes from STL (text and
//! binary), OFF and OBJ; outlines from SVG and DXF. No library: the SVG's
//! XML and the DXF's groups are read here, the subset OpenSCAD itself
//! understands.

use std::collections::HashMap;

use crate::geom::{V2, V3, cos_deg, sin_deg};

/// The faces of a mesh, each a loop of corners, counter-clockwise seen from
/// outside.
pub type Faces = Vec<Vec<V3>>;

/// The faces of an STL, OFF or OBJ file, by its extension.
pub fn mesh(ext: &str, bytes: &[u8]) -> Result<Faces, String> {
    match ext {
        "stl" => opensolve_kernel::read_stl(bytes)
            .map(|t| t.into_iter().map(|t| t.to_vec()).collect())
            .ok_or_else(|| "not an STL file".to_string()),
        "off" => off(&String::from_utf8_lossy(bytes)),
        "obj" => Ok(obj(&String::from_utf8_lossy(bytes))),
        _ => Err(format!("{ext} files are not supported")),
    }
}

/// OFF: `OFF`, the counts, the corners, the faces (`n i1 … in`); `#`
/// begins a comment.
fn off(text: &str) -> Result<Faces, String> {
    let mut words = text
        .lines()
        .map(|l| l.split('#').next().unwrap_or(""))
        .flat_map(str::split_whitespace);
    let head = words.next().ok_or("an empty OFF file")?;
    if !head.ends_with("OFF") {
        return Err("not an OFF file".into());
    }
    // `OFF3 …` or the counts at once.
    let num = |w: &mut dyn Iterator<Item = &str>| -> Result<f64, String> {
        w.next()
            .and_then(|x| x.parse().ok())
            .ok_or_else(|| "an OFF file cut short".to_string())
    };
    let nv = num(&mut words)? as usize;
    let nf = num(&mut words)? as usize;
    let _edges = num(&mut words)?;
    let mut points = Vec::with_capacity(nv);
    // A corner has three numbers, maybe more (colour, normal) on its line.
    let lines: Vec<&str> = text
        .lines()
        .map(|l| l.split('#').next().unwrap_or("").trim())
        .filter(|l| !l.is_empty())
        .collect();
    let mut at = 1;
    // The counts may be on the header's line.
    if lines
        .first()
        .is_some_and(|l| l.split_whitespace().count() == 1)
    {
        at = 2;
    }
    for l in lines.iter().skip(at).take(nv) {
        let p: Vec<f64> = l
            .split_whitespace()
            .take(3)
            .filter_map(|x| x.parse().ok())
            .collect();
        if p.len() < 3 {
            return Err("an OFF corner without three numbers".into());
        }
        points.push([p[0], p[1], p[2]]);
    }
    let mut faces = Vec::with_capacity(nf);
    for l in lines.iter().skip(at + nv).take(nf) {
        let n: Vec<usize> = l
            .split_whitespace()
            .filter_map(|x| x.parse().ok())
            .collect();
        let Some((&k, rest)) = n.split_first() else {
            continue;
        };
        let face: Option<Vec<V3>> = rest
            .iter()
            .take(k)
            .map(|&i| points.get(i).copied())
            .collect();
        faces.push(face.ok_or("an OFF face with a corner that does not exist")?);
    }
    Ok(faces)
}

/// OBJ: `v x y z` and `f a b c …`, the corners counted from 1, or back
/// from the last when negative; `a/t/n` takes `a`.
fn obj(text: &str) -> Faces {
    let mut points: Vec<V3> = Vec::new();
    let mut faces = Vec::new();
    for l in text.lines() {
        let mut w = l.split_whitespace();
        match w.next() {
            Some("v") => {
                let p: Vec<f64> = w.take(3).filter_map(|x| x.parse().ok()).collect();
                if p.len() == 3 {
                    points.push([p[0], p[1], p[2]]);
                }
            }
            Some("f") => {
                let face: Option<Vec<V3>> = w
                    .map(|x| {
                        let i: i64 = x.split('/').next()?.parse().ok()?;
                        let i = if i < 0 {
                            points.len() as i64 + i
                        } else {
                            i - 1
                        };
                        points.get(usize::try_from(i).ok()?).copied()
                    })
                    .collect();
                if let Some(f) = face.filter(|f| f.len() >= 3) {
                    faces.push(f);
                }
            }
            _ => {}
        }
    }
    faces
}

/// What an import of an SVG or DXF file takes besides the file.
#[derive(Clone)]
pub struct Options {
    pub center: bool,
    pub dpi: f64,
    pub layer: Option<String>,
    pub id: Option<String>,
    pub origin: V2,
    pub scale: f64,
    /// The pieces of a whole circle of a radius, as `$fn`, `$fa`, `$fs`
    /// make them.
    pub fragments: std::rc::Rc<dyn Fn(f64) -> usize>,
    /// `$fn`, for the curves of a path.
    pub fn_: f64,
}

/// The shapes of an SVG file, each its outlines (filled by the even-odd
/// rule), in millimetres, y up, as OpenSCAD places them.
pub fn svg(text: &str, o: &Options) -> Result<Vec<Vec<Vec<V2>>>, String> {
    let root = xml(text)?;
    let svg = find(&root, &|e| e.name == "svg").ok_or("no <svg> element")?;
    let mut defs: HashMap<&str, &Element> = HashMap::new();
    collect_defs(svg, false, &mut defs);
    let mut shapes: Vec<Vec<Vec<V2>>> = Vec::new();
    let ctx = Walk {
        o,
        defs: &defs,
        depth: 0,
    };
    let selected = o.id.is_none() && o.layer.is_none();
    ctx.walk(svg, IDENTITY2, selected, false, false, &mut shapes);
    // The page: its size in millimetres, the view box onto it.
    let length = |n: &str| length(svg.attr(n).unwrap_or(""));
    let (w, h) = (length("width"), length("height"));
    let vb: Option<[f64; 4]> = svg.attr("viewBox").and_then(|v| {
        let n: Vec<f64> = numbers(v);
        (n.len() == 4 && n[2] >= 0.0 && n[3] >= 0.0).then(|| [n[0], n[1], n[2], n[3]])
    });
    let to_mm = |l: (f64, &str), vbl: f64| -> f64 {
        let valid = vb.is_some();
        match l.1 {
            "" => 25.4 * l.0 / o.dpi,
            "px" => 25.4 * l.0 / 96.0,
            "pt" => 25.4 * l.0 / 72.0,
            "pc" => 25.4 * l.0 / 6.0,
            "in" => 25.4 * l.0,
            "cm" => 10.0 * l.0,
            "mm" => l.0,
            "%" if valid => 25.4 * l.0 / 100.0 * vbl / o.dpi,
            "%" => 0.0,
            "?" if valid => 25.4 * vbl / o.dpi,
            _ if valid => vbl,
            _ => 0.0,
        }
    };
    let (vbw, vbh) = vb.map_or((0.0, 0.0), |v| (v[2], v[3]));
    let (wmm, hmm) = (to_mm(w, vbw), to_mm(h, vbh));
    let mut scale = [1.0, 1.0];
    let mut align = [0.0, 0.0];
    let mut view = [0.0, 0.0];
    if let Some(v) = vb {
        let px = if w.1 == "%" { w.0 / 100.0 } else { 1.0 };
        let py = if h.1 == "%" { h.0 / 100.0 } else { 1.0 };
        view = [px * v[0], py * v[1]];
        scale = [wmm / v[2], hmm / v[3]];
        let aspect = svg.attr("preserveAspectRatio").unwrap_or("");
        let words: Vec<&str> = aspect
            .split_whitespace()
            .filter(|w| *w != "defer")
            .collect();
        let how = words.first().copied().unwrap_or("xMidYMid");
        if how != "none" {
            let slice = words.get(1) == Some(&"slice");
            let k = if slice {
                scale[0].max(scale[1])
            } else {
                scale[0].min(scale[1])
            };
            scale = [k, k];
            let at = |s: &str, page: f64, size: f64| match s {
                "Mid" => page / 2.0 - k * size / 2.0,
                "Max" => page - k * size,
                _ => 0.0,
            };
            let (ax, ay) = if how.len() == 8 {
                (&how[1..4], &how[5..8])
            } else {
                ("Mid", "Mid")
            };
            align = [at(ax, wmm, v[2]), at(ay, hmm, v[3])];
        }
    }
    let (mut lo, mut hi) = ([f64::MAX; 2], [f64::MIN; 2]);
    for p in shapes.iter().flatten().flatten() {
        for k in 0..2 {
            lo[k] = lo[k].min(scale[k] * p[k]);
            hi[k] = hi[k].max(scale[k] * p[k]);
        }
    }
    let (cx, cy) = if o.center {
        ((lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0)
    } else {
        (-align[0], hmm - align[1])
    };
    for p in shapes.iter_mut().flatten().flatten() {
        *p = [
            scale[0] * (p[0] - view[0]) - cx,
            scale[1] * (-view[1] - p[1]) + cy,
        ];
    }
    Ok(shapes)
}

/// An element of an XML file: its name, attributes, children.
#[derive(Debug, Default)]
struct Element {
    name: String,
    attrs: Vec<(String, String)>,
    children: Vec<Element>,
}

impl Element {
    fn attr(&self, n: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k == n)
            .map(|(_, v)| v.as_str())
    }

    /// A property, from `style` first, else the attribute.
    fn style(&self, n: &str) -> Option<&str> {
        self.attr("style")
            .and_then(|s| {
                s.split(';').find_map(|d| {
                    let (k, v) = d.split_once(':')?;
                    (k.trim() == n).then(|| v.trim())
                })
            })
            .or_else(|| self.attr(n))
    }
}

fn find<'a>(e: &'a Element, f: &dyn Fn(&Element) -> bool) -> Option<&'a Element> {
    if f(e) {
        return Some(e);
    }
    e.children.iter().find_map(|c| find(c, f))
}

/// The elements with an `id` inside `<defs>`: what `<use>` refers to.
fn collect_defs<'a>(e: &'a Element, inside: bool, out: &mut HashMap<&'a str, &'a Element>) {
    let inside = inside || e.name == "defs";
    if inside && let Some(id) = e.attr("id") {
        out.insert(id, e);
    }
    for c in &e.children {
        collect_defs(c, inside, out);
    }
}

/// The elements of an XML text, under a root without a name.
fn xml(text: &str) -> Result<Element, String> {
    let mut stack = vec![Element::default()];
    let mut rest = text;
    while let Some(i) = rest.find('<') {
        rest = &rest[i..];
        if let Some(r) = rest.strip_prefix("<!--") {
            rest = r.find("-->").map_or("", |e| &r[e + 3..]);
        } else if let Some(r) = rest.strip_prefix("<![CDATA[") {
            rest = r.find("]]>").map_or("", |e| &r[e + 3..]);
        } else if rest.starts_with("<?") || rest.starts_with("<!") {
            // A declaration, maybe with `[…]` inside.
            let mut depth = 0;
            let mut end = rest.len();
            for (k, c) in rest.char_indices() {
                match c {
                    '[' => depth += 1,
                    ']' => depth -= 1,
                    '>' if depth <= 0 => {
                        end = k + 1;
                        break;
                    }
                    _ => {}
                }
            }
            rest = &rest[end..];
        } else if let Some(r) = rest.strip_prefix("</") {
            let end = r.find('>').ok_or("an end tag without its end")?;
            rest = &r[end + 1..];
            if stack.len() > 1 {
                let e = stack.pop().unwrap_or_default();
                if let Some(parent) = stack.last_mut() {
                    parent.children.push(e);
                }
            }
        } else {
            let (e, closed, r) = tag(&rest[1..])?;
            rest = r;
            if closed {
                if let Some(parent) = stack.last_mut() {
                    parent.children.push(e);
                }
            } else {
                stack.push(e);
            }
        }
    }
    while stack.len() > 1 {
        let e = stack.pop().unwrap_or_default();
        if let Some(parent) = stack.last_mut() {
            parent.children.push(e);
        }
    }
    stack.pop().ok_or_else(|| "no elements".to_string())
}

/// A start tag after its `<`: the element, whether it ends there (`/>`),
/// and the text after it.
fn tag(s: &str) -> Result<(Element, bool, &str), String> {
    let name_end = s
        .find(|c: char| c.is_whitespace() || c == '>' || c == '/')
        .ok_or("a tag without its end")?;
    let mut e = Element {
        name: s[..name_end].to_string(),
        ..Element::default()
    };
    let mut rest = &s[name_end..];
    loop {
        rest = rest.trim_start();
        if let Some(r) = rest.strip_prefix("/>") {
            return Ok((e, true, r));
        }
        if let Some(r) = rest.strip_prefix('>') {
            return Ok((e, false, r));
        }
        if rest.is_empty() {
            return Err("a tag without its end".into());
        }
        let eq = rest.find('=').ok_or("an attribute without a value")?;
        let key = rest[..eq].trim().to_string();
        let r = rest[eq + 1..].trim_start();
        let quote = r.chars().next().ok_or("an attribute without a value")?;
        if quote != '"' && quote != '\'' {
            return Err("an attribute without quotes".into());
        }
        let end = r[1..].find(quote).ok_or("an attribute without its end")?;
        e.attrs.push((key, entities(&r[1..1 + end])));
        rest = &r[end + 2..];
    }
}

/// `&amp;` and its kind as the characters they stand for.
fn entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let r = &rest[i + 1..];
        let Some(end) = r.find(';') else {
            out.push('&');
            rest = r;
            continue;
        };
        let name = &r[..end];
        let c = match name {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => name
                .strip_prefix("#x")
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .or_else(|| name.strip_prefix('#').and_then(|d| d.parse().ok()))
                .and_then(char::from_u32),
        };
        match c {
            Some(c) => out.push(c),
            None => {
                out.push('&');
                out.push_str(name);
                out.push(';');
            }
        }
        rest = &r[end + 1..];
    }
    out.push_str(rest);
    out
}

/// A 2D affine map: `[a c e; b d f]` as SVG writes it.
type M2 = [f64; 6];
const IDENTITY2: M2 = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

/// `a` after `b`.
fn mul(a: &M2, b: &M2) -> M2 {
    [
        a[0] * b[0] + a[2] * b[1],
        a[1] * b[0] + a[3] * b[1],
        a[0] * b[2] + a[2] * b[3],
        a[1] * b[2] + a[3] * b[3],
        a[0] * b[4] + a[2] * b[5] + a[4],
        a[1] * b[4] + a[3] * b[5] + a[5],
    ]
}

fn map(m: &M2, p: V2) -> V2 {
    [
        m[0] * p[0] + m[2] * p[1] + m[4],
        m[1] * p[0] + m[3] * p[1] + m[5],
    ]
}

/// The numbers in a text, as SVG writes them: `1-2.5.5e3` is 1, -2.5,
/// .5e3.
fn numbers(s: &str) -> Vec<f64> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if !(c.is_ascii_digit() || c == b'.' || c == b'-' || c == b'+') {
            i += 1;
            continue;
        }
        let (n, len) = number_at(&s[i..]);
        match n {
            Some(n) => {
                out.push(n);
                i += len;
            }
            None => i += 1,
        }
    }
    out
}

/// The number at the start of `s` and its length.
fn number_at(s: &str) -> (Option<f64>, usize) {
    let b = s.as_bytes();
    let mut i = 0;
    if i < b.len() && (b[i] == b'-' || b[i] == b'+') {
        i += 1;
    }
    let digits = |mut i: usize| {
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        i
    };
    let start = i;
    i = digits(i);
    if i < b.len() && b[i] == b'.' {
        i = digits(i + 1);
    }
    if i == start || (i == start + 1 && b[start] == b'.') {
        return (None, 0);
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        let mut j = i + 1;
        if j < b.len() && (b[j] == b'-' || b[j] == b'+') {
            j += 1;
        }
        if j < b.len() && b[j].is_ascii_digit() {
            i = digits(j);
        }
    }
    (s[..i].parse().ok(), i)
}

/// A length with its unit: `""` for none, `"?"` when it is not given.
fn length(s: &str) -> (f64, &str) {
    let s = s.trim();
    if s.is_empty() {
        return (0.0, "?");
    }
    let (n, len) = number_at(s);
    let unit = s[len..].trim();
    match n {
        Some(n) if ["", "px", "pt", "pc", "in", "cm", "mm", "%", "em", "ex"].contains(&unit) => {
            (n, unit)
        }
        _ => (0.0, "?"),
    }
}

/// A transform attribute as one map.
fn transform(s: &str) -> M2 {
    let mut m = IDENTITY2;
    let mut rest = s;
    while let Some(open) = rest.find('(') {
        let name = rest[..open]
            .trim()
            .trim_start_matches(',')
            .trim()
            .to_string();
        let Some(close) = rest[open..].find(')') else {
            break;
        };
        let n = numbers(&rest[open + 1..open + close]);
        rest = &rest[open + close + 1..];
        let arg = |i: usize, or: f64| n.get(i).copied().unwrap_or(or);
        let t: M2 = match name.as_str() {
            "matrix" if n.len() == 6 => [n[0], n[1], n[2], n[3], n[4], n[5]],
            "translate" => [1.0, 0.0, 0.0, 1.0, arg(0, 0.0), arg(1, 0.0)],
            "scale" => {
                let x = arg(0, 1.0);
                [x, 0.0, 0.0, arg(1, x), 0.0, 0.0]
            }
            "rotate" => {
                let (s, c) = (sin_deg(arg(0, 0.0)), cos_deg(arg(0, 0.0)));
                let (cx, cy) = (arg(1, 0.0), arg(2, 0.0));
                let r = [c, s, -s, c, 0.0, 0.0];
                mul(
                    &[1.0, 0.0, 0.0, 1.0, cx, cy],
                    &mul(&r, &[1.0, 0.0, 0.0, 1.0, -cx, -cy]),
                )
            }
            "skewX" => [1.0, 0.0, (arg(0, 0.0)).to_radians().tan(), 1.0, 0.0, 0.0],
            "skewY" => [1.0, (arg(0, 0.0)).to_radians().tan(), 0.0, 1.0, 0.0, 0.0],
            _ => IDENTITY2,
        };
        m = mul(&m, &t);
    }
    m
}

struct Walk<'a> {
    o: &'a Options,
    defs: &'a HashMap<&'a str, &'a Element>,
    depth: usize,
}

impl Walk<'_> {
    /// The shapes of an element and those inside it; `selected` when an
    /// element around it is what the import asks for.
    fn walk(
        &self,
        e: &Element,
        m: M2,
        selected: bool,
        in_defs: bool,
        hidden: bool,
        out: &mut Vec<Vec<Vec<V2>>>,
    ) {
        if e.name == "defs" || in_defs || self.depth > 64 {
            return;
        }
        let hidden = hidden || e.style("display") == Some("none");
        let is_shape = matches!(
            e.name.as_str(),
            "svg"
                | "g"
                | "path"
                | "rect"
                | "circle"
                | "ellipse"
                | "line"
                | "polyline"
                | "polygon"
                | "use"
        );
        let m = if is_shape && e.name != "svg" {
            mul(&m, &transform(e.attr("transform").unwrap_or("")))
        } else {
            m
        };
        let layer = e.attr("inkscape:groupmode") == Some("layer")
            && e.attr("inkscape:label").is_some()
            && e.attr("inkscape:label") == self.o.layer.as_deref();
        let by_id = self.o.id.is_some() && e.attr("id") == self.o.id.as_deref();
        let selected = selected
            || match (&self.o.id, &self.o.layer) {
                (Some(_), _) => by_id,
                (None, Some(_)) => layer,
                _ => false,
            };
        if e.name == "use" {
            let href = e
                .attr("href")
                .or_else(|| e.attr("xlink:href"))
                .unwrap_or("");
            if let Some(target) = href.strip_prefix('#').and_then(|id| self.defs.get(id)) {
                let at = |n: &str| {
                    e.attr(n)
                        .map_or(0.0, |v| numbers(v).first().copied().unwrap_or(0.0))
                };
                let m = mul(&m, &[1.0, 0.0, 0.0, 1.0, at("x"), at("y")]);
                let inner = Walk {
                    o: self.o,
                    defs: self.defs,
                    depth: self.depth + 1,
                };
                inner.walk_target(target, m, selected, hidden, out);
            }
            return;
        }
        if is_shape && selected && !hidden {
            let (loops, pieces) = self.paths(e);
            let moved = |p: Vec<V2>| p.into_iter().map(|q| map(&m, q)).collect::<Vec<V2>>();
            let loops: Vec<Vec<V2>> = loops
                .into_iter()
                .map(moved)
                .filter(|p| p.len() >= 3)
                .collect();
            if !loops.is_empty() {
                out.push(loops);
            }
            // The pieces of a stroke overlap: each a shape of its own, so
            // that they are joined.
            out.extend(pieces.into_iter().map(|p| vec![moved(p)]));
        }
        for c in &e.children {
            self.walk(c, m, selected, false, hidden, out);
        }
    }

    /// What `<use>` refers to, though it lies in `<defs>`.
    fn walk_target(
        &self,
        e: &Element,
        m: M2,
        selected: bool,
        hidden: bool,
        out: &mut Vec<Vec<Vec<V2>>>,
    ) {
        if e.name == "symbol" {
            for c in &e.children {
                self.walk(c, m, selected, false, hidden, out);
            }
        } else {
            self.walk(e, m, selected, false, hidden, out);
        }
    }

    fn stroke_width(&self, e: &Element) -> f64 {
        let w = e
            .attr("stroke-width")
            .or_else(|| e.style("stroke-width"))
            .and_then(|v| v.trim().parse::<f64>().ok())
            .unwrap_or(0.0);
        if w < 0.01 { 1.0 } else { w }
    }

    /// The outlines of one element, in its own coordinates: closed loops,
    /// and the pieces of open lines stroked as OpenSCAD strokes them.
    fn paths(&self, e: &Element) -> (Vec<Vec<V2>>, Vec<Vec<V2>>) {
        let num = |n: &str| {
            e.attr(n)
                .and_then(|v| v.trim().parse::<f64>().ok())
                .unwrap_or(0.0)
        };
        let width = || self.stroke_width(e);
        let cap = || e.style("stroke-linecap").unwrap_or("butt").to_string();
        match e.name.as_str() {
            "path" => self.path(e.attr("d").unwrap_or(""), width(), &cap()),
            "rect" => {
                let (x, y, w, h) = (num("x"), num("y"), num("width"), num("height"));
                let (mut rx, mut ry) = (num("rx"), num("ry"));
                if rx.abs() < 1e-8 && ry.abs() < 1e-8 {
                    return (
                        vec![vec![[x, y], [x + w, y], [x + w, y + h], [x, y + h]]],
                        Vec::new(),
                    );
                }
                if rx.abs() < 1e-8 {
                    rx = ry;
                } else if ry.abs() < 1e-8 {
                    ry = rx;
                }
                rx = rx.min(w / 2.0);
                ry = ry.min(h / 2.0);
                let d = format!(
                    "M {},{} H {} A {rx},{ry} 0 0,1 {},{} V {} A {rx},{ry} 0 0,1 {},{} H {} \
                     A {rx},{ry} 0 0,1 {},{} V {} A {rx},{ry} 0 0,1 {},{} z",
                    x + rx,
                    y,
                    x + w - rx,
                    x + w,
                    y + ry,
                    y + h - ry,
                    x + w - rx,
                    y + h,
                    x + rx,
                    x,
                    y + h - ry,
                    y + ry,
                    x + rx,
                    y
                );
                self.path(&d, width(), &cap())
            }
            "circle" => (
                vec![self.ellipse(num("cx"), num("cy"), num("r"), num("r"))],
                Vec::new(),
            ),
            "ellipse" => (
                vec![self.ellipse(num("cx"), num("cy"), num("rx"), num("ry"))],
                Vec::new(),
            ),
            "polygon" => {
                let n = numbers(e.attr("points").unwrap_or(""));
                (
                    vec![n.chunks_exact(2).map(|c| [c[0], c[1]]).collect()],
                    Vec::new(),
                )
            }
            "polyline" => {
                let n = numbers(e.attr("points").unwrap_or(""));
                let line: Vec<V2> = n.chunks_exact(2).map(|c| [c[0], c[1]]).collect();
                (Vec::new(), stroke(&line, width(), &cap()))
            }
            "line" => {
                let line = vec![[num("x1"), num("y1")], [num("x2"), num("y2")]];
                (Vec::new(), stroke(&line, width(), &cap()))
            }
            _ => (Vec::new(), Vec::new()),
        }
    }

    /// An ellipse in at least 40 pieces, as OpenSCAD draws it.
    fn ellipse(&self, x: f64, y: f64, rx: f64, ry: f64) -> Vec<V2> {
        let n = (self.o.fragments)(rx.max(ry)).max(40);
        (1..=n)
            .map(|i| {
                let a = i as f64 * 360.0 / n as f64;
                [rx * sin_deg(a) + x, ry * cos_deg(a) + y]
            })
            .collect()
    }

    /// The loops of a path's `d`, and the pieces of its open lines.
    fn path(&self, d: &str, width: f64, cap: &str) -> (Vec<Vec<V2>>, Vec<Vec<V2>>) {
        let mut out: Vec<Vec<V2>> = Vec::new();
        let mut pieces: Vec<Vec<V2>> = Vec::new();
        let mut cur: Vec<V2> = Vec::new();
        let curve_n = (self.o.fn_ as usize).max(20);
        let (mut x, mut y) = (0.0, 0.0);
        // The control point the next smooth curve mirrors, and of what kind.
        let mut last_c: Option<V2> = None;
        let mut last_q: Option<V2> = None;
        let b = d.as_bytes();
        let mut i = 0;
        let mut cmd = b' ';
        // A loop ends: an open one, its ends more than 0.1 apart, is
        // stroked.
        let finish = |cur: &mut Vec<V2>, out: &mut Vec<Vec<V2>>, pieces: &mut Vec<Vec<V2>>| {
            if cur.len() >= 2 {
                let (a, z) = (cur[0], cur[cur.len() - 1]);
                if (a[0] - z[0]).hypot(a[1] - z[1]) > 0.1 {
                    pieces.extend(stroke(cur, width, cap));
                } else {
                    out.push(std::mem::take(cur));
                }
            }
            cur.clear();
        };
        let mut args: Vec<f64> = Vec::new();
        loop {
            // Skip separators.
            while i < b.len() && (b[i].is_ascii_whitespace() || b[i] == b',') {
                i += 1;
            }
            let at_end = i >= b.len();
            let letter = !at_end && b[i].is_ascii_alphabetic() && b[i] != b'e' && b[i] != b'E';
            let need = match cmd.to_ascii_uppercase() {
                b'M' | b'L' | b'T' => 2,
                b'H' | b'V' => 1,
                b'C' => 6,
                b'S' | b'Q' => 4,
                b'A' => 7,
                _ => 0,
            };
            if !letter && !at_end && need > 0 && args.len() < need {
                // The flags of an arc may be written without separators.
                if cmd.eq_ignore_ascii_case(&b'A')
                    && (args.len() == 3 || args.len() == 4)
                    && (b[i] == b'0' || b[i] == b'1')
                {
                    args.push(f64::from(b[i] - b'0'));
                    i += 1;
                } else {
                    let (n, len) = number_at(&d[i..]);
                    match n {
                        Some(n) => {
                            args.push(n);
                            i += len;
                        }
                        None => i += 1,
                    }
                }
                if args.len() < need {
                    continue;
                }
            }
            if need > 0 && args.len() == need {
                let rel = cmd.is_ascii_lowercase();
                let (ox, oy) = if rel { (x, y) } else { (0.0, 0.0) };
                let a = std::mem::take(&mut args);
                match cmd.to_ascii_uppercase() {
                    b'M' => {
                        finish(&mut cur, &mut out, &mut pieces);
                        x = ox + a[0];
                        y = oy + a[1];
                        cur.push([x, y]);
                        cmd = if rel { b'l' } else { b'L' };
                        last_c = None;
                        last_q = None;
                    }
                    b'L' => {
                        x = ox + a[0];
                        y = oy + a[1];
                        cur.push([x, y]);
                        last_c = None;
                        last_q = None;
                    }
                    b'H' => {
                        x = ox + a[0];
                        cur.push([x, y]);
                        last_c = None;
                        last_q = None;
                    }
                    b'V' => {
                        y = oy + a[0];
                        cur.push([x, y]);
                        last_c = None;
                        last_q = None;
                    }
                    b'C' | b'S' => {
                        let (c1, c2, to) = if cmd.eq_ignore_ascii_case(&b'C') {
                            (
                                [ox + a[0], oy + a[1]],
                                [ox + a[2], oy + a[3]],
                                [ox + a[4], oy + a[5]],
                            )
                        } else {
                            let c1 = last_c.map_or([x, y], |c| [2.0 * x - c[0], 2.0 * y - c[1]]);
                            (c1, [ox + a[0], oy + a[1]], [ox + a[2], oy + a[3]])
                        };
                        if cur.is_empty() {
                            cur.push([x, y]);
                        }
                        for k in 1..=curve_n {
                            let t = k as f64 / curve_n as f64;
                            let u = 1.0 - t;
                            let w = [u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t];
                            cur.push([
                                w[0] * x + w[1] * c1[0] + w[2] * c2[0] + w[3] * to[0],
                                w[0] * y + w[1] * c1[1] + w[2] * c2[1] + w[3] * to[1],
                            ]);
                        }
                        [x, y] = to;
                        last_c = Some(c2);
                        last_q = None;
                    }
                    b'Q' | b'T' => {
                        let (c, to) = if cmd.eq_ignore_ascii_case(&b'Q') {
                            ([ox + a[0], oy + a[1]], [ox + a[2], oy + a[3]])
                        } else {
                            let c = last_q.map_or([x, y], |c| [2.0 * x - c[0], 2.0 * y - c[1]]);
                            (c, [ox + a[0], oy + a[1]])
                        };
                        if cur.is_empty() {
                            cur.push([x, y]);
                        }
                        for k in 1..=curve_n {
                            let t = k as f64 / curve_n as f64;
                            let u = 1.0 - t;
                            let w = [u * u, 2.0 * u * t, t * t];
                            cur.push([
                                w[0] * x + w[1] * c[0] + w[2] * to[0],
                                w[0] * y + w[1] * c[1] + w[2] * to[1],
                            ]);
                        }
                        [x, y] = to;
                        last_q = Some(c);
                        last_c = None;
                    }
                    b'A' => {
                        let to = [ox + a[5], oy + a[6]];
                        if cur.is_empty() {
                            cur.push([x, y]);
                        }
                        self.arc(
                            &mut cur,
                            [x, y],
                            a[0].abs(),
                            a[1].abs(),
                            to,
                            a[2],
                            a[3] != 0.0,
                            a[4] != 0.0,
                        );
                        [x, y] = to;
                        last_c = None;
                        last_q = None;
                    }
                    _ => {}
                }
                continue;
            }
            if at_end {
                break;
            }
            if letter {
                let c = b[i];
                i += 1;
                if c == b'z' || c == b'Z' {
                    if let Some(&first) = cur.first() {
                        cur.push(first);
                        [x, y] = first;
                        out.push(std::mem::take(&mut cur));
                    }
                    // After a close, a line starts where the loop did.
                    cmd = b' ';
                    last_c = None;
                    last_q = None;
                    continue;
                }
                cmd = c;
                args.clear();
                // A new loop after a close begins at its first point.
                if cur.is_empty() && !matches!(c, b'M' | b'm') {
                    cur.push([x, y]);
                }
                continue;
            }
            i += 1;
        }
        finish(&mut cur, &mut out, &mut pieces);
        (out, pieces)
    }

    /// An elliptic arc as the SVG notes on it compute it, its points
    /// after the first.
    #[allow(clippy::too_many_arguments)]
    fn arc(
        &self,
        out: &mut Vec<V2>,
        p1: V2,
        mut rx: f64,
        mut ry: f64,
        p2: V2,
        angle: f64,
        large: bool,
        sweep: bool,
    ) {
        if rx < 1e-12 || ry < 1e-12 {
            out.push(p2);
            return;
        }
        let (c, s) = (cos_deg(angle), sin_deg(angle));
        let (dx, dy) = ((p1[0] - p2[0]) / 2.0, (p1[1] - p2[1]) / 2.0);
        let x1 = c * dx + s * dy;
        let y1 = -s * dx + c * dy;
        let d = x1 * x1 / (rx * rx) + y1 * y1 / (ry * ry);
        if d > 1.0 {
            rx *= d.sqrt();
            ry *= d.sqrt();
        }
        let t1 = (rx * rx * ry * ry - rx * rx * y1 * y1 - ry * ry * x1 * x1).max(0.0);
        let t2 = rx * rx * y1 * y1 + ry * ry * x1 * x1;
        let mut t3 = if t2 > 0.0 { (t1 / t2).sqrt() } else { 0.0 };
        if large == sweep {
            t3 = -t3;
        }
        let cx_ = t3 * rx * y1 / ry;
        let cy_ = t3 * -ry * x1 / rx;
        let cx = c * cx_ - s * cy_ + (p1[0] + p2[0]) / 2.0;
        let cy = s * cx_ + c * cy_ + (p1[1] + p2[1]) / 2.0;
        let angle_of = |ux: f64, uy: f64, vx: f64, vy: f64| {
            let a = vy.atan2(vx).to_degrees() - uy.atan2(ux).to_degrees();
            if a < 0.0 { a + 360.0 } else { a }
        };
        let (ux, uy) = ((x1 - cx_) / rx, (y1 - cy_) / ry);
        let (vx, vy) = ((-x1 - cx_) / rx, (-y1 - cy_) / ry);
        let theta = angle_of(1.0, 0.0, ux, uy);
        let mut delta = angle_of(ux, uy, vx, vy);
        if !sweep {
            delta -= 360.0;
        }
        let n = ((self.o.fragments)(rx.max(ry)) as f64 * delta.abs() / 360.0).ceil() as usize;
        let steps = n.max((delta.abs() * 10.0 / 180.0) as usize + 4);
        for k in 1..=steps {
            let phi = theta + delta * k as f64 / steps as f64;
            let (xx, yy) = (cos_deg(phi) * rx, sin_deg(phi) * ry);
            out.push([c * xx - s * yy + cx, s * xx + c * yy + cy]);
        }
    }
}

/// An open line stroked `width` wide: a loop for each piece and a wedge
/// at each corner, to be joined; ends cut off, or with a square or half a
/// round beyond them.
fn stroke(line: &[V2], width: f64, cap: &str) -> Vec<Vec<V2>> {
    let mut line: Vec<V2> = line.to_vec();
    line.dedup_by(|a, b| (a[0] - b[0]).hypot(a[1] - b[1]) < 1e-12);
    let h = width / 2.0;
    let mut out = Vec::new();
    if line.len() < 2 {
        return out;
    }
    let normal = |a: V2, b: V2| {
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let l = dx.hypot(dy).max(1e-300);
        [-dy / l * h, dx / l * h]
    };
    let last = line.len() - 1;
    for k in 0..last {
        let (mut a, mut b) = (line[k], line[k + 1]);
        let n = normal(a, b);
        if cap == "square" {
            let d = [n[1], -n[0]];
            if k == 0 {
                a = [a[0] - d[0], a[1] - d[1]];
            }
            if k + 1 == last {
                b = [b[0] + d[0], b[1] + d[1]];
            }
        }
        out.push(vec![
            [a[0] + n[0], a[1] + n[1]],
            [b[0] + n[0], b[1] + n[1]],
            [b[0] - n[0], b[1] - n[1]],
            [a[0] - n[0], a[1] - n[1]],
        ]);
    }
    // The gap at a corner, on the outer side.
    for k in 1..last {
        let (p, n1, n2) = (
            line[k],
            normal(line[k - 1], line[k]),
            normal(line[k], line[k + 1]),
        );
        for sign in [1.0, -1.0] {
            out.push(vec![
                p,
                [p[0] + sign * n1[0], p[1] + sign * n1[1]],
                [p[0] + sign * n2[0], p[1] + sign * n2[1]],
            ]);
        }
    }
    if cap == "round" {
        for p in [line[0], line[last]] {
            out.push(
                (0..16)
                    .map(|i| {
                        let a = i as f64 * 22.5;
                        [p[0] + h * cos_deg(a), p[1] + h * sin_deg(a)]
                    })
                    .collect(),
            );
        }
    }
    out
}

/// The outlines of a DXF file as OpenSCAD reads them: its lines, polylines,
/// circles, arcs and ellipses (and blocks inserted), of one layer if
/// asked, chained by their ends into paths; open paths closed.
pub fn dxf(text: &str, o: &Options) -> Vec<Vec<V2>> {
    let lines_in: Vec<&str> = text.lines().map(str::trim).collect();
    let (ox, oy, sc) = (o.origin[0], o.origin[1], o.scale);
    let mut segs: Vec<[V2; 2]> = Vec::new();
    let mut blocks: HashMap<String, Vec<[V2; 2]>> = HashMap::new();
    let mut block = String::new();
    let (mut in_entities, mut in_blocks) = (false, false);
    let mut mode = String::new();
    let mut layer = String::new();
    let mut iddata = String::new();
    let mut flags = 0i64;
    let (mut xs, mut ys): (Vec<f64>, Vec<f64>) = (Vec::new(), Vec::new());
    let mut radius = 0.0;
    let (mut a50, mut a51, mut e41, mut e42) = (0.0, 0.0, 0.0, 0.0);
    let mut k = 0;
    while k + 1 < lines_in.len() {
        let Ok(id) = lines_in[k].parse::<i64>() else {
            break;
        };
        let data = lines_in[k + 1];
        k += 2;
        let num = || data.parse::<f64>().unwrap_or(0.0);
        if id == 0 {
            let wanted = o.layer.as_ref().is_none_or(|l| *l == layer);
            let inserted = if mode == "INSERT" {
                blocks.get(&iddata).cloned().unwrap_or_default()
            } else {
                Vec::new()
            };
            let mut add = |a: V2, b: V2| {
                if in_entities && wanted {
                    segs.push([a, b]);
                }
                if in_blocks && !block.is_empty() {
                    blocks.entry(block.clone()).or_default().push([a, b]);
                }
            };
            let pieces = |r: f64, angle: f64| {
                (((o.fragments)(r) as f64) * angle / 360.0).ceil().max(1.0) as usize
            };
            let usable = (in_entities && wanted) || in_blocks;
            match mode.as_str() {
                "SECTION" => {
                    in_entities = iddata == "ENTITIES";
                    in_blocks = iddata == "BLOCKS";
                }
                "LINE" if usable && xs.len() >= 2 && ys.len() >= 2 => {
                    add([xs[0], ys[0]], [xs[1], ys[1]]);
                }
                "LWPOLYLINE" if usable => {
                    let n = xs.len().min(ys.len());
                    for i in 1..n {
                        add([xs[i - 1], ys[i - 1]], [xs[i], ys[i]]);
                    }
                    if flags & 1 != 0 && n > 1 {
                        add([xs[n - 1], ys[n - 1]], [xs[0], ys[0]]);
                    }
                }
                "CIRCLE" if usable && !xs.is_empty() && !ys.is_empty() => {
                    let n = (o.fragments)(radius);
                    let c = [xs[0], ys[0]];
                    for i in 0..n {
                        let (a1, a2) = (
                            360.0 * i as f64 / n as f64,
                            360.0 * (i + 1) as f64 / n as f64,
                        );
                        add(
                            [c[0] + radius * cos_deg(a1), c[1] + radius * sin_deg(a1)],
                            [c[0] + radius * cos_deg(a2), c[1] + radius * sin_deg(a2)],
                        );
                    }
                }
                "ARC" if usable && !xs.is_empty() && !ys.is_empty() => {
                    while a50 > a51 {
                        a51 += 360.0;
                    }
                    let sweep = a51 - a50;
                    let n = pieces(radius, sweep);
                    let c = [xs[0], ys[0]];
                    for i in 0..n {
                        let a1 = a50 + sweep * i as f64 / n as f64;
                        let a2 = a50 + sweep * (i + 1) as f64 / n as f64;
                        add(
                            [c[0] + radius * cos_deg(a1), c[1] + radius * sin_deg(a1)],
                            [c[0] + radius * cos_deg(a2), c[1] + radius * sin_deg(a2)],
                        );
                    }
                }
                "ELLIPSE" if usable && xs.len() >= 2 && ys.len() >= 2 => {
                    while e41 > e42 {
                        e42 += std::f64::consts::TAU;
                    }
                    let (c, ce) = ([xs[0], ys[0]], [xs[1], ys[1]]);
                    let major = ce[0].hypot(ce[1]);
                    let rot = ce[1].atan2(ce[0]);
                    let minor = major * radius;
                    let sweep = e42 - e41;
                    let n = pieces(major, sweep.to_degrees());
                    let at = |a: f64| {
                        let (px, py) = (a.cos() * major, a.sin() * minor);
                        [
                            rot.cos() * px - rot.sin() * py + c[0],
                            rot.sin() * px + rot.cos() * py + c[1],
                        ]
                    };
                    for i in 0..n {
                        add(
                            at(e41 + sweep * i as f64 / n as f64),
                            at(e41 + sweep * (i + 1) as f64 / n as f64),
                        );
                    }
                }
                "INSERT" if usable && !xs.is_empty() && !ys.is_empty() => {
                    let (s, c) = (sin_deg(a50), cos_deg(a50));
                    let (kx, ky) = (e41, e42);
                    for [p, q] in inserted {
                        let t = |p: V2| {
                            let (lx, ly) = (p[0] * kx, p[1] * ky);
                            [
                                (c * lx - s * ly) * sc + xs[0],
                                (s * lx + c * ly) * sc + ys[0],
                            ]
                        };
                        add(t(p), t(q));
                    }
                }
                "BLOCK" => block = iddata.clone(),
                "ENDBLK" => block.clear(),
                _ => {}
            }
            mode = data.to_string();
            layer.clear();
            iddata.clear();
            flags = 0;
            xs.clear();
            ys.clear();
            radius = 0.0;
            (a50, a51) = (0.0, 0.0);
            (e41, e42) = if mode == "INSERT" {
                (1.0, 1.0)
            } else {
                (0.0, 0.0)
            };
            continue;
        }
        match id {
            2 => iddata = data.to_string(),
            8 => layer = data.to_string(),
            10 | 11 => xs.push(if in_blocks { num() } else { (num() - ox) * sc }),
            20 | 21 => ys.push(if in_blocks { num() } else { (num() - oy) * sc }),
            40 => radius = if in_blocks { num() } else { num() * sc },
            41 => e41 = num(),
            42 => e42 = num(),
            50 => a50 = num(),
            51 => a51 = num(),
            70 => flags = data.parse().unwrap_or(0),
            _ => {}
        }
    }
    chain(&segs)
}

/// Segments chained by their ends into paths: those with a loose end
/// first, from that end, then the loops.
fn chain(segs: &[[V2; 2]]) -> Vec<Vec<V2>> {
    let key = |p: V2| p.map(|x| (x * 1e6).round() as i64);
    let mut at: HashMap<[i64; 2], Vec<usize>> = HashMap::new();
    for (i, s) in segs.iter().enumerate() {
        for p in s {
            at.entry(key(*p)).or_default().push(i);
        }
    }
    let mut used = vec![false; segs.len()];
    let mut out = Vec::new();
    let walk = |start: usize, from: usize, used: &mut Vec<bool>| {
        let mut path = vec![segs[start][from]];
        let (mut cur, mut end) = (start, 1 - from);
        loop {
            used[cur] = true;
            let p = segs[cur][end];
            path.push(p);
            let next = at.get(&key(p)).and_then(|l| {
                l.iter().copied().find(|&j| !used[j]).map(|j| {
                    let e = if key(segs[j][0]) == key(p) { 1 } else { 0 };
                    (j, e)
                })
            });
            match next {
                Some((j, e)) => (cur, end) = (j, e),
                None => break,
            }
        }
        path
    };
    for (i, s) in segs.iter().enumerate() {
        for (from, p) in s.iter().enumerate() {
            if used[i] {
                continue;
            }
            let loose = at
                .get(&key(*p))
                .is_some_and(|l| l.iter().all(|&j| j == i || used[j]));
            if loose {
                out.push(walk(i, from, &mut used));
            }
        }
    }
    for i in 0..segs.len() {
        if !used[i] {
            out.push(walk(i, 0, &mut used));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options() -> Options {
        Options {
            center: false,
            dpi: 72.0,
            layer: None,
            id: None,
            origin: [0.0, 0.0],
            scale: 1.0,
            fragments: std::rc::Rc::new(|_| 8),
            fn_: 0.0,
        }
    }

    fn area(p: &[V2]) -> f64 {
        (0..p.len())
            .map(|i| {
                let (a, b) = (p[i], p[(i + 1) % p.len()]);
                a[0] * b[1] - a[1] * b[0]
            })
            .sum::<f64>()
            / 2.0
    }

    #[test]
    fn off_and_obj_faces() {
        let off = "OFF\n# a tetrahedron\n4 4 6\n0 0 0\n1 0 0\n0 1 0\n0 0 1\n\
                   3 0 2 1\n3 0 1 3\n3 0 3 2\n3 1 2 3\n";
        let f = mesh("off", off.as_bytes()).unwrap();
        assert_eq!(f.len(), 4);
        assert_eq!(f[0], vec![[0.0; 3], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0]]);
        let obj =
            "v 0 0 0\nv 1 0 0\nv 0 1 0\nv 0 0 1\nf 1 3 2\nf 1/1 2/2 4/4\nf -4 -1 -2\nf 2 3 4\n";
        let f = mesh("obj", obj.as_bytes()).unwrap();
        assert_eq!(f.len(), 4);
        assert_eq!(f[2], vec![[0.0; 3], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0]]);
    }

    #[test]
    fn svg_shapes_in_millimetres_y_up() {
        // 100 by 50 user units on a page 100 mm by 50 mm.
        let text = r##"<?xml version="1.0"?>
            <!-- a comment -->
            <svg xmlns="http://www.w3.org/2000/svg" width="100mm" height="50mm" viewBox="0 0 100 50">
              <defs><rect id="r" width="10" height="10"/></defs>
              <g transform="translate(10,5)">
                <rect x="0" y="0" width="20" height="10"/>
                <path d="M30 0 h10 v10 h-10 z m2 2 h6 v6 h-6 z"/>
              </g>
              <use href="#r" x="60" y="30"/>
              <circle cx="80" cy="10" r="5" style="display:none"/>
            </svg>"##;
        let shapes = svg(text, &options()).unwrap();
        assert_eq!(shapes.len(), 3);
        // The rectangle: x from 10 to 30, y from 50 - 15 to 50 - 5.
        let r = &shapes[0][0];
        let xs: Vec<f64> = r.iter().map(|p| p[0]).collect();
        let ys: Vec<f64> = r.iter().map(|p| p[1]).collect();
        let near = |a: f64, b: f64| assert!((a - b).abs() < 1e-9, "{a} is not {b}");
        near(xs.iter().cloned().fold(f64::MAX, f64::min), 10.0);
        near(ys.iter().cloned().fold(f64::MAX, f64::min), 35.0);
        near(ys.iter().cloned().fold(f64::MIN, f64::max), 45.0);
        // The path: two loops, 100 and 36 in area.
        let a: Vec<f64> = shapes[1].iter().map(|l| area(l).abs()).collect();
        assert!(
            (a[0] - 100.0).abs() < 1e-9 && (a[1] - 36.0).abs() < 1e-9,
            "{a:?}"
        );
        // What <use> refers to, moved.
        assert!(
            shapes[2][0]
                .iter()
                .all(|p| p[0] >= 60.0 - 1e-9 && p[1] <= 20.0 + 1e-9)
        );
    }

    #[test]
    fn svg_arcs_and_curves_end_where_they_should() {
        let text =
            r#"<svg width="10" height="10"><path d="M0,0 a5,5 0 1,0 10,0 Q 5,-5 0,0 z"/></svg>"#;
        let o = Options {
            dpi: 25.4,
            ..options()
        };
        let shapes = svg(text, &o).unwrap();
        let l = &shapes[0][0];
        // The half circle below, the curve above; the page is 10 high.
        let ys: Vec<f64> = l.iter().map(|p| p[1]).collect();
        let lo = ys.iter().cloned().fold(f64::MAX, f64::min);
        assert!((lo - 5.0).abs() < 1e-9, "{lo}");
        assert!(
            l.iter()
                .any(|p| (p[0] - 10.0).abs() < 1e-9 && (p[1] - 10.0).abs() < 1e-9)
        );
    }

    #[test]
    fn dxf_lines_and_circles_in_their_layers() {
        let mut t = String::from("0\nSECTION\n2\nENTITIES\n");
        for (a, b) in [
            ((0, 0), (10, 0)),
            ((10, 0), (10, 10)),
            ((0, 10), (10, 10)),
            ((0, 0), (0, 10)),
        ] {
            t += &format!(
                "0\nLINE\n8\nA\n10\n{}\n20\n{}\n11\n{}\n21\n{}\n",
                a.0, a.1, b.0, b.1
            );
        }
        t += "0\nCIRCLE\n8\nB\n10\n20\n20\n0\n40\n2\n0\nENDSEC\n0\nEOF\n";
        let paths = dxf(&t, &options());
        assert_eq!(paths.len(), 2);
        assert!((area(&paths[0]).abs() - 100.0).abs() < 1e-9);
        let only_b = dxf(
            &t,
            &Options {
                layer: Some("B".into()),
                ..options()
            },
        );
        assert_eq!(only_b.len(), 1);
        assert_eq!(only_b[0].len(), 9);
    }
}
