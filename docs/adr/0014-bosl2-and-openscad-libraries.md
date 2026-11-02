# ADR 0014 — BOSL2 and OpenSCAD's libraries

- Status: experimental (branch `bosl2`)
- Date: 2026-11-02

## Context

OpenSCAD files are linked as parts and built by OpenSolve's own interpreter
(ADR 0011), and edited in the app (ADR 0012). Many OpenSCAD files start with
`include <BOSL2/std.scad>`: BOSL2 (github.com/BelfrySCAD/BOSL2,
BSD-2-Clause) is OpenSCAD's large standard library, rounded and chamfered
shapes (`cuboid`, `cyl`), attachments (`attach`, `position`, `diff`, `tag`),
paths and regions, `offset_sweep`, `path_sweep`, `skin`, gears, threads,
screws, VNFs. The user wants BOSL2 to work, with all that belongs to it:
finding it, the language it is written in, the geometry it needs, speed,
the editor, the assistant, the documentation.

## Getting BOSL2

| Way | For | Against |
|---|---|---|
| A pinned release in the source, built into the program | works out of the box and offline; the tests use the same copy, no network in `make check`; one version that the interpreter is tested against | 4.6 MB of `.scad` in the repository (1.1 MB compressed) and in the binary |
| Fetched on first use, pinned by tag and SHA-256 (as `opensolve-occt` fetches OCCT) | a smaller binary | needs the network when a part is first built; the app has no `tar`/`gzip` (new dependencies); the tests need a copy anyway |

Decision: **bundled**. BOSL2 v2.0.774 (tag of 2026-10-18; the tarball from
GitHub has the SHA-256
`0250251c0653fbe723e78a2cfd5e8d0b7eff0eb66a1bfdfae4fce21cf9a22ea1`): its
`.scad` files and `LICENSE` in `crates/opensolve-scad/library/BOSL2`, its own
test suite in `crates/opensolve-scad/tests/bosl2` (0.9 MB). `build.rs` lists
the files for `include_str!` (not on wasm32). BSD-2 asks that a binary
reproduce the copyright notice: the `LICENSE` is written out with the files,
the README names it, and `make macos-app` puts it into the app's resources.
A newer release: replace the folder, `BUNDLED_VERSION` in `library.rs`, run
the suite.

## Finding libraries (`opensolve_scad::library`)

- An `include <…>` or `use <…>` is looked for as OpenSCAD looks: beside the
  file that names it, then in the folders of `OPENSCADPATH`, OpenSCAD's
  user library folder (`~/Documents/OpenSCAD/libraries` on macOS and
  Windows, `~/.local/share/OpenSCAD/libraries` on Linux), then OpenSolve's
  own: `libraries` in its configuration folder (`OPENSOLVE_CONFIG_DIR`,
  `--config-dir`). The interpreter, the language server and the editor
  resolve the same way.
- The bundled BOSL2 goes into OpenSolve's folder when no other folder has a
  BOSL2: at the app's start, and on the first include of `BOSL2/…`
  anywhere (the CLI and the MCP server too). A file `.opensolve-bundled`
  holds its version; a folder without it is the user's and never written,
  an older bundled copy is replaced. It is written to a folder beside and
  then renamed. If it cannot be written, its files are read from memory.
  A user's own BOSL2 (as this machine's 2.0.716) wins, as in OpenSCAD.
- Library files are read from the disk directly, not through the reader
  the caller gives: the MCP server confines files to its root, but the
  library folders are the user's and their files are only interpreted.
- Settings, "OpenSCAD-Bibliotheken": which BOSL2 is used (bundled with
  its version, or the user's with its folder), its folder shown, the
  bundled copy set up again; a line says where includes are looked for.

## The language

BOSL2's own suite (977 tests in `tests/bosl2/*.scadtest`, assert-based;
`expect_success`, `assert_no_echoes`, `assert_no_warnings` as its runner
reads them) runs in `make check` (`tests/bosl2.rs`, 3 s): **all 977 pass**.
What it took, each checked against OpenSCAD 2026.10 where in doubt:

- Functions have names of their own: `len(s)` calls `len` though a
  variable `len` exists; a variable holding a function literal is called
  only where no function has the name. The last definition of a name
  wins.
- `search` with all of OpenSCAD's cases; vectors ordered element by
  element (`[1, 2] < [1, 4]`); a range's begin, step and end by index;
  `each` of a string; NaN in ranges compared as OpenSCAD does.
- The next values of a C-style `for` are assigned one after the other,
  each seeing those before.
- `str` of a function literal prints it as OpenSCAD does (BOSL2's tests
  compare them); a failed assertion without a message says its condition.
- `parent_module(n)` and `$parent_modules`; `$children` is the module's
  where it is written, other `$` names come from who called; `$vpr`,
  `$vpt`, `$vpd`, `$vpf` have OpenSCAD's start values.
