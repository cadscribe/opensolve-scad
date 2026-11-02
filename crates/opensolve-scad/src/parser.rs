//! A recursive descent parser of OpenSCAD's language, as of 2021.01: its
//! statements, expressions with OpenSCAD's precedence, list comprehensions
//! and function literals.

use std::sync::Arc;

use crate::Error;
use crate::ast::{Arg, Elem, Expr, Func, Modifier, Module, Pos, Stmt};
use crate::lexer::{Tok, Token, tokens};

struct Parser<'a> {
    toks: Vec<Token>,
    at: usize,
    file: u32,
    text: &'a str,
}

/// The statements of a file; `file` is its index among the files read.
pub fn parse(text: &str, file: u32) -> Result<Vec<Stmt>, Error> {
    let mut p = Parser {
        toks: tokens(text)?,
        at: 0,
        file,
        text,
    };
    let mut out = Vec::new();
    while p.tok() != &Tok::End {
        if let Some(s) = p.statement()? {
            put(&mut out, s);
        }
    }
    Ok(out)
}

/// A statement added to a list; braces on their own only group, as in
/// OpenSCAD: what they hold is of the scope around them (modules defined in
/// a `{ // fold` block are the file's).
fn put(out: &mut Vec<Stmt>, s: Stmt) {
    match s {
        Stmt::Block(b) => out.extend(b.iter().cloned()),
        s => out.push(s),
    }
}

/// One expression, as an override's value.
pub fn parse_expr(text: &str) -> Result<Expr, Error> {
    let mut p = Parser {
        toks: tokens(text)?,
        at: 0,
        file: 0,
        text,
    };
    let e = p.expr()?;
    if p.tok() != &Tok::End {
        return Err(p.error("expected the end"));
    }
    Ok(e)
}

