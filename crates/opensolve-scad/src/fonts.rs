//! The fonts of `text()`, found as OpenSCAD finds them through fontconfig:
//! by family and style name (`"Liberation Sans:style=Bold"`), read from
//! the name tables of the machine's font files (TrueType, OpenType, and
//! collections of them), and OpenSCAD's own fonts where it is installed.
//! Without a name, or one the machine does not have, Liberation Sans, as in
//! OpenSCAD; where the machine has none, the copy that comes with OpenSolve
//! (SIL Open Font License, `assets/fonts/LiberationSans-LICENSE.txt`).

use std::path::{Path, PathBuf};

/// Liberation Sans 2.00.1, OpenSCAD's default font.
static SANS: &[u8] = include_bytes!("../../../assets/fonts/LiberationSans-Regular.ttf");
static SANS_BOLD: &[u8] = include_bytes!("../../../assets/fonts/LiberationSans-Bold.ttf");

/// A face of a font file: its family and style.
#[derive(Clone, Debug)]
pub struct Face {
    pub family: String,
    pub style: String,
    /// Where it is: a file and its number in a collection, or built in.
    pub at: Source,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    File(PathBuf, u32),
    BuiltIn(&'static [u8]),
}

/// A font found: its bytes and the number of the face in them.
#[derive(Clone, Copy, Debug)]
pub struct Found {
    pub data: &'static [u8],
    pub index: u32,
    /// The family asked for is not there: another stands in.
    pub stand_in: bool,
}

/// `family` and `style=…` of an OpenSCAD font name; the style also from a
/// bare word (`Arial:bold`).
pub fn parse(spec: &str) -> (String, String) {
    let mut parts = spec.split(':');
    let family = parts.next().unwrap_or("").trim().to_string();
    let mut style = String::new();
    for p in parts {
        match p.split_once('=') {
            Some((k, v)) if k.trim().eq_ignore_ascii_case("style") => style = v.trim().to_string(),
            Some(_) => {}
            None if !p.trim().is_empty() => {
                if !style.is_empty() {
                    style.push(' ');
                }
                style.push_str(p.trim());
            }
            None => {}
        }
    }
    (family, style)
}

/// Lower case, without blanks: as fontconfig compares names.
fn plain(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect()
}

/// How well a face's style fits the one asked for: higher is better.
fn style_score(asked: &str, has: &str) -> i32 {
    let (a, h) = (plain(asked), plain(has));
    if a == h {
        return 100;
    }
    let words = |s: &str| -> (bool, bool, bool) {
        let s = plain(s);
        (
            s.contains("bold") || s.contains("black") || s.contains("heavy"),
            s.contains("italic") || s.contains("oblique"),
            s.contains("light") || s.contains("thin"),
        )
    };
    let (wa, wh) = (words(asked), words(has));
    // As fontconfig weighs them: the slant before the weight.
    let mut score = 0;
    score += if wa.1 == wh.1 { 30 } else { 0 };
    score += if wa.0 == wh.0 { 20 } else { 0 };
    score += if wa.2 == wh.2 { 10 } else { 0 };
    if a.is_empty() || a == "regular" {
        for (k, name) in ["regular", "normal", "book", "roman", "medium"]
            .iter()
            .enumerate()
        {
            if h == *name {
                score += 9 - k as i32;
            }
        }
    }
    score
}

/// The face for an OpenSCAD font name among `faces`, else the default.
pub fn choose(spec: &str, faces: &[Face]) -> Found {
    let (family, style) = parse(spec);
    let wanted = match plain(&family).as_str() {
        "" | "sans" | "sans-serif" => "liberationsans".to_string(),
        "serif" => "liberationserif".to_string(),
        "monospace" | "mono" => "liberationmono".to_string(),
        f => f.to_string(),
    };
    let pick = |fam: &str| {
        faces
            .iter()
            .filter(|f| plain(&f.family) == fam)
            .max_by_key(|f| style_score(&style, &f.style))
            .and_then(load)
    };
    if let Some(f) = pick(&wanted) {
        return f;
    }
    let mut f = pick("liberationsans").unwrap_or_else(|| {
        let bold = style_score(&style, "Bold") > style_score(&style, "Regular");
        Found {
            data: if bold { SANS_BOLD } else { SANS },
            index: 0,
            stand_in: false,
        }
    });
    f.stand_in = !family.is_empty() && wanted != "liberationsans";
    f
}

/// The bytes of a face, read once and kept.
fn load(f: &Face) -> Option<Found> {
    match &f.at {
        Source::BuiltIn(d) => Some(Found {
            data: d,
            index: 0,
            stand_in: false,
        }),
        Source::File(path, index) => Some(Found {
            data: read(path)?,
            index: *index,
            stand_in: false,
        }),
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn read(path: &Path) -> Option<&'static [u8]> {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    type Read = Mutex<HashMap<PathBuf, Option<&'static [u8]>>>;
    static READ: OnceLock<Read> = OnceLock::new();
    let mut read = READ.get_or_init(Default::default).lock().ok()?;
    *read.entry(path.to_path_buf()).or_insert_with(|| {
        // Kept for the life of the program, as the built-in fonts are.
        std::fs::read(path)
            .ok()
            .map(|b| &*Box::leak(b.into_boxed_slice()))
    })
}

#[cfg(target_arch = "wasm32")]
fn read(_: &Path) -> Option<&'static [u8]> {
    None
}

/// The faces OpenSolve carries.
fn built_in() -> Vec<Face> {
    [(SANS, "Regular"), (SANS_BOLD, "Bold")]
        .into_iter()
        .map(|(d, style)| Face {
            family: "Liberation Sans".into(),
            style: style.into(),
            at: Source::BuiltIn(d),
        })
        .collect()
}

/// The font for an OpenSCAD font name: among the machine's fonts, the
/// files `extra` names (`use <font.ttf>`), and the built-in ones.
pub fn find(spec: &str, extra: &[PathBuf]) -> Found {
    let mut faces: Vec<Face> = Vec::new();
    for p in extra {
        faces.extend(faces_of_file(p));
    }
    faces.extend(system().iter().cloned());
    faces.extend(built_in());
    choose(spec, &faces)
}

/// The machine's faces, found once.
#[cfg(not(target_arch = "wasm32"))]
fn system() -> &'static [Face] {
    use std::sync::OnceLock;
    static FACES: OnceLock<Vec<Face>> = OnceLock::new();
    FACES.get_or_init(|| {
        let mut out = Vec::new();
        for dir in folders() {
            scan(&dir, 5, &mut out);
        }
        out
    })
}

