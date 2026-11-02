//! OpenSCAD's language: what `echo` says after a file is evaluated, as
//! OpenSCAD 2021.01 says it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use opensolve_scad::{Output, build};

/// Builds `main.scad` among `files`, with `overrides`.
fn run_with(files: &[(&str, &str)], overrides: &[(&str, &str)]) -> Result<Output, String> {
    let files: HashMap<PathBuf, String> = files
        .iter()
        .map(|(n, t)| (Path::new("/p").join(n), t.to_string()))
        .collect();
    let read = |p: &Path| {
        files
            .get(p)
            .map(|t| t.as_bytes().to_vec())
            .ok_or_else(|| format!("{} not found", p.display()))
    };
    let overrides: Vec<(String, String)> = overrides
        .iter()
        .map(|(n, v)| (n.to_string(), v.to_string()))
        .collect();
    build(Path::new("/p/main.scad"), &read, &overrides)
}

/// The echo lines of a file that also makes a cube.
fn echoes(text: &str) -> Vec<String> {
    let out = run_with(&[("main.scad", &format!("{text}\ncube(1);"))], &[]).unwrap();
    out.notes
        .into_iter()
        .filter_map(|n| n.strip_prefix("ECHO: ").map(str::to_string))
        .collect()
}

fn echo(text: &str) -> String {
    let e = echoes(text);
    assert_eq!(e.len(), 1, "{e:?}");
    e[0].clone()
}

#[test]
fn values_and_operators() {
    assert_eq!(
        echo("echo(1 + 2 * 3, 2 ^ 3, 7 % 3, -2 ^ 2);"),
        "7, 8, 1, -4"
    );
    assert_eq!(echo("echo(1 / 3, 1e6, 0.1 + 0.2);"), "0.333333, 1e+06, 0.3");
    assert_eq!(
        echo("echo([1, 2] + [3, 4], [1, 2, 3] * 2, -[1, 2]);"),
        "[4, 6], [2, 4, 6], [-1, -2]"
    );
    assert_eq!(echo("echo([1, 2, 3] * [4, 5, 6]);"), "32");
    assert_eq!(
        echo("echo(1 < 2, \"a\" < \"b\", 1 == 1.0, [1] == [1], undef == undef);"),
        "true, true, true, true, true"
    );
    assert_eq!(
        echo("echo(true && 0, false || \"x\", !undef, 1 + \"a\");"),
        "false, true, true, undef"
    );
    assert_eq!(echo("echo(1 > 0 ? \"yes\" : \"no\");"), "\"yes\"");
    assert_eq!(
        echo("v = [10, 20, 30]; echo(v[1], v.z, v[5], \"abc\"[1]);"),
        "20, 30, undef, \"b\""
    );
}

#[test]
fn the_last_assignment_wins_at_the_place_of_the_first() {
    // As OpenSCAD: `a` is 2 everywhere, `b` sees it.
    assert_eq!(echo("a = 1; b = a + 1; a = 2; echo(a, b);"), "2, 3");
}

#[test]
fn functions_with_defaults_names_and_recursion() {
    let text = "function f(a, b = 2, c = 3) = a + 10 * b + 100 * c;\n\
                function fact(n) = n <= 1 ? 1 : n * fact(n - 1);\n\
                function sum(v, i = 0) = i >= len(v) ? 0 : v[i] + sum(v, i + 1);\n\
                echo(f(1), f(1, c = 5), f(b = 0, a = 4), fact(10), sum([for (i = [1:1000]) i]));";
    assert_eq!(echo(text), "321, 521, 304, 3.6288e+06, 500500");
}

#[test]
fn function_literals_and_let() {
    let text = "add = function (a) function (b) a + b;\n\
                twice = function (f, x) f(f(x));\n\
                echo(add(2)(3), twice(function (x) x * 3, 2), let (a = 2, b = a * 3) a + b);";
    assert_eq!(echo(text), "5, 18, 8");
}

