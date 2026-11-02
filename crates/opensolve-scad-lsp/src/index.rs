//! What a file defines, read off its syntax tree: its modules, functions
//! and variables at the top, what it includes and uses, and the names a
//! place inside it sees (parameters, `let`, `for`, a block's own).

use std::ops::Range;

use tree_sitter::{Node, Tree};

/// What a name is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DefKind {
    Module,
    Function,
    Variable,
    Parameter,
}

/// A name defined in a file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Def {
    pub name: String,
    pub kind: DefKind,
    /// The name where it is defined.
    pub name_range: Range<usize>,
    /// The whole definition.
    pub range: Range<usize>,
    /// `module box(w = 2)`, `function f(x)`, `w = 2`.
    pub signature: String,
    /// The comment above it (and a variable's behind it, the Customizer's).
    pub doc: String,
}

/// An `include <…>` or `use <…>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Import {
    /// The path as written, without `<` and `>`.
    pub path: String,
    /// The path in the text, with `<` and `>`.
    pub range: Range<usize>,
    pub include: bool,
}

/// What a file defines at its top level.
#[derive(Clone, Debug, Default)]
pub struct FileIndex {
    pub defs: Vec<Def>,
    pub imports: Vec<Import>,
}

pub fn index(tree: &Tree, text: &str) -> FileIndex {
    let mut out = FileIndex::default();
    let root = tree.root_node();
    let mut cursor = root.walk();
    for node in root.named_children(&mut cursor) {
        match node.kind() {
            "include_statement" | "use_statement" => {
                if let Some(p) = child_of_kind(node, "include_path") {
                    let raw = &text[p.byte_range()];
                    let path = raw.trim_start_matches('<').trim_end_matches('>');
                    out.imports.push(Import {
                        path: path.trim().to_string(),
                        range: p.byte_range(),
                        include: node.kind() == "include_statement",
                    });
                }
            }
            _ => out.defs.extend(item_defs(node, text)),
        }
    }
    // A library's documentation blocks, as BOSL2 writes them, say more than
    // the comment just above a definition.
    if text.contains("// Module: ") || text.contains("// Function") {
        let docs = library_docs(text);
        for d in &mut out.defs {
            if let Some(doc) = docs.get(&d.name) {
                d.doc.clone_from(doc);
            }
        }
    }
    out
}

/// The heads of BOSL2's documentation blocks, and what the names after
/// them are.
const HEADS: [&str; 5] = [
    "// Function&Module: ",
    "// Module: ",
    "// Function: ",
    "// Constant: ",
    "// Constants: ",
];

/// The documentation blocks of a library in BOSL2's way, as Markdown by
/// the names they document:
///
/// ```text
/// // Module: cuboid()
/// // Synopsis: Creates a cube with chamfering and roundovers.
/// // Usage: Standard Cubes
/// //   cuboid(size, [anchor=], [spin=], [orient=]);
/// // Description:
/// //   Creates a cube or cuboid object, …
/// // Arguments:
/// //   size = The size of the cube, a number or length 3 vector.
/// ```
///
/// The synopsis, the usages as code, the description and the arguments
/// as a list; not the examples, topics and the like.
pub fn library_docs(text: &str) -> std::collections::HashMap<String, String> {
    let mut out = std::collections::HashMap::new();
    let lines: Vec<&str> = text.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let Some(head) = HEADS.iter().find(|h| lines[i].starts_with(**h)) else {
            i += 1;
            continue;
        };
        let names: Vec<String> = lines[i][head.len()..]
            .split(',')
            .map(|n| n.split('(').next().unwrap_or("").trim().to_string())
            .filter(|n| !n.is_empty() && !n.contains(' '))
            .collect();
        i += 1;
        // Sections: a key at the start (`// Usage: Title`) and the lines
        // indented under it.
        let mut sections: Vec<(String, String, Vec<String>)> = Vec::new();
        while i < lines.len() && lines[i].starts_with("//") {
            let body = &lines[i][2..];
            let indented = body.starts_with("  ") || body.trim().is_empty();
            if !indented && let Some((key, rest)) = body.trim().split_once(':') {
                sections.push((key.trim().to_string(), rest.trim().to_string(), Vec::new()));
            } else if let Some(s) = sections.last_mut() {
                s.2.push(
                    body.strip_prefix("   ")
                        .unwrap_or(body.trim_start())
                        .to_string(),
                );
            }
            i += 1;
        }
        let md = render_doc(&sections);
        for n in names {
            out.entry(n).or_insert_with(|| md.clone());
        }
    }
    out
}

/// BOSL2's `{{name()}}` links as code.
fn unlink(s: &str) -> String {
    s.replace("{{", "`").replace("}}", "`")
}

