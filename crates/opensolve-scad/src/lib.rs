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
mod flat;
mod fonts;
mod geom;
mod import;
mod lexer;
pub mod library;
mod paint;
mod parser;
mod region;
mod store;
mod trig;
mod value;

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

use opensolve_kernel::{Facets, Solid};

pub use customizer::{Choices, Kind, Param, parameters};
pub use store::{Cache, CacheStats};

/// This crate's version, as its release is tagged (`v` and this): the same
/// for all crates of the workspace.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub use opensolve_kernel::Stop;
pub use paint::{Rgba, text as rgba_text};

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

/// The message of the error an evaluation ends with when it was asked to
/// stop (see [`Stop`]): what [`build_with_stop`] returns then, and the
/// message of [`Error::cancelled`].
pub const CANCELLED: &str = "the evaluation was cancelled";

impl Error {
    pub fn at(line: u32, col: u32, message: impl Into<String>) -> Error {
        Error {
            file: None,
            line,
            col,
            message: message.into(),
        }
    }

    /// The error of an evaluation asked to stop: no place, [`CANCELLED`].
    pub fn cancelled() -> Error {
        Error::at(0, 0, CANCELLED)
    }

    /// Whether this is [`Error::cancelled`] rather than a fault of the file.
    pub fn is_cancelled(&self) -> bool {
        self.line == 0 && self.file.is_none() && self.message == CANCELLED
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
    build_with_stop(path, read, overrides, &Stop::new())
}

/// [`build`], ended early when `stop` is set (from another thread): then
/// the error is [`CANCELLED`], and nothing of the run is kept (see
/// [`Stop`] for how soon it ends).
pub fn build_with_stop(
    path: &Path,
    read: Reader<'_>,
    overrides: &[(String, String)],
    stop: &Stop,
) -> Result<Output, String> {
    build_in(path, read, overrides, stop, None)
}

/// [`build_with_stop`] with a cache of results across runs (see
/// [`Cache`]): what the runs before made and nothing it depends on
/// changed since is not made again. The result is the same as without.
pub fn build_with_cache(
    path: &Path,
    read: Reader<'_>,
    overrides: &[(String, String)],
    stop: &Stop,
    cache: &Cache,
) -> Result<Output, String> {
    build_in(path, read, overrides, stop, Some(cache))
}

fn build_in(
    path: &Path,
    read: Reader<'_>,
    overrides: &[(String, String)],
    stop: &Stop,
    cache: Option<&Cache>,
) -> Result<Output, String> {
    let path = plain(path);
    let mut sources = Sources::default();
    gather(&path, read, &mut sources)?;
    let cache = cache.cloned().or_else(shared_cache);
    deep(move || {
        opensolve_kernel::stopping(stop, || {
            let out = evaluate(path, sources, overrides, stop, cache.as_ref());
            if stop.is_stopped() {
                return Err(CANCELLED.to_string());
            }
            out
        })
    })?
}

/// Which of a file's coloured parts a run makes: all, as OpenSCAD renders
/// them; only what no `color()` is around; or only what the outermost
/// `color()` gives this colour. The booleans keep no colours, so the parts
/// of a file in each colour are made one colour at a time.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Paint {
    #[default]
    All,
    Uncoloured,
    Only(Rgba),
}

/// How a file is run by [`make`].
#[derive(Clone, Debug, Default)]
pub struct Run {
    /// Statements after the main file's, each the text of one of
    /// OpenSCAD's `-D`: `name=value`, or any statement, such as a module
    /// that replaces one of the file's or a built-in one.
    pub defines: Vec<String>,
    pub paint: Paint,
    /// Told every echo (`ECHO: …`) and warning (`WARNING: …`) as it comes,
    /// each time: OpenSCAD's console. The notes of [`Made`] have each once
    /// and at most a few dozen.
    pub log: Option<fn(&str)>,
    /// Set (from another thread), the run ends early with
    /// [`Error::cancelled`].
    pub stop: Stop,
    /// Results kept across runs of the file (see [`Cache`]).
    pub cache: Option<Cache>,
}

/// What a file makes, as [`make`] finds it.
#[derive(Clone, Debug, Default)]
pub enum Shape {
    /// Nothing.
    #[default]
    Empty,
    /// A solid.
    Solid(Solid, Facets),
    /// A 2D shape: closed loops that do not cross, outlines
    /// counter-clockwise, holes clockwise.
    Flat(Vec<Vec<[f64; 2]>>),
}