#[test]
fn list_comprehensions() {
    assert_eq!(
        echo("echo([for (i = [0:4]) if (i % 2 == 0) i * i]);"),
        "[0, 4, 16]"
    );
    assert_eq!(
        echo("echo([for (i = [1:2], j = [0:1]) [i, j]]);"),
        "[[1, 0], [1, 1], [2, 0], [2, 1]]"
    );
    assert_eq!(
        echo("echo([for (i = 0, s = 0; i < 4; i = i + 1, s = s + i) s]);"),
        "[0, 1, 3, 6]"
    );
    assert_eq!(
        echo("echo([each [1, 2], each [5:7], for (x = [1, 2]) let (y = x * 10) y]);"),
        "[1, 2, 5, 6, 7, 10, 20]"
    );
    assert_eq!(
        echo("echo([for (i = [0:3]) if (i < 2) \"a\" else \"b\"]);"),
        "[\"a\", \"a\", \"b\", \"b\"]"
    );
    assert_eq!(
        echo("echo([0:2:7], [for (x = [5:-2:0]) x], [for (x = [3:1]) x]);"),
        "[0 : 2 : 7], [5, 3, 1], [1, 2, 3]"
    );
}

#[test]
fn built_in_functions() {
    assert_eq!(
        echo("echo(sin(30), cos(60), tan(45), atan2(1, 0), asin(1));"),
        "0.5, 0.5, 1, 90, 90"
    );
    assert_eq!(
        echo("echo(abs(-3), sign(-2), floor(2.7), ceil(2.1), round(2.5), sqrt(16), pow(2, 10));"),
        "3, -1, 2, 3, 3, 4, 1024"
    );
    assert_eq!(
        echo("echo(min(3, 1, 2), max([4, 9, 2]), norm([3, 4]), cross([1, 0, 0], [0, 1, 0]));"),
        "1, 9, 5, [0, 0, 1]"
    );
    assert_eq!(
        echo("echo(len([1, 2, 3]), len(\"hello\"), concat([1], [2, 3], 4));"),
        "3, 5, [1, 2, 3, 4]"
    );
    assert_eq!(
        echo("echo(str(\"w=\", 2.5, [1, \"a\"]), chr(65), ord(\"a\"));"),
        "\"w=2.5[1, \\\"a\\\"]\", \"A\", 97"
    );
    assert_eq!(
        echo("echo(lookup(15, [[10, 1], [20, 3]]), search(\"b\", \"abc\"), search(2, [1, 2, 3]));"),
        "2, [1], [1]"
    );
    assert_eq!(
        echo("echo(is_num(1), is_string(1), is_list([]), is_undef(undef), is_bool(true));"),
        "true, false, true, true, true"
    );
    assert_eq!(
        echo("echo(exp(0), ln(1), log(100), PI > 3.14);"),
        "1, 0, 2, true"
    );
}

#[test]
fn modules_children_and_special_variables() {
    let text = "module m(n = 1) { echo(n = n, kids = $children, fn = $fn); children(); }\n\
                module outer() { $fn = 7; m(2) inner(); }\n\
                module inner() echo(inner_fn = $fn);\n\
                outer();";
    assert_eq!(echoes(text), ["n = 2, kids = 1, fn = 7", "inner_fn = 7"]);
    // A `$` argument goes to what is called, and on to its callees.
    let text = "module a() b(); module b() echo($fn);\na($fn = 12); a();";
    assert_eq!(echoes(text), ["12", "0"]);
    // Plain names are found where they are written, not where called.
    let text = "x = 1; module show() echo(x); module wrap() { x = 2; show(); } wrap();";
    assert_eq!(echoes(text), ["1"]);
}

#[test]
fn children_by_index() {
    let text = "module pick() { children(1); echo($children); }\n\
                pick() { echo(\"a\"); echo(\"b\"); echo(\"c\"); }";
    assert_eq!(echoes(text), ["\"b\"", "3"]);
}

#[test]
fn for_if_let_and_echo_as_modules() {
    let text = "for (i = [0:1], j = [5, 6]) echo(i, j);\n\
                if (1 > 2) echo(\"no\"); else echo(\"else\");\n\
                let (k = 3) echo(k);";
    assert_eq!(
        echoes(text),
        ["0, 5", "0, 6", "1, 5", "1, 6", "\"else\"", "3"]
    );
}

#[test]
fn an_assertion_stops_with_its_message() {
    let e = run_with(
        &[(
            "main.scad",
            "w = 1;\nassert(w > 2, \"too thin\");\ncube(w);",
        )],
        &[],
    )
    .unwrap_err();
    assert!(e.contains("line 2") && e.contains("too thin"), "{e}");
    let e = run_with(
        &[(
            "main.scad",
            "function f(x) = assert(x > 0) x;\ncube(f(-1));",
        )],
        &[],
    )
    .unwrap_err();
    assert!(e.contains("assertion failed"), "{e}");
}

