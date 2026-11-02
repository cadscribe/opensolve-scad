//! A language server for OpenSCAD (ADR 0012): the syntax by tree-sitter
//! and OpenSCAD's own grammar, what goes wrong when the file is evaluated
//! by OpenSolve's interpreter (ADR 0011), completion, hover, definitions
//! across `include` and `use`, symbols and signatures. The app's editor
//! uses [`Workspace`] in its own process; the `opensolve-scad-lsp` binary
//! speaks the Language Server Protocol over stdio for other editors.

pub mod builtins;
pub mod index;
pub mod server;
pub mod syntax;
pub mod text;
pub mod workspace;

use std::ops::Range;

pub use index::DefKind;
pub use syntax::{Span, Token};
pub use workspace::{
    Completion, CompletionKind, Evaluation, Hover, Location, Signature, Symbol, Workspace,
};

/// How bad a diagnostic is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    Error,
    Warning,
    /// What `echo` writes.
    Info,
}

/// Something wrong in a file, or worth knowing, and where: bytes of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub range: Range<usize>,
    pub severity: Severity,
    pub message: String,
}
