//! An evaluation asked to stop ends soon, says it was cancelled, and leaves
//! nothing behind that a later run would take.

use std::path::Path;
use std::time::{Duration, Instant};

use opensolve_scad::{CANCELLED, Run, Stop, build, build_with_stop, diagnose_with_stop, make};

/// A loop that runs for minutes, making nothing.
const LOOP: &str = "for (i = [0:1e5]) for (j = [0:1e5]) if (i * j < 0) cube();\n";

/// Many spheres that overlap, joined in one batch: a second or so of the
/// mesh library's work in rounds of pairs.
const BATCH: &str = "union() for (i = [0:299]) \
    translate([(i % 20) * 1.5, floor(i / 20) * 1.5, 0]) sphere(r = 1.2, $fn = 40);\n";

/// A solid's triangles by their corners, each from its least corner, in
/// order: the same for the same surface, whichever order the mesh library's
/// threads gave them in.
fn surface(s: &opensolve_kernel::Solid) -> Vec<[[u64; 3]; 3]> {
    let mut out: Vec<[[u64; 3]; 3]> = s
        .tris
        .iter()
        .map(|t| {
            let c = t.map(|i| s.points[i as usize].map(f64::to_bits));
            let k = (0..3).min_by_key(|&k| c[k]).unwrap_or(0);
            [c[k], c[(k + 1) % 3], c[(k + 2) % 3]]
        })
        .collect();
    out.sort_unstable();
    out
}

fn read(text: &'static str) -> impl Fn(&Path) -> Result<Vec<u8>, String> {
    move |_: &Path| Ok(text.as_bytes().to_vec())
}

/// Runs `go` on a thread, sets the stop after `after`, and gives what it
/// returned and how long it took from the stop to its end.
fn stopped_after<T: Send + 'static>(
    after: Duration,
    go: impl FnOnce(Stop) -> T + Send + 'static,
) -> (T, Duration) {
    let stop = Stop::new();
    let theirs = stop.clone();
    let worker = std::thread::spawn(move || go(theirs));
    std::thread::sleep(after);
    let at = Instant::now();
    stop.stop();
    let out = worker.join().expect("the evaluation panicked");
    (out, at.elapsed())
}

#[test]
fn a_long_loop_stops_soon() {
    let (out, took) = stopped_after(Duration::from_millis(200), |stop| {
        build_with_stop(Path::new("main.scad"), &read(LOOP), &[], &stop).map(|_| ())
    });
    assert_eq!(out, Err(CANCELLED.to_string()));
    assert!(took < Duration::from_millis(200), "it took {took:?}");
}

#[test]
fn a_long_list_comprehension_stops_soon() {
    let text = "x = [for (i = [0:1e5]) len([for (j = [0:1e4]) i * j])];\ncube();\n";
    let (d, took) = stopped_after(Duration::from_millis(200), move |stop| {
        diagnose_with_stop(Path::new("main.scad"), &read(text), &[], &stop)
    });
    assert!(
        d.error
            .as_ref()
            .is_some_and(opensolve_scad::Error::is_cancelled)
    );
    assert!(d.notes.is_empty());
    assert!(took < Duration::from_millis(200), "it took {took:?}");
}

#[test]
fn a_run_stopped_before_it_begins_makes_nothing() {
    let stop = Stop::new();
    stop.stop();
    let run = Run {
        stop,
        ..Run::default()
    };
    let e = make(Path::new("main.scad"), &read("cube();\n"), &run).unwrap_err();
    assert!(e.is_cancelled(), "{e}");
    // Not every error is that one.
    let e = make(
        Path::new("main.scad"),
        &read("assert(false);\n"),
        &Run::default(),
    )
    .unwrap_err();
    assert!(!e.is_cancelled());
}

#[test]
fn a_batch_stops_between_its_rounds_and_a_later_run_is_the_same() {
    // With a cache for all runs (as the tests can be run), the run to stop
    // finds the whole result kept: nothing to stop.
    if std::env::var_os("OPENSOLVE_SCAD_SHARED_CACHE").is_some() {
        return;
    }
    let start = Instant::now();
    let whole = build(Path::new("main.scad"), &read(BATCH), &[]).unwrap();
    let full = start.elapsed();
    // Stopped halfway, in the union of the spheres.
    let (out, took) = stopped_after(full / 2, |stop| {
        build_with_stop(Path::new("main.scad"), &read(BATCH), &[], &stop).map(|o| o.solid)
    });
    assert_eq!(out.unwrap_err(), CANCELLED);
    assert!(
        took < Duration::from_millis(500).max(full / 3),
        "it took {took:?} of {full:?}"
    );
    // Nothing of the stopped run is kept: the same again.
    let again = build(Path::new("main.scad"), &read(BATCH), &[]).unwrap();
    assert!(surface(&again.solid) == surface(&whole.solid));
}

#[test]
fn many_runs_stopped_at_any_time_leave_the_result_alone() {
    let text = "difference() { cube(20, center = true); \
        for (i = [0:59]) rotate([0, 0, i * 6]) translate([8, 0, 0]) cylinder(r = 1, h = 30, center = true, $fn = 24); }\n";
    let whole = build(Path::new("main.scad"), &read(text), &[]).unwrap();
    for ms in [0, 1, 2, 5, 10, 20, 40] {
        let (out, _) = stopped_after(Duration::from_millis(ms), move |stop| {
            build_with_stop(Path::new("main.scad"), &read(text), &[], &stop)
        });
        // Done before the stop, or stopped: never something in between.
        if let Ok(o) = out {
            assert!(surface(&o.solid) == surface(&whole.solid));
        } else {
            assert_eq!(out.unwrap_err(), CANCELLED);
        }
        let again = build(Path::new("main.scad"), &read(text), &[]).unwrap();
        assert!(surface(&again.solid) == surface(&whole.solid));
    }
}
