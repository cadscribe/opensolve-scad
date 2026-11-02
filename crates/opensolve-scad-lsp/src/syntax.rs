//! OpenSCAD's syntax by tree-sitter and OpenSCAD's own grammar
//! (tree-sitter-openscad-ng): the tree, its errors and the highlighting.

use std::ops::Range;
use std::sync::OnceLock;

use tree_sitter::{Language, Node, Parser, Query, QueryCursor, StreamingIterator, Tree};

use crate::{Diagnostic, Severity};

/// The grammar.
pub fn language() -> Language {
    tree_sitter_openscad::LANGUAGE.into()
}

/// The syntax tree of a text; tree-sitter recovers from errors, so there
/// is always one.
pub fn parse(text: &str) -> Tree {
    let mut parser = Parser::new();
    parser
        .set_language(&language())
        .expect("the grammar fits tree-sitter");
    parser
        .parse(text, None)
        .expect("a parser with a language and no timeout parses")
}

/// The syntax errors of a tree: what does not parse, and what is missing.
pub fn errors(tree: &Tree, text: &str) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    collect_errors(tree.root_node(), text, &mut out);
    out
}

fn collect_errors(node: Node<'_>, text: &str, out: &mut Vec<Diagnostic>) {
    if !node.has_error() {
        return;
    }
    if node.is_missing() {
        let at = node.start_byte();
        out.push(Diagnostic {
            range: at..at,
            severity: Severity::Error,
            message: format!("missing {}", shown(node.kind())),
        });
        return;
    }
    if node.is_error() {
        let range = node.byte_range();
        let found = text[range.clone()].trim();
        let message = match found.chars().next() {
            None => "syntax error".to_string(),
            Some(_) if found.chars().count() <= 24 && !found.contains('\n') => {
                format!("syntax error: unexpected `{found}`")
            }
            Some(_) => "syntax error".to_string(),
        };
        // An error that covers several lines is marked on its first only.
        let end = text[range.clone()]
            .find('\n')
            .map_or(range.end, |n| range.start + n);
        out.push(Diagnostic {
            range: range.start..end.max(range.start),
            severity: Severity::Error,
            message,
        });
        // Missing parts inside an error say more than the error.
        return;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_errors(child, text, out);
    }
}

/// A node kind as a person reads it.
fn shown(kind: &str) -> String {
    if kind.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && kind.len() > 1 {
        kind.replace('_', " ")
    } else {
        format!("`{kind}`")
    }
}

/// What a piece of text is, for its colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Token {
    Plain,
    Comment,
    Keyword,
    /// A module or function defined or called.
    Function,
    /// One of OpenSCAD's own modules or functions.
    Builtin,
    Variable,
    Parameter,
    /// `$fn`, `PI`, `undef`, `true`.
    Constant,
    Number,
    String,
    Operator,
    Punctuation,
}

impl Token {
    /// The token of one of the grammar's highlight names.
    fn of(capture: &str) -> Option<Token> {
        let head = capture.split('.').next().unwrap_or(capture);
        Some(match (head, capture) {
            (_, "function.builtin") => Token::Builtin,
            (_, "variable.builtin" | "constant.builtin") => Token::Constant,
            (_, "variable.parameter") => Token::Parameter,
            (_, "string.escape" | "string.special.path") => Token::String,
            ("comment", _) => Token::Comment,
            ("keyword", _) => Token::Keyword,
            ("function", _) => Token::Function,
            ("variable", _) => Token::Variable,
            ("constant" | "boolean", _) => Token::Constant,
            ("number", _) => Token::Number,
            ("string", _) => Token::String,
            ("operator", _) => Token::Operator,
            ("punctuation", _) => Token::Punctuation,
            _ => return None,
        })
    }
}

/// The grammar's highlights, with OpenSolve's own built-ins added (the
/// grammar's list misses a few) and the `use` keyword's path.
fn query() -> &'static Query {
    static QUERY: OnceLock<Query> = OnceLock::new();
    QUERY.get_or_init(|| {
        let own = "\n(module_call name: (identifier) @function.builtin\n  (#any-of? @function.builtin \"children\" \"render\" \"group\" \"assert\" \"import\" \"roof\" \"fill\"))\n(function_call name: (identifier) @function.builtin\n  (#any-of? @function.builtin \"abs\" \"sign\" \"sin\" \"cos\" \"tan\" \"asin\" \"acos\" \"atan\" \"atan2\" \"floor\" \"ceil\" \"round\" \"sqrt\" \"exp\" \"ln\" \"log\" \"pow\" \"min\" \"max\" \"norm\" \"cross\" \"len\" \"concat\" \"lookup\" \"str\" \"chr\" \"ord\" \"search\" \"rands\" \"version\" \"version_num\" \"is_undef\" \"is_bool\" \"is_num\" \"is_string\" \"is_list\" \"is_function\"))\n";
        let source = format!("{}{own}", tree_sitter_openscad::HIGHLIGHTS_QUERY);
        Query::new(&language(), &source).expect("the highlight query fits the grammar")
    })
}

