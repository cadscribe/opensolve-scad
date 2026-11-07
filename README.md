# opensolve-scad

OpenSCAD files read and built into solids in Rust, without OpenSCAD: a
parser and interpreter of OpenSCAD's language, its shapes cut into as many
pieces as OpenSCAD cuts them, the mesh booleans by
[Manifold](https://github.com/elalish/manifold) (the library OpenSCAD
uses), 2D shapes by Clipper2, and [BOSL2](https://github.com/BelfrySCAD/BOSL2)
built in. With a command line that takes `openscad`'s options and a
language server for editors.

It is the OpenSCAD part of [OpenSolve](https://github.com/cadscribe/opensolve),
which opens `.scad` files as parts, and lives here so that other programs
can use it.

## Result

On 216 OpenSCAD files of one user (gridfinity bins, cases, brackets, logos,
BOSL2 models), one process each: OpenSCAD 2026.10.11 73.6 s in all, this
33.1 s. Against OpenSCAD's results 208 identical, 1 close, 1 differs, none
fails. Details in [ADR 0019](docs/adr/0019-manifold-and-clipper.md).

## Crates

| Crate                | Purpose                                                        |
|----------------------|----------------------------------------------------------------|
| `opensolve-scad`     | the interpreter: a file to a solid or a 2D shape (ADR 0011, 0014) |
| `opensolve-scad-cli` | the command line `opensolve-scad`                               |
| `opensolve-scad-lsp` | the language server `opensolve-scad-lsp` (ADR 0012)             |
| `opensolve-kernel`   | closed meshes with named faces, booleans, STL, 3MF, STEP (ADR 0003) |
| `opensolve-manifold` | Manifold and oneTBB, built from source, for the booleans (ADR 0019) |
| `opensolve-glyphs`   | the letters of a text as outlines of a TrueType or OpenType font |

`opensolve-scad`, `opensolve-kernel` and `opensolve-glyphs` build for
`wasm32-unknown-unknown` too; there the booleans are
[boolmesh](https://crates.io/crates/boolmesh), a Rust port of Manifold.

## Command line

```sh
cargo install --git https://github.com/cadscribe/opensolve-scad --tag v0.1.4 opensolve-scad-cli

opensolve-scad -o part.stl part.scad
opensolve-scad -o part.3mf -D 'width=40' -D 'label="A"' part.scad
opensolve-scad --export-format binstl -o - part.scad > part.stl
```

It takes the options of `openscad` that scripts use: `-o` (more than one
writes each), `-D name=value` (or any statement, such as a module that
replaces `color`), `--export-format`, `-q`, `--hardwarnings`, `-d` (make
dependencies) and `--preview`; `--render`, `--backend` and `--enable` are
taken and ignored. `--cache-stats` writes to stderr what the cache of
results did (see below; the runs of one call, the whole model and each
colour, share one). Formats: `stl` (ASCII, as OpenSCAD writes it), `binstl`,
`off`, `obj`, `3mf`, `step`, `csg`, and for 2D shapes `svg` and `dxf`;
`echo` writes the echoes.

`ECHO:` and `WARNING:` lines go to stderr as OpenSCAD writes them. The
exit code is 0 when the files are written and 1 when not; an empty model,
or a 2D one where a solid is wanted, fails with OpenSCAD's words
(`Current top level object is empty.`, `… is not a 3D object.`).

3MF and CSG keep colours: the parts of each colour of the outermost
`color()` are made one colour at a time, as colorscad makes them with
OpenSCAD (the booleans keep no colours), and written as one part each, in
3MF with its colour as a base material. What cuts in a `difference` is
made whatever its colour. A CSG file holds each part as a `polyhedron` in
its `color()`, which this program and OpenSCAD read again: GoSTL's colour
passes (`-o model.csg`, then `-D 'module color(c, alpha) {…}'` on the CSG)
work as with OpenSCAD.

## Language server

```sh
cargo install --git https://github.com/cadscribe/opensolve-scad --tag v0.1.4 opensolve-scad-lsp
```

`opensolve-scad-lsp` speaks the Language Server Protocol on stdin and
stdout (`--stdio` is accepted): diagnostics from the syntax and from
running the file (errors, warnings, echoes, with their places), completion
of OpenSCAD's names, the file's and those of what it includes and uses
(BOSL2 too), signature help, hover, definitions and document symbols. An
open file keeps the results of its evaluation for the next (up to 128 MB
each), so that an edit evaluates again only what it reaches. In
an editor, set it as the server for
OpenSCAD files, e.g. in Helix:

```toml
[language-server.opensolve-scad-lsp]
command = "opensolve-scad-lsp"

[[language]]
name = "openscad"
language-servers = ["opensolve-scad-lsp"]
```

## Use as a library

```toml
[dependencies]
opensolve-scad = { git = "https://github.com/cadscribe/opensolve-scad", tag = "v0.1.4" }
```

```rust
let read = |p: &std::path::Path| std::fs::read(p).map_err(|e| e.to_string());
let out = opensolve_scad::build(std::path::Path::new("part.scad"), &read, &[])?;
std::fs::write("part.stl", out.solid.to_stl()).map_err(|e| e.to_string())?;
```

`build` makes a solid; `make` runs a file as the command line does (`-D`
statements, colours, 2D shapes); `diagnose` tells what an editor marks;
`parameters` reads the Customizer's.

An evaluation on another thread can be stopped: give `build_with_stop`,
`diagnose_with_stop` or `Run { stop, .. }` a `Stop` and call `stop()` on a
clone of it. The run ends soon after (between statements, calls, steps of a
loop and the booleans of a batch; a single boolean runs to its end) with
the error `CANCELLED` (`Error::is_cancelled` for `make` and `diagnose`),
and keeps nothing of what it made (but what a cache was given of it).

```rust
let stop = opensolve_scad::Stop::new();
let theirs = stop.clone();
let run = std::thread::spawn(move || {
    let read = |p: &std::path::Path| std::fs::read(p).map_err(|e| e.to_string());
    opensolve_scad::build_with_stop(std::path::Path::new("part.scad"), &read, &[], &theirs)
});
stop.stop();
// Unless it was done before: Err(CANCELLED).
let out = run.join().unwrap();
```

A file that is built again and again while it is open (a parameter
changed, an edit) keeps the results of one run for the next in a `Cache`
that its owner gives every run: `build_with_cache`, `diagnose_with_cache`
or `Run { cache: Some(cache), .. }`. What each module call made is kept
by a hash of what it depends on: the module's code and, by name, that of
the functions and modules it calls, the values of the variables they read
where they are defined, the arguments, the `$` variables the call sees
(`$fn`, `$fa`, `$fs`, …) and its children; each union, difference,
intersection, hull, Minkowski sum, extrusion and import by its name,
arguments and the shapes that go in. A run then makes again only what a
changed value reaches, and makes the same as without the cache. The cache
holds at most its capacity in bytes (512 MB unless told), the results used
longest ago dropped first; a file included, used or imported that changed
drops the results that depend on it (`Cache::invalidate` drops them at
once). Clones of a cache are the same cache, and runs on several threads
may use it at once.

```rust
let cache = opensolve_scad::Cache::new();
let read = |p: &std::path::Path| std::fs::read(p).map_err(|e| e.to_string());
let stop = opensolve_scad::Stop::new();
let path = std::path::Path::new("part.scad");
let a = opensolve_scad::build_with_cache(path, &read, &[], &stop, &cache)?;
// Only what `width` reaches is made again.
let width = [("width".to_string(), "42".to_string())];
let b = opensolve_scad::build_with_cache(path, &read, &width, &stop, &cache)?;
println!("{}", cache.stats());
```

Set `OPENSOLVE_SCAD_SHARED_CACHE` to have every run without a cache of its
own use one cache of the whole process, keeping every result however
quickly made: the test suite run so (`OPENSOLVE_SCAD_SHARED_CACHE=1 cargo
test`) checks that results of one file used for another change nothing.

## Building

Rust 1.95 (`rust-toolchain.toml`) and a C++17 compiler (Xcode's command
line tools, GCC or Clang, MSVC): `opensolve-manifold` compiles Manifold
3.5.2 and oneTBB 2023.1.0 from their sources. The first build downloads
their release tarballs from GitHub (about 4 MB unpacked), checks them against the
SHA-256 pinned in `crates/opensolve-manifold/build.rs` and keeps them in
`target/`; set `MANIFOLD_SRC` and `TBB_SRC` to checkouts of your own to
build offline. No CMake is needed.

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo build --locked --target wasm32-unknown-unknown -p opensolve-scad
```

macOS is what it is developed on; CI builds on macOS and Linux.

## Decisions

[docs/adr](docs/adr) holds the decisions this code was built by, from
OpenSolve, with their numbers there: 0003 the mesh kernel, 0011 OpenSCAD
files as parts, 0012 the editor and the language server, 0014 BOSL2 and
OpenSCAD's libraries, 0019 Manifold and Clipper2. What they say about
OpenSolve's app is kept for the context.

## Licence

The code is licensed under either of [MIT](LICENSE-MIT) or
[Apache-2.0](LICENSE-APACHE), at your option.

What comes with it or is built into it is under its own licence; see
[THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md): BOSL2 (BSD-2-Clause),
Liberation Sans (SIL Open Font License 1.1), Manifold and oneTBB
(Apache-2.0), boolmesh (MPL-2.0), clipper2-rust (BSL-1.0).
