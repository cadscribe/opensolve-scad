//! Where `include <BOSL2/std.scad>` finds its file (ADR 0014): as OpenSCAD
//! looks, first beside the file that names it, then in the folders of
//! `OPENSCADPATH`, then in OpenSCAD's user library folder, then in
//! OpenSolve's own library folder. BOSL2 comes with the app: when no
//! folder has it, the copy built into the app is written into OpenSolve's
//! library folder and read from there (from memory, if it cannot be
//! written), so that it works out of the box and an editor can open its
//! files.

use std::path::{Path, PathBuf};
use std::sync::RwLock;

/// The library that comes with the app.
pub const BUNDLED: &str = "BOSL2";
/// Its release, pinned: github.com/BelfrySCAD/BOSL2, tag v2.0.774.
pub const BUNDLED_VERSION: &str = "2.0.774";
/// The file in an installed copy that says it is ours, with its version:
/// a folder without it is the user's and never written.
pub const MARKER: &str = ".opensolve-bundled";

/// The library folders set by the caller (`None`: the default ones).
static DIRS: RwLock<Option<Vec<PathBuf>>> = RwLock::new(None);
/// OpenSolve's library folder set by the caller (`--config-dir`).
static OWN: RwLock<Option<PathBuf>> = RwLock::new(None);

/// Sets the folders searched for libraries, in their order, instead of
/// the default ones; `None` goes back to those. The bundled copy is still
/// found in the last of them that is OpenSolve's (see [`own_dir`]).
pub fn set_dirs(dirs: Option<Vec<PathBuf>>) {
    if let Ok(mut d) = DIRS.write() {
        *d = dirs;
    }
}

/// Sets OpenSolve's library folder, e.g. `libraries` in the folder given
/// by `--config-dir`.
pub fn set_own_dir(dir: PathBuf) {
    if let Ok(mut d) = OWN.write() {
        *d = Some(dir);
    }
}

