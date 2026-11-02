//! Runs BOSL2's own test suite (`tests/bosl2/*.scadtest`, assert-based)
//! with the bundled BOSL2 and says what passes:
//! `cargo run --release -p opensolve-scad --example bosl2_tests -- [-v]
//! [FILTER]`. A test passes when it runs to its end without an error.

use std::path::Path;
use std::sync::Mutex;
use std::time::Instant;

#[path = "../tests/common/scadtest.rs"]
mod scadtest;

fn main() {
    let mut verbose = false;
    let mut filter = None;
    for a in std::env::args().skip(1) {
        if a == "-v" {
            verbose = true;
        } else {
            filter = Some(a);
        }
    }
    let tests: Vec<scadtest::Test> = scadtest::all()
        .into_iter()
        .filter(|t| {
            filter
                .as_deref()
                .is_none_or(|f| t.name.contains(f) || t.file.contains(f))
        })
        .collect();
    let started = Instant::now();
    let results = Mutex::new(Vec::new());
    let next = Mutex::new(0usize);
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
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
                    let t0 = Instant::now();
                    let r = scadtest::run(t);
                    results
                        .lock()
                        .unwrap()
                        .push((i, r, t0.elapsed().as_secs_f64()));
                }
            });
        }
    });
    let mut results = results.into_inner().unwrap();
    results.sort_by_key(|r| r.0);
    let mut passed = 0;
    let mut causes: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    for (i, r, secs) in &results {
        let t = &tests[*i];
        match r {
            Ok(()) => {
                passed += 1;
                if verbose {
                    println!("ok   {} ({secs:.2} s)", t.name);
                }
            }
            Err(e) => {
                let first = e.lines().next().unwrap_or("").to_string();
                println!("FAIL {}/{} ({secs:.2} s): {first}", t.file, t.name);
                if verbose {
                    for l in e.lines().skip(1).take(8) {
                        println!("       {l}");
                    }
                }
                causes
                    .entry(scadtest::cause(e))
                    .or_default()
                    .push(t.name.clone());
            }
        }
    }
    println!(
        "\n{passed} of {} passed ({:.1} %), {:.1} s",
        results.len(),
        100.0 * passed as f64 / results.len().max(1) as f64,
        started.elapsed().as_secs_f64()
    );
    let mut by: Vec<_> = causes.into_iter().collect();
    by.sort_by_key(|(_, v)| std::cmp::Reverse(v.len()));
    for (c, names) in by.iter().take(40) {
        println!("{:4}  {c}", names.len());
    }
    let _ = Path::new("");
}
