//! Compiles Manifold 3.5.2 (the version OpenSCAD 2026.10 links), oneTBB
//! 2023.1.0 that it runs in parallel on (as OpenSCAD's Manifold does), and
//! the bridge to it.
//!
//! The sources come from `MANIFOLD_SRC` and `TBB_SRC` when set (checkouts of
//! one's own, used as they are), else from their release tarballs,
//! downloaded once into `target/manifold-3.5.2` and `target/onetbb-2023.1.0`
//! and checked against a SHA-256 pinned here: a mismatch fails the build.
//! Only the sources, the headers and the licence are unpacked; nothing of a
//! tarball is run. Manifold is built without its 2D part (Clipper2), TBB as
//! a static library without its allocator.

use std::env;
use std::path::{Path, PathBuf};

/// A source tarball: where it comes from, its SHA-256, the directory its
/// entries are in, and what of it is unpacked.
struct Tarball {
    name: &'static str,
    url: &'static str,
    sha256: &'static str,
    top: &'static str,
    keep: &'static [&'static str],
}

/// Manifold's release asset of v3.5.2 (2026-06-27) on
/// github.com/elalish/manifold, Apache-2.0. The hash was computed from a
/// download on 2026-11-03 and is equal to GitHub's digest of the asset.
const MANIFOLD: Tarball = Tarball {
    name: "manifold-3.5.2",
    url: "https://github.com/elalish/manifold/releases/download/v3.5.2/manifold-3.5.2.tar.gz",
    sha256: "ce5f4d87877daca99a910d0af73c028f2a36b9288dd2bf3cd1cecf8faff9f7c8",
    top: "manifold-3.5.2",
    keep: &["src", "include", "LICENSE"],
};

/// oneTBB's sources at tag v2023.1.0 (2026-07-13) on
/// github.com/uxlfoundation/oneTBB, Apache-2.0, as GitHub archives the tag.
/// The hash was computed from a download on 2026-11-03.
const TBB: Tarball = Tarball {
    name: "onetbb-2023.1.0",
    url: "https://github.com/uxlfoundation/oneTBB/archive/refs/tags/v2023.1.0.tar.gz",
    sha256: "191288b52e1e6b17198000b64d77d194bb65e791be46ebc606e9b091781e2070",
    top: "oneTBB-2023.1.0",
    keep: &["src/tbb", "include", "LICENSE.txt"],
};

/// Manifold's sources, as its `src/CMakeLists.txt` lists them without the
/// cross section.
const MANIFOLD_SOURCES: &[&str] = &[
    "boolean3.cpp",
    "boolean_result.cpp",
    "constructors.cpp",
    "csg_tree.cpp",
    "edge_op.cpp",
    "execution_impl.cpp",
    "face_op.cpp",
    "impl.cpp",
    "manifold.cpp",
    "minkowski.cpp",
    "polygon.cpp",
    "properties.cpp",
    "quickhull.cpp",
    "sdf.cpp",
    "smoothing.cpp",
    "sort.cpp",
    "subdivision.cpp",
    "tree2d.cpp",
];

/// TBB's sources, as its `src/tbb/CMakeLists.txt` lists them.
const TBB_SOURCES: &[&str] = &[
    "address_waiter.cpp",
    "allocator.cpp",
    "arena.cpp",
    "arena_slot.cpp",
    "concurrent_bounded_queue.cpp",
    "dynamic_link.cpp",
    "exception.cpp",
    "governor.cpp",
    "global_control.cpp",
    "itt_notify.cpp",
    "main.cpp",
    "market.cpp",
    "numa_allocation.cpp",
    "tcm_adaptor.cpp",
    "misc.cpp",
    "misc_ex.cpp",
    "observer_proxy.cpp",
    "parallel_pipeline.cpp",
    "private_server.cpp",
    "profiling.cpp",
    "rml_tbb.cpp",
    "rtm_mutex.cpp",
    "rtm_rw_mutex.cpp",
    "semaphore.cpp",
    "small_object_pool.cpp",
    "task.cpp",
    "task_dispatcher.cpp",
    "task_group_context.cpp",
    "thread_dispatcher.cpp",
    "thread_request_serializer.cpp",
    "threading_control.cpp",
    "version.cpp",
    "queuing_rw_mutex.cpp",
];

/// The file a checked download leaves in its directory, naming the hash.
const STAMP: &str = "opensolve-manifold.sha256";