fn env(k: &str) -> Option<PathBuf> {
    std::env::var_os(k)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// OpenSolve's library folder: `libraries` in its configuration folder
/// (`$OPENSOLVE_CONFIG_DIR`, else `~/Library/Application Support/OpenSolve`
/// on macOS, `%APPDATA%\OpenSolve` on Windows, `~/.config/opensolve`).
pub fn own_dir() -> Option<PathBuf> {
    if let Some(d) = OWN.read().ok().and_then(|d| d.clone()) {
        return Some(d);
    }
    let config = env("OPENSOLVE_CONFIG_DIR").or_else(|| {
        if cfg!(target_os = "macos") {
            env("HOME").map(|h| h.join("Library/Application Support/OpenSolve"))
        } else if cfg!(target_os = "windows") {
            env("APPDATA").map(|a| a.join("OpenSolve"))
        } else {
            env("XDG_CONFIG_HOME")
                .or_else(|| env("HOME").map(|h| h.join(".config")))
                .map(|c| c.join("opensolve"))
        }
    })?;
    Some(config.join("libraries"))
}

/// OpenSCAD's user library folder: `~/Documents/OpenSCAD/libraries` on
/// macOS and Windows, `~/.local/share/OpenSCAD/libraries` on Linux.
pub fn openscad_user_dir() -> Option<PathBuf> {
    if cfg!(target_os = "windows") {
        return env("USERPROFILE").map(|h| h.join("Documents/OpenSCAD/libraries"));
    }
    if cfg!(target_os = "macos") {
        return env("HOME").map(|h| h.join("Documents/OpenSCAD/libraries"));
    }
    env("XDG_DATA_HOME")
        .or_else(|| env("HOME").map(|h| h.join(".local/share")))
        .map(|d| d.join("OpenSCAD/libraries"))
}

/// The folders of `OPENSCADPATH`.
pub fn openscad_path() -> Vec<PathBuf> {
    std::env::var_os("OPENSCADPATH")
        .map(|v| {
            std::env::split_paths(&v)
                .filter(|p| !p.as_os_str().is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// The folders searched for libraries, in their order.
pub fn dirs() -> Vec<PathBuf> {
    if let Some(d) = DIRS.read().ok().and_then(|d| d.clone()) {
        return d.iter().map(|p| crate::plain(p)).collect();
    }
    let mut out = openscad_path();
    out.extend(openscad_user_dir());
    out.extend(own_dir());
    out.iter().map(|p| crate::plain(p)).collect()
}

/// The folders an include is looked for in after the file's own: [`dirs`],
/// and OpenSolve's own folder at the end, where the bundled copy goes.
pub fn search() -> Vec<PathBuf> {
    let mut out = dirs();
    if let Some(own) = own_dir()
        && !out.contains(&own)
    {
        out.push(own);
    }
    out
}

/// Where a library folder `name` is found, if in any: the first folder of
/// [`dirs`] that holds it; for the bundled one, OpenSolve's own folder
/// otherwise.
pub fn find(name: &str) -> Option<PathBuf> {
    search()
        .into_iter()
        .map(|d| d.join(name))
        .find(|p| p.is_dir())
        .or_else(|| (name == BUNDLED).then(|| own_dir().map(|d| d.join(BUNDLED)))?)
}

/// Reads a file of a library folder: from the disk, or the bundled copy
/// where it should be and is not (yet).
pub fn read(path: &Path) -> Result<Vec<u8>, String> {
    match std::fs::read(path) {
        Ok(b) => Ok(b),
        Err(e) => bundled_file(path)
            .map(|t| t.as_bytes().to_vec())
            .ok_or_else(|| format!("{}: {e}", path.display())),
    }
}

/// The bundled text of `path`, if it names a file of the bundled copy in
/// OpenSolve's library folder.
fn bundled_file(path: &Path) -> Option<&'static str> {
    let rest = path.strip_prefix(own_dir()?.join(BUNDLED)).ok()?;
    let name = rest.to_str()?;
    bundled::FILES
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, t)| *t)
}

/// Whether `path` lies in one of the library folders.
pub fn is_library(path: &Path) -> bool {
    search().iter().any(|d| path.starts_with(d))
}

/// What is installed of the bundled copy in `dir`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Installed {
    /// Nothing there.
    Missing,
    /// Our copy, of this version.
    Bundled(String),
    /// A folder of the user's own: never written.
    Own,
}

/// What the folder `dir` holds of the bundled library.
pub fn installed(dir: &Path) -> Installed {
    if !dir.is_dir() {
        return Installed::Missing;
    }
    match std::fs::read_to_string(dir.join(MARKER)) {
        Ok(v) => Installed::Bundled(v.trim().to_string()),
        Err(_) => Installed::Own,
    }
}

/// Writes the bundled copy into `dir` (e.g. `…/libraries/BOSL2`) unless the
/// user's own is there; an older bundled copy is replaced. Written into a
/// folder beside first and then moved, so that no half copy is read.
pub fn install(dir: &Path) -> Result<(), String> {
    match installed(dir) {
        Installed::Own => return Err(format!("{} is not OpenSolve's", dir.display())),
        Installed::Bundled(v) if v == BUNDLED_VERSION => return Ok(()),
        _ => {}
    }
    if bundled::FILES.is_empty() {
        return Err("no library is built into this program".into());
    }
    let parent = dir.parent().ok_or("no folder")?;
    std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    let tmp = parent.join(format!(".{BUNDLED}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let write = || -> std::io::Result<()> {
        std::fs::create_dir_all(&tmp)?;
        for (name, text) in bundled::FILES {
            std::fs::write(tmp.join(name), text)?;
        }
        std::fs::write(tmp.join(MARKER), format!("{BUNDLED_VERSION}\n"))?;
        if dir.exists() {
            std::fs::remove_dir_all(dir)?;
        }
        std::fs::rename(&tmp, dir)
    };
    write().map_err(|e| {
        let _ = std::fs::remove_dir_all(&tmp);
        format!("{}: {e}", dir.display())
    })
}

/// Makes sure the bundled copy is in OpenSolve's library folder when no
/// other folder has one, once a run: what an include of it needs.
pub fn ensure_bundled() {
    static DONE: std::sync::Once = std::sync::Once::new();
    DONE.call_once(|| {
        let others = dirs()
            .into_iter()
            .filter(|d| Some(d) != own_dir().as_ref())
            .any(|d| d.join(BUNDLED).is_dir());
        if let (false, Some(own)) = (others, own_dir()) {
            let _ = install(&own.join(BUNDLED));
        }
    });
}

#[cfg(not(target_arch = "wasm32"))]
mod bundled {
    //! BOSL2's files, built in (BSD-2-Clause, its LICENSE among them).
    include!(concat!(env!("OUT_DIR"), "/bundled.rs"));
}

#[cfg(target_arch = "wasm32")]
mod bundled {
    pub const FILES: &[(&str, &str)] = &[];
}

/// The bundled files' names and texts.
pub fn bundled_files() -> &'static [(&'static str, &'static str)] {
    bundled::FILES
}
