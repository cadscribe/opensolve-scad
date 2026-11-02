//! BOSL2's test files (`tests/bosl2/*.scadtest`): a `[[test]]` per test,
//! its `name` and its `script` in `'''`. Each script is run as if it lay
//! in BOSL2's `tests` folder, as its `include <../std.scad>` wants.

#![allow(dead_code)]

use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct Test {
    /// The test file, without `test_` and `.scadtest`.
    pub file: String,
    pub name: String,
    pub script: String,
    /// It is to fail (`expect_success = false`).
    pub fails: bool,
    /// It may echo (`assert_no_echoes = false`).
    pub echoes: bool,
    /// It may warn (`assert_no_warnings = false`).
    pub warns: bool,
}

/// The bundled BOSL2's folder of libraries.
pub fn library() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("library")
}

/// All tests of all files, by file name.
pub fn all() -> Vec<Test> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/bosl2");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map(|r| r.filter_map(Result::ok).map(|e| e.path()).collect())
        .unwrap_or_default();
    files.retain(|p| p.extension().is_some_and(|e| e == "scadtest"));
    files.sort();
    let mut out = Vec::new();
    for f in files {
        let text = std::fs::read_to_string(&f).unwrap_or_default();
        let file = f
            .file_stem()
            .map(|s| s.to_string_lossy().trim_start_matches("test_").to_string())
            .unwrap_or_default();
        out.extend(parse(&file, &text));
    }
    out
}

/// The tests of one file.
pub fn parse(file: &str, text: &str) -> Vec<Test> {
    let mut out = Vec::new();
    for block in text.split("[[test]]").skip(1) {
        let name = block
            .lines()
            .find_map(|l| l.trim().strip_prefix("name"))
            .and_then(|r| r.split('"').nth(1))
            .unwrap_or("")
            .to_string();
        let flag = |key: &str| {
            block
                .lines()
                .take_while(|l| !l.contains("'''"))
                .any(|l| l.replace(' ', "") == format!("{key}=false"))
        };
        let Some(start) = block.find("'''") else {
            continue;
        };
        let rest = &block[start + 3..];
        let script = rest.find("'''").map_or(rest, |e| &rest[..e]);
        out.push(Test {
            file: file.to_string(),
            name,
            script: script.trim_start_matches('\n').to_string(),
            fails: flag("expect_success"),
            echoes: flag("assert_no_echoes"),
            warns: flag("assert_no_warnings"),
        });
    }
    out
}

/// Runs a test: `Err` with the error and the notes if it does not reach
/// its end, or echoes or warns where it may not; one that is to fail
/// passes if it ends with an error.
pub fn run(t: &Test) -> Result<(), String> {
    opensolve_scad::library::set_dirs(Some(vec![library()]));
    let main = library()
        .join("BOSL2/tests")
        .join(format!("{}.scad", t.name));
    let read = |p: &Path| {
        if p == main {
            Ok(t.script.as_bytes().to_vec())
        } else {
            std::fs::read(p).map_err(|e| e.to_string())
        }
    };
    let d = opensolve_scad::diagnose(&main, &read, &[]);
    let describe = |head: String| {
        let mut s = head;
        for n in d.notes.iter().filter(|n| !n.echo).take(10) {
            s.push_str(&format!("\n{}:{}: {}", n.line, n.col, n.message));
        }
        for n in d.notes.iter().filter(|n| n.echo).take(4) {
            s.push_str(&format!("\n{}", n.message));
        }
        s
    };
    match (&d.error, t.fails) {
        (Some(_), true) => Ok(()),
        (None, true) => Err(describe("no error, but one was expected".into())),
        (Some(e), false) => Err(describe(e.to_string())),
        (None, false) => {
            if !t.echoes
                && let Some(n) = d.notes.iter().find(|n| n.echo)
            {
                return Err(describe(format!("echoes: {}", n.message)));
            }
            if !t.warns
                && let Some(n) = d.notes.iter().find(|n| !n.echo)
            {
                return Err(describe(format!("warns: {}", n.message)));
            }
            Ok(())
        }
    }
}

/// What a failure is about, for counting: what is unknown or not
/// supported, else the error without places and numbers.
pub fn cause(e: &str) -> String {
    for l in e.lines().skip(1) {
        let m = l.splitn(3, ':').nth(2).unwrap_or(l).trim();
        if m.starts_with("unknown") || m.starts_with("not supported") {
            return m.to_string();
        }
    }
    let first = e.lines().next().unwrap_or("");
    let msg = first
        .rsplit_once(": ")
        .map_or(first, |(_, m)| m)
        .chars()
        .take(70)
        .collect::<String>();
    if first.starts_with("echoes") || first.starts_with("warns") || first.starts_with("no error") {
        return first
            .chars()
            .take(70)
            .collect::<String>()
            .replace(|c: char| c.is_ascii_digit(), "#");
    }
    let head = if first.contains("assertion failed") {
        "assertion failed: "
    } else {
        ""
    };
    format!("{head}{}", msg.replace(|c: char| c.is_ascii_digit(), "#"))
}
