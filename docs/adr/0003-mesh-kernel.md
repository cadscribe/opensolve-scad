# ADR 0003 — Mesh kernel for printing

- Status: accepted; natively the booleans are Manifold's since ADR 0019,
  `boolmesh` stays for the browser
- Date: 2026-10-31

## Context

Export failures in SolveSpace come from booleans on exact surfaces. The user's
output is mainly STL (46 against 9 STEP), and they chose printing first.

## Decision

Solids are manifold meshes, with booleans by `boolmesh` 0.2.1 (2026-10-11,
MPL-2.0, pure Rust, no `unsafe`, a port of the Manifold algorithm), which builds
for wasm. MPL applies to its files, not to ours. `manifold-rs` needs a C++ build,
and an own exact kernel is what fails in SolveSpace.

The candidate carries a value per triangle corner through booleans. Each face
gets a tag `(feature, sketch entity, side)`. An edge is then a pair of tags,
which gives chamfers and sketches on faces names that survive parameter changes.

## Evaluation

Run on a throwaway probe, `boolmesh` built with default features (`f64`, one
thread), after a static review of its source: no file, network, process or
environment access, no `unsafe`, no build script, `glam` as the only required
dependency. `glam` itself was not reviewed.

| Case | Result |
|---|---|
| coplanar through cut, flush and touching unions, identical union | closed, exact volume |
| hole tangent to a wall, 1e-9 near-coincidence, wedge on an edge | closed |
| 36 rotated slots, 24 rotated copies, 100 stacked flush blocks | closed |
| 300 random rotated cuts | closed, 0.7 s |
| 400 holes one by one / as one cutter | closed, relative volume error 1e-14; 35 s / 4.3 s |

Findings the kernel handles:

- Subtracting everything returns an error, not an empty mesh: an empty `Solid`.
- Corners sharing a vertex can blend the tags of two faces: 108 of 2472
  triangles after 300 cuts. Clean triangles were never wrong. `from_manifold`
  repairs blended ones by the plane a tag has to lie in, then from a neighbour.
- Sequential cuts get slow on large meshes: `Solid::merged` cuts a batch at once.

## Consequences

- Fillets are approximated by facets, and exact STEP needs the surfaces from the
  sketch, not the mesh.
- 4.3 s for 400 holes is slow when a parameter changes: features must cache their
  solids and rebuild only from what changed. `rayon` is available natively.
- Single maintainer: the version stays pinned, and MPL-2.0 allows us to carry it.
