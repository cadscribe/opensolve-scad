# ADR 0019 — Manifold for the mesh booleans, Clipper2 for 2D

- Status: accepted (on `main` since 2026-11-03)
- Date: 2026-11-03

## Result

On the user's 216 timed OpenSCAD files, against OpenSCAD 2026.10.11: all
210 that both build come out identical (symmetric difference under 0.1 %
of the volume; before scad-parity 208, 1 close, 1 differs), none fails.
Timed with other builds running on the machine (load 19 to 38), the
fastest of five runs: OpenSCAD 100.6 s in all, OpenSolve before
scad-parity 44.5 s, now 40.7 s; 215 files faster than OpenSCAD, one by
the load's noise slower (`connector-server-room-male`, 0.42 s against
0.34 s; 0.17 s against 0.24 s run again). `gridfinity-rebuilt-lite` and
`logo_full`, slower before, are now as fast as OpenSCAD. On a quiet
machine before scad-parity: OpenSCAD 73.6 s, OpenSolve 33.1 s (before
this ADR 273.2 s). Details under Corpus below; the numbers in Context
are those before.

## Context

ADR 0003 chose `boolmesh` 0.2.1, a Rust port of Manifold, for the mesh
booleans, and kept 2D shapes of OpenSCAD files as slabs one unit thick that
went through those booleans (ADR 0011). Against OpenSCAD 2026.10.11 (its
default backend, Manifold in C++) on the user's 235 own OpenSCAD files, 210
timed, a cold process each, before this ADR: OpenSCAD 69.8 s in all, here 261.8 s; 146 files
faster here, 64 slower, and the slow ones the large models
(`render_baskets` 104 s against 26.6 s, the logos 68.7 s against 1.8 s and
24 s against 0.56 s, `tamper-station` 13.9 s against 6.9 s). The time of a
file also varied from run to run (3 s or 16 s), and so did its triangles.

Sampled (macOS `sample`, the evaluation thread, shares of its time) the 15
files slowest against OpenSCAD spent 63 % to 99 % in the booleans, almost
all inside `boolmesh` (its edge and winding kernels), the conversions
around it under 10 %, parsing and evaluation under 2 %:

| file | OpenSCAD | here | union | difference | other geometry | in the booleans |
|---|---:|---:|---:|---:|---:|---:|
| render_baskets | 26.6 s | 103.6 s | 73 % | 23 % | 3 % (extrude) | 93 % |
| logo_full (almplus) | 1.8 s | 68.7 s | 88 % | 9 % | 3 % (import) | 97 % |
| logo_full_chip | 0.56 s | 24.4 s | 83 % | 14 % | 3 % (import) | 98 % |
| render_modules (2D) | 2.0 s | 10.5 s | 76 % | 17 % | 6 % (extrude, text) | 90 % |
| tamper-station | 6.9 s | 13.9 s | 31 % | 66 % | 2 % | 63 % |
| voronoi_test (2D) | 1.1 s | 6.1 s | 0 % | 95 % | 3 % | 95 % |
| circular_woven_basket | 0.50 s | 4.8 s | 94 % | 4 % | 2 % | 95 % |
| gridfinity-rebuilt-lite | 0.47 s | 4.2 s | 75 % | 23 % | 1 % | 96 % |
| rectangular_woven_basket | 0.71 s | 4.0 s | 94 % | 4 % | 2 % | 94 % |
| connector-smoke-ventilation-pipes | 0.32 s | 2.0 s | 28 % | 72 % | 0 % | 98 % |
| abdeckung | 0.17 s | 1.8 s | 10 % | 89 % | 0 % | 100 % |
| gridfinity-rebuilt-baseplate | 0.23 s | 1.7 s | 65 % | 32 % | 2 % (offset) | 92 % |
| Station | 0.52 s | 1.8 s | 4 % | 63 % | 33 % (minkowski) | 90 % |
| connector | 0.23 s | 1.4 s | 41 % | 59 % | 0 % | 97 % |
| connector-heating-room-ventilation-female | 0.24 s | 1.2 s | 38 % | 62 % | 0 % | 96 % |

