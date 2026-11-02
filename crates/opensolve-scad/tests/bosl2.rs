//! BOSL2, as it comes with the app: its own test suite, and the shapes it
//! is used for most.

use std::path::Path;

#[path = "common/scadtest.rs"]
mod scadtest;

/// The tests of BOSL2's suite that do not pass yet, and why.
const KNOWN: &[(&str, &str)] = &[];

#[test]
fn bosl2s_own_tests_pass() {
    let tests = scadtest::all();
    assert!(tests.len() > 900, "{} tests", tests.len());
    let next = std::sync::Mutex::new(0usize);
    let failed = std::sync::Mutex::new(Vec::new());
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get().min(8));
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| {
                loop {
                    let i = {
                        let mut n = next.lock().unwrap();
                        *n += 1;
                        *n - 1
                    };
                    let Some(t) = tests.get(i) else { break };
                    if let Err(e) = scadtest::run(t) {
                        failed.lock().unwrap().push((t.name.clone(), e));
                    }
                }
            });
        }
    });
    let failed = failed.into_inner().unwrap();
    let new: Vec<String> = failed
        .iter()
        .filter(|(n, _)| !KNOWN.iter().any(|(k, _)| k == n))
        .map(|(n, e)| format!("{n}: {e}"))
        .collect();
    assert!(new.is_empty(), "{}", new.join("\n\n"));
    let fixed: Vec<&str> = KNOWN
        .iter()
        .filter(|(k, _)| !failed.iter().any(|(n, _)| n == k))
        .map(|(k, _)| *k)
        .collect();
    assert!(fixed.is_empty(), "these pass now: {fixed:?}");
}

/// Builds `text` as a file beside BOSL2's tests.
fn build(text: &str) -> opensolve_scad::Output {
    opensolve_scad::library::set_dirs(Some(vec![scadtest::library()]));
    let main = Path::new("/nowhere/main.scad");
    let read = |p: &Path| {
        if p == main {
            Ok(text.as_bytes().to_vec())
        } else {
            std::fs::read(p).map_err(|e| e.to_string())
        }
    };
    let out = opensolve_scad::build(main, &read, &[]).unwrap_or_else(|e| panic!("{e}"));
    assert!(out.solid.is_closed(), "not closed");
    out
}

#[test]
fn a_rounded_block_with_a_hole() {
    let out = build(
        "include <BOSL2/std.scad>\n\
         diff() cuboid([40, 30, 10], rounding = 2, edges = \"Z\", $fn = 24)\n\
         attach(TOP) tag(\"remove\") cyl(d = 6, h = 12, anchor = TOP, $fn = 24);",
    );
    // As OpenSCAD makes it.
    let want = 11_684.709;
    let v = out.solid.volume();
    assert!((v - want).abs() < 1e-3, "{v} is not {want}");
}

#[test]
fn an_attachment_tree() {
    let out = build(
        "include <BOSL2/std.scad>\n\
         cuboid(20) {\n\
           attach(TOP) cyl(d = 8, h = 10, anchor = BOT);\n\
           position(FRONT + BOT) cuboid([5, 5, 5], anchor = BACK + BOT);\n\
         }",
    );
    let (lo, hi) = out.solid.bounds().unwrap();
    for (a, b) in lo
        .iter()
        .chain(&hi)
        .zip([-10.0, -15.0, -10.0, 10.0, 10.0, 20.0])
    {
        assert!((a - b).abs() < 1e-6, "{lo:?} {hi:?}");
    }
}
