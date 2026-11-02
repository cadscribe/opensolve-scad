//! A file that includes BOSL2 (the copy that comes with the source):
//! no unknown names, BOSL2's documentation on hover and in completion,
//! its files reached by going to a definition.

use std::path::{Path, PathBuf};

use opensolve_scad_lsp::{CompletionKind, Severity, Workspace};

fn library() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../opensolve-scad/library")
}

const TEXT: &str = "include <BOSL2/std.scad>\n\
                    diff() cuboid([40, 30, 10], rounding = 2, edges = \"Z\")\n\
                    \x20 attach(TOP) tag(\"remove\") cyl(d = 6, h = 12, anchor = TOP);\n";

fn workspace() -> (Workspace, PathBuf) {
    opensolve_scad::library::set_dirs(Some(vec![library()]));
    let main = Path::new("/nowhere/part.scad").to_path_buf();
    let mut w = Workspace::default();
    w.set(&main, TEXT.to_string());
    (w, main)
}

#[test]
fn bosl2_names_are_known() {
    let (w, main) = workspace();
    let d = w.diagnostics(&main);
    assert!(
        d.iter().all(|d| d.severity == Severity::Info),
        "{:?}",
        d.iter().map(|d| &d.message).collect::<Vec<_>>()
    );
}

#[test]
fn hover_shows_bosl2s_documentation() {
    let (w, main) = workspace();
    let h = w.hover(&main, TEXT.find("cuboid").unwrap() + 2).unwrap();
    assert!(h.signature.starts_with("module cuboid("), "{}", h.signature);
    assert!(
        h.doc
            .starts_with("Creates a cube with chamfering and roundovers."),
        "{}",
        h.doc
    );
    assert!(h.doc.contains("```openscad\n"), "{}", h.doc);
    assert!(h.doc.contains("- `size`: "), "{}", h.doc);
    assert!(h.doc.contains("*shapes3d.scad*"), "{}", h.doc);
    // A function and module documented as one, and a constant.
    let h = w.hover(&main, TEXT.find("cyl(").unwrap() + 1).unwrap();
    assert!(h.doc.contains("cylinder"), "{}", h.doc);
    let h = w.hover(&main, TEXT.find("TOP").unwrap() + 1).unwrap();
    assert!(!h.doc.is_empty(), "{}", h.signature);
}

#[test]
fn definitions_go_into_the_library() {
    let (w, main) = workspace();
    let l = w
        .definition(&main, TEXT.find("cuboid").unwrap() + 2)
        .unwrap();
    assert!(
        l.path.ends_with("BOSL2/shapes3d.scad"),
        "{}",
        l.path.display()
    );
    let text = std::fs::read_to_string(&l.path).unwrap();
    assert_eq!(&text[l.range.clone()], "cuboid");
    // The path of the include.
    let l = w.definition(&main, TEXT.find("std.scad").unwrap()).unwrap();
    assert!(l.path.ends_with("BOSL2/std.scad"), "{}", l.path.display());
}

#[test]
fn completion_offers_bosl2_but_not_its_inner_names() {
    let (mut w, main) = workspace();
    let text = format!("{TEXT}cubo");
    w.set(&main, text.clone());
    let c = w.completion(&main, text.len());
    let c = c.iter().find(|c| c.label == "cuboid").unwrap();
    assert_eq!(c.kind, CompletionKind::Module);
    let text = format!("{TEXT}_cut");
    w.set(&main, text.clone());
    assert!(
        w.completion(&main, text.len())
            .iter()
            .any(|c| c.label.starts_with("_cut"))
    );
    let text = format!("{TEXT}cut");
    w.set(&main, text.clone());
    assert!(
        !w.completion(&main, text.len())
            .iter()
            .any(|c| c.label.starts_with('_'))
    );
    // Files of the library folders after `include <`.
    let text = format!("{TEXT}include <BOSL2/gea");
    w.set(&main, text.clone());
    let c = w.completion(&main, text.len());
    assert!(c.iter().any(|c| c.label == "gears.scad"), "{c:?}");
}
