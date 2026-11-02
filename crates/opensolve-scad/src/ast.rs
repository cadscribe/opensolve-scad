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

impl std::fmt::Display for Expr {
    /// The expression as OpenSCAD writes it back: what `str` makes of a
    /// function literal, and what a failed assertion says.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Expr::Num(x) => f.write_str(&crate::value::number(*x)),
            Expr::Str(s) => f.write_str(&crate::value::repr(&crate::value::Value::Str(s.clone()))),
            Expr::Bool(b) => write!(f, "{b}"),
            Expr::Undef => f.write_str("undef"),
            Expr::Var(n, _) => f.write_str(n),
            Expr::List(e) => {
                f.write_str("[")?;
                for (i, x) in e.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{x}")?;
                }
                f.write_str("]")
            }
            Expr::Range(b, s, e) => match s {
                Some(s) => write!(f, "[{b} : {s} : {e}]"),
                None => write!(f, "[{b} : {e}]"),
            },
            Expr::Unary(op, x) => write!(f, "{op}{x}"),
            Expr::Binary(op, a, b) => write!(f, "({a} {op} {b})"),
            Expr::Ternary(c, y, n) => write!(f, "({c} ? {y} : {n})"),
            Expr::Call(c, args, _) => {
                write!(f, "{c}(")?;
                write_args(f, args)?;
                f.write_str(")")
            }
            Expr::Index(v, i) => write!(f, "{v}[{i}]"),
            Expr::Member(v, m) => write!(f, "{v}.{m}"),
            Expr::Let(args, body) => {
                f.write_str("let(")?;
                write_args(f, args)?;
                write!(f, ") {body}")
            }
            Expr::Echo(args, body, _) => {
                f.write_str("echo(")?;
                write_args(f, args)?;
                write!(f, ") {body}")
            }
            Expr::Assert(args, body, _) => {
                f.write_str("assert(")?;
                write_args(f, args)?;
                write!(f, ") {body}")
            }
            Expr::Lambda(func) => write!(f, "{func}"),
        }
    }
}

impl std::fmt::Display for Func {
    /// `function(a, b = 1) body`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("function(")?;
        write_args(f, &self.params)?;
        write!(f, ") {}", self.body)
    }
}

impl std::fmt::Display for Elem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Elem::Expr(e) => write!(f, "{e}"),
            Elem::For(args, body) => {
                f.write_str("for(")?;
                write_args(f, args)?;
                write!(f, ") ({body})")
            }
            Elem::ForC(init, cond, next, body) => {
                f.write_str("for(")?;
                write_args(f, init)?;
                write!(f, ";{cond};")?;
                write_args(f, next)?;
                write!(f, ") {body}")
            }
            Elem::If(c, then, or) => {
                write!(f, "if({c}) ({then})")?;
                match or {
                    Some(o) => write!(f, " else ({o})"),
                    None => Ok(()),
                }
            }
            Elem::Let(args, body) => {
                f.write_str("let(")?;
                write_args(f, args)?;
                write!(f, ") ({body})")
            }
            Elem::Each(body) => write!(f, "each ({body})"),
        }
    }
}

fn write_args(f: &mut std::fmt::Formatter<'_>, args: &[Arg]) -> std::fmt::Result {
    for (i, a) in args.iter().enumerate() {
        if i > 0 {
            f.write_str(", ")?;
        }
        match (&a.name, &a.value) {
            (Some(n), Some(v)) => write!(f, "{n} = {v}")?,
            (Some(n), None) => f.write_str(n)?,
            (None, Some(v)) => write!(f, "{v}")?,
            (None, None) => {}
        }
    }
    Ok(())
}
