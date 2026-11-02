//! What the language server answers about files: diagnostics, completion,
//! definitions, hover, symbols and signatures.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use opensolve_scad_lsp::{CompletionKind, DefKind, External, Severity, Workspace};

/// A workspace whose files on "disk" are `files`, below `/p`.
fn workspace(files: &[(&str, &str)]) -> Workspace {
    let files: HashMap<PathBuf, String> = files
        .iter()
        .map(|(n, t)| (Path::new("/p").join(n), t.to_string()))
        .collect();
    Workspace::new(Box::new(move |p| files.get(p).cloned()))
}

fn main_path() -> &'static Path {
    Path::new("/p/main.scad")
}

/// The byte offset just after the first `marker` in `text`.
fn after(text: &str, marker: &str) -> usize {
    text.find(marker).unwrap() + marker.len()
}

#[test]
fn a_syntax_error_is_found_where_it_is() {
    let text = "cube(1);\ntranslate([1, 0, 0] sphere(2);\n";
    let mut w = workspace(&[]);
    w.set(main_path(), text.to_string());
    let d = w.diagnostics(main_path());
    assert!(!d.is_empty());
    assert!(d.iter().all(|d| d.severity == Severity::Error), "{d:?}");
    let line = text[..d[0].range.start].matches('\n').count();
    assert_eq!(line, 1, "{d:?}");
}

#[test]
fn an_unknown_module_is_marked_at_its_name() {
    let text = "cube(1);\n  frobnicate(2);\necho(\"hi\");\nminkowski() cube(1);\n";
    let mut w = workspace(&[]);
    w.set(main_path(), text.to_string());
    let d = w.diagnostics(main_path());
    let unknown = d
        .iter()
        .find(|d| d.message.contains("unknown module frobnicate"))
        .unwrap_or_else(|| panic!("{d:?}"));
    assert_eq!(unknown.severity, Severity::Warning);
    assert_eq!(&text[unknown.range.clone()], "frobnicate");
    let echo = d.iter().find(|d| d.message.contains("ECHO")).unwrap();
    assert_eq!(echo.severity, Severity::Info);
    assert!(
        d.iter()
            .any(|d| d.message.contains("not supported: minkowski"))
    );
    // A missing include is said at once.
    w.set(main_path(), "include <gone.scad>\ncube(1);".to_string());
    let d = w.syntax_diagnostics(main_path());
    assert!(d.iter().any(|d| d.message.contains("gone.scad")), "{d:?}");
}

#[test]
fn completion_in_a_module_body_offers_user_modules_and_built_ins() {
    let lib = "module rounded(r = 1) { sphere(r); }\nfunction twice(x) = 2 * x;\nhidden = 3;\n";
    let text = "use <lib.scad>\nwidth = 4;\nmodule plate(t) {\n  cu\n}\nmodule peg() {}\n";
    let mut w = workspace(&[("lib.scad", lib)]);
    w.set(main_path(), text.to_string());
    let at = after(text, "  cu");
    let items = w.completion(main_path(), at);
    let has = |label: &str, kind: CompletionKind| {
        items.iter().any(|c| c.label == label && c.kind == kind)
    };
    assert!(has("cube", CompletionKind::Module), "{items:?}");
    assert!(
        !has("cylinder", CompletionKind::Module),
        "filtered by what is typed"
    );
    assert!(
        !has("sphere", CompletionKind::Module),
        "filtered by what is typed"
    );
    assert_eq!(items[0].replace, at - 2..at);
    // With nothing typed: everything the place sees.
    let at = after(text, "  cu") - 2;
    let items = w.completion(main_path(), at);
    let has = |label: &str, kind: CompletionKind| {
        items.iter().any(|c| c.label == label && c.kind == kind)
    };
    assert!(has("rounded", CompletionKind::Module), "from use <>");
    assert!(has("twice", CompletionKind::Function));
    assert!(has("peg", CompletionKind::Module));
    assert!(has("plate", CompletionKind::Module));
    assert!(has("width", CompletionKind::Variable));
    assert!(has("t", CompletionKind::Parameter));
    assert!(has("$fn", CompletionKind::Constant));
    assert!(has("for", CompletionKind::Keyword));
    assert!(
        !items.iter().any(|c| c.label == "hidden"),
        "use <> brings no variables"
    );
    let rounded = items.iter().find(|c| c.label == "rounded").unwrap();
    assert_eq!(rounded.detail, "module rounded(r = 1)");
}