Where the time went, besides `boolmesh` being slower than Manifold on the
same work: caps of circles as a fan from one corner (long slivers whose
boxes reach over much of the disc, so a boolean tests every one against
everything there), transformed solids with a facet per triangle (nothing
coplanar joined up again), unions in rounds of pairs re-reading each
result, and 2D booleans and offsets done in 3D.

## Options

- **(a) Use `boolmesh` better** (batches, results kept in its form, no
  re-welding, its `rayon` feature): the caps and the batching are done
  below for any engine; the rest leaves `boolmesh`'s own kernels, which
  are the 63–99 %.
- **(b) `boolmesh` 0.2.2** (2026-10-27): not measured; its change (an
  error instead of a panic on edges of more than two faces) does not touch
  the kernels where the time goes.
- **(c) Manifold itself.** Crates exist (`manifold3d` / `manifold-csg-sys`
  3.5.105, `manifold-rs` 0.7.0), but they clone the sources with git at
  build time by tag, run CMake and fetch TBB with it; none pins what it
  builds by a hash. A bridge of our own, as `opensolve-occt` has, is small.

Measured on `logo_full_chip`: main (`boolmesh`) 17 s; Manifold on one
thread 11 s, with caps cut as Manifold cuts them 1.8 s, on TBB 1.1 s, with
the rest of this branch 0.49 s (OpenSCAD 0.55 s). On `tamper-station`:
main 14 s; Manifold on one thread, batched, 6.0 s; on TBB 2.6 s (OpenSCAD
6.9 s). (a) leaves `boolmesh`'s own kernels as they are, which were 63–99
% of the time; it cannot reach OpenSCAD on the large models.

## Decision

- **`opensolve-manifold`** builds Manifold 3.5.2 (Apache-2.0; the version
  OpenSCAD 2026.10 links) and oneTBB 2023.1.0 (Apache-2.0) from their
  release sources with the `cc` crate: tarballs from GitHub, checked
  against a SHA-256 pinned in `build.rs`, only `src/`, `include/` and the
  licence unpacked into `target/`, nothing of them run; `MANIFOLD_SRC` and
  `TBB_SRC` point at checkouts of one's own instead. A cxx bridge exposes
  a solid from a mesh whose triangles carry a number each, its booleans of
  two, a batch, its mesh back, and Manifold's triangulation of polygons.
  Manifold without its 2D part (Clipper2), TBB static without its
  allocator, as their CMake builds them.
- **The kernel** keeps its API. Natively (`cfg(not(target_arch =
  "wasm32"))`) it uses Manifold; in the browser `boolmesh` as before. Each
  triangle's facet goes through Manifold as its face number, which its
  pieces keep, so no blended tag has to be repaired. `Solid::batch` joins
  many solids as OpenSCAD does: those that meet (`apart::meeting`, now on
  threads) in rounds of pairs, the small ones first, the pairs of a round
  on threads; those apart put together without a boolean; a difference
  cuts the union of its cutters at once. `Solid::mapped` moves a solid
  with one facet per plane, and `triangulate_loops` cuts polygons as
  Manifold does.
- **Long booleans can be stopped** (2026-11-03, ADR 0011): the
  `Stop` of the evaluation's thread (`opensolve_kernel::stopping`) is
  looked at between the rounds of pairs of `Solid::batch` and of
  `Solid::booleans`, by each pair before it starts on its thread, between
  the cutters' union and the cut, and before a result is read back from
  Manifold; then `Error::Stopped`. One pair inside Manifold runs to its
  end: Manifold 3.5.2's `ExecutionContext` too only checks between the
  booleans of a tree, which the kernel already goes through itself.
