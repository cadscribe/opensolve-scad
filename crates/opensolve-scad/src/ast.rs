//! A parsed OpenSCAD file.

use std::rc::Rc;

/// Where something is: the file (an index into the files read) and line.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pos {
    pub file: u32,
    pub line: u32,
    pub col: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    Num(f64),
    Str(Rc<str>),
    Bool(bool),
    Undef,
    Var(Rc<str>, Pos),
    /// `[…]`, its elements maybe comprehensions.
    List(Vec<Elem>),
    /// `[begin : step : end]`.
    Range(Box<Expr>, Option<Box<Expr>>, Box<Expr>),
    Unary(&'static str, Box<Expr>),
    Binary(&'static str, Box<Expr>, Box<Expr>),
    Ternary(Box<Expr>, Box<Expr>, Box<Expr>),
    Call(Box<Expr>, Vec<Arg>, Pos),
    Index(Box<Expr>, Box<Expr>),
    /// `.x`, `.y`, `.z`.
    Member(Box<Expr>, Rc<str>),
    Let(Vec<Arg>, Box<Expr>),
    /// `echo(…) expr` and `assert(…) expr`.
    Echo(Vec<Arg>, Box<Expr>, Pos),
    Assert(Vec<Arg>, Box<Expr>, Pos),
    /// `function (params) body`.
    Lambda(Rc<Func>),
}

/// An element of a list.
#[derive(Clone, Debug, PartialEq)]
pub enum Elem {
    Expr(Expr),
    For(Vec<Arg>, Box<Elem>),
    /// `for (init; condition; next)`.
    ForC(Vec<Arg>, Expr, Vec<Arg>, Box<Elem>),
    If(Expr, Box<Elem>, Option<Box<Elem>>),
    Let(Vec<Arg>, Box<Elem>),
    Each(Box<Elem>),
}

/// An argument, named or not; also a parameter with its default.
#[derive(Clone, Debug, PartialEq)]
pub struct Arg {
    pub name: Option<Rc<str>>,
    pub value: Option<Expr>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Func {
    pub params: Vec<Arg>,
    pub body: Expr,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Module {
    pub params: Vec<Arg>,
    pub body: Vec<Stmt>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Stmt {
    Assign {
        name: Rc<str>,
        value: Expr,
        pos: Pos,
        /// The value's text in its file, in bytes, and the end of the line
        /// the statement ends on: what the Customizer reads.
        span: (usize, usize),
        end_line: u32,
    },
    Module(Rc<str>, Rc<Module>),
    Function(Rc<str>, Rc<Func>),
    /// A module called, with its children.
    Call {
        name: Rc<str>,
        args: Vec<Arg>,
        children: Rc<[Stmt]>,
        modifier: Modifier,
        pos: Pos,
    },
    If {
        cond: Expr,
        then: Rc<[Stmt]>,
        or: Rc<[Stmt]>,
        modifier: Modifier,
    },
    /// `{ … }` on its own.
    Block(Rc<[Stmt]>),
    /// `include <…>`, until the loader puts the file's statements here.
    Include(String, Pos),
    Use(String, Pos),
}

/// `*` leaves it out, `%` shows it only (both make nothing here), `!` makes
/// it the whole result, `#` only highlights.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Modifier {
    #[default]
    None,
    Disable,
    Background,
    Root,
    Highlight,
}