#[cfg(target_arch = "wasm32")]
fn system() -> &'static [Face] {
    &[]
}

/// The folders fonts are installed in, on macOS, Linux and Windows, and
/// those of OpenSCAD's own fonts (Liberation).
#[cfg(not(target_arch = "wasm32"))]
fn folders() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = [
        "/Applications/OpenSCAD.app/Contents/Resources/fonts",
        "/System/Library/Fonts",
        "/Library/Fonts",
        "/usr/share/openscad/fonts",
        "/usr/share/fonts",
        "/usr/local/share/fonts",
        "C:\\Program Files\\OpenSCAD\\fonts",
        "C:\\Windows\\Fonts",
    ]
    .iter()
    .map(PathBuf::from)
    .collect();
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        out.push(home.join("Library/Fonts"));
        out.push(home.join(".local/share/fonts"));
        out.push(home.join(".fonts"));
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        out.push(PathBuf::from(local).join("Microsoft\\Windows\\Fonts"));
    }
    out
}

#[cfg(not(target_arch = "wasm32"))]
fn scan(dir: &Path, depth: usize, out: &mut Vec<Face>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let path = e.path();
        if path.is_dir() {
            if depth > 0 {
                scan(&path, depth - 1, out);
            }
        } else {
            out.extend(faces_of_file(&path));
        }
    }
}

/// The faces of a font file, by its name table; read in pieces, not whole.
#[cfg(not(target_arch = "wasm32"))]
pub fn faces_of_file(path: &Path) -> Vec<Face> {
    use std::io::{Read, Seek, SeekFrom};
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if !["ttf", "otf", "ttc", "otc"].contains(&ext.as_str()) {
        return Vec::new();
    }
    let Ok(mut file) = std::fs::File::open(path) else {
        return Vec::new();
    };
    let mut at = |offset: u64, len: usize| -> Option<Vec<u8>> {
        let mut b = vec![0; len];
        file.seek(SeekFrom::Start(offset)).ok()?;
        file.read_exact(&mut b).ok()?;
        Some(b)
    };
    let Some(head) = at(0, 12) else {
        return Vec::new();
    };
    let offsets: Vec<u64> = if &head[0..4] == b"ttcf" {
        let n = u32::from_be_bytes([head[8], head[9], head[10], head[11]]).min(256) as usize;
        at(12, 4 * n)
            .map(|b| {
                b.chunks(4)
                    .map(|c| u64::from(u32::from_be_bytes([c[0], c[1], c[2], c[3]])))
                    .collect()
            })
            .unwrap_or_default()
    } else {
        vec![0]
    };
    let mut out = Vec::new();
    for (index, &off) in offsets.iter().enumerate() {
        let Some(h) = at(off, 12) else { continue };
        let tables = u16::from_be_bytes([h[4], h[5]]) as usize;
        let Some(dir) = at(off + 12, 16 * tables) else {
            continue;
        };
        let name = dir.chunks(16).find(|r| &r[0..4] == b"name").map(|r| {
            (
                u64::from(u32::from_be_bytes([r[8], r[9], r[10], r[11]])),
                u32::from_be_bytes([r[12], r[13], r[14], r[15]]) as usize,
            )
        });
        let Some((offset, len)) = name else { continue };
        let Some(table) = at(offset, len.min(1 << 20)) else {
            continue;
        };
        if let Some((family, style)) = names(&table) {
            out.push(Face {
                family,
                style,
                at: Source::File(path.to_path_buf(), index as u32),
            });
        }
    }
    out
}