#[test]
fn errors_and_notes_say_where() {
    let e = run_with(&[("main.scad", "cube(1);\nsphere(2\n")], &[]).unwrap_err();
    assert!(e.starts_with("line 3, column 1:"), "{e}");
    let out = run_with(
        &[(
            "main.scad",
            "cube(size);\nprojection() { cube(1); sphere(1); }\nfoo();",
        )],
        &[],
    )
    .unwrap();
    assert!(
        out.notes
            .contains(&"line 1: unknown variable size".to_string()),
        "{:?}",
        out.notes
    );
    assert!(
        out.notes
            .contains(&"line 2: not supported: projection".to_string())
    );
    assert!(
        out.notes
            .contains(&"line 3: unknown module foo".to_string())
    );
}

#[test]
fn endless_recursion_is_an_error() {
    let e = run_with(
        &[("main.scad", "function f(x) = f(x + 1);\ncube(f(0));")],
        &[],
    )
    .unwrap_err();
    assert!(e.contains("too deep"), "{e}");
}

#[test]
fn include_and_use_from_the_files_folder() {
    let files = [
        (
            "main.scad",
            "include <lib/sizes.scad>\nuse <lib/parts.scad>\npeg(); echo(size, double(size), hidden);",
        ),
        ("lib/sizes.scad", "size = 4;\ninclude <more.scad>"),
        ("lib/more.scad", "extra = 1;"),
        (
            "lib/parts.scad",
            "hidden = 9;\nfunction double(x) = 2 * x;\nmodule peg() cube(hidden);\ncube(100);",
        ),
    ];
    let out = run_with(&files, &[]).unwrap();
    // `use` brings modules and functions, not variables, and nothing made.
    assert!(
        out.notes.contains(&"ECHO: 4, 8, undef".to_string()),
        "{:?}",
        out.notes
    );
    assert!((out.solid.volume() - 729.0).abs() < 1e-6);
    let e = run_with(&[("main.scad", "include <gone.scad>\ncube(1);")], &[]).unwrap_err();
    assert!(e.contains("gone.scad"), "{e}");
}

#[test]
fn overrides_set_top_level_variables_as_d_does() {
    let text = "w = 2; // [1:10]\nh = w * 2;\ncube([w, w, h]);";
    let v = |o: &[(&str, &str)]| run_with(&[("main.scad", text)], o).unwrap().solid.volume();
    assert!((v(&[]) - 16.0).abs() < 1e-6);
    assert!((v(&[("w", "3")]) - 54.0).abs() < 1e-6);
    // A value that does not parse is noted and left out.
    let out = run_with(&[("main.scad", text)], &[("w", "3 +")]).unwrap();
    assert!((out.solid.volume() - 16.0).abs() < 1e-6);
    assert!(
        out.notes.iter().any(|n| n.starts_with("w = 3 +")),
        "{:?}",
        out.notes
    );
}

#[test]
fn only_2d_or_nothing_is_said() {
    let e = run_with(&[("main.scad", "square(2);")], &[]).unwrap_err();
    assert!(e.contains("2D"), "{e}");
    let e = run_with(&[("main.scad", "x = 1;")], &[]).unwrap_err();
    assert!(e.contains("no solid"), "{e}");
}

#[test]
fn a_diagnosis_says_where() {
    let files: HashMap<PathBuf, String> = [
        (
            "/p/main.scad",
            "use <lib.scad>\nx = 1;\n  nothing(2);\necho(x);\nlib();\nassert(x > 2, \"small\");",
        ),
        ("/p/lib.scad", "module lib() { gone(); }"),
    ]
    .into_iter()
    .map(|(n, t)| (PathBuf::from(n), t.to_string()))
    .collect();
    let read = |p: &Path| {
        files
            .get(p)
            .map(|t| t.as_bytes().to_vec())
            .ok_or_else(|| format!("{} not found", p.display()))
    };
    let d = opensolve_scad::diagnose(Path::new("/p/main.scad"), &read, &[]);
    let at = |m: &str| d.notes.iter().find(|n| n.message.contains(m)).cloned();
    let unknown = at("unknown module nothing").expect("unknown module");
    assert_eq!((unknown.file, unknown.line, unknown.col), (None, 3, 3));
    let echo = at("ECHO: 1").expect("echo");
    assert!(echo.echo);
    assert_eq!(echo.line, 4);
    let gone = at("unknown module gone").expect("in the library");
    assert_eq!(gone.file, Some(PathBuf::from("/p/lib.scad")));
    let e = d.error.expect("the assertion");
    assert_eq!(e.line, 6);
    assert!(e.message.contains("small"), "{}", e.message);
}