/// The result of [`make`]: the shape, the colours of the outermost
/// `color()`s met on the way (each once), and the notes as in [`Output`].
#[derive(Clone, Debug, Default)]
pub struct Made {
    pub shape: Shape,
    pub colours: Vec<Rgba>,
    pub notes: Vec<String>,
}

/// Runs the file at `path` as OpenSCAD's command line does: with the
/// statements of `run.defines` after its own, making the parts that
/// `run.paint` asks for. What it includes and uses is read by `read`, as
/// in [`build`]. An empty or 2D result is no error here.
pub fn make(path: &Path, read: Reader<'_>, run: &Run) -> Result<Made, Error> {
    let path = plain(path);
    let mut sources = Sources::default();
    gather(&path, read, &mut sources).map_err(|e| Error::at(0, 0, e))?;
    let stop = &run.stop;
    let cache = run.cache.clone().or_else(shared_cache);
    deep(move || {
        let mut it = interp(sources, &path, cache.as_ref());
        it.paint = run.paint;
        it.log = run.log;
        it.stop = stop.clone();
        let made = (|| {
            let mut stmts = it.load(path, &mut Vec::new())?;
            for d in &run.defines {
                // As OpenSCAD adds them: each after the file, with a `;`.
                stmts.extend(parser::parse(&format!("{d};\n"), 0).map_err(|mut e| {
                    e.message = format!("-D {d}: {}", e.message);
                    e.line = 0;
                    e
                })?);
            }
            opensolve_kernel::stopping(stop, || it.run(&stmts, &eval::Interp::root()))
        })();
        it.finish();
        let made = made?;
        if stop.is_stopped() {
            return Err(Error::cancelled());
        }
        let shape = match made {
            Some(geom::Geom::D3(s)) if !s.is_empty() => {
                let mut facets = Facets::default();
                let solid = s.rehome(&it.geo.facets, &mut facets);
                Shape::Solid(solid, facets)
            }
            Some(geom::Geom::D2(r)) if !r.loops.is_empty() => Shape::Flat(r.loops),
            _ => Shape::Empty,
        };
        Ok(Made {
            shape,
            colours: std::mem::take(&mut it.colours),
            notes: it.take_notes(),
        })
    })
    .map_err(|e| Error::at(0, 0, e))?
}

/// Runs the file at `path` as [`build`] does and tells what goes wrong,
/// with places: what an editor marks. The shapes it makes do not matter.
pub fn diagnose(path: &Path, read: Reader<'_>, overrides: &[(String, String)]) -> Diagnosis {
    diagnose_with_stop(path, read, overrides, &Stop::new())
}

/// [`diagnose`], ended early when `stop` is set (from another thread):
/// then its error is [`Error::cancelled`] and it has no notes.
pub fn diagnose_with_stop(
    path: &Path,
    read: Reader<'_>,
    overrides: &[(String, String)],
    stop: &Stop,
) -> Diagnosis {
    diagnose_in(path, read, overrides, stop, None)
}

/// [`diagnose_with_stop`] with a cache of results across runs (see
/// [`Cache`]): the notes of results kept are told again.
pub fn diagnose_with_cache(
    path: &Path,
    read: Reader<'_>,
    overrides: &[(String, String)],
    stop: &Stop,
    cache: &Cache,
) -> Diagnosis {
    diagnose_in(path, read, overrides, stop, Some(cache))
}

