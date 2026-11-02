//! OpenSCAD files read and built into solids, in Rust, without OpenSCAD
//! (ADR 0011): a parser and interpreter of OpenSCAD's language, and its
//! shapes on the mesh kernel, cut into as many pieces as OpenSCAD cuts them.
//! What is not supported yet (`minkowski`, `offset`, `text`, …) is named in
//! a note, not left out silently.

mod ast;
mod builtins;
mod cache;
mod customizer;
mod eval;
mod geom;
mod lexer;
pub mod library;
mod parser;
mod trig;
mod value;

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

use opensolve_kernel::{Facets, Solid};

pub use customizer::{Choices, Kind, Param, parameters};

/// Reads a whole file.
pub type Reader<'a> = &'a dyn Fn(&Path) -> Result<Vec<u8>, String>;

/// What is wrong, and where: line and column from 1, in `file` if it is
/// not the main one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    pub file: Option<String>,
    pub line: u32,
    pub col: u32,
    pub message: String,
}

impl Error {
    pub fn at(line: u32, col: u32, message: impl Into<String>) -> Error {
        Error {
            file: None,
            line,
            col,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(file) = &self.file {
            write!(f, "{file}, ")?;
        }
        if self.line > 0 {
            write!(f, "line {}, column {}: ", self.line, self.col)?;
        }
        f.write_str(&self.message)
    }
}

impl std::error::Error for Error {}

/// A note with where it is, for an editor: line and column from 1 (0 when
/// it has no place), in `file` if that is not the main one. `echo` says it
/// is what `echo` wrote rather than a problem.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Note {
    pub file: Option<PathBuf>,
    pub line: u32,
    pub col: u32,
    pub message: String,
    pub echo: bool,
}

/// What an evaluation of a file finds, for an editor: the error that ends
/// it, if any, and the notes on the way, each with its place.
#[derive(Clone, Debug, Default)]
pub struct Diagnosis {
    pub error: Option<Error>,
    pub notes: Vec<Note>,
}

/// A file's solid, and what is worth knowing about it: its `echo` output,
/// what is not supported, unknown names.
#[derive(Clone, Debug, Default)]
pub struct Output {
    pub solid: Solid,
    pub facets: Facets,
    pub notes: Vec<String>,
}

/// Builds the file at `path`, as OpenSCAD renders it, its top-level
/// variables set to `overrides` (name, value as OpenSCAD writes it), as
/// OpenSCAD's `-D` sets them. What it includes and uses is read by `read`,
/// from the folder of the file that names it.
pub fn build(
    path: &Path,
    read: Reader<'_>,
    overrides: &[(String, String)],
) -> Result<Output, String> {
    let path = plain(path);
    let mut sources = HashMap::new();
    gather(&path, read, &mut sources)?;
    deep(move || evaluate(path, sources, overrides))?
}

/// Runs the file at `path` as [`build`] does and tells what goes wrong,
/// with places: what an editor marks. The shapes it makes do not matter.
pub fn diagnose(path: &Path, read: Reader<'_>, overrides: &[(String, String)]) -> Diagnosis {
    let path = plain(path);
    let mut sources = HashMap::new();
    if let Err(e) = gather(&path, read, &mut sources) {
        return Diagnosis {
            error: Some(Error::at(0, 0, e)),
            notes: Vec::new(),
        };
    }
    deep(move || {
        let mut it = eval::Interp::new(sources);
        let error = it
            .load(path, &mut Vec::new())
            .and_then(|mut stmts| {
                override_values(&mut it, &mut stmts, overrides);
                it.run(&stmts, &eval::Interp::root())
            })
            .err();
        Diagnosis {
            error,
            notes: std::mem::take(&mut it.marks),
        }
    })
    .unwrap_or_else(|e| Diagnosis {
        error: Some(Error::at(0, 0, e)),
        notes: Vec::new(),
    })
}

/// The file at `path` and those it includes and uses, as far as they can
/// be read: what to watch for changes.
pub fn files(path: &Path, read: Reader<'_>) -> Vec<PathBuf> {
    let mut sources = HashMap::new();
    let _ = gather(&plain(path), read, &mut sources);
    let mut out: Vec<PathBuf> = sources.into_keys().collect();
    out.sort();
    out
}

/// Whether `text` is one value or expression as OpenSCAD writes it: what an
/// override may be.
pub fn check_value(text: &str) -> Result<(), Error> {
    parser::parse_expr(text).map(|_| ())
}

/// The paths of `include <…>` and `use <…>` in a file's text, as written.
pub fn includes(text: &str) -> Vec<String> {
    let Ok(tokens) = lexer::tokens(text) else {
        return Vec::new();
    };
    tokens
        .into_iter()
        .filter_map(|t| match t.tok {
            lexer::Tok::Include(p) | lexer::Tok::Use(p) => Some(p),
            _ => None,
        })
        .collect()
}

