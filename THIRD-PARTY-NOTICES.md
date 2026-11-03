# Third-party notices

The code of this repository is under MIT OR Apache-2.0. These come with it
or are built into what it builds, each under its own licence.

## BOSL2

`crates/opensolve-scad/library/BOSL2`: the OpenSCAD library
[BOSL2](https://github.com/BelfrySCAD/BOSL2), unmodified, built into
`opensolve-scad` and written out where OpenSCAD files look for libraries
(ADR 0014). BSD-2-Clause, Copyright (c) 2017-2019, Revar Desmera; the
licence is [its LICENSE file](crates/opensolve-scad/library/BOSL2/LICENSE),
which programs that bundle the library should ship with it.

## Liberation Sans

`assets/fonts/LiberationSans-Regular.ttf` and `LiberationSans-Bold.ttf`,
version 2.00.1, unmodified: OpenSCAD's default font, built into
`opensolve-scad` for `text()` where the machine has none. Digitized data
copyright (c) 2010 Google Corporation, copyright (c) 2012 Red Hat, Inc.,
under the SIL Open Font License 1.1, see
[LiberationSans-LICENSE.txt](assets/fonts/LiberationSans-LICENSE.txt).

## Manifold and oneTBB

Compiled from their sources by `crates/opensolve-manifold/build.rs`, which
downloads them at build time (not part of this repository) and checks them
by SHA-256:

- [Manifold](https://github.com/elalish/manifold) 3.5.2, Apache-2.0,
  Copyright the Manifold authors.
- [oneTBB](https://github.com/uxlfoundation/oneTBB) 2023.1.0, Apache-2.0,
  Copyright Intel Corporation and the UXL Foundation.

Binaries that link them carry their code and should include the
Apache-2.0 licence and their notices.

## Rust crates

Dependencies from crates.io, linked into the binaries, among them:

- [boolmesh](https://crates.io/crates/boolmesh) 0.2.1 (the booleans in the
  browser), MPL-2.0: its source is available from crates.io; changes to its
  files would have to be published under the MPL-2.0.
- [clipper2-rust](https://crates.io/crates/clipper2-rust) 1.2.0 (the 2D
  booleans and offsets, a port of Angus Johnson's Clipper2), BSL-1.0.
- [ttf-parser](https://crates.io/crates/ttf-parser) 0.25.1, MIT OR
  Apache-2.0.
- [tree-sitter](https://crates.io/crates/tree-sitter) 0.27.1 and
  [tree-sitter-openscad-ng](https://crates.io/crates/tree-sitter-openscad-ng)
  0.7.1, MIT.

`cargo tree` lists them all with `Cargo.lock`'s versions.