fn diagnose_in(
    path: &Path,
    read: Reader<'_>,
    overrides: &[(String, String)],
    stop: &Stop,
    cache: Option<&Cache>,
) -> Diagnosis {
    let path = plain(path);
    let mut sources = Sources::default();
    if let Err(e) = gather(&path, read, &mut sources) {
        return Diagnosis {
            error: Some(Error::at(0, 0, e)),
            notes: Vec::new(),
        };
    }
    let cache = cache.cloned().or_else(shared_cache);
    deep(move || {
        let mut it = interp(sources, &path, cache.as_ref());
        it.stop = stop.clone();
        let error = it
            .load(path, &mut Vec::new())
            .and_then(|mut stmts| {
                override_values(&mut it, &mut stmts, overrides);
                opensolve_kernel::stopping(stop, || it.run(&stmts, &eval::Interp::root()))
            })
            .err();
        it.finish();
        if stop.is_stopped() {
            return Diagnosis {
                error: Some(Error::cancelled()),
                notes: Vec::new(),
            };
        }
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

/// The file at `path` and those it includes, uses and imports (those
/// named as text), as far as they can be read: what to watch for changes.
pub fn files(path: &Path, read: Reader<'_>) -> Vec<PathBuf> {
    let mut sources = Sources::default();
    let _ = gather(&plain(path), read, &mut sources);
    let mut out: Vec<PathBuf> = sources.files.into_keys().collect();
    out.extend(sources.blobs.into_keys());
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

/// The paths `import(…)` names as text in a file's text, as written:
/// `import("a.stl")`, `import(file = "a.svg")`.
pub fn imports(text: &str) -> Vec<String> {
    let Ok(tokens) = lexer::tokens(text) else {
        return Vec::new();
    };
    let at = |i: usize| tokens.get(i).map(|t| &t.tok);
    let mut out = Vec::new();
    for i in 0..tokens.len() {
        if at(i) != Some(&lexer::Tok::Name("import".into()))
            || at(i + 1) != Some(&lexer::Tok::Sym("("))
        {
            continue;
        }
        let path = match (at(i + 2), at(i + 3), at(i + 4)) {
            (Some(lexer::Tok::Str(s)), _, _) => Some(s),
            (Some(lexer::Tok::Name(n)), Some(lexer::Tok::Sym("=")), Some(lexer::Tok::Str(s)))
                if n == "file" || n == "filename" =>
            {
                Some(s)
            }
            _ => None,
        };
        out.extend(path.cloned());
    }
    out
}

/// The files read: the OpenSCAD files, each parsed once (see [`cache`]),
/// and those imported.
#[derive(Default)]
struct Sources {
    files: HashMap<PathBuf, std::sync::Arc<cache::Parsed>>,
    blobs: HashMap<PathBuf, std::sync::Arc<Vec<u8>>>,
}

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
    if sources.files.contains_key(path) {
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
    sources.files.insert(path.to_path_buf(), parsed.clone());
    for p in &parsed.imports {
        let file = plain(&dir.join(p));
        if !sources.blobs.contains_key(&file)
            && let Ok(b) = read(&file)
        {
            sources.blobs.insert(file, std::sync::Arc::new(b));
        }
    }
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

/// The cache of runs not given one, when `OPENSOLVE_SCAD_SHARED_CACHE` is
/// set: one for the whole process, which keeps every result however
/// quickly made. It is there to run the tests with a cache: results of
/// one test used by the others, of files alike or not, must change
/// nothing.
fn shared_cache() -> Option<Cache> {
    static SHARED: std::sync::OnceLock<Option<Cache>> = std::sync::OnceLock::new();
    SHARED
        .get_or_init(|| {
            std::env::var_os("OPENSOLVE_SCAD_SHARED_CACHE")
                .map(|_| Cache::with_limits(1 << 30, std::time::Duration::ZERO))
        })
        .clone()
}

/// An interpreter of the files read, with `cache` if given.
fn interp(sources: Sources, main: &Path, cache: Option<&Cache>) -> eval::Interp {
    let list: Vec<store::Source> = match cache {
        Some(_) => sources
            .files
            .iter()
            .map(|(p, f)| store::Source {
                path: p.clone(),
                hash: f.hash,
                code: true,
            })
            .chain(sources.blobs.iter().map(|(p, b)| store::Source {
                path: p.clone(),
                hash: store::hash_bytes(b),
                code: false,
            }))
            .collect(),
        None => Vec::new(),
    };
    let mut it = eval::Interp::new(sources.files, sources.blobs);
    if let Some(c) = cache {
        it.use_cache(c, &list, main);
    }
    it
}

fn evaluate(
    path: PathBuf,
    sources: Sources,
    overrides: &[(String, String)],
    stop: &Stop,
    cache: Option<&Cache>,
) -> Result<Output, String> {
    let mut it = interp(sources, &path, cache);
    it.stop = stop.clone();
    let made = it.load(path, &mut Vec::new()).and_then(|mut stmts| {
        override_values(&mut it, &mut stmts, overrides);
        it.run(&stmts, &eval::Interp::root())
    });
    it.finish();
    let made = made.map_err(|e| e.to_string())?;
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