/// The files read, each parsed once (see [`cache`]).
type Sources = HashMap<PathBuf, std::sync::Arc<cache::Parsed>>;

/// Reads a file and, as far as they can be read, those it names: from
/// the folder of the file that names them, else from the library folders.
fn gather(path: &Path, read: Reader<'_>, sources: &mut Sources) -> Result<(), String> {
    gather_in(path, read, sources, &library::search())
}

fn gather_in(
    path: &Path,
    read: Reader<'_>,
    sources: &mut Sources,
    search: &[PathBuf],
) -> Result<(), String> {
    if sources.contains_key(path) {
        return Ok(());
    }
    let bytes = match read(path) {
        Ok(b) => b,
        Err(e) if search.iter().any(|d| path.starts_with(d)) => {
            library::read(path).map_err(|_| e)?
        }
        Err(e) => return Err(e),
    };
    let text = String::from_utf8(bytes)
        .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned());
    let parsed = cache::file(path, text);
    let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
    sources.insert(path.to_path_buf(), parsed.clone());
    for p in &parsed.includes {
        let local = plain(&dir.join(p));
        if gather_in(&local, read, sources, search).is_ok() || Path::new(p).is_absolute() {
            continue;
        }
        let found = search
            .iter()
            .any(|lib| gather_in(&plain(&lib.join(p)), read, sources, search).is_ok());
        if !found && Path::new(p).starts_with(library::BUNDLED) {
            library::ensure_bundled();
            if let Some(own) = library::own_dir() {
                let _ = gather_in(&plain(&own.join(p)), read, sources, search);
            }
        }
    }
    Ok(())
}

fn evaluate(
    path: PathBuf,
    sources: Sources,
    overrides: &[(String, String)],
) -> Result<Output, String> {
    let mut it = eval::Interp::new(sources);
    let mut stmts = it.load(path, &mut Vec::new()).map_err(|e| e.to_string())?;
    override_values(&mut it, &mut stmts, overrides);
    let made = it
        .run(&stmts, &eval::Interp::root())
        .map_err(|e| e.to_string())?;
    let notes = it.take_notes();
    match made {
        Some(geom::Geom::D3(s)) if !s.is_empty() => {
            let mut facets = Facets::default();
            let solid = s.rehome(&it.geo.facets, &mut facets);
            Ok(Output {
                solid,
                facets,
                notes,
            })
        }
        Some(geom::Geom::D2(_)) => {
            Err("it makes only 2D shapes: extrude them to make a solid".to_string())
        }
        _ => Err(match notes.first() {
            Some(n) => format!("it makes no solid ({n})"),
            None => "it makes no solid".to_string(),
        }),
    }
}

/// The values of `overrides` assigned after the file's own, as `-D` does.
fn override_values(
    it: &mut eval::Interp,
    stmts: &mut Vec<ast::Stmt>,
    overrides: &[(String, String)],
) {
    for (name, text) in overrides {
        match parser::parse_expr(text) {
            Ok(value) => stmts.push(ast::Stmt::Assign {
                name: name.as_str().into(),
                value,
                pos: ast::Pos::default(),
                span: (0, 0),
                end_line: 0,
            }),
            Err(e) => it.note(None, format!("{name} = {text}: {}", e.message)),
        }
    }
}

/// Room for deep recursion, which OpenSCAD files use for loops.
#[cfg(not(target_arch = "wasm32"))]
const STACK: usize = 256 << 20;

/// So much of the stack is kept free: deeper calls end the evaluation
/// with an error rather than the program.
#[cfg(not(target_arch = "wasm32"))]
const RESERVE: usize = 48 << 20;

thread_local! {
    /// Where the stack of the evaluation's thread began, about.
    static BASE: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Whether the stack is used up but its reserve: calls must go no deeper.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn stack_low() -> bool {
    let here = 0u8;
    let at = std::ptr::addr_of!(here) as usize;
    let base = BASE.with(std::cell::Cell::get);
    base != 0 && base.saturating_sub(at) > STACK - RESERVE
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn stack_low() -> bool {
    false
}

/// Runs `f` on a thread of its own with a large stack.
#[cfg(not(target_arch = "wasm32"))]
fn deep<T: Send>(f: impl FnOnce() -> T + Send) -> Result<T, String> {
    std::thread::scope(|s| {
        let h = std::thread::Builder::new()
            .name("openscad".into())
            .stack_size(STACK)
            .spawn_scoped(s, || {
                let here = 0u8;
                BASE.with(|b| b.set(std::ptr::addr_of!(here) as usize));
                f()
            })
            .map_err(|e| e.to_string())?;
        h.join().map_err(|_| "the evaluation failed".to_string())
    })
}

#[cfg(target_arch = "wasm32")]
fn deep<T>(f: impl FnOnce() -> T) -> Result<T, String> {
    Ok(f())
}

/// `a/b/../c` as `a/c`, without asking the disk.
fn plain(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            c => out.push(c),
        }
    }
    out
}