- **2D shapes are polygons** (`flat.rs`): their booleans, offsets and
  Minkowski sums by `clipper2-rust` 1.2.0 (BSL-1.0, a pure Rust port of
  Clipper2, which builds for wasm), on integers scaled and simplified as
  OpenSCAD's `ClipperUtils` does it: a primitive filled by the even-odd
  rule, booleans by the non-zero rule, `offset` with OpenSCAD's arc
  tolerance and miter limit. A shape becomes a mesh only when extruded,
  its caps triangulated by Manifold (in the browser by ear clipping as
  before).
- **The interpreter** keeps the result of a batch by what went into it (a
  hash of the meshes), as OpenSCAD's geometry cache does: the cells of a
  grid are joined once.
- **Corners stay on the kernel's grid** of 0.1 µm, also OpenSCAD's
  (`from_indexed`, `mapped` as `from_triangles`). Without it the triangles
  came out as OpenSCAD's (a slanted quad stays one plane for Manifold,
  which otherwise cuts each of its triangles on its own: 5740 triangles
  against 2876 for a chamfered ring, and the files with such faces 10 to
  40 % slower), but a doll's house lost 1.4 % of its volume: its roof,
  turned, meets a wall cut to the roof's slope by script, the two a few
  last bits apart, and Manifold's union of such a near contact is not
  sound. Snapped at 1e-9, 1e-10 or 1e-11, or only after turns that are not
  quarters, it failed on the same house or its sister; at 0.1 µm, as main
  has it, both are right.
- **A boolean's result is moved by Manifold** (scad-parity), as OpenSCAD
  has it moved: `mapped` of a solid that has Manifold's mesh calls
  Manifold's `Transform` (composed with those before, applied to the
  corners once), and only primitives still go to the grid when moved. The
  house above was not about the grid: its roof, a union, was moved by
  `translate([0, 0, 0])`, and the mesh of a union read into Manifold again
  (even Manifold's own mesh, unchanged) lost the roof in the next union;
  the grid had only hidden that. The facets follow by Manifold's runs: each
  run says which transform took its triangles where they are.
- **Letters are cut as OpenSCAD cuts them**: every curve of a glyph into an
  eighth of the pieces of a circle of the text's size, plus one (it was a
  chord tolerance, ten times the points); the text files of the corpus come
  out identical now, with OpenSCAD's very triangles.

## Consequences

- The kernel's booleans need a C++17 compiler (as `opensolve-occt` does),
  about 20 s of build once, and a first build downloads Manifold's release
  (31 MB, of which 0.7 MB are kept) and oneTBB's sources; the wasm crates
  need neither. Built and tested on macOS arm64; the flags for Linux and
  Windows (MSVC and MinGW) follow the two projects' CMake files but have
  not been tried.
- A file builds the same each time: the even-odd edges are sorted, and the
  booleans no longer depend on thread timing. (Not quite: Station.scad of
  the tamper station, a Minkowski sum, writes its triangles in another
  order from run to run, the same volume and count; so did main.)
- Manifold's results are triangulated differently from `boolmesh`'s; the
  Raspberry case builds to the same volume (24 623.05 mm³) in 0.11 s
  instead of 0.19 s; the blend tests pass. With scad-parity it builds to
  the very same STL, in the same time.
- Manifold's tolerance stays its own (from the size of the box): set to
  the kernel's grid (0.1 µm) it moved a large model (a doll's house of a
  metre) by 2 % of its volume. Splinters that a boolean leaves between a
  corner on the grid and one computed next to it go with the facet the
  rest of theirs goes with.

## Corpus

The same 216 files, one process at a time, OpenSCAD, OpenSolve before
scad-parity and with it back to back per file, the fastest of five runs
under 5 s (`run_parity.py`, `report_parity.html` in the corpus folder).
Other builds ran on the machine the whole time (load 19 to 38): OpenSCAD
100.6 s in all, before 44.5 s, now 40.7 s; 215 faster than OpenSCAD, one
slower by noise (above). Against OpenSCAD's results 210 identical (208
before), none close (1), none differs (1), none fails.