fn render_doc(sections: &[(String, String, Vec<String>)]) -> String {
    let mut md = String::new();
    let push = |md: &mut String, part: &str| {
        if !part.trim().is_empty() {
            if !md.is_empty() {
                md.push_str("\n\n");
            }
            md.push_str(part.trim_end());
        }
    };
    for (key, title, _) in sections {
        if key == "Synopsis" {
            push(&mut md, &unlink(title));
        }
    }
    let usages: Vec<&(String, String, Vec<String>)> =
        sections.iter().filter(|s| s.0 == "Usage").collect();
    if !usages.is_empty() {
        let mut code = String::from("```openscad\n");
        for (_, title, lines) in &usages {
            if !title.is_empty() && usages.len() > 1 {
                code.push_str(&format!("// {title}\n"));
            }
            for l in lines.iter().filter(|l| !l.trim().is_empty()) {
                code.push_str(l.trim_end());
                code.push('\n');
            }
        }
        code.push_str("```");
        push(&mut md, &code);
    }
    for (key, title, lines) in sections {
        match key.as_str() {
            "Description" => {
                // `.` alone parts paragraphs.
                let mut text = unlink(title);
                for l in lines {
                    let t = l.trim();
                    if t == "." || t.is_empty() {
                        text.push_str("\n\n");
                    } else {
                        if !text.is_empty() && !text.ends_with('\n') {
                            text.push(' ');
                        }
                        text.push_str(&unlink(t));
                    }
                }
                push(&mut md, &text);
            }
            "Arguments" => {
                let mut list = String::new();
                for l in lines {
                    let t = l.trim();
                    if t.is_empty() || t.starts_with("---") {
                        continue;
                    }
                    match t.split_once(" = ") {
                        Some((name, what)) if !name.contains(' ') => {
                            if !list.is_empty() {
                                list.push('\n');
                            }
                            list.push_str(&format!("- `{name}`: {}", unlink(what)));
                        }
                        _ => {
                            list.push(' ');
                            list.push_str(&unlink(t));
                        }
                    }
                }
                if !list.is_empty() {
                    push(&mut md, &format!("**Arguments**\n\n{list}"));
                }
            }
            _ => {}
        }
    }
    md
}

/// The definitions of an item of a block: a module, a function or a
/// variable.
fn item_defs(node: Node<'_>, text: &str) -> Option<Def> {
    match node.kind() {
        "module_item" | "function_item" => {
            let name = node.child_by_field_name("name")?;
            let params = node
                .child_by_field_name("parameters")
                .map_or("()".to_string(), |p| squeeze(&text[p.byte_range()]));
            let (kind, word) = if node.kind() == "module_item" {
                (DefKind::Module, "module")
            } else {
                (DefKind::Function, "function")
            };
            let name_text = text[name.byte_range()].to_string();
            Some(Def {
                signature: format!("{word} {name_text}{params}"),
                name: name_text,
                kind,
                name_range: name.byte_range(),
                range: node.byte_range(),
                doc: doc_above(node, text),
            })
        }
        "var_declaration" => {
            let a = child_of_kind(node, "assignment")?;
            let mut d = assignment_def(a, text, DefKind::Variable)?;
            d.range = node.byte_range();
            let mut doc = doc_above(node, text);
            if let Some(behind) = comment_behind(node, text) {
                if !doc.is_empty() {
                    doc.push_str("\n\n");
                }
                doc.push_str(&behind);
            }
            d.doc = doc;
            Some(d)
        }
        _ => None,
    }
}

fn assignment_def(a: Node<'_>, text: &str, kind: DefKind) -> Option<Def> {
    let name = a.child_by_field_name("name")?;
    let value = a
        .child_by_field_name("value")
        .map_or(String::new(), |v| squeeze(&text[v.byte_range()]));
    let name_text = text[name.byte_range()].to_string();
    let value = if value.chars().count() > 60 {
        let cut: String = value.chars().take(59).collect();
        format!("{cut}…")
    } else {
        value
    };
    Some(Def {
        signature: if value.is_empty() {
            name_text.clone()
        } else {
            format!("{name_text} = {value}")
        },
        name: name_text,
        kind,
        name_range: name.byte_range(),
        range: a.byte_range(),
        doc: String::new(),
    })
}

/// The names a parameter list defines.
fn parameter_defs(params: Node<'_>, text: &str, out: &mut Vec<Def>) {
    let mut cursor = params.walk();
    for p in params.named_children(&mut cursor) {
        if p.kind() != "parameter" {
            continue;
        }
        let Some(inner) = p.named_child(0) else {
            continue;
        };
        let def = match inner.kind() {
            "assignment" => assignment_def(inner, text, DefKind::Parameter),
            "identifier" | "special_variable" => Some(Def {
                name: text[inner.byte_range()].to_string(),
                kind: DefKind::Parameter,
                name_range: inner.byte_range(),
                range: inner.byte_range(),
                signature: text[inner.byte_range()].to_string(),
                doc: String::new(),
            }),
            _ => None,
        };
        out.extend(def);
    }
}

/// The names of an `assignments` list: `(i = [0:3], j = 1)`.
fn assignments_defs(list: Node<'_>, text: &str, out: &mut Vec<Def>) {
    let mut cursor = list.walk();
    for a in list.named_children(&mut cursor) {
        if a.kind() == "assignment" {
            out.extend(assignment_def(a, text, DefKind::Variable));
        }
    }
}

