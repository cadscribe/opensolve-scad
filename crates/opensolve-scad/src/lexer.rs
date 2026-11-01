//! The tokens of an OpenSCAD file: numbers, strings, names, symbols, and
//! `include <…>` and `use <…>` with their path. Comments are skipped.

use crate::Error;

#[derive(Clone, Debug, PartialEq)]
pub enum Tok {
    Num(f64),
    Str(String),
    /// A name, a keyword, or a `$` name.
    Name(String),
    Sym(&'static str),
    Include(String),
    Use(String),
    End,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Token {
    pub tok: Tok,
    pub line: u32,
    pub col: u32,
    /// Where it starts and ends in the text, in bytes.
    pub start: usize,
    pub end: usize,
}

/// Longest first.
const SYMBOLS: &[&str] = &[
    "<=", ">=", "==", "!=", "&&", "||", "+", "-", "*", "/", "%", "^", "!", "<", ">", "?", ":", "=",
    ";", ",", ".", "(", ")", "[", "]", "{", "}", "#",
];

struct Lexer<'a> {
    text: &'a str,
    at: usize,
    line: u32,
    line_start: usize,
}

/// The tokens of `text`, the last one [`Tok::End`].
pub fn tokens(text: &str) -> Result<Vec<Token>, Error> {
    let mut l = Lexer {
        text,
        at: 0,
        line: 1,
        line_start: 0,
    };
    let mut out = Vec::new();
    loop {
        l.space()?;
        let (start, line, col) = (l.at, l.line, l.col());
        let tok = l.next()?;
        let done = tok == Tok::End;
        out.push(Token {
            tok,
            line,
            col,
            start,
            end: l.at,
        });
        if done {
            return Ok(out);
        }
    }
}

impl Lexer<'_> {
    fn col(&self) -> u32 {
        self.text[self.line_start..self.at].chars().count() as u32 + 1
    }

    fn peek(&self) -> Option<char> {
        self.text[self.at..].chars().next()
    }

    fn peek2(&self) -> Option<char> {
        self.text[self.at..].chars().nth(1)
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.at += c.len_utf8();
        if c == '\n' {
            self.line += 1;
            self.line_start = self.at;
        }
        Some(c)
    }

    fn error(&self, message: impl Into<String>) -> Error {
        Error::at(self.line, self.col(), message)
    }

    /// Skips white space and comments.
    fn space(&mut self) -> Result<(), Error> {
        loop {
            match (self.peek(), self.peek2()) {
                (Some(c), _) if c.is_whitespace() => {
                    self.bump();
                }
                (Some('/'), Some('/')) => {
                    while self.peek().is_some_and(|c| c != '\n') {
                        self.bump();
                    }
                }
                (Some('/'), Some('*')) => {
                    let (line, col) = (self.line, self.col());
                    self.bump();
                    self.bump();
                    loop {
                        match self.bump() {
                            Some('*') if self.peek() == Some('/') => {
                                self.bump();
                                break;
                            }
                            Some(_) => {}
                            None => return Err(Error::at(line, col, "comment without its end")),
                        }
                    }
                }
                _ => return Ok(()),
            }
        }
    }

    fn next(&mut self) -> Result<Tok, Error> {
        let Some(c) = self.peek() else {
            return Ok(Tok::End);
        };
        if c.is_ascii_digit() || (c == '.' && self.peek2().is_some_and(|d| d.is_ascii_digit())) {
            return self.number();
        }
        if c == '"' {
            return self.string();
        }
        if c.is_alphanumeric() || c == '_' || c == '$' {
            let start = self.at;
            self.bump();
            while self.peek().is_some_and(|c| c.is_alphanumeric() || c == '_') {
                self.bump();
            }
            let word = &self.text[start..self.at];
            if word == "include" || word == "use" {
                let save = (self.at, self.line, self.line_start);
                self.space()?;
                if self.peek() == Some('<') {
                    self.bump();
                    let from = self.at;
                    while self.peek().is_some_and(|c| c != '>' && c != '\n') {
                        self.bump();
                    }
                    if self.peek() != Some('>') {
                        return Err(self.error("a path in <…> without its end"));
                    }
                    let path = self.text[from..self.at].trim().to_string();
                    self.bump();
                    return Ok(if word == "include" {
                        Tok::Include(path)
                    } else {
                        Tok::Use(path)
                    });
                }
                (self.at, self.line, self.line_start) = save;
            }
            return Ok(Tok::Name(word.to_string()));
        }
        let rest = &self.text[self.at..];
        for s in SYMBOLS {
            if rest.starts_with(s) {
                for _ in 0..s.len() {
                    self.bump();
                }
                return Ok(Tok::Sym(s));
            }
        }
        Err(self.error(format!("unexpected “{c}”")))
    }

