//! A cache of results across runs (`Cache`): a run with it makes the same
//! as one without (the same corners, triangles, planes, faces told apart
//! alike, notes), whatever ran before, and makes again what a changed
//! value or file reaches.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use opensolve_kernel::{Facets, Solid};
use opensolve_scad::{Cache, Output, Stop, build, build_with_cache, build_with_stop};

/// Files in memory, which a test may change between runs.
struct Files {
    dir: PathBuf,
    texts: Mutex<HashMap<PathBuf, Vec<u8>>>,
}

impl Files {
    fn new(dir: &str, files: &[(&str, &str)]) -> Files {
        let dir = Path::new("/cache-tests").join(dir);
        let texts = files
            .iter()
            .map(|(n, t)| (dir.join(n), t.as_bytes().to_vec()))
            .collect();
        Files {
            dir,
            texts: Mutex::new(texts),
        }
    }

    fn set(&self, name: &str, text: &str) {
        self.texts
            .lock()
            .unwrap()
            .insert(self.dir.join(name), text.as_bytes().to_vec());
    }

    fn main(&self) -> PathBuf {
        self.dir.join("main.scad")
    }

    fn read(&self, p: &Path) -> Result<Vec<u8>, String> {
        self.texts
            .lock()
            .unwrap()
            .get(p)
            .cloned()
            .ok_or_else(|| format!("{} not found", p.display()))
    }

    fn build(&self, values: &[(&str, &str)], cache: Option<&Cache>) -> Result<Output, String> {
        let values: Vec<(String, String)> = values
            .iter()
            .map(|(n, v)| (n.to_string(), v.to_string()))
            .collect();
        let read = |p: &Path| self.read(p);
        match cache {
            Some(c) => build_with_cache(&self.main(), &read, &values, &Stop::new(), c),
            None => build(&self.main(), &read, &values),
        }
    }
}

/// A solid's triangles, each by its corners from its least one and its
/// facet's plane, in groups by face, all sorted: the same for the same
/// surface cut into the same faces, in whichever order the boolean
/// library's threads gave the triangles.
fn shape(s: &Solid, f: &Facets) -> Vec<Vec<[u64; 13]>> {
    let mut faces: HashMap<u64, Vec<[u64; 13]>> = HashMap::new();
    for (t, &facet) in s.tris.iter().zip(&s.facets) {
        let c = t.map(|i| s.points[i as usize].map(f64::to_bits));
        let k = (0..3).min_by_key(|&k| c[k]).unwrap_or(0);
        let x = f.get(facet);
        let mut row = [0u64; 13];
        for (j, p) in [c[k], c[(k + 1) % 3], c[(k + 2) % 3]].iter().enumerate() {
            row[3 * j..3 * j + 3].copy_from_slice(p);
        }
        for (j, v) in [x.normal[0], x.normal[1], x.normal[2], x.offset]
            .iter()
            .enumerate()
        {
            row[9 + j] = v.to_bits();
        }
        faces.entry(x.face.0).or_default().push(row);
    }
    let mut out: Vec<Vec<[u64; 13]>> = faces
        .into_values()
        .map(|mut v| {
            v.sort_unstable();
            v
        })
        .collect();
    out.sort_unstable();
    out
}

fn assert_same(a: &Result<Output, String>, b: &Result<Output, String>, what: &str) {
    match (a, b) {
        (Ok(a), Ok(b)) => {
            assert_eq!(a.notes, b.notes, "{what}: the notes");
            assert!(
                shape(&a.solid, &a.facets) == shape(&b.solid, &b.facets),
                "{what}: the solids differ ({} / {} triangles)",
                a.solid.tris.len(),
                b.solid.tris.len()
            );
        }
        (Err(x), Err(y)) => assert_eq!(x, y, "{what}"),
        _ => panic!("{what}: {:?} / {:?}", a.as_ref().err(), b.as_ref().err()),
    }
}

