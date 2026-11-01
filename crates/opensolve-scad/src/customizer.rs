//! The parameters OpenSCAD's Customizer shows: the main file's top-level
//! assignments of a plain value, with the comment above as description,
//! a `// [min:max]`, `// [min:step:max]` or `// [a, b:Label]` behind as
//! their range or choices, and `/* [Group] */` before them as their group;
//! those of the group `Hidden` are not shown.

use crate::Error;
use crate::ast::{Elem, Expr, Stmt};
use crate::parser::parse;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Number,
    Text,
    Bool,
    Vector,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Choices {
    Free,
    /// A slider.
    Range {
        min: f64,
        step: Option<f64>,
        max: f64,
    },
    /// Values as OpenSCAD writes them, each with its label.
    List(Vec<(String, String)>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Param {
    pub name: String,
    /// The value as written in the file.
    pub value: String,
    pub kind: Kind,
    pub description: String,
    pub group: String,
    pub choices: Choices,
}

/// The parameters of a file's text, in its order.
pub fn parameters(text: &str) -> Result<Vec<Param>, Error> {
    let stmts = parse(text, 0)?;
    let lines: Vec<&str> = text.lines().collect();
    let groups = groups(&lines);
    let starts: Vec<usize> = std::iter::once(0)
        .chain(text.match_indices('\n').map(|(i, _)| i + 1))
        .collect();
    let mut out: Vec<Param> = Vec::new();
    for s in &stmts {
        let Stmt::Assign {
            name,
            value,
            pos,
            span,
            end_line,
        } = s
        else {
            continue;
        };
        let Some(kind) = kind(value) else {
            continue;
        };
        let group = groups
            .iter()
            .rev()
            .find(|(l, _)| *l < pos.line as usize)
            .map_or(String::new(), |(_, g)| g.clone());
        let line = |n: u32| lines.get(n as usize - 1).copied().unwrap_or("");
        // The comment behind: after the value, on the line it ends on.
        let start = starts.get(*end_line as usize - 1).copied().unwrap_or(0);
        let end = text[start..].find('\n').map_or(text.len(), |e| start + e);
        let behind = text.get(span.1.max(start)..end).unwrap_or("");
        let behind = behind.split_once("//").map_or("", |(_, c)| c.trim());
        let above = if pos.line > 1 {
            line(pos.line - 1).trim()
        } else {
            ""
        };
        let description = above
            .strip_prefix("//")
            .map(str::trim)
            .filter(|d| !d.starts_with('['))
            .unwrap_or("")
            .to_string();
        let param = Param {
            name: name.to_string(),
            value: text[span.0..span.1].to_string(),
            kind,
            description,
            group,
            choices: choices(behind, kind),
        };
        // A name given twice: the last value, at the place of the first.
        match out.iter_mut().find(|p| p.name == param.name) {
            Some(p) => (p.value, p.kind) = (param.value, param.kind),
            None => out.push(param),
        }
    }
    out.retain(|p| !p.group.eq_ignore_ascii_case("hidden"));
    Ok(out)
}

/// The kind of a plain value; `None` for an expression.
fn kind(e: &Expr) -> Option<Kind> {
    match e {
        Expr::Num(_) => Some(Kind::Number),
        Expr::Str(_) => Some(Kind::Text),
        Expr::Bool(_) => Some(Kind::Bool),
        Expr::List(items) => items
            .iter()
            .all(|i| matches!(i, Elem::Expr(Expr::Num(_) | Expr::Str(_) | Expr::Bool(_))))
            .then_some(Kind::Vector),
        _ => None,
    }
}

/// `/* [Name] */` lines: the line (from 1) and the name.
fn groups(lines: &[&str]) -> Vec<(usize, String)> {
    lines
        .iter()
        .enumerate()
        .filter_map(|(i, l)| {
            let inner = l.trim().strip_prefix("/*")?.strip_suffix("*/")?.trim();
            let name = inner.strip_prefix('[')?.strip_suffix(']')?;
            Some((i + 1, name.trim().to_string()))
        })
        .collect()
}

fn choices(comment: &str, kind: Kind) -> Choices {
    let Some(inner) = comment
        .strip_prefix('[')
        .and_then(|c| c.split_once(']'))
        .map(|(c, _)| c.trim())
    else {
        return Choices::Free;
    };
    let numbers: Option<Vec<f64>> = inner.split(':').map(|x| x.trim().parse().ok()).collect();
    if !inner.contains(',')
        && let Some(n) = numbers
    {
        return match n[..] {
            [max] => Choices::Range {
                min: 0.0,
                step: None,
                max,
            },
            [min, max] => Choices::Range {
                min,
                step: None,
                max,
            },
            [min, step, max] => Choices::Range {
                min,
                step: Some(step),
                max,
            },
            _ => Choices::Free,
        };
    }
    let items = inner
        .split(',')
        .map(str::trim)
        .filter(|i| !i.is_empty())
        .map(|i| {
            let (value, label) = i
                .split_once(':')
                .map_or((i, i), |(v, l)| (v.trim(), l.trim()));
            let value = if kind == Kind::Text {
                format!("\"{}\"", value.trim_matches('"'))
            } else {
                value.to_string()
            };
            (value, label.to_string())
        })
        .collect();
    Choices::List(items)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_customizers_parameters() {
        let text = "/* [Size] */\n\
                    // Width of the box\n\
                    width = 40; // [10:100]\n\
                    depth = 30; // [10:5:80]\n\
                    wall = 2;\n\
                    /* [Style] */\n\
                    lid = true;\n\
                    shape = \"round\"; // [round:Round, square:Square]\n\
                    holes = [3, 4]; // [3, 4, 5]\n\
                    derived = width * 2;\n\
                    /* [Hidden] */\n\
                    eps = 0.01;\n\
                    module box() cube([width, depth, 10]);\n\
                    wall = 3;\n";
        let p = parameters(text).unwrap();
        let names: Vec<&str> = p.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["width", "depth", "wall", "lid", "shape", "holes"]);
        assert_eq!(p[0].description, "Width of the box");
        assert_eq!(p[0].group, "Size");
        assert_eq!(
            p[0].choices,
            Choices::Range {
                min: 10.0,
                step: None,
                max: 100.0
            }
        );
        assert_eq!(
            p[1].choices,
            Choices::Range {
                min: 10.0,
                step: Some(5.0),
                max: 80.0
            }
        );
        assert_eq!(p[2].value, "3");
        assert_eq!((p[3].kind, p[3].group.as_str()), (Kind::Bool, "Style"));
        assert_eq!(
            p[4].choices,
            Choices::List(vec![
                ("\"round\"".into(), "Round".into()),
                ("\"square\"".into(), "Square".into())
            ])
        );
        assert_eq!(p[5].kind, Kind::Vector);
    }
}
