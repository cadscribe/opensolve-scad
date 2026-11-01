# ADR 0011 — OpenSCAD files as parts, without OpenSCAD

- Status: experimental (branch `scad-parts`)
- Date: 2026-11-01

## Context

Many printable parts exist as OpenSCAD files, often parametric with the
Customizer's comments (`width = 40; // [10:100]`). The user wants to link them
as parts like `.slvs`, `.osolve` and `.stl` files, and wants no dependency on
OpenSCAD: no binary to call, no C++ library.

## Evaluation

Rust crates on crates.io for OpenSCAD's language (checked 2026-11-01):

| Crate | What | Licence | Verdict |
|---|---|---|---|
| `openscad-rs` 0.1.0 (2026-02-27) | parser only | GPL-3.0-or-later | no: licence |
| `luacad-scad-syntax`, `-eval`, `-ir` 1.2.0 (2026-09-23) | parser and evaluator, vendored from OpenRSCAD | MIT OR Apache-2.0 | no: one release, about 80 downloads, brings `fontdb`, `png`, `serde`, `logos`; its tessellation is its own |
| `tree-sitter-openscad`, `-ng` | grammar | MIT | no: C, a syntax tree only |
| `scad`, `scad_tree`, `solidrs`, `scadman`, `flowscad` | write OpenSCAD from Rust | — | no: the other way round |

None is both maintained and fit, and an evaluator that cuts circles as
OpenSCAD does is the part that matters for parts that fit.

## Decision

- A crate of its own, `opensolve-scad`, pure Rust, for wasm too: a hand-written
  lexer and recursive descent parser of OpenSCAD 2021.01's language, an
  interpreter with OpenSCAD's rules (the last assignment of a name wins at the
  place of the first; functions and modules known in their whole scope; `$`
  names passed from caller to callee; `children()`, `$children`; `include`
  spliced in, `use` bringing only modules and functions), and its shapes built
  on the mesh kernel's booleans (ADR 0003). OpenSCAD itself works on polygons,
  so the result is the same mesh, not an approximation of it.
- Circles, cylinders, spheres and extrusions are cut into as many pieces as
  OpenSCAD cuts them (`$fn`, else `$fa` and `$fs`, at least five), at the same
  places, with OpenSCAD's exact sines and cosines at multiples of 30°, the
  sphere's rings at the middle of even bands, a twisted extrusion's sides split
  as OpenSCAD splits them. A part thus fits what was printed from OpenSCAD.
  The one difference: at most 1,000 pieces, with a note, so that a huge `$fn`
  does not keep the app busy for minutes.
- A 2D shape is a slab one unit thick: its booleans are the kernel's, and its
  top is the shape an extrusion takes. No polygon clipper of its own.
- Not supported yet, and named in a note rather than left out silently:
  `minkowski`, `offset`, `projection`, `text`, `import`, `surface`, `roof`.
  `color` is kept as a group and its colour dropped: a link is one feature, and
  colours are per part. `rands` gives other numbers than OpenSCAD.
- A `.scad` file is linked like the others (`Feature::Link`): the file dialogs
  offer it, the solid is built when the model's links are read, and its
  problems are the link's, so the rest of the model still builds. A parse error
  says the line and column; `echo` output, unknown names and what is not
  supported are notes of the link.
- The main file's top-level assignments of a plain value are its parameters,
  as OpenSCAD's Customizer shows them: the comment above as description,
  `// [min:max]`, `// [min:step:max]` or `// [a, b:Label]` behind as range or
  choices, `/* [Group] */` as group, `[Hidden]` left out. A link stores the
  values it sets (`Feature::Link::overrides`, name and value as OpenSCAD writes
  it), written as `set` lines after the link in `.osolve`; they act as
  OpenSCAD's `-D` does. The linked solid is kept by file and values together.
- The app looks at the linked files once a second (and at what a `.scad` file
  includes and uses) and reads a link again when one has changed, for Undo
  and Redo too. A link's properties show its file's parameters by group: a
  switch for yes or no, a choice for a list, else a field (words without their
  quotes); a value as in the file is not stored. The MCP server lists a link's
  values and parameters in `get_feature` and sets them with
  `set_link_parameters`, also for the assistant in the app.
- Evaluation runs on a thread with a large stack: OpenSCAD files loop by
  recursion. Calls deeper than 3,000 end with an error.

## Consequences

- No exact surfaces for these parts yet: like an STL, a `.scad` link has no
  B-rep, so its part stays a mesh and its STEP is not exact. Cylinders,
  spheres and extrusions could later go to the B-rep for exact STEP: the
  evaluator would carry each primitive and its placement beside its mesh, and
  `opensolve-brep`'s extrude, revolve and booleans would build them. Not
  done in v1.
- Every linked file is now watched in the app, not only `.scad`: a `.slvs`,
  `.osolve` or `.stl` changed by another program is read again too.
- `Feature::Link` has a field more, and a linked solid carries its notes:
  `Linked` is `Arc<(Solid, Facets, Vec<String>)>`.
- BOSL2 and other large libraries need what v1 leaves out (`offset`,
  `minkowski`, `text`, `import`, `include` inside a module); nothing in the
  design blocks them.
