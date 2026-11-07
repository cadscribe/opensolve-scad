//! `opensolve-scad`: OpenSCAD files made into STL, 3MF, OFF, OBJ, CSG,
//! STEP, SVG or DXF without OpenSCAD, with the options of OpenSCAD's own
//! command line that scripts use: `-o`, `-D`, `--export-format`, `-q`,
//! `--hardwarnings`, `-d`. Echoes and warnings go to stderr as OpenSCAD
//! writes them; the exit code is 0 when the files were written, 1 when not.
//!
//! The booleans keep no colours, so for formats that hold colours (3MF and
//! CSG) a file's parts are made one colour at a time, by the outermost
//! `color()` around them, as colorscad makes them with OpenSCAD. The runs
//! of one call share a cache of results (`opensolve_scad::Cache`);
//! `--cache-stats` tells what it did.

mod write;

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use opensolve_scad::{Cache, Made, Paint, Run, Shape};
use write::{Dims, Format, Parts, Piece};

const USAGE: &str = "\
usage: opensolve-scad [options] -o OUTPUT FILE.scad

Makes an OpenSCAD file into OUTPUT, as `openscad -o` does; the format is
the output's extension or --export-format.

  -o FILE               the file to write; `-` is stdout (with
                        --export-format); more than one -o writes each
  -D NAME=VALUE         sets a variable after the file's own, as in
                        OpenSCAD; any statement, such as a module, works too
  --export-format FMT   stl (ASCII), asciistl, binstl, off, obj, 3mf, step,
                        csg, svg, dxf, echo
  -d FILE               writes the files read, as a make dependency file
  -q, --quiet           no echoes and warnings on stderr
  --hardwarnings        a warning fails the run
  --cache-stats         writes to stderr what the cache of results did (the
                        runs of one call, the whole and each colour, share it)
  --preview             $preview is true
  --render, --backend X, --enable X
                        accepted as OpenSCAD takes them, without effect
  -h, --help            this text
  -v, --version         the version

3MF and CSG keep colours: each colour of the outermost color() is a part.
";

/// The lines OpenSCAD's console would show, for an `.echo` file.
static LINES: Mutex<Vec<String>> = Mutex::new(Vec::new());
static QUIET: AtomicBool = AtomicBool::new(false);
/// The results the runs of this call share.
static CACHE: std::sync::LazyLock<Cache> = std::sync::LazyLock::new(Cache::new);
static WARNED: AtomicBool = AtomicBool::new(false);

fn log(line: &str) {
    if line.starts_with("WARNING") {
        WARNED.store(true, Ordering::Relaxed);
    }
    if !QUIET.load(Ordering::Relaxed) {
        eprintln!("{line}");
    }
    if let Ok(mut l) = LINES.lock() {
        l.push(line.to_string());
    }
}

#[derive(Debug, Default)]
struct Options {
    outputs: Vec<String>,
    format: Option<Format>,
    defines: Vec<String>,
    deps: Option<String>,
    hard_warnings: bool,
    cache_stats: bool,
    input: Option<PathBuf>,
}

/// What went wrong: the message for stderr, and whether to show the usage.
type Failure = (String, bool);

fn options(args: impl IntoIterator<Item = String>) -> Result<Option<Options>, Failure> {
    let mut o = Options::default();
    let mut args = args.into_iter();
    let usage = |m: String| (m, true);
    while let Some(a) = args.next() {
        // `--name=value` as well as `--name value`.
        let (flag, inline) = match a.split_once('=') {
            Some((f, v)) if f.starts_with("--") => (f.to_string(), Some(v.to_string())),
            _ => (a.clone(), None),
        };
        let mut value = |name: &str| {
            inline
                .clone()
                .or_else(|| args.next())
                .ok_or_else(|| usage(format!("{name} needs a value")))
        };
        match flag.as_str() {
            "-h" | "--help" => {
                print!("{USAGE}");
                return Ok(None);
            }
            "-v" | "--version" => {
                println!("opensolve-scad {}", opensolve_scad::VERSION);
                return Ok(None);
            }
            "-o" | "--o" | "--output" => o.outputs.push(value("-o")?),
            "-D" => o.defines.push(value("-D")?),
            "-d" => o.deps = Some(value("-d")?),
            "--export-format" => {
                let f = value("--export-format")?;
                o.format =
                    Some(Format::named(&f).ok_or_else(|| usage(format!("unknown format {f}")))?);
            }
            "-q" | "--quiet" => QUIET.store(true, Ordering::Relaxed),
            "--hardwarnings" => o.hard_warnings = true,
            "--cache-stats" => o.cache_stats = true,
            "--preview" => o.defines.insert(0, "$preview=true".into()),
            "--render" => {}
            "--backend" | "--enable" => {
                value(&flag)?;
            }
            "-p" | "-P" => {
                return Err((
                    format!("{flag}: parameter sets are not supported; use -D"),
                    false,
                ));
            }
            _ if a.starts_with('-') && a.len() > 1 => {
                return Err(usage(format!("unknown option {a}")));
            }
            _ if o.input.is_some() => return Err(usage("more than one input file".into())),
            _ => o.input = Some(PathBuf::from(a)),
        }
    }
    if o.input.is_none() {
        return Err(usage("no input file".into()));
    }
    if o.outputs.is_empty() {
        return Err(usage("no output file: -o is needed".into()));
    }
    Ok(Some(o))
}