/// A highlighted piece of text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub range: Range<usize>,
    pub token: Token,
}

/// The text cut into pieces by what they are, one after the other, the
/// whole text covered. Of the grammar's patterns for one place the last
/// wins, as the grammar's query is written.
pub fn highlight(tree: &Tree, text: &str) -> Vec<Span> {
    let q = query();
    let names = q.capture_names();
    let mut found: Vec<(usize, Range<usize>, Token)> = Vec::new();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(q, tree.root_node(), text.as_bytes());
    while let Some(m) = matches.next() {
        for c in m.captures() {
            // `$fn` is one name, not `$` and a variable.
            if c.node
                .parent()
                .is_some_and(|p| p.kind() == "special_variable")
            {
                continue;
            }
            if let Some(token) = Token::of(names[c.index as usize]) {
                found.push((m.pattern_index, c.node.byte_range(), token));
            }
        }
    }
    // Wider first, then by pattern: narrower and later ones paint over.
    found.sort_by_key(|(p, r, _)| (std::cmp::Reverse(r.len()), *p));
    let mut tokens = vec![Token::Plain; text.len()];
    for (_, r, t) in found {
        if r.end <= tokens.len() {
            tokens[r].fill(t);
        }
    }
    // What `*` disables is not built: grey like a comment.
    disabled(tree.root_node(), text, &mut tokens);
    let mut out: Vec<Span> = Vec::new();
    let mut start = 0;
    for i in 1..=tokens.len() {
        if i == tokens.len() || tokens[i] != tokens[start] {
            out.push(Span {
                range: start..i,
                token: tokens[start],
            });
            start = i;
        }
    }
    out
}

/// Marks as comment the statements behind a `*` modifier below `node`.
fn disabled(node: Node<'_>, text: &str, tokens: &mut [Token]) {
    if node.kind() == "modifier" && text.get(node.byte_range()) == Some("*") {
        let end = node.parent().map_or(node.end_byte(), |p| p.end_byte());
        if let Some(t) = tokens.get_mut(node.start_byte()..end.min(text.len())) {
            t.fill(Token::Comment);
        }
        return;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        disabled(child, text, tokens);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens(text: &str) -> Vec<(&str, Token)> {
        highlight(&parse(text), text)
            .into_iter()
            .filter(|s| !text[s.range.clone()].trim().is_empty())
            .map(|s| (&text[s.range], s.token))
            .collect()
    }

    #[test]
    fn a_sample_in_its_token_classes() {
        let text =
            "// box\nmodule box(w = 2) { cube([w, 1, $fn]); }\nx = sin(30) + 1.5;\nbox(w = x);";
        let t = tokens(text);
        let class = |s: &str| t.iter().find(|(x, _)| *x == s).map(|(_, c)| *c);
        assert_eq!(class("// box"), Some(Token::Comment));
        assert_eq!(class("module"), Some(Token::Keyword));
        assert_eq!(class("box"), Some(Token::Function));
        assert_eq!(class("cube"), Some(Token::Builtin));
        assert_eq!(class("sin"), Some(Token::Builtin));
        assert_eq!(class("$fn"), Some(Token::Constant));
        assert_eq!(class("1.5"), Some(Token::Number));
        assert_eq!(class("x"), Some(Token::Variable));
        assert!(t.contains(&("w", Token::Parameter)), "{t:?}");
        // Every byte is in one piece, in order.
        let spans = highlight(&parse(text), text);
        assert_eq!(spans.first().unwrap().range.start, 0);
        assert_eq!(spans.last().unwrap().range.end, text.len());
        assert!(spans.windows(2).all(|w| w[0].range.end == w[1].range.start));
    }

    #[test]
    fn what_a_star_disables_is_grey() {
        let text = "*translate([0, 0, -1])\n    cylinder(d = 5, h = 60);\ncube(1);\ntranslate([1, 0, 0]) *sphere(2);\n";
        let t = tokens(text);
        let class = |s: &str| t.iter().find(|(x, _)| x.contains(s)).map(|(_, c)| *c);
        assert_eq!(class("cylinder"), Some(Token::Comment), "{t:?}");
        assert_eq!(class("translate([0"), Some(Token::Comment), "{t:?}");
        assert_eq!(class("cube"), Some(Token::Builtin), "{t:?}");
        assert_eq!(class("sphere"), Some(Token::Comment), "{t:?}");
        // Only what follows it.
        assert!(t.contains(&("translate", Token::Builtin)), "{t:?}");
    }

    #[test]
    fn syntax_errors_say_where() {
        let text = "cube(1);\nsphere(2;\ncylinder(h = 1);";
        let e = errors(&parse(text), text);
        assert!(!e.is_empty());
        let line = text[..e[0].range.start].matches('\n').count();
        assert_eq!(line, 1, "{e:?}");
        assert!(errors(&parse("cube(1);"), "cube(1);").is_empty());
    }
}