What scad-parity changed:

- `RuggedBoxV1` differed (3 % by sampling) for real: `rotate_extrude` of
  a profile left of the axis turned its mirror image, where OpenSCAD 2026
  turns the profile as it lies (x · sin a, x · cos a), so a part turn
  lands on the other side of the axis. The four corners of the box stood
  in the wrong quadrant. Now identical.
- `batteries` (close, 0.16 %): letters a micrometre off. OpenSCAD asks
  FreeType for an em of 10⁵ 26.6 units at 100 dpi; for a TrueType font
  whose head asks for whole pixels, FreeType makes that 2170 pixels, not
  2170.14, and puts every point on its 26.6 grid; HarfBuzz's advances keep
  the unrounded em; valign goes by the ink's box fitted to whole pixels.
  `text()` now does all that (`opensolve_glyphs::openscad`): 0.0004 %.
- `gridfinity-rebuilt-lite` (0.55 s against OpenSCAD's 0.46 s before):
  97 % of its time was Manifold's booleans, on 132 000 triangles against
  OpenSCAD's 82 000. The results of booleans are now moved by Manifold
  (see Decision): 103 000 triangles, the CPU time 2.61 s now 1.98 s
  (OpenSCAD 1.85 s), the wall time 0.62 s now 0.52 s (OpenSCAD 0.52 s),
  measured back to back.
- `logo_full` and `logo_full_chip`: since the colours of the extracted
  crate, `logo()`, cut from the base and then shown in a colour, was made
  twice (the SVG read, extruded and cut again): a module's result is now
  kept across a colour when all colours are made. What is left is 97 %
  Manifold: the union of the base with the logo, whose walls meet the
  base's along the whole outline (81 %), and the cut of the logo from the
  base (15 %), the same two booleans OpenSCAD makes, in the same CPU time
  (9.6 to 9.8 s): 1.99 s against OpenSCAD's 1.93 s, back to back.
- `Station` (tamper station): the hulls of a Minkowski sum of a solid that
  is not convex on threads (29 % of its time, one after the other).
- The comparison itself: OpenSCAD writes parts that touch along an edge
  as an STL that is not closed, and Manifold's booleans of such a mesh
  came out as anything (3.4 % apart for two identical meshes of the doll's
  house); `scad_corpus compare` samples such pairs now.
- `Solid::feature_edges` (the edges OpenSolve's view draws) by buckets of
  corners instead of a map: 460 ms for render_baskets' 2.4 million
  triangles, now 130 ms.

| file | OpenSCAD | before | now |
|---|---:|---:|---:|
| render_baskets | 47.10 s | 19.61 s | 17.04 s |
| tamper-station | 8.36 s | 2.74 s | 2.68 s |
| logo_full | 1.93 s | 1.96 s | 1.99 s |
| gridfinity-rebuilt-lite | 0.52 s | 0.62 s | 0.52 s |
| logo_full_chip | 0.58 s | 0.53 s | 0.53 s |
| Station | 0.51 s | 0.51 s | 0.47 s |
| circular_woven_basket | 0.52 s | 0.50 s | 0.42 s |
| connector-smoke-ventilation-pipes | 0.49 s | 0.35 s | 0.30 s |
| RuggedBoxV1 | 0.38 s | 0.30 s | 0.28 s |

(The corpus run; `logo_full` and `gridfinity-rebuilt-lite` back to back,
nine runs each, as the corpus run had OpenSCAD's logo at 5 s.)

Open: `logo_full` and `gridfinity-rebuilt-lite` are as fast as OpenSCAD,
not faster. The logo's time is Manifold's two booleans, as OpenSCAD's.
The bin still has 103 000 triangles where OpenSCAD has 82 000: primitives
moved still go to the grid (moved by Manifold unsnapped they gave
OpenSCAD's count, but 3.7 mm³ less volume than OpenSCAD's and half the
triangles of the vehub chip, which its comparison could not cut). The
timing wants a quiet machine.