fn main() -> ExitCode {
    match options(std::env::args().skip(1)) {
        Ok(None) => ExitCode::SUCCESS,
        Ok(Some(o)) => {
            let done = run(&o);
            if o.cache_stats {
                eprintln!("cache: {}", CACHE.stats());
            }
            match done {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("ERROR: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        Err((e, usage)) => {
            eprintln!("opensolve-scad: {e}");
            if usage {
                eprint!("\n{USAGE}");
            }
            ExitCode::FAILURE
        }
    }
}

fn read(p: &Path) -> Result<Vec<u8>, String> {
    std::fs::read(p).map_err(|e| format!("{}: {e}", p.display()))
}

/// The format of an output: `--export-format`, else its extension.
fn format_of(out: &str, given: Option<Format>) -> Result<Format, String> {
    if let Some(f) = given {
        return Ok(f);
    }
    let ext = Path::new(out)
        .extension()
        .map(|e| e.to_string_lossy().into_owned())
        .unwrap_or_default();
    Format::named(&ext).ok_or_else(|| {
        if out == "-" {
            "-o - needs --export-format".to_string()
        } else {
            format!("{out}: unknown format \"{ext}\"")
        }
    })
}

fn run(o: &Options) -> Result<(), String> {
    let input = o.input.as_deref().unwrap_or(Path::new(""));
    let formats: Vec<Format> = o
        .outputs
        .iter()
        .map(|out| format_of(out, o.format))
        .collect::<Result<_, _>>()?;
    let made = make(input, &o.defines, Paint::All, Some(log))?;
    if o.hard_warnings && WARNED.load(Ordering::Relaxed) {
        return Err("a warning, and --hardwarnings".into());
    }
    let name = input.file_stem().map_or("OpenSCAD Model".into(), |s| {
        s.to_string_lossy().into_owned()
    });
    // Each colour apart, made once for all outputs that keep colours.
    let mut parts: Option<Parts<Shape>> = None;
    for (out, format) in o.outputs.iter().zip(formats) {
        let bytes = match format {
            Format::Echo => {
                let lines = LINES.lock().map(|l| l.join("\n")).unwrap_or_default();
                format!("{lines}\n").into_bytes()
            }
            f if f.keeps_colours() && !made.colours.is_empty() => {
                if parts.is_none() {
                    parts = Some(painted(input, &o.defines, &made)?);
                }
                encode_parts(f, &name, parts.as_ref().unwrap_or(&Parts::default()))?
            }
            f => encode(f, &name, &made.shape)?,
        };
        put(out, &bytes)?;
    }
    if let Some(deps) = &o.deps {
        let mut text = format!("{}:", o.outputs.join(" "));
        for f in opensolve_scad::files(input, &read) {
            text.push_str(" \\\n\t");
            text.push_str(&f.display().to_string().replace(' ', "\\ "));
        }
        text.push('\n');
        put(deps, text.as_bytes())?;
    }
    Ok(())
}

fn make(
    input: &Path,
    defines: &[String],
    paint: Paint,
    log: Option<fn(&str)>,
) -> Result<Made, String> {
    let run = Run {
        defines: defines.to_vec(),
        paint,
        log,
        cache: Some(CACHE.clone()),
        ..Run::default()
    };
    opensolve_scad::make(input, &read, &run).map_err(|mut e| {
        if e.file.is_none() && e.line > 0 {
            e.file = Some(input.display().to_string());
        }
        e.to_string()
    })
}

/// The parts of each colour and the one of none, made at once on threads
/// of their own.
fn painted(input: &Path, defines: &[String], all: &Made) -> Result<Parts<Shape>, String> {
    let paints: Vec<Paint> = std::iter::once(Paint::Uncoloured)
        .chain(all.colours.iter().map(|c| Paint::Only(*c)))
        .collect();
    let made: Vec<Result<Made, String>> = std::thread::scope(|s| {
        let handles: Vec<_> = paints
            .iter()
            .map(|&p| s.spawn(move || make(input, defines, p, None)))
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().unwrap_or_else(|_| Err("a part failed".into())))
            .collect()
    });
    let mut parts = Parts::default();
    for (p, m) in paints.into_iter().zip(made) {
        let shape = m?.shape;
        if matches!(shape, Shape::Empty) {
            continue;
        }
        match p {
            Paint::Only(c) => parts.painted.push((c, shape)),
            _ => parts.plain = Some(shape),
        }
    }
    Ok(parts)
}

const EMPTY: &str = "Current top level object is empty.";
const NOT_3D: &str = "Current top level object is not a 3D object.";
const NOT_2D: &str = "Current top level object is not a 2D object.";

/// A model without colours in a format.
fn encode(f: Format, name: &str, shape: &Shape) -> Result<Vec<u8>, String> {
    match (shape, f.dims()) {
        (Shape::Empty, _) if f == Format::Csg => Ok(write::csg(&Parts::default())),
        (Shape::Empty, _) => Err(EMPTY.into()),
        (Shape::Flat(_), Dims::Three) => Err(NOT_3D.into()),
        (Shape::Solid(..), Dims::Two) => Err(NOT_2D.into()),
        (Shape::Solid(s, facets), _) => Ok(match f {
            Format::AsciiStl => write::ascii_stl(s),
            Format::BinStl => s.to_stl(),
            Format::Off => write::off(s),
            Format::Obj => write::obj(s),
            Format::ThreeMf => opensolve_kernel::three_mf(s, name),
            Format::Step => opensolve_kernel::step(s, facets, name).into_bytes(),
            _ => write::csg(&Parts {
                plain: Some(Piece::Solid(s.clone())),
                painted: Vec::new(),
            }),
        }),
        (Shape::Flat(loops), _) => Ok(match f {
            Format::Svg => write::svg(loops),
            Format::Dxf => write::dxf(loops),
            _ => write::csg(&Parts {
                plain: Some(Piece::Flat(loops.clone())),
                painted: Vec::new(),
            }),
        }),
    }
}

/// A model in colours in a format that keeps them (3MF, CSG).
fn encode_parts(f: Format, name: &str, parts: &Parts<Shape>) -> Result<Vec<u8>, String> {
    let all = parts
        .plain
        .iter()
        .chain(parts.painted.iter().map(|(_, s)| s));
    if f == Format::Csg {
        let piece = |s: &Shape| match s {
            Shape::Solid(s, _) => Some(Piece::Solid(s.clone())),
            Shape::Flat(l) => Some(Piece::Flat(l.clone())),
            Shape::Empty => None,
        };
        return Ok(write::csg(&Parts {
            plain: parts.plain.as_ref().and_then(piece),
            painted: parts
                .painted
                .iter()
                .filter_map(|(c, s)| Some((*c, piece(s)?)))
                .collect(),
        }));
    }
    if all.clone().any(|s| matches!(s, Shape::Flat(_))) && !all.clone().any(is_solid) {
        return Err(NOT_3D.into());
    }
    let solid = |s: &Shape| match s {
        Shape::Solid(s, f) => Some((s.clone(), f.clone())),
        _ => None,
    };
    let solids = Parts {
        plain: parts.plain.as_ref().and_then(solid),
        painted: parts
            .painted
            .iter()
            .filter_map(|(c, s)| Some((*c, solid(s)?)))
            .collect(),
    };
    if solids.plain.is_none() && solids.painted.is_empty() {
        return Err(EMPTY.into());
    }
    Ok(write::three_mf(name, &solids))
}

fn is_solid(s: &Shape) -> bool {
    matches!(s, Shape::Solid(..))
}

/// Writes a file, or stdout for `-`.
fn put(out: &str, bytes: &[u8]) -> Result<(), String> {
    if out == "-" {
        let mut so = std::io::stdout().lock();
        return so
            .write_all(bytes)
            .and_then(|()| so.flush())
            .map_err(|e| format!("stdout: {e}"));
    }
    std::fs::write(out, bytes).map_err(|e| format!("Can't write {out}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn options_as_openscad_takes_them() {
        let o = options(args(&[
            "-o",
            "a.stl",
            "-D",
            "w=5",
            "--export-format=binstl",
            "--backend",
            "manifold",
            "--render",
            "x.scad",
        ]))
        .unwrap()
        .unwrap();
        assert_eq!(o.outputs, ["a.stl"]);
        assert_eq!(o.defines, ["w=5"]);
        assert!(!o.cache_stats);
        assert_eq!(o.format, Some(Format::BinStl));
        assert_eq!(o.input, Some(PathBuf::from("x.scad")));
        assert!(options(args(&["x.scad"])).is_err());
        assert!(options(args(&["-o", "a.stl"])).is_err());
        assert!(options(args(&["--nonsense", "x.scad"])).is_err());
    }

    #[test]
    fn formats_by_extension() {
        assert_eq!(format_of("a.STL", None), Ok(Format::AsciiStl));
        assert_eq!(format_of("a.3mf", None), Ok(Format::ThreeMf));
        assert_eq!(format_of("a.csg", None), Ok(Format::Csg));
        assert!(format_of("a.xyz", None).is_err());
        assert!(format_of("-", None).is_err());
        assert_eq!(format_of("-", Some(Format::Off)), Ok(Format::Off));
    }
}