#[test]
fn go_to_definition_across_use_and_include() {
    let lib = "include <deep.scad>\n// A rounded thing.\nmodule rounded(r = 1) { sphere(r); }\n";
    let deep = "function far(x) = x;\n";
    let text = "use <lib.scad>\nrounded(2);\ny = far(1);\nk = 2;\ncube(k);\n";
    let mut w = workspace(&[("lib.scad", lib), ("deep.scad", deep)]);
    w.set(main_path(), text.to_string());
    let at = text.find("rounded(2)").unwrap() + 2;
    let l = w.definition(main_path(), at).expect("rounded");
    assert_eq!(l.path, Path::new("/p/lib.scad"));
    assert_eq!(&lib[l.range], "rounded");
    // What lib.scad includes is part of it for `use`, its functions too.
    let l = w
        .definition(main_path(), text.find("far(").unwrap())
        .unwrap();
    assert_eq!(l.path, Path::new("/p/deep.scad"));
    // A variable in the same file.
    let l = w
        .definition(main_path(), text.find("cube(k").unwrap() + 5)
        .unwrap();
    assert_eq!(l.path, main_path());
    assert_eq!(
        l.range,
        text.find("k = 2").unwrap()..text.find("k = 2").unwrap() + 1
    );
    // The path of `use` opens the file.
    let l = w.definition(main_path(), 7).unwrap();
    assert_eq!(l.path, Path::new("/p/lib.scad"));
    // The comment above is the explanation.
    let h = w
        .hover(main_path(), text.find("rounded(2)").unwrap())
        .unwrap();
    assert_eq!(h.signature, "module rounded(r = 1)");
    assert!(h.doc.starts_with("A rounded thing."), "{}", h.doc);
}

#[test]
fn hover_on_a_built_in_shows_its_signature_and_what_it_does() {
    let text = "cylinder(h = 2, r = 1);\nx = sin(30);\nsphere(r = 1, $fn = 20);\n";
    let mut w = workspace(&[]);
    w.set(main_path(), text.to_string());
    let h = w.hover(main_path(), 3).expect("cylinder");
    assert_eq!(&text[h.range.clone()], "cylinder");
    assert!(h.signature.starts_with("cylinder(h = 1"), "{}", h.signature);
    assert!(h.markdown.contains("cone"), "{}", h.markdown);
    let h = w.hover(main_path(), after(text, "x = si")).unwrap();
    assert_eq!(h.signature, "sin(degrees)");
    let h = w.hover(main_path(), after(text, "$f")).unwrap();
    assert_eq!(h.signature, "$fn");
}

#[test]
fn symbols_and_signatures() {
    let text = "w = 3;\nmodule box(size, center = false) { cube(size, center); }\nfunction f(a, b) = a + b;\nbox([1, 2, 3], );\n";
    let mut w = workspace(&[]);
    w.set(main_path(), text.to_string());
    let s: Vec<_> = w
        .symbols(main_path())
        .into_iter()
        .map(|s| (s.name, s.kind))
        .collect();
    assert_eq!(
        s,
        [
            ("w".to_string(), DefKind::Variable),
            ("box".to_string(), DefKind::Module),
            ("f".to_string(), DefKind::Function)
        ]
    );
    let sig = w
        .signature(main_path(), after(text, "box([1, 2, 3], "))
        .unwrap();
    assert_eq!(sig.label, "box(size, center = false)");
    assert_eq!(sig.parameters, ["size", "center = false"]);
    assert_eq!(sig.active, 1);
    let sig = w.signature(main_path(), after(text, "cube(")).unwrap();
    assert_eq!(sig.active, 0);
    assert!(sig.label.starts_with("cube("));
}