- `rands` draws from C++'s Mersenne Twister as OpenSCAD does, seeded as
  OpenSCAD seeds it (Python's hash of the number, to 31 bits): the same
  numbers for a seed; without one it goes on. Built-in functions take
  their arguments in order whatever they are named (`seed = …`).
- The trigonometry of OpenSCAD's `degree_trig`: exact at multiples of 30°
  and 45°, the inverse functions exact where a whole number of degrees is
  the answer (`acos(-0.5)` is 120, which decides how many pieces an arc
  gets), rotation about an axis as OpenSCAD computes it.
- `version()` stays 2021.01, the language this interpreter follows; BOSL2
  asks for 2021.01 and takes its older ways for what came later
  (`textmetrics`, `object`, `em` of `text`).
- A call in a function's tail runs in a loop, as in OpenSCAD: a loop by
  recursion may take a million steps. Other calls may go 100,000 deep, or
  until the stack (256 MB) is used but 48 MB, which ends the evaluation
  with an error, not the program.
- `difference()` whose first child makes nothing makes nothing, as in
  OpenSCAD (BOSL2's `diff()` with `keep`), an `if` that runs nothing is no
  child.

## The geometry

- `minkowski`: of convex shapes the hull of the sums of their corners; of
  one that is not convex, the union of such hulls of each of its triangles
  with the other (or the other's triangles), with the insides: a boolean
  for each, fine for BOSL2's uses, slow for big meshes, refused beyond
  20,000 hulls. In 2D the same on the triangles of the shapes' tops.
- `offset(r)`, `offset(delta)`, `offset(delta, chamfer = true)`: each
  outline moved and its corners joined round (as many pieces as a circle
  of that radius), mitred, or squared off at `delta` from the corner (as
  Clipper's square join, which OpenSCAD uses); where the moved outlines
  cross, the shape with strips along its edges and pieces at its corners,
  overlapping a little. A negative offset is the positive one of what lies
  around the shape.
- `projection()`: the shadow (each patch of faces that look up, a patch
  that folds over itself triangle by triangle) and `cut = true`, the
  section at z = 0.
- `polygon` fills paths that cross themselves or each other by the
  even-odd rule, as OpenSCAD: the paths cut where they cross, the border
  chained into loops and triangulated with the holes joined in, no
  booleans; parts that touch at a point are moved apart by 10 nm, so the
  mesh stays closed (a pentagram's tips, a ring gear's undercut).
- `text()`: the glyph outlines the sketches use, a font of the machine by
  its file's name, else Bitstream Vera Sans standing in for Liberation
  Sans with capitals as high; the em is `size / 0.72` as in OpenSCAD.
  About as large as OpenSCAD's letters, not the same shapes.
- The hull adds its points farthest first and removes only the faces a
  point sees that hang together, so points close to its faces (sums with
  a tiny sphere) no longer break it.
- A union the mesh library cannot make (BOSL2's open surfaces, strokes of
  debug drawings, some meshes where boolmesh 0.2.1 panics) puts both shapes
  together as they are, with a note; OpenSCAD shows them so too.

BOSL2's documentation has 2,731 examples. Built here and by OpenSCAD
2026.10 (`--backend=manifold`) and compared by volume: of the 2,344 that
OpenSCAD makes a solid of, 2,096 come out the same (within 0.1 %) and
128 within 2 % (text in another font, offsets of curves); 100 differ,
mostly open surfaces and the strokes of debug drawings, whose overlaps a
union here cannot join (see below), and 20 fail here: boolmesh panics on
cuts of `edge_profile` masks and jigsaw `partition`s (11), examples whose
2D and 3D parts are mixed (9). A rounded block with a hole, a gear,
a threaded rod, an attachment tree and an `offset_sweep` have OpenSCAD's
volume to the third decimal (tests in `tests/bosl2.rs`, `geometry.rs`).

## Speed

- Files are parsed once and kept by path and text (`cache.rs`, shared by
  all evaluations; the statements hold `Arc`); a changed file is parsed
  again. Including BOSL2 costs 90 ms the first time, then the sixty files
  are only read and compared.
- The names of a scope are found by an index (FxHash) once there are more
  than twelve: BOSL2's thousands of functions and constants.
- Built after the first time (release, M-series Mac): a rounded cuboid
  9 ms, a block with a hole by `diff()` 34 ms, an attachment tree 13 ms, a
  gear 41 ms, a threaded rod 26 ms, an `offset_sweep` 45 ms. OpenSCAD
  takes about 300 ms for each. The large-`$fn` cap of ADR 0011 stays.

## The editor and the language server

- Includes resolve through the library folders: no unknown names in a file
  that includes BOSL2, go to definition opens BOSL2's file at the name, and
  `include <…` completes files of the library folders.
- A library's files are read and parsed once and again when their time on
  the disk changes; the first completion after including BOSL2 indexes it
  (0.3 s), then about 2 ms.
- Hover and completion show BOSL2's documentation blocks (`// Module:`,
  `// Function:`, `// Function&Module:`, `// Constant:`): synopsis,
  usages as code, description, arguments as a list, as Markdown, which the
  editor draws as the chat draws Markdown. A library's inner names (`_…`)
  are offered only when an underscore is typed.
- A file of a library folder opens to read only and is not evaluated on
  its own.

## The assistant

The MCP server and the chat have no tool that writes `.scad` files; the
description of `set_link_parameters` says that linked OpenSCAD files may
use BOSL2 and how it is included.

## Not supported, and differences

- `import`, `surface`, `roof`, `fill`; `text`'s `direction`, `language`,
  `script`; OpenSCAD 2025+'s `object()`, `textmetrics()`, `fontmetrics()`.
- `random_polygon` in BOSL2 gives other points than OpenSCAD 2026 for some
  seeds (`rands` itself is the same).
- boolmesh 0.2.1 panics on some unions (BOSL2's `cubetruss`, `joiners`):
  they become shapes put together with a note. boolmesh 0.2.2 (2026-10-27)
  may fix it; it may be taken once it is a week old.

## Consequences

- `opensolve-scad` has a build script, carries 4.6 MB of BOSL2 in native
  builds and depends on `opensolve-sketch` for glyph outlines; it is built
  optimised in debug builds too, BOSL2's suite runs in the tests.
- A user's own BOSL2 is used where OpenSCAD would use it, even an older
  one; the settings show which.
- Every build reads the included files from the disk and compares their
  texts with what was parsed (a few ms for BOSL2).
