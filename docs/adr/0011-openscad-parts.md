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
  (2026-11-03, ADR 0019: 2D shapes are polygons now, their booleans,
  offsets and Minkowski sums by Clipper2 as in OpenSCAD; they become meshes
  only when extruded.)
- Not supported yet, and named in a note rather than left out silently:
  `import`, `surface`, `roof`, `fill`. `color` is kept as a group and its
  colour dropped: a link is one feature, and colours are per part.
  (2026-11-02, ADR 0014: `minkowski`, `offset`, `projection` and `text`
  are built now, `rands` gives OpenSCAD's numbers for a seed, and
  `include <…>` finds libraries as OpenSCAD does, BOSL2 coming with the
  app.)
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
- The model's own parameters reach the file (2026-11-02). A value a link sets
  may be an expression in the model's parameters (`wall`,
  `width - 2 * wall`, `ring_abstand / 2`): a value that names something
  other than OpenSCAD's `true`, `false`, `undef` and `PI`, and is no string
  or list. The model's evaluator (that of dimensions) gives it in mm, and the
  file gets the number; a text parameter named alone gives its words. Other
  values are OpenSCAD's, as before, except that a number with a unit or a
  sum the model can work out (`1cm + 5`) is given as its number. So an
  override that named one of the file's own variables (`depth = width / 2`)
  is now an unknown name: the Customizer only ever writes plain values.
- A link may pass the model's parameters (`Feature::Link::model_params`,
  "Parameter übergeben" in its properties): each number parameter and
  each text parameter, as far as it has a value, is a top-level variable of
  the file, as OpenSCAD's `-D` makes one, in the model's order, then the
  link's own values; the later of one name wins at the place of the first.
  The model's value thus wins over the file's own top-level assignment, the
  link's over the model's, and a name the file does not assign is still
  defined. It is on for a newly linked OpenSCAD file, off for links read
  from older files and for other files: a model's parameters are its
  user's names for its sizes, so a file that uses the same name means the
  same size; a clash shows, as the properties mark a file parameter given
  by the model and say so on hover, and the switch turns it off. A link
  that passes them is written with a line `model-params` after it; files
  without it read as before.
- The linked solid is kept by file and the values it is built with, the
  evaluated ones (`Model::link_values`, `Model::link_key_of`): a changed
  parameter of the model makes a new key, and the app, the MCP server and
  the CLI read what is not there yet after each change. Every change of a
  passed parameter builds the file again, used by it or not. A value that
  cannot be worked out (an unknown name, a parameter that depends on
  itself, words where a number goes) is the link's problem, naming the
  value and why; the rest of the model builds.
- The editor's language server is told the variables a file is given
  (`Workspace::set_external`): the app gives each open file the parameters
  of the links that pass them. They are not unknown when the file is
  checked, completion offers them as variables ("Parameter = 73"),
  and hover shows their value and expression, before the file's own
  top-level value of the name, which they replace. The stdio server has
  none.
- The MCP server's `set_link_parameters` takes such expressions and
  `model_parameters`; `get_feature` shows `model_parameters` and
  `built_with`, the values the file gets.
- Evaluation runs on a thread with a large stack: OpenSCAD files loop by
  recursion. Calls deeper than 3,000 end with an error. (ADR 0014: a call
  in a function's tail runs in a loop, up to a million steps; others may go
  100,000 deep or until the stack runs short.)

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
  design blocks them. Done for BOSL2 in ADR 0014, but `import`.
