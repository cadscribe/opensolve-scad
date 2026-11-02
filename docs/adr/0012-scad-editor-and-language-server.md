# ADR 0012 — An editor and a language server for OpenSCAD files

- Status: experimental (branch `scad-editor`)
- Date: 2026-11-02

## Context

OpenSCAD files are linked as parts (ADR 0011). To change one, the user so far
needs another program. The user wants an editor in the app, written in Rust,
with a language server for OpenSCAD and tree-sitter for the syntax: colours,
errors while typing, completion, explanations, going to a definition. And at
the part, a choice of how to edit its file: in the app, in OpenSCAD, in the
system's app for `.scad`, or by a command of one's own.

## Evaluation

Crates and grammars (checked on crates.io and GitHub, 2026-11-02):

| Candidate | What | Licence | Verdict |
|---|---|---|---|
| `tree-sitter` 0.27.1 (2026-10-08) | the parser runtime, C compiled by `cc` | MIT | yes |
| `tree-sitter-openscad-ng` 0.7.1 (2026-03-21) | OpenSCAD's grammar, by the OpenSCAD organisation (github.com/openscad/tree-sitter-openscad), with highlight and locals queries | MIT | yes: maintained where OpenSCAD is, its queries reused |
| `tree-sitter-openscad` 0.5.1 (2024-02-24) | the same grammar's older home (bollian) | MIT | no: moved to the one above |
| `openscad-lsp` 2.0.2 (2026-05-11, Leathong) | a language server binary on tree-sitter | MIT OR Apache-2.0 | no: a binary, not a library; brings `clap`, `topiary-core`, `regex`, `lazy_static`, `gen-lsp-types`, `shellexpand`, `directories`, and pins `tree-sitter` 0.25. Its approach (tree-sitter for names and scopes) is the one taken here |
| `lsp-types` 0.97.0, `lsp-server` 0.10.0 | the protocol's types and its stdio loop | MIT (OR Apache-2.0) | no: `serde` derive, `serde_repr`, `bitflags` 1, `fluent-uri`, `crossbeam-channel`, `log` for a dozen messages; `serde_json`, already here for the MCP server, is enough |
| `tree-sitter-highlight` 0.27.1 | highlighting by queries | MIT | no: brings `thiserror`; a query cursor does it in 40 lines |

The new crates in `Cargo.lock`: `tree-sitter`, `tree-sitter-openscad-ng`,
`tree-sitter-language`, `streaming-iterator`, `regex`, `regex-automata`,
`regex-syntax`, `aho-corasick` (all MIT or Apache-2.0 or Unlicense/MIT, the
newest from 2026-10-08).

## Decision

- A crate `opensolve-scad-lsp`: a library and a binary of that name, which
  speaks the Language Server Protocol over stdio (`Content-Length` framing,
  UTF-16 positions, whole texts on change) for VS Code, Zed, Helix and others.
  The app uses the same library in its own process (`Workspace`), no
  subprocess: one implementation for both.
- The syntax by tree-sitter and OpenSCAD's own grammar: syntax errors
  (`ERROR` and missing nodes) at once while typing, the highlighting by the
  grammar's `highlights.scm` (with a few of OpenSolve's built-ins added; of
  several patterns for one place the last wins, as the query is written),
  and the definitions and scopes read off the tree.
- The meaning by OpenSolve's own interpreter (ADR 0011): `opensolve-scad`
  now also tells its notes with their places (`diagnose`, `Note`), so that
  unknown modules, functions and variables, what is not supported, failed
  assertions and `echo` output are marked where they are. The evaluation
  builds the shapes too: the server runs it when a file is opened or saved,
  the app's editor in the background a moment after typing stops. The
  syntax is checked on every change.
- Completion: keywords, OpenSCAD's modules, functions and special variables
  with their signatures, the file's own modules, functions and variables,
  the parameters, `let` and `for` names of the place, all of what it
  includes and the modules and functions of what it uses (by OpenSCAD's
  rules for `include` and `use`), and file names in `include <…>`. Hover:
  the signature and the comment above the definition (a variable's
  Customizer comment behind it too), or OpenSCAD's explanation of a
  built-in. Go to definition across `include` and `use`, and from their
  path to the file. Document symbols, signature help. No formatting yet.
- tree-sitter is C, so it stays out of the crates built for wasm32
  (`make wasm`): `opensolve-scad` stays pure Rust; the language server and
  the app's editor are native only.
- The editor in the app (`scad_editor.rs`): a tab of its own in the middle
  pane beside 3D and the sketch (egui's text field with a layouter), with OpenCAD's tokens for its colours in
  light and dark, line numbers, the diagnostics as squiggles, gutter marks
  and a list of problems, completion (Ctrl+Space, or while typing a name),
  hover, go to definition (Cmd+click or F12, also into included and used
  files, as tabs), find (Cmd+F), undo and redo of its own (quick edits as
  one step; the menu's Undo, Redo and Save go to the editor while it is
  typed in), and Cmd/Ctrl+S to save. A file changed on the disk and not
  in the editor is read again. On saving it reads the links of that file
  again at once and gives the watcher of linked files (ADR 0011) the new
  times, so that a save rebuilds once.
- The choice at the part: a link to a `.scad` file has a setting how to edit
  it: the editor in the app (the default), OpenSCAD (when installed), the
  system's app for the file, or a command (`code {file}`; `{file}` is the
  path). It is stored with the link in the `.osolve` (`editor` line after
  it); a link without one follows the app's setting, stored with the app's
  other settings. The button "Bearbeiten" opens the file that way.

## Consequences

- The app and the language server link tree-sitter's C runtime and the
  grammar's parser (about 300 kB of C).
- The binary is not shipped in the macOS bundle yet; `cargo install --path
  crates/opensolve-scad-lsp` makes it available to other editors.
- The built-in texts of the language server are English, as OpenSCAD's
  manual and the interpreter's notes are; the editor's own texts are in the
  app's languages.
- The evaluation for diagnostics builds the shapes; a file that takes long
  to build takes as long to check. A dry run without shapes would need the
  interpreter to skip its geometry; not done in v1.