#[cfg(target_arch = "wasm32")]
pub fn faces_of_file(_: &Path) -> Vec<Face> {
    Vec::new()
}

/// The family and style of a name table: the typographic ones (16, 17)
/// where it has them, else 1 and 2; English Windows names first, then Mac.
fn names(t: &[u8]) -> Option<(String, String)> {
    let u16_at =
        |i: usize| -> Option<u16> { Some(u16::from_be_bytes([*t.get(i)?, *t.get(i + 1)?])) };
    let count = u16_at(2)? as usize;
    let strings = u16_at(4)? as usize;
    // (rank, name id) → text.
    let mut best: [(i32, String); 4] = Default::default();
    for r in 0..count {
        let at = 6 + 12 * r;
        let (platform, encoding, language, id) = (
            u16_at(at)?,
            u16_at(at + 2)?,
            u16_at(at + 4)?,
            u16_at(at + 6)?,
        );
        let (len, off) = (u16_at(at + 8)? as usize, u16_at(at + 10)? as usize);
        let slot = match id {
            1 => 0,
            2 => 1,
            16 => 2,
            17 => 3,
            _ => continue,
        };
        let Some(bytes) = t.get(strings + off..strings + off + len) else {
            continue;
        };
        let (rank, text) = match (platform, encoding) {
            (3, 1 | 10) | (0, _) => {
                let units: Vec<u16> = bytes
                    .chunks_exact(2)
                    .map(|c| u16::from_be_bytes([c[0], c[1]]))
                    .collect();
                let english = if platform == 3 && language == 0x409 {
                    2
                } else {
                    1
                };
                (2 + english, String::from_utf16_lossy(&units))
            }
            (1, 0) => (
                if language == 0 { 2 } else { 1 },
                bytes.iter().map(|&b| b as char).collect(),
            ),
            _ => continue,
        };
        if rank > best[slot].0 {
            best[slot] = (rank, text);
        }
    }
    let pick = |a: usize, b: usize| {
        if best[a].0 > 0 {
            best[a].1.clone()
        } else {
            best[b].1.clone()
        }
    };
    let family = pick(2, 0);
    let style = pick(3, 1);
    (!family.is_empty()).then_some((family, style))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn face(family: &str, style: &str) -> Face {
        Face {
            family: family.into(),
            style: style.into(),
            at: Source::BuiltIn(SANS),
        }
    }

    #[test]
    fn names_and_styles_as_fontconfig_takes_them() {
        assert_eq!(
            parse("Liberation Sans:style=Bold"),
            ("Liberation Sans".into(), "Bold".into())
        );
        assert_eq!(parse("Arial:bold"), ("Arial".into(), "bold".into()));
        assert_eq!(
            parse(":style=Bold Italic"),
            (String::new(), "Bold Italic".into())
        );
        assert!(style_score("Bold", "Bold") > style_score("Bold", "Bold Italic"));
        assert!(style_score("Bold", "Regular") > style_score("Bold", "Bold Italic"));
        assert!(style_score("", "Regular") > style_score("", "Italic"));
        assert!(style_score("light", "Light") > style_score("light", "Regular"));
    }

    #[test]
    fn the_built_in_fonts_have_their_names() {
        // Read as a file would be: the name table of the bytes.
        let f = ttf_name(SANS);
        assert_eq!(f, Some(("Liberation Sans".into(), "Regular".into())));
        assert_eq!(
            ttf_name(SANS_BOLD),
            Some(("Liberation Sans".into(), "Bold".into()))
        );
        // A family not there: Liberation Sans stands in, bold if asked.
        let found = choose("No Such Font:style=Bold", &[face("Other", "Regular")]);
        assert!(found.stand_in);
        assert_eq!(found.data.as_ptr(), SANS_BOLD.as_ptr());
        assert!(!choose("", &[]).stand_in);
    }

    fn ttf_name(d: &[u8]) -> Option<(String, String)> {
        let tables = u16::from_be_bytes([d[4], d[5]]) as usize;
        let r = d[12..12 + 16 * tables]
            .chunks(16)
            .find(|r| &r[0..4] == b"name")?;
        let off = u32::from_be_bytes([r[8], r[9], r[10], r[11]]) as usize;
        let len = u32::from_be_bytes([r[12], r[13], r[14], r[15]]) as usize;
        names(&d[off..off + len])
    }
}