/// The names the place `offset` sees besides the file's top level, the
/// innermost first: parameters of the module or function it is in, names
/// of `let` and `for`, the variables, modules and functions of the blocks
/// around it.
pub fn locals(tree: &Tree, text: &str, offset: usize) -> Vec<Def> {
    let mut out = Vec::new();
    let root = tree.root_node();
    let Some(mut node) = root.descendant_for_byte_range(offset, offset) else {
        return out;
    };
    loop {
        match node.kind() {
            "module_item" | "function_item" | "function_lit" => {
                if let Some(p) = node.child_by_field_name("parameters")
                    && !p.byte_range().contains(&offset)
                {
                    parameter_defs(p, text, &mut out);
                }
            }
            "union_block" => {
                let mut cursor = node.walk();
                for item in node.named_children(&mut cursor) {
                    out.extend(item_defs(item, text));
                }
            }
            _ => {}
        }
        // `for (i = …)`, `let (…)`, `[for (…) …]`, `let (…) each …`.
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            match child.kind() {
                "assignments" => assignments_defs(child, text, &mut out),
                "let_prefix" => {
                    if let Some(list) = child_of_kind(child, "assignments") {
                        assignments_defs(list, text, &mut out);
                    }
                }
                "condition_update_clause" => {
                    let mut c = child.walk();
                    for a in child.named_children(&mut c) {
                        if a.kind() == "assignment" {
                            out.extend(assignment_def(a, text, DefKind::Variable));
                        }
                    }
                }
                _ => {}
            }
        }
        match node.parent() {
            Some(p) if p.id() != root.id() => node = p,
            _ => break,
        }
    }
    out
}

fn child_of_kind<'t>(node: Node<'t>, kind: &str) -> Option<Node<'t>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor).find(|c| c.kind() == kind)
}

fn is_comment(n: &Node<'_>) -> bool {
    matches!(n.kind(), "line_comment" | "block_comment")
}

/// The comments right above a node, without blank lines between.
fn doc_above(node: Node<'_>, text: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    let mut row = node.start_position().row;
    let mut at = node.prev_sibling();
    while let Some(c) = at {
        if !is_comment(&c) || c.end_position().row + 1 < row {
            break;
        }
        // A comment behind the item before belongs to that.
        if let Some(before) = c.prev_sibling()
            && !is_comment(&before)
            && before.end_position().row == c.start_position().row
        {
            break;
        }
        lines.insert(0, uncomment(&text[c.byte_range()]));
        row = c.start_position().row;
        at = c.prev_sibling();
    }
    lines.join("\n").trim().to_string()
}

/// The comment behind a node on its last line.
fn comment_behind(node: Node<'_>, text: &str) -> Option<String> {
    let c = node.next_sibling()?;
    (is_comment(&c) && c.start_position().row == node.end_position().row)
        .then(|| uncomment(&text[c.byte_range()]))
}

/// A comment's words without its marks.
fn uncomment(c: &str) -> String {
    if let Some(line) = c.strip_prefix("//") {
        return line.trim().to_string();
    }
    let inner = c
        .trim_start_matches("/*")
        .trim_start_matches('*')
        .trim_end_matches("*/");
    inner
        .lines()
        .map(|l| l.trim().trim_start_matches('*').trim())
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

/// Whitespace, line breaks included, as one space.
fn squeeze(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::parse;

    #[test]
    fn top_level_definitions_with_their_comments() {
        let text = "include <a.scad>\nuse <lib/b.scad>\n// The width.\nw = 40; // [10:100]\n/** A box. */\nmodule box(size = [1, 2], center) { cube(size); }\nfunction twice(x) = 2 * x;\n";
        let i = index(&parse(text), text);
        let names: Vec<_> = i.defs.iter().map(|d| (d.name.as_str(), d.kind)).collect();
        assert_eq!(
            names,
            [
                ("w", DefKind::Variable),
                ("box", DefKind::Module),
                ("twice", DefKind::Function)
            ]
        );
        assert_eq!(i.defs[0].doc, "The width.\n\n[10:100]");
        assert_eq!(i.defs[1].doc, "A box.");
        assert_eq!(i.defs[1].signature, "module box(size = [1, 2], center)");
        assert_eq!(i.defs[2].signature, "function twice(x)");
        assert_eq!(i.imports.len(), 2);
        assert_eq!(i.imports[1].path, "lib/b.scad");
        assert!(!i.imports[1].include);
    }

    #[test]
    fn a_place_sees_parameters_lets_and_loops() {
        let text = "module m(a, b = 1) {\n  inner = 2;\n  for (i = [0:3]) let (j = i) translate([i, j, 0]) cube(a);\n}\n";
        let tree = parse(text);
        let at = text.find("cube(a").unwrap() + 5;
        let names: Vec<String> = locals(&tree, text, at)
            .into_iter()
            .map(|d| d.name)
            .collect();
        for n in ["a", "b", "inner", "i", "j"] {
            assert!(names.contains(&n.to_string()), "{n} in {names:?}");
        }
    }
}