    fn number(&mut self) -> Result<Tok, Error> {
        let start = self.at;
        let digits = |l: &mut Self| {
            while l.peek().is_some_and(|c| c.is_ascii_digit()) {
                l.bump();
            }
        };
        digits(self);
        if self.peek() == Some('.') && self.peek2().is_none_or(|c| c != '.') {
            self.bump();
            digits(self);
        }
        if matches!(self.peek(), Some('e' | 'E')) {
            let save = (self.at, self.line, self.line_start);
            self.bump();
            if matches!(self.peek(), Some('+' | '-')) {
                self.bump();
            }
            if self.peek().is_some_and(|c| c.is_ascii_digit()) {
                digits(self);
            } else {
                (self.at, self.line, self.line_start) = save;
            }
        }
        self.text[start..self.at]
            .parse()
            .map(Tok::Num)
            .map_err(|_| self.error("not a number"))
    }

    fn string(&mut self) -> Result<Tok, Error> {
        let (line, col) = (self.line, self.col());
        self.bump();
        let mut out = String::new();
        loop {
            match self.bump() {
                None => return Err(Error::at(line, col, "a string without its end")),
                Some('"') => return Ok(Tok::Str(out)),
                Some('\\') => match self.bump() {
                    Some('n') => out.push('\n'),
                    Some('t') => out.push('\t'),
                    Some('r') => out.push('\r'),
                    Some('x') => out.push(self.hex(2).unwrap_or('?')),
                    Some('u') => out.push(self.hex(4).unwrap_or('\u{fffd}')),
                    Some('U') => out.push(self.hex(6).unwrap_or('\u{fffd}')),
                    Some(c) => out.push(c),
                    None => return Err(Error::at(line, col, "a string without its end")),
                },
                Some(c) => out.push(c),
            }
        }
    }

    /// The character of up to `n` hex digits.
    fn hex(&mut self, n: usize) -> Option<char> {
        let start = self.at;
        while self.at - start < n && self.peek().is_some_and(|c| c.is_ascii_hexdigit()) {
            self.bump();
        }
        u32::from_str_radix(&self.text[start..self.at], 16)
            .ok()
            .and_then(char::from_u32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(text: &str) -> Vec<Tok> {
        tokens(text).unwrap().into_iter().map(|t| t.tok).collect()
    }

    #[test]
    fn numbers_names_and_symbols() {
        assert_eq!(
            toks("x=1.5e2;// no\n$fn<=.5 /* not\n this */ a&&b"),
            [
                Tok::Name("x".into()),
                Tok::Sym("="),
                Tok::Num(150.0),
                Tok::Sym(";"),
                Tok::Name("$fn".into()),
                Tok::Sym("<="),
                Tok::Num(0.5),
                Tok::Name("a".into()),
                Tok::Sym("&&"),
                Tok::Name("b".into()),
                Tok::End,
            ]
        );
    }

    #[test]
    fn strings_and_paths() {
        assert_eq!(
            toks(r#"include <lib/a.scad> use<b.scad> "a\"b\x41ä""#),
            [
                Tok::Include("lib/a.scad".into()),
                Tok::Use("b.scad".into()),
                Tok::Str("a\"bAä".into()),
                Tok::End,
            ]
        );
    }

    #[test]
    fn a_range_is_not_a_fraction() {
        assert_eq!(
            toks("[0:1:2]")[1..4],
            [Tok::Num(0.0), Tok::Sym(":"), Tok::Num(1.0)]
        );
    }

    #[test]
    fn errors_say_where() {
        let e = tokens("a = 1;\n  b = @;").unwrap_err();
        assert_eq!((e.line, e.col), (2, 7));
    }
}