impl Parser<'_> {
    fn tok(&self) -> &Tok {
        &self.toks[self.at].tok
    }

    fn peek(&self, k: usize) -> &Tok {
        &self.toks[(self.at + k).min(self.toks.len() - 1)].tok
    }

    fn pos(&self) -> Pos {
        let t = &self.toks[self.at];
        Pos {
            file: self.file,
            line: t.line,
            col: t.col,
        }
    }

    fn error(&self, message: &str) -> Error {
        let t = &self.toks[self.at];
        let found = match &t.tok {
            Tok::End => "the end of the file".to_string(),
            _ => format!("“{}”", &self.text[t.start..t.end]),
        };
        Error::at(t.line, t.col, format!("{message}, found {found}"))
    }

    fn is(&self, sym: &str) -> bool {
        matches!(self.tok(), Tok::Sym(s) if *s == sym)
    }

    fn is_name(&self, word: &str) -> bool {
        matches!(self.tok(), Tok::Name(n) if n == word)
    }

    fn eat(&mut self, sym: &str) -> bool {
        let yes = self.is(sym);
        if yes {
            self.at += 1;
        }
        yes
    }

    fn expect(&mut self, sym: &str) -> Result<(), Error> {
        if self.eat(sym) {
            Ok(())
        } else {
            Err(self.error(&format!("expected “{sym}”")))
        }
    }

    fn name(&mut self) -> Result<Arc<str>, Error> {
        match self.tok().clone() {
            Tok::Name(n) => {
                self.at += 1;
                Ok(n.into())
            }
            _ => Err(self.error("expected a name")),
        }
    }

    /// One statement; `None` for an empty one.
    fn statement(&mut self) -> Result<Option<Stmt>, Error> {
        let pos = self.pos();
        match self.tok().clone() {
            Tok::Sym(";") => {
                self.at += 1;
                Ok(None)
            }
            Tok::Sym("{") => {
                self.at += 1;
                Ok(Some(Stmt::Block(self.block()?.into())))
            }
            Tok::Include(path) => {
                self.at += 1;
                Ok(Some(Stmt::Include(path, pos)))
            }
            Tok::Use(path) => {
                self.at += 1;
                Ok(Some(Stmt::Use(path, pos)))
            }
            Tok::Name(n) if n == "module" => {
                self.at += 1;
                let name = self.name()?;
                let params = self.params()?;
                let body = self.child()?;
                Ok(Some(Stmt::Module(name, Arc::new(Module { params, body }))))
            }
            Tok::Name(n) if n == "function" && matches!(self.peek(1), Tok::Name(_)) => {
                self.at += 1;
                let name = self.name()?;
                let params = self.params()?;
                self.expect("=")?;
                let body = self.expr()?;
                self.expect(";")?;
                Ok(Some(Stmt::Function(name, Arc::new(Func { params, body }))))
            }
            Tok::Name(n) if *self.peek(1) == Tok::Sym("=") => {
                self.at += 2;
                let start = self.toks[self.at].start;
                let value = self.expr()?;
                let end = self.toks[self.at - 1].end;
                let end_line = self.toks[self.at].line;
                self.expect(";")?;
                Ok(Some(Stmt::Assign {
                    name: n.into(),
                    value,
                    pos,
                    span: (start, end),
                    end_line,
                }))
            }
            _ => self.instantiation().map(Some),
        }
    }

    /// Statements up to the closing brace.
    fn block(&mut self) -> Result<Vec<Stmt>, Error> {
        let mut out = Vec::new();
        while !self.eat("}") {
            if self.tok() == &Tok::End {
                return Err(self.error("expected “}”"));
            }
            if let Some(s) = self.statement()? {
                put(&mut out, s);
            }
        }
        Ok(out)
    }

    /// What a module call or `if` applies to: one statement, a block, or
    /// nothing (`;`).
    fn child(&mut self) -> Result<Vec<Stmt>, Error> {
        if self.eat(";") {
            return Ok(Vec::new());
        }
        if self.eat("{") {
            return self.block();
        }
        let mut out = Vec::new();
        if let Some(s) = self.statement()? {
            put(&mut out, s);
        }
        Ok(out)
    }

    fn instantiation(&mut self) -> Result<Stmt, Error> {
        let modifier = if self.eat("!") {
            Modifier::Root
        } else if self.eat("#") {
            Modifier::Highlight
        } else if self.eat("%") {
            Modifier::Background
        } else if self.eat("*") {
            Modifier::Disable
        } else {
            Modifier::None
        };
        if modifier != Modifier::None {
            let mut s = self.instantiation()?;
            if let Stmt::Call { modifier: m, .. } | Stmt::If { modifier: m, .. } = &mut s {
                // The outer one counts: `*` leaves out whatever is inside.
                if *m == Modifier::None || modifier == Modifier::Disable {
                    *m = modifier;
                }
            }
            return Ok(s);
        }
        let pos = self.pos();
        if self.is_name("if") {
            self.at += 1;
            self.expect("(")?;
            let cond = self.expr()?;
            self.expect(")")?;
            let then = self.child()?;
            let or = if self.is_name("else") {
                self.at += 1;
                self.child()?
            } else {
                Vec::new()
            };
            return Ok(Stmt::If {
                cond,
                then: then.into(),
                or: or.into(),
                modifier,
            });
        }
        let Tok::Name(name) = self.tok().clone() else {
            return Err(self.error("expected a statement"));
        };
        self.at += 1;
        if !self.is("(") {
            return Err(self.error("expected “(” or “=”"));
        }
        let args = self.args()?;
        let children = self.child()?;
        Ok(Stmt::Call {
            name: name.into(),
            args,
            children: children.into(),
            modifier,
            pos,
        })
    }

    /// `(a, b = 1, …)` of a definition.
    fn params(&mut self) -> Result<Vec<Arg>, Error> {
        self.expect("(")?;
        let mut out = Vec::new();
        while !self.eat(")") {
            let name = self.name()?;
            let value = if self.eat("=") {
                Some(self.expr()?)
            } else {
                None
            };
            out.push(Arg {
                name: Some(name),
                value,
            });
            if !self.eat(",") {
                self.expect(")")?;
                break;
            }
        }
        Ok(out)
    }

    /// `(1, b = 2, …)` of a call.
    fn args(&mut self) -> Result<Vec<Arg>, Error> {
        self.expect("(")?;
        let mut out = Vec::new();
        while !self.eat(")") {
            let named = matches!(self.tok(), Tok::Name(_)) && *self.peek(1) == Tok::Sym("=");
            let name = if named {
                let n = self.name()?;
                self.at += 1;
                Some(n)
            } else {
                None
            };
            out.push(Arg {
                name,
                value: Some(self.expr()?),
            });
            if !self.eat(",") {
                self.expect(")")?;
                break;
            }
        }
        Ok(out)
    }

    pub fn expr(&mut self) -> Result<Expr, Error> {
        let pos = self.pos();
        if self.is_name("function") && *self.peek(1) == Tok::Sym("(") {
            self.at += 1;
            let params = self.params()?;
            let body = self.expr()?;
            return Ok(Expr::Lambda(Arc::new(Func { params, body })));
        }
        if self.is_name("let") && *self.peek(1) == Tok::Sym("(") {
            self.at += 1;
            let args = self.args()?;
            return Ok(Expr::Let(args, Box::new(self.expr()?)));
        }
        for word in ["echo", "assert"] {
            if self.is_name(word) && *self.peek(1) == Tok::Sym("(") {
                self.at += 1;
                let args = self.args()?;
                let ends = [";", ")", "]", ",", ":", "}"].iter().any(|s| self.is(s))
                    || self.tok() == &Tok::End;
                let then = if ends { Expr::Undef } else { self.expr()? };
                return Ok(if word == "echo" {
                    Expr::Echo(args, Box::new(then), pos)
                } else {
                    Expr::Assert(args, Box::new(then), pos)
                });
            }
        }
        let cond = self.binary(0)?;
        if self.eat("?") {
            let yes = self.expr()?;
            self.expect(":")?;
            let no = self.expr()?;
            return Ok(Expr::Ternary(Box::new(cond), Box::new(yes), Box::new(no)));
        }
        Ok(cond)
    }

    /// Binary operators by level, loosest first.
    fn binary(&mut self, level: usize) -> Result<Expr, Error> {
        const LEVELS: [&[&str]; 6] = [
            &["||"],
            &["&&"],
            &["==", "!="],
            &["<", "<=", ">", ">="],
            &["+", "-"],
            &["*", "/", "%"],
        ];
        let Some(ops) = LEVELS.get(level) else {
            return self.unary();
        };
        let mut left = self.binary(level + 1)?;
        loop {
            let Some(op) = ops.iter().find(|o| self.is(o)) else {
                return Ok(left);
            };
            self.at += 1;
            let right = self.binary(level + 1)?;
            left = Expr::Binary(op, Box::new(left), Box::new(right));
        }
    }

    fn unary(&mut self) -> Result<Expr, Error> {
        for op in ["-", "+", "!"] {
            if self.eat(op) {
                let e = self.unary()?;
                return Ok(match (op, e) {
                    ("-", Expr::Num(x)) => Expr::Num(-x),
                    ("+", e) => e,
                    (op, e) => Expr::Unary(op, Box::new(e)),
                });
            }
        }
        let base = self.postfix()?;
        if self.eat("^") {
            let power = self.unary()?;
            return Ok(Expr::Binary("^", Box::new(base), Box::new(power)));
        }
        Ok(base)
    }

    fn postfix(&mut self) -> Result<Expr, Error> {
        let mut e = self.primary()?;
        loop {
            if self.is("(") {
                let pos = self.pos();
                let args = self.args()?;
                e = Expr::Call(Box::new(e), args, pos);
            } else if self.eat("[") {
                let i = self.expr()?;
                self.expect("]")?;
                e = Expr::Index(Box::new(e), Box::new(i));
            } else if self.eat(".") {
                e = Expr::Member(Box::new(e), self.name()?);
            } else {
                return Ok(e);
            }
        }
    }

    fn primary(&mut self) -> Result<Expr, Error> {
        let pos = self.pos();
        match self.tok().clone() {
            Tok::Num(x) => {
                self.at += 1;
                Ok(Expr::Num(x))
            }
            Tok::Str(s) => {
                self.at += 1;
                Ok(Expr::Str(s.into()))
            }
            Tok::Sym("(") => {
                self.at += 1;
                let e = self.expr()?;
                self.expect(")")?;
                Ok(e)
            }
            Tok::Sym("[") => {
                self.at += 1;
                self.list()
            }
            Tok::Name(n) => {
                self.at += 1;
                Ok(match n.as_str() {
                    "true" => Expr::Bool(true),
                    "false" => Expr::Bool(false),
                    "undef" => Expr::Undef,
                    _ => Expr::Var(n.into(), pos),
                })
            }
            _ => Err(self.error("expected a value")),
        }
    }

    /// After `[`: a range, or a list.
    fn list(&mut self) -> Result<Expr, Error> {
        if self.eat("]") {
            return Ok(Expr::List(Vec::new()));
        }
        let first = self.elem()?;
        if let Elem::Expr(begin) = &first
            && self.eat(":")
        {
            let second = self.expr()?;
            let range = if self.eat(":") {
                let end = self.expr()?;
                Expr::Range(
                    Box::new(begin.clone()),
                    Some(Box::new(second)),
                    Box::new(end),
                )
            } else {
                Expr::Range(Box::new(begin.clone()), None, Box::new(second))
            };
            self.expect("]")?;
            return Ok(range);
        }
        let mut out = vec![first];
        while self.eat(",") {
            if self.is("]") {
                break;
            }
            out.push(self.elem()?);
        }
        self.expect("]")?;
        Ok(Expr::List(out))
    }

    fn elem(&mut self) -> Result<Elem, Error> {
        let call = |p: &Self, w: &str| p.is_name(w) && *p.peek(1) == Tok::Sym("(");
        if call(self, "for") {
            self.at += 1;
            self.expect("(")?;
            let init = self.assignments(&[";", ")"])?;
            if self.eat(";") {
                let cond = self.expr()?;
                self.expect(";")?;
                let next = self.assignments(&[")"])?;
                self.expect(")")?;
                let body = self.elem()?;
                return Ok(Elem::ForC(init, cond, next, Box::new(body)));
            }
            self.expect(")")?;
            return Ok(Elem::For(init, Box::new(self.elem()?)));
        }
        if call(self, "if") {
            self.at += 1;
            self.expect("(")?;
            let cond = self.expr()?;
            self.expect(")")?;
            let then = self.elem()?;
            let or = if self.is_name("else") {
                self.at += 1;
                Some(Box::new(self.elem()?))
            } else {
                None
            };
            return Ok(Elem::If(cond, Box::new(then), or));
        }
        if call(self, "let") {
            self.at += 1;
            let args = self.args()?;
            return Ok(Elem::Let(args, Box::new(self.elem()?)));
        }
        if self.is_name("each") {
            self.at += 1;
            return Ok(Elem::Each(Box::new(self.elem()?)));
        }
        // A comprehension in parentheses.
        if self.is("(")
            && matches!(self.peek(1), Tok::Name(n) if ["for", "if", "let", "each"].contains(&n.as_str()))
        {
            self.at += 1;
            let e = self.elem()?;
            self.expect(")")?;
            return Ok(e);
        }
        Ok(Elem::Expr(self.expr()?))
    }

    /// `a = 1, b = 2` up to one of `ends`; a value without a name, as
    /// OpenSCAD takes it (`for ([0:2]) …` runs three times).
    fn assignments(&mut self, ends: &[&str]) -> Result<Vec<Arg>, Error> {
        let mut out = Vec::new();
        while !ends.iter().any(|e| self.is(e)) {
            let named = matches!(self.tok(), Tok::Name(_)) && *self.peek(1) == Tok::Sym("=");
            let name = if named {
                let n = self.name()?;
                self.at += 1;
                Some(n)
            } else {
                None
            };
            out.push(Arg {
                name,
                value: Some(self.expr()?),
            });
            if !self.eat(",") {
                break;
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(text: &str) -> Expr {
        parse_expr(text).unwrap()
    }

    #[test]
    fn precedence_is_openscads() {
        let Expr::Binary("+", _, r) = one("1 + 2 * 3") else {
            panic!("not a sum")
        };
        assert!(matches!(*r, Expr::Binary("*", ..)));
        // `-2^2` is -(2^2), and `^` binds to the right.
        assert!(matches!(one("-2^2"), Expr::Unary("-", _)));
        let Expr::Binary("^", _, r) = one("2^3^2") else {
            panic!("not a power")
        };
        assert!(matches!(*r, Expr::Binary("^", ..)));
        assert!(matches!(one("a || b && c"), Expr::Binary("||", ..)));
        assert!(matches!(one("a ? b : c ? d : e"), Expr::Ternary(..)));
    }

    #[test]
    fn ranges_lists_and_comprehensions() {
        assert!(matches!(one("[0:2:10]"), Expr::Range(_, Some(_), _)));
        assert!(matches!(one("[1:5]"), Expr::Range(_, None, _)));
        let Expr::List(e) =
            one("[for (i = [0:3], j = [1, 2]) if (i > j) [i, j] else 0, each [1, 2], 7,]")
        else {
            panic!("not a list")
        };
        assert_eq!(e.len(), 3);
        assert!(matches!(&e[0], Elem::For(a, b) if a.len() == 2 && matches!(**b, Elem::If(..))));
        assert!(matches!(&e[1], Elem::Each(_)));
        assert!(matches!(
            one("[for (i = 0; i < 3; i = i + 1) i]"),
            Expr::List(_)
        ));
        assert!(
            matches!(one("[for ([0:2]) 5]"), Expr::List(e) if matches!(&e[0], Elem::For(a, _) if a[0].name.is_none()))
        );
    }

    #[test]
    fn statements() {
        let s = parse(
            "x = 3; // [1:10]\nmodule m(a, b = 2) { cube(a); children(); }\n\
             function f(x) = x * 2;\nif (x > 1) m(1) sphere(); else { cylinder(h = 2); }\n\
             !translate([1, 0, 0]) %cube(); *m(); for (i = [0:2]) echo(i);",
            0,
        )
        .unwrap();
        assert_eq!(s.len(), 7);
        let Stmt::Assign { span, end_line, .. } = &s[0] else {
            panic!("not an assignment")
        };
        assert_eq!((*span, *end_line), ((4, 5), 1));
        assert!(matches!(&s[1], Stmt::Module(n, m) if &**n == "m" && m.params.len() == 2));
        assert!(matches!(&s[3], Stmt::If { or, .. } if or.len() == 1));
        assert!(
            matches!(&s[4], Stmt::Call { modifier: Modifier::Root, children, .. }
            if matches!(&children[0], Stmt::Call { modifier: Modifier::Background, .. }))
        );
        assert!(matches!(
            &s[5],
            Stmt::Call {
                modifier: Modifier::Disable,
                ..
            }
        ));
    }

    #[test]
    fn braces_on_their_own_only_group() {
        let s = parse(
            "{ // fold\n module m() cube(); x = 1; { cube(); } }\nm();",
            0,
        )
        .unwrap();
        assert_eq!(s.len(), 4);
        assert!(matches!(&s[0], Stmt::Module(n, _) if &**n == "m"));
    }

    #[test]
    fn lambdas_let_echo_and_assert() {
        assert!(matches!(one("function (x) x + 1"), Expr::Lambda(_)));
        assert!(matches!(one("let (a = 1, b = a) a + b"), Expr::Let(a, _) if a.len() == 2));
        assert!(matches!(one("assert(x > 0, \"no\") x"), Expr::Assert(..)));
        assert!(matches!(one("f(1)(2)[0].x"), Expr::Member(..)));
    }

    #[test]
    fn a_parse_error_says_where_and_what() {
        let e = parse("x = 1;\ny = 2\ncube(3);", 0).unwrap_err();
        assert_eq!((e.line, e.col), (3, 1));
        assert!(
            e.message.contains("“;”") && e.message.contains("“cube”"),
            "{}",
            e.message
        );
    }
}
