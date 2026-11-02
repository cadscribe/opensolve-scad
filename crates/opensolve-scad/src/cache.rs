//! Files parsed once: kept by path and text for every evaluation after,
//! so that `include <BOSL2/std.scad>` (some sixty files, 4.6 MB) is read
//! but not parsed again while it does not change. Each file has a number
//! for good, which the places in its statements carry.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use crate::Error;
use crate::ast::Stmt;

/// A file as read, what it includes and uses, and its statements once
/// asked for.
#[derive(Debug)]
pub struct Parsed {
    pub id: u32,
    pub text: Arc<str>,
    /// The paths of its `include <…>` and `use <…>`, as written.
    pub includes: Vec<String>,
    /// The paths its `import` calls name as text, as written.
    pub imports: Vec<String>,
    stmts: OnceLock<Result<Arc<Vec<Stmt>>, Error>>,
}

impl Parsed {
    /// Its statements, parsed the first time they are asked for.
    pub fn stmts(&self) -> Result<Arc<Vec<Stmt>>, Error> {
        self.stmts
            .get_or_init(|| crate::parser::parse(&self.text, self.id).map(Arc::new))
            .clone()
    }
}

#[derive(Default)]
struct Cache {
    files: HashMap<PathBuf, Arc<Parsed>>,
    /// The paths by number, from 1; 0 is no file.
    paths: Vec<PathBuf>,
    ids: HashMap<PathBuf, u32>,
    /// The texts kept, in bytes.
    size: usize,
}

/// More text than this kept, and all is dropped: files of the past.
const MOST: usize = 256 << 20;

fn cache() -> std::sync::MutexGuard<'static, Cache> {
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    CACHE
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The number of a file, the same for good.
pub fn id(path: &Path) -> u32 {
    let mut c = cache();
    if let Some(&id) = c.ids.get(path) {
        return id;
    }
    c.paths.push(path.to_path_buf());
    let id = c.paths.len() as u32;
    c.ids.insert(path.to_path_buf(), id);
    id
}

/// The path of file number `id`.
pub fn path(id: u32) -> Option<PathBuf> {
    let c = cache();
    c.paths.get((id as usize).checked_sub(1)?).cloned()
}

/// The file at `path` with `text`: as kept, if it was kept with this text.
pub fn file(path: &Path, text: String) -> Arc<Parsed> {
    let id = id(path);
    if let Some(p) = cache().files.get(path)
        && *p.text == *text
    {
        return p.clone();
    }
    let includes = crate::includes(&text);
    let imports = crate::imports(&text);
    let parsed = Arc::new(Parsed {
        id,
        text: text.into(),
        includes,
        imports,
        stmts: OnceLock::new(),
    });
    let mut c = cache();
    c.size += parsed.text.len();
    if c.size > MOST {
        c.files.clear();
        c.size = parsed.text.len();
    }
    c.files.insert(path.to_path_buf(), parsed.clone());
    parsed
}