/// A part made with BOSL2: a block with rounded edges, holes in a row and
/// a module that echoes; its parameters in front.
const PART: &str = "include <BOSL2/std.scad>
width = 40;
holes = 3;
hole = 4;
module block(w) {
    echo(str(\"block \", w));
    cuboid([w, 20, 8], rounding = 2, edges = \"Z\", $fn = 24);
}
module peg(d) cyl(d = d, h = 12, $fn = 32);
difference() {
    block(width);
    for (i = [0 : holes - 1])
        translate([(i - (holes - 1) / 2) * 8, 0, 0]) peg(hole);
}
translate([0, 30, 0]) block(30);
";

#[test]
fn a_parameter_changed_makes_the_same_as_without() {
    let files = Files::new("part", &[("main.scad", PART)]);
    let cache = Cache::with_limits(64 << 20, Duration::ZERO);
    let steps: [&[(&str, &str)]; 5] = [
        &[],
        &[],
        &[("hole", "5")],
        &[("hole", "5"), ("holes", "4")],
        &[("width", "44")],
    ];
    for (i, values) in steps.iter().enumerate() {
        let before = cache.stats();
        let with = files.build(values, Some(&cache));
        let without = files.build(values, None);
        assert_same(&without, &with, &format!("step {i}"));
        let after = cache.stats();
        if i > 0 {
            assert!(after.hits > before.hits, "step {i} used nothing: {after}");
        }
    }
    // The second run, unchanged, made nothing anew.
    let s = cache.stats();
    assert!(s.entries > 0 && s.stored > 0, "{s}");
}

#[test]
fn notes_and_echoes_are_told_again() {
    let text = "module m(x) { echo(x = x); undefined_thing(); cube(x); }\n\
        m(2); translate([5, 0, 0]) m(3);\n";
    let files = Files::new("notes", &[("main.scad", text)]);
    let cache = Cache::with_limits(16 << 20, Duration::ZERO);
    let first = files.build(&[], Some(&cache));
    let again = files.build(&[], Some(&cache));
    let without = files.build(&[], None);
    assert_same(&without, &first, "first");
    assert_same(&without, &again, "again");
    let notes = &again.unwrap().notes;
    assert!(notes.iter().any(|n| n.contains("ECHO: x = 3")), "{notes:?}");
    assert!(
        notes.iter().any(|n| n.contains("undefined_thing")),
        "{notes:?}"
    );
    assert!(cache.stats().hits > 0);
}

#[test]
fn a_used_file_changed_is_read_anew() {
    let lib = "module plate(w) cube([w, w, 2]);\nfunction size() = 10;\n";
    let main = "use <lib.scad>\nplate(size());\ntranslate([20, 0, 0]) plate(5);\n";
    let files = Files::new("used", &[("main.scad", main), ("lib.scad", lib)]);
    let cache = Cache::with_limits(16 << 20, Duration::ZERO);
    let a = files.build(&[], Some(&cache)).unwrap();
    files.set(
        "lib.scad",
        "module plate(w) cube([w, w, 3]);\nfunction size() = 12;\n",
    );
    let b = files.build(&[], Some(&cache));
    assert_same(&files.build(&[], None), &b, "after the change");
    assert!(b.unwrap().solid.volume() > a.solid.volume() * 1.4);
    assert!(cache.stats().invalidated > 0, "{}", cache.stats());
}

/// A cube of `s` as an ASCII STL.
fn stl(s: f64) -> String {
    let p = |x: f64, y: f64, z: f64| format!("vertex {} {} {}", x * s, y * s, z * s);
    let quads = [
        [(0., 0., 0.), (0., 1., 0.), (1., 1., 0.), (1., 0., 0.)],
        [(0., 0., 1.), (1., 0., 1.), (1., 1., 1.), (0., 1., 1.)],
        [(0., 0., 0.), (1., 0., 0.), (1., 0., 1.), (0., 0., 1.)],
        [(0., 1., 0.), (0., 1., 1.), (1., 1., 1.), (1., 1., 0.)],
        [(0., 0., 0.), (0., 0., 1.), (0., 1., 1.), (0., 1., 0.)],
        [(1., 0., 0.), (1., 1., 0.), (1., 1., 1.), (1., 0., 1.)],
    ];
    let mut out = String::from("solid c\n");
    for q in quads {
        for t in [[q[0], q[1], q[2]], [q[0], q[2], q[3]]] {
            out.push_str("facet normal 0 0 0\nouter loop\n");
            for (x, y, z) in t {
                out.push_str(&p(x, y, z));
                out.push('\n');
            }
            out.push_str("endloop\nendfacet\n");
        }
    }
    out.push_str("endsolid c\n");
    out
}

#[test]
fn an_imported_file_changed_is_read_anew() {
    let main = "module part() import(\"part.stl\");\n\
        difference() { part(); translate([1, 1, -1]) cylinder(r = 0.5, h = 30, $fn = 64); }\n";
    let files = Files::new("import", &[("main.scad", main), ("part.stl", &stl(4.0))]);
    let cache = Cache::with_limits(16 << 20, Duration::ZERO);
    let a = files.build(&[], Some(&cache)).unwrap();
    let again = files.build(&[], Some(&cache));
    assert_same(&Ok(a.clone()), &again, "again");
    files.set("part.stl", &stl(6.0));
    let b = files.build(&[], Some(&cache));
    assert_same(&files.build(&[], None), &b, "after the change");
    assert!(b.unwrap().solid.volume() > a.solid.volume() * 3.0);
}

#[test]
fn what_draws_random_numbers_is_made_again() {
    let text = "module dots() for (p = rands(0, 10, 6)) translate([p, 0, 0]) cube(0.5);\n\
        k = 1;\ndots();\ntranslate([0, 5 * k, 0]) dots();\n";
    let files = Files::new("rands", &[("main.scad", text)]);
    let cache = Cache::with_limits(16 << 20, Duration::ZERO);
    for k in ["1", "2", "1"] {
        let with = files.build(&[("k", k)], Some(&cache));
        assert_same(&files.build(&[("k", k)], None), &with, k);
    }
}

#[test]
fn the_cache_holds_no_more_than_its_capacity() {
    let files = Files::new("small", &[("main.scad", PART)]);
    let cache = Cache::with_limits(256 << 10, Duration::ZERO);
    for hole in ["3", "4", "5", "6"] {
        let with = files.build(&[("hole", hole)], Some(&cache));
        assert_same(&files.build(&[("hole", hole)], None), &with, hole);
        let s = cache.stats();
        assert!(s.bytes <= s.capacity, "{s}");
    }
    assert!(cache.stats().evicted > 0, "{}", cache.stats());
    cache.clear();
    assert_eq!(cache.stats().entries, 0);
}

#[test]
fn runs_on_threads_share_a_cache() {
    let files = Files::new("threads", &[("main.scad", PART)]);
    let cache = Cache::with_limits(64 << 20, Duration::ZERO);
    let values = ["3", "4", "3", "5", "4", "3"];
    std::thread::scope(|s| {
        for v in values {
            let (files, cache) = (&files, &cache);
            s.spawn(move || {
                let with = files.build(&[("hole", v)], Some(cache));
                assert_same(&files.build(&[("hole", v)], None), &with, v);
            });
        }
    });
    for v in values {
        let with = files.build(&[("hole", v)], Some(&cache));
        assert_same(&files.build(&[("hole", v)], None), &with, v);
    }
}

#[test]
fn a_run_stopped_leaves_the_cache_sound() {
    let files = Files::new("stopped", &[("main.scad", PART)]);
    let cache = Cache::with_limits(64 << 20, Duration::ZERO);
    for ms in [5, 20, 60, 150] {
        let stop = Stop::new();
        let read = |p: &Path| files.read(p);
        std::thread::scope(|s| {
            let stop2 = stop.clone();
            s.spawn(move || {
                std::thread::sleep(Duration::from_millis(ms));
                stop2.stop();
            });
            let _ = build_with_cache(&files.main(), &read, &[], &stop, &cache);
        });
        let with = files.build(&[], Some(&cache));
        assert_same(&files.build(&[], None), &with, &format!("after {ms} ms"));
    }
    // Without a cache, stopping is as before.
    let read = |p: &Path| files.read(p);
    let stop = Stop::new();
    stop.stop();
    assert!(build_with_stop(&files.main(), &read, &[], &stop).is_err());
}

/// A pad shared within a run (a module called again alike): the rows of
/// pads joined by booleans and the pad on its own have its faces' names
/// in a run without the cache. The pad's key changes with `$n`, so a later
/// run makes it anew; a boolean kept of an earlier run's pads, alike in
/// shape, must not be used for this run's (its faces would have the old
/// names, not shared with the pad on its own).
#[test]
fn a_boolean_of_shapes_made_anew_is_made_anew_with_their_faces() {
    let text = "w = 3;\n\
        module pad() cylinder(r = 2, h = 2, $fn = 48);\n\
        module grid() {\n\
            for (y = [0 : $n - 1]) translate([0, y * 6, 0])\n\
                union() { for (x = [0 : 3]) translate([x * 6, 0, 0]) pad(); }\n\
            translate([-10, 0, 0]) pad();\n\
        }\n\
        grid($n = w);\n";
    let files = Files::new("shared", &[("main.scad", text)]);
    let cache = Cache::with_limits(64 << 20, Duration::ZERO);
    for w in ["3", "4", "3", "5"] {
        let with = files.build(&[("w", w)], Some(&cache));
        assert_same(&files.build(&[("w", w)], None), &with, w);
    }
}

/// A logo cut from a base and shown again in a colour: a run without the
/// cache uses what the call made outside any colour inside one too, its
/// faces the same; so must a run whose base is a result kept.
#[test]
fn a_call_in_a_colour_shares_what_it_made_outside_one() {
    let text = "$fn = 500;\nh = 2.2;\n\
        module logo() scale([0.13, 0.13, 1]) translate([-90, -90, -h / 2])\n\
            linear_extrude(height = h) scale([70, 60]) circle(r = 1);\n\
        module base() difference() {\n\
            color([0.72, 0, 0.2]) cylinder(d = 23, h = h, center = true); logo(); }\n\
        base();\n\
        color([1, 1, 0.98]) logo();\n";
    let files = Files::new("colour", &[("main.scad", text)]);
    let cache = Cache::with_limits(64 << 20, Duration::ZERO);
    for h in ["2.2", "2.2", "3", "2.2"] {
        let with = files.build(&[("h", h)], Some(&cache));
        assert_same(&files.build(&[("h", h)], None), &with, h);
    }
}