#[test]
fn in_a_call_the_parameters_are_offered_and_no_modules() {
    let text = "module peg(h = 2, d) {}\ncube(4);\ncylinder(d\npeg(h = 3, \n";
    let mut w = workspace(&[]);
    w.set(main_path(), text.to_string());
    let at = after(text, "cylinder(d");
    let items = w.completion(main_path(), at);
    assert_eq!(items[0].label, "d", "{items:?}");
    assert_eq!(items[0].kind, CompletionKind::Parameter);
    assert_eq!(items[0].insert, "d = ");
    assert!(items[0].doc.contains("diameter"), "{items:?}");
    assert_eq!(items[0].replace, at - 1..at);
    assert!(items.iter().any(|c| c.label == "d1"));
    assert!(
        !items.iter().any(|c| c.label == "difference"),
        "a module is no value: {items:?}"
    );
    // A user module's, without the one given by name already.
    let at = after(text, "peg(h = 3, ");
    let items = w.completion(main_path(), at);
    let params: Vec<&str> = items
        .iter()
        .filter(|c| c.kind == CompletionKind::Parameter)
        .map(|c| c.label.as_str())
        .collect();
    assert_eq!(params, ["d"], "{items:?}");
    // After `=` a value goes: no names.
    let text = "cylinder(h = ";
    w.set(main_path(), text.to_string());
    let items = w.completion(main_path(), text.len());
    assert!(!items.iter().any(|c| c.kind == CompletionKind::Parameter));
}

#[test]
fn variables_given_from_outside_are_known_completed_and_shown() {
    // `wall` is given by the model; `size` is the file's own and given too.
    let text = "size = 4;\ncube([size, wa, wall]);\nmodule m(wall) { cube(wall); }\n";
    let mut w = workspace(&[]);
    w.set(main_path(), text.to_string());
    let unknown = |w: &Workspace| {
        w.diagnostics(main_path())
            .into_iter()
            .filter(|d| d.message.contains("unknown variable"))
            .map(|d| d.message)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        unknown(&w),
        ["unknown variable wa", "unknown variable wall"]
    );
    let given = |name: &str, value: &str| External {
        name: name.into(),
        value: value.into(),
        detail: format!("model parameter = {value}"),
        doc: format!("Model parameter `{name} = ring / 2`"),
    };
    assert!(w.set_external(main_path(), vec![given("wall", "73"), given("size", "9")]));
    assert!(!w.set_external(main_path(), vec![given("wall", "73"), given("size", "9")]));
    assert_eq!(unknown(&w), ["unknown variable wa"]);
    // Completion offers it as a variable, with its value.
    let items = w.completion(main_path(), after(text, "size, wa"));
    let wall = items.iter().find(|c| c.label == "wall").unwrap();
    assert_eq!(wall.kind, CompletionKind::Variable);
    assert_eq!(wall.detail, "model parameter = 73");
    // The given value wins over the file's own.
    let items = w.completion(main_path(), after(text, "cube([si"));
    let size: Vec<_> = items.iter().filter(|c| c.label == "size").collect();
    assert_eq!(size.len(), 1, "{items:?}");
    assert_eq!(size[0].detail, "model parameter = 9");
    // Hover shows its value and expression; not on a parameter of a module.
    let h = w.hover(main_path(), after(text, "size, wa, wa")).unwrap();
    assert_eq!(h.signature, "wall = 73");
    assert!(h.markdown.contains("ring / 2"), "{}", h.markdown);
    let h = w.hover(main_path(), after(text, "cube(wa"));
    assert!(h.is_none_or(|h| h.signature != "wall = 73"));
    // None given: unknown again.
    assert!(w.set_external(main_path(), Vec::new()));
    assert_eq!(unknown(&w).len(), 2);
}

#[test]
fn what_a_command_click_goes_to_is_a_link() {
    let lib = "module rounded(r = 1) { sphere(r); }\n";
    let text = "use <lib.scad>\nrounded(2);\ncube(1);\nuse <gone.scad>\n";
    let mut w = workspace(&[("lib.scad", lib)]);
    w.set(main_path(), text.to_string());
    let link = |at: usize| w.link_at(main_path(), at).map(|r| &text[r]);
    assert_eq!(link(text.find("rounded").unwrap() + 3), Some("rounded"));
    assert_eq!(link(text.find("lib.scad").unwrap()), Some("<lib.scad>"));
    // A built-in and a file not there go nowhere.
    assert_eq!(link(text.find("cube").unwrap() + 1), None);
    assert_eq!(link(text.find("gone").unwrap()), None);
}