fn main() {
    println!("cargo:rerun-if-env-changed=MANIFOLD_SRC");
    println!("cargo:rerun-if-env-changed=TBB_SRC");
    println!("cargo:rerun-if-changed=src/lib.rs");
    println!("cargo:rerun-if-changed=src/bridge.h");
    println!("cargo:rerun-if-changed=src/bridge.cpp");
    let manifold = env::var("MANIFOLD_SRC").map_or_else(|_| downloaded(&MANIFOLD), PathBuf::from);
    let tbb = env::var("TBB_SRC").map_or_else(|_| downloaded(&TBB), PathBuf::from);
    assert!(
        manifold.join("src/manifold.cpp").is_file() && manifold.join("include/manifold").is_dir(),
        "{} has no Manifold src/ and include/",
        manifold.display()
    );
    assert!(
        tbb.join("src/tbb/task.cpp").is_file() && tbb.join("include/oneapi").is_dir(),
        "{} has no oneTBB src/tbb and include/",
        tbb.display()
    );
    let target_env = env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let msvc = target_env == "msvc";
    let release = env::var("PROFILE").as_deref() == Ok("release");

    // TBB, static, as its CMake builds it without shared libraries: nothing
    // loaded at run time.
    let mut t = cc::Build::new();
    t.cpp(true)
        .std("c++17")
        .files(TBB_SOURCES.iter().map(|f| tbb.join("src/tbb").join(f)))
        .include(tbb.join("include"))
        .define("__TBB_BUILD", None)
        .define("__TBB_DYNAMIC_LOAD_ENABLED", "0")
        .define("__TBB_SOURCE_DIRECTLY_INCLUDED", "1")
        .define("__TBB_SKIP_DEPENDENCY_SIGNATURE_VERIFICATION", "1")
        .warnings(false);
    if msvc {
        t.define("_CRT_SECURE_NO_WARNINGS", None)
            .flag("/EHsc")
            .flag("/volatile:iso")
            .define("_WIN32_WINNT", "0x0A00");
    } else if target_os != "windows" {
        // ucontext, for TBB's resumable tasks.
        t.define("_XOPEN_SOURCE", None);
    }
    if release {
        t.define("NDEBUG", None);
    }
    t.compile("opensolve-tbb");

    let mut build = cxx_build::bridge("src/lib.rs");
    build
        .file("src/bridge.cpp")
        .files(
            MANIFOLD_SOURCES
                .iter()
                .map(|f| manifold.join("src").join(f)),
        )
        .include(manifold.join("include"))
        .include(manifold.join("src"))
        .include(tbb.join("include"))
        .std("c++17")
        .define("MANIFOLD_PAR", "1")
        .define("MANIFOLD_NO_IOSTREAM", None)
        .define("MANIFOLD_NO_FILESYSTEM", None)
        .define("__TBB_NO_IMPLICIT_LINKAGE", "1")
        .define("_USE_MATH_DEFINES", None)
        // Its code, not ours: its warnings are not ours to fix.
        .warnings(false);
    if release {
        build.define("NDEBUG", None);
    }
    if msvc {
        build.flag("/utf-8").flag("/bigobj").flag("/EHsc");
    }
    build.compile("opensolve-manifold");
    // TBB after Manifold, which calls it.
    println!("cargo:rustc-link-lib=static=opensolve-tbb");
    if target_os == "linux" {
        println!("cargo:rustc-link-lib=dl");
    }
}

/// The unpacked tarball: from the cache when it was checked, else
/// downloaded, checked and unpacked.
fn downloaded(t: &Tarball) -> PathBuf {
    let root = cargo_target_dir().join(t.name);
    let stamp = root.join(STAMP);
    println!("cargo:rerun-if-changed={}", stamp.display());
    if std::fs::read_to_string(&stamp).is_ok_and(|s| s.trim() == t.sha256) {
        return root;
    }
    let url = t.url;
    println!("cargo:warning=Downloading {} from {url}", t.name);
    let response = minreq::get(url)
        .send()
        .unwrap_or_else(|e| panic!("download of {url} failed: {e}"));
    if response.status_code != 200 {
        panic!("download of {url} failed: HTTP {}", response.status_code);
    }
    let bytes = response.into_bytes();
    let got = hex(ring::digest::digest(&ring::digest::SHA256, &bytes).as_ref());
    if got != t.sha256 {
        panic!(
            "{url}: SHA-256 is {got}, expected {}; refusing to build with it",
            t.sha256
        );
    }
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(&bytes[..]));
    for entry in archive
        .entries()
        .unwrap_or_else(|e| panic!("reading {url} failed: {e}"))
    {
        let mut entry = entry.unwrap_or_else(|e| panic!("reading {url} failed: {e}"));
        let path = entry.path().unwrap().into_owned();
        let Ok(rel) = path.strip_prefix(t.top) else {
            continue;
        };
        let wanted = t.keep.iter().any(|k| rel.starts_with(k));
        if wanted && entry.header().entry_type().is_file() {
            // `unpack_in` refuses paths that leave the directory.
            entry
                .unpack_in(&root)
                .unwrap_or_else(|e| panic!("unpacking {url} failed: {e}"));
        }
    }
    // The entries are under the tarball's top directory: moved up a level.
    let inner = root.join(t.top);
    for part in t.keep {
        let first = Path::new(part).components().next().unwrap();
        let from = inner.join(first);
        if from.exists() {
            std::fs::rename(&from, root.join(first))
                .unwrap_or_else(|e| panic!("{url}: moving {part} failed: {e}"));
        }
    }
    let _ = std::fs::remove_dir_all(&inner);
    std::fs::write(&stamp, t.sha256).unwrap();
    root
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The cargo target directory, from `OUT_DIR`:
/// `<target_dir>[/<triple>]/<profile>/build/<pkg>-<hash>/out`.
fn cargo_target_dir() -> PathBuf {
    let target = env::var("TARGET").unwrap();
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let above_profile = out_dir
        .ancestors()
        .nth(4)
        .expect("unexpected OUT_DIR layout");
    if above_profile
        .file_name()
        .is_some_and(|n| n == target.as_str())
    {
        above_profile.parent().unwrap().to_path_buf()
    } else {
        above_profile.to_path_buf()
    }
}
