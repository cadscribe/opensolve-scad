//! `opensolve-scad-lsp`: OpenSolve's language server for OpenSCAD files,
//! over stdio, for editors such as VS Code, Zed or Helix (ADR 0012).

use std::io::{self, BufReader};

const USAGE: &str = "usage: opensolve-scad-lsp [--stdio]\n\nA language server for OpenSCAD files: speaks the Language Server Protocol on stdin and stdout.\n";

fn main() {
    for a in std::env::args().skip(1) {
        match a.as_str() {
            "--stdio" => {}
            "--version" => {
                println!("opensolve-scad-lsp {}", env!("CARGO_PKG_VERSION"));
                return;
            }
            _ => {
                eprint!("{USAGE}");
                std::process::exit(if a == "--help" || a == "-h" { 0 } else { 2 });
            }
        }
    }
    let mut input = BufReader::new(io::stdin().lock());
    let mut output = io::stdout().lock();
    if let Err(e) = opensolve_scad_lsp::server::serve(&mut input, &mut output) {
        eprintln!("opensolve-scad-lsp: {e}");
        std::process::exit(1);
    }
}
