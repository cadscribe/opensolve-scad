//! Results kept from one run of a file for the next, while it is open
//! (see [`Cache`]): what a module made and what a boolean, hull, Minkowski
//! sum, extrusion or import made, by a hash of what went in, so that a
//! parameter changed makes again only what depends on it.
//!
//! A run that uses a cache is a [`Session`]: it looks results up, keeps
//! those it made (not yet for others) and hands them to the cache when it
//! ends, done or not. The solids a run makes index into its table of
//! facets, and the boolean library's meshes of them carry those numbers;
//! so the facets of a run whose results are kept are kept as well, under
//! their numbers, and the next run's table goes on from them
//! ([`Facets::after`]). Each run is given numbers of its own for its
//! facets and the names of its faces: runs side by side (one cancelled,
//! the next begun) do not mix them.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::Duration;

use opensolve_kernel::{Facet, Facets};

use crate::ast::Pos;
use crate::geom::Geom;
use crate::paint::Rgba;

/// How many bytes a cache holds unless told otherwise.
pub const DEFAULT_CAPACITY: usize = 512 << 20;

/// A result that took less than this to make is not kept: looking it up
/// would cost about as much.
pub const DEFAULT_MIN_COST: Duration = Duration::from_micros(500);

/// The numbers of facets a run may give: more, and its results are not
/// kept.
const WINDOW: u32 = 1 << 27;

/// The names of faces a run may give.
const FACE_WINDOW: u64 = 1 << 40;

/// What a result told on its way, told again when it is used: notes,
/// echoes, the colours met and the files imported (with a hash of their
/// contents, which must be the same for the result to be used); and the
/// batches of booleans it made, which a run that made it keeps for the
/// rest of it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Event {
    Note(Option<Pos>, String),
    Echo(Pos, String),
    Colour(Rgba),
    Read(PathBuf, u64),
    /// A batch of booleans made (see [`crate::geom::Builder::remember`]).
    Batch((u64, u64), Arc<opensolve_kernel::Solid>),
}

impl Event {
    fn bytes(&self) -> usize {
        std::mem::size_of::<Event>()
            + match self {
                Event::Note(_, t) | Event::Echo(_, t) => t.len(),
                Event::Read(p, _) => p.as_os_str().len(),
                Event::Colour(_) => 0,
                Event::Batch(_, s) => s.bytes(),
            }
    }
}

/// The files whose code a result's key went through, as a set of their
/// numbers modulo 256: a file changed drops the results that may depend
/// on it (some more, never fewer).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Files([u64; 4]);

impl Files {
    pub fn add(&mut self, id: u32) {
        if id != 0 {
            self.0[(id as usize >> 6) & 3] |= 1 << (id & 63);
        }
    }

    pub fn join(&mut self, o: Files) {
        for (a, b) in self.0.iter_mut().zip(o.0) {
            *a |= b;
        }
    }

    pub fn has(&self, id: u32) -> bool {
        id != 0 && self.0[(id as usize >> 6) & 3] & (1 << (id & 63)) != 0
    }
}

/// A run's facets kept: the run's number (one for good), the number of
/// its first facet, and the facets.
type Run = (u64, u32, Arc<[Facet]>);

struct Entry {
    made: Arc<Option<Geom>>,
    /// Which facets its solid has (see [`fingerprint`]).
    print: u64,
    events: Arc<[Event]>,
    /// The results within it, made or used on the way.
    inner: Arc<[Use]>,
    /// The facets its solids may index, kept while it is.
    runs: Arc<[Run]>,
    files: Files,
    bytes: usize,
    tick: u64,
}

struct Inner {
    capacity: usize,
    min_cost: Duration,
    entries: HashMap<u128, Entry>,
    /// The entries by when they were last used, the oldest first.
    lru: BTreeMap<u64, u128>,
    tick: u64,
    /// The bytes of the entries, without the facets they keep.
    bytes: usize,
    /// The runs of facets kept by some entry: the run's number, its first
    /// facet's, the facets while an entry holds them, and their bytes.
    runs: Vec<(u64, u32, Weak<[Facet]>, usize)>,
    /// The number of the next run that keeps facets.
    next_run: u64,
    next_facet: u32,
    next_face: u64,
    /// Counts the times all was dropped: a run begun before does not
    /// hand in what it made.
    generation: u64,
    /// The hash of each file read as last seen.
    sources: HashMap<PathBuf, u64>,
    stats: CacheStats,
}

impl Inner {
    fn new(capacity: usize, min_cost: Duration) -> Inner {
        Inner {
            capacity,
            min_cost,
            entries: HashMap::new(),
            lru: BTreeMap::new(),
            tick: 0,
            bytes: 0,
            runs: Vec::new(),
            next_run: 0,
            next_facet: 0,
            next_face: 0,
            generation: 0,
            sources: HashMap::new(),
            stats: CacheStats::default(),
        }
    }

    fn remove(&mut self, key: u128) -> bool {
        let Some(e) = self.entries.remove(&key) else {
            return false;
        };
        self.lru.remove(&e.tick);
        self.bytes -= e.bytes;
        true
    }

    /// The bytes of the facets still kept; forgets those no entry keeps.
    fn run_bytes(&mut self) -> usize {
        self.runs.retain(|(_, _, w, _)| w.strong_count() > 0);
        self.runs.iter().map(|(.., b)| b).sum()
    }

    fn evict(&mut self) {
        let mut runs = self.run_bytes();
        let mut n = 0;
        while self.bytes + runs > self.capacity {
            let Some((_, key)) = self.lru.pop_first() else {
                break;
            };
            if let Some(e) = self.entries.remove(&key) {
                self.bytes -= e.bytes;
                self.stats.evicted += 1;
                n += 1;
                // The facets go with the last entry that keeps them.
                if n % 8 == 0 || self.bytes + runs <= self.capacity {
                    drop(e);
                    runs = self.run_bytes();
                }
            }
        }
        self.run_bytes();
    }

    /// Drops the entries that `drop` picks; how many.
    fn drop_where(&mut self, drop: impl Fn(&Entry) -> bool) -> usize {
        let keys: Vec<u128> = self
            .entries
            .iter()
            .filter(|(_, e)| drop(e))
            .map(|(k, _)| *k)
            .collect();
        for k in &keys {
            self.remove(*k);
        }
        self.stats.invalidated += keys.len() as u64;
        self.run_bytes();
        keys.len()
    }

    fn clear(&mut self) {
        self.entries.clear();
        self.lru.clear();
        self.bytes = 0;
        self.runs.clear();
        self.sources.clear();
        self.next_facet = 0;
        self.generation += 1;
    }
}

/// Results of runs of an OpenSCAD file kept for the next runs of it, while
/// it is open: what each module made, by the module, the values of its
/// arguments, of the `$` variables it sees, of every variable it and the
/// functions and modules it calls read, and its children; and what each
/// union, difference, intersection, hull, Minkowski sum, extrusion and
/// import made, by what went in. A run with a parameter changed makes again
/// only what depends on it, and makes the same as without the cache.
///
/// The one who opens the file owns the cache (one for each open file, as
/// OpenSolve keeps one for each part), and gives it to every run of the
/// file: [`crate::build_with_cache`], [`crate::diagnose_with_cache`], or
/// [`crate::Run::cache`]. It may be cloned (each clone is the same cache)
/// and used by runs on several threads at once. It holds at most its
/// capacity in bytes, the results used longest ago dropped first. A file
/// included, used or imported that changes drops the results that depend
/// on it (the next run finds it changed; [`Cache::invalidate`] drops them
/// at once); the main file's code is part of the keys, so that what an
/// edit does not touch is still used.
#[derive(Clone)]
pub struct Cache(Arc<Mutex<Inner>>);

impl Default for Cache {
    fn default() -> Cache {
        Cache::new()
    }
}

impl std::fmt::Debug for Cache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Cache({})", self.stats())
    }
}

/// What a cache holds and did: entries and their bytes (with the facets
/// they keep), the capacity, results used, looked up in vain, kept,
/// dropped for room and dropped because a file changed, and the runs that
/// used it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CacheStats {
    pub entries: usize,
    pub bytes: usize,
    pub capacity: usize,
    pub hits: u64,
    pub misses: u64,
    pub stored: u64,
    pub evicted: u64,
    pub invalidated: u64,
    pub runs: u64,
}

impl std::fmt::Display for CacheStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} entries, {:.1} of {:.0} MB, {} hits, {} misses, {} stored, {} evicted, {} invalidated, {} runs",
            self.entries,
            self.bytes as f64 / 1048576.0,
            self.capacity as f64 / 1048576.0,
            self.hits,
            self.misses,
            self.stored,
            self.evicted,
            self.invalidated,
            self.runs
        )
    }
}

impl Cache {
    /// A cache of [`DEFAULT_CAPACITY`] bytes.
    pub fn new() -> Cache {
        Cache::with_capacity(DEFAULT_CAPACITY)
    }

    /// A cache of at most `bytes` (0 keeps nothing).
    pub fn with_capacity(bytes: usize) -> Cache {
        Cache::with_limits(bytes, DEFAULT_MIN_COST)
    }

    /// A cache of at most `bytes` that keeps only results that took at
    /// least `min_cost` to make (`Duration::ZERO` keeps all).
    pub fn with_limits(bytes: usize, min_cost: Duration) -> Cache {
        Cache(Arc::new(Mutex::new(Inner::new(bytes, min_cost))))
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn stats(&self) -> CacheStats {
        let mut c = self.lock();
        let runs = c.run_bytes();
        CacheStats {
            entries: c.entries.len(),
            bytes: c.bytes + runs,
            capacity: c.capacity,
            ..c.stats
        }
    }

    /// Holds at most `bytes` from now on.
    pub fn set_capacity(&self, bytes: usize) {
        let mut c = self.lock();
        c.capacity = bytes;
        c.evict();
    }

    /// Drops all results.
    pub fn clear(&self) {
        self.lock().clear();
    }

    /// Drops the results that depend on the file at `path` (one included,
    /// used or imported), as when it changed; how many.
    pub fn invalidate(&self, path: &Path) -> usize {
        let id = crate::cache::id(path);
        let mut c = self.lock();
        c.sources.remove(path);
        c.drop_where(|e| e.files.has(id) || reads(e, path))
    }
}

fn reads(e: &Entry, path: &Path) -> bool {
    e.events
        .iter()
        .any(|ev| matches!(ev, Event::Read(p, _) if p == path))
}

/// A file a run reads: its path, a hash of its contents, and whether it
/// is code (included or used) rather than imported.
pub(crate) struct Source {
    pub path: PathBuf,
    pub hash: u64,
    pub code: bool,
}

/// What a result a run made is kept as until the run ends.
struct Pending {
    made: Arc<Option<Geom>>,
    print: u64,
    events: Arc<[Event]>,
    inner: Arc<[Use]>,
    files: Files,
    bytes: usize,
}

/// One run with a cache.
pub(crate) struct Session {
    cache: Cache,
    generation: u64,
    start: u32,
    face_start: u64,
    /// The runs of facets this run's table goes on from.
    known: HashSet<u64>,
    /// The runs of facets of the results it used.
    runs: Vec<Run>,
    /// The results made or used in this run, in order: those within a
    /// result are those after the place its making began.
    log: Vec<Use>,
    /// The same by key, with the facets of each and whether it is shared.
    ///
    /// A run without the cache names the faces of what it makes anew each
    /// time, but for what it shares: a module called again alike within
    /// the run, a batch of booleans made again alike. So must a run with
    /// it: a result is used at most once a run, unless it is shared and
    /// the same as the one met before; and a result is not used when one
    /// it shared within it was met in this run as another (it would have
    /// been shared, with this run's names).
    seen: HashMap<u128, (u64, bool)>,
    pending: HashMap<u128, Pending>,
    pending_bytes: usize,
    pub min_cost: Duration,
    hits: u64,
    misses: u64,
}

impl Session {
    /// Begins a run of the file at `main` that reads `sources`: results that
    /// depend on a file changed since the last run are dropped. Gives the
    /// table of facets the run builds in, and the first name of its faces.
    pub fn begin(cache: &Cache, sources: &[Source], main: &Path) -> (Session, Facets, u64) {
        let mut c = cache.lock();
        let mut changed: Vec<&Source> = Vec::new();
        for s in sources {
            if s.path == main {
                continue;
            }
            if let Some(old) = c.sources.insert(s.path.clone(), s.hash)
                && old != s.hash
            {
                changed.push(s);
            }
        }
        if !changed.is_empty() {
            let ids: Vec<u32> = changed
                .iter()
                .filter(|s| s.code)
                .map(|s| crate::cache::id(&s.path))
                .collect();
            let paths: Vec<&Path> = changed
                .iter()
                .filter(|s| !s.code)
                .map(|s| s.path.as_path())
                .collect();
            c.drop_where(|e| {
                ids.iter().any(|id| e.files.has(*id)) || paths.iter().any(|p| reads(e, p))
            });
        }
        if c.next_facet > u32::MAX - 2 * WINDOW {
            // The numbers are used up: all begins anew.
            let sources = std::mem::take(&mut c.sources);
            c.clear();
            c.sources = sources;
        }
        let start = c.next_facet;
        c.next_facet = start + WINDOW;
        let face_start = c.next_face;
        c.next_face = face_start.saturating_add(FACE_WINDOW);
        let kept: Vec<Run> = c
            .runs
            .iter()
            .filter_map(|(id, at, w, _)| Some((*id, *at, w.upgrade()?)))
            .collect();
        c.stats.runs += 1;
        let session = Session {
            cache: cache.clone(),
            generation: c.generation,
            start,
            face_start,
            known: kept.iter().map(|(id, ..)| *id).collect(),
            runs: Vec::new(),
            log: Vec::new(),
            seen: HashMap::new(),
            pending: HashMap::new(),
            pending_bytes: 0,
            min_cost: c.min_cost,
            hits: 0,
            misses: 0,
        };
        let table = kept
            .into_iter()
            .filter(|(.., f)| !f.is_empty())
            .map(|(_, at, f)| (at, f))
            .collect();
        (session, Facets::after(table, start), face_start)
    }

    /// Where the making of a result begins: what [`Session::keep`] is
    /// given.
    pub fn mark(&self) -> usize {
        self.log.len()
    }

    /// The result kept under `key`, with what it told, and whether it was
    /// met in this run before; none if it was used or made in this run
    /// already (unless it is `shared` and the same), if what it shared was
    /// met as another, if `fits` refuses what it told, or, with `again`,
    /// if it was not met before.
    pub fn recall(
        &mut self,
        key: u128,
        shared: bool,
        again: bool,
        fits: impl FnOnce(&[Event]) -> bool,
    ) -> Option<(Option<Geom>, Arc<[Event]>, bool)> {
        let before = self.seen.get(&key).copied();
        if (before.is_some() && !shared) || (again && before.is_none()) {
            return None;
        }
        let found = {
            let mut c = self.cache.lock();
            let tick = c.tick + 1;
            // A cache cleared since this run began has none of its runs.
            let current = c.generation == self.generation;
            match c.entries.get_mut(&key) {
                Some(e) if current && e.runs.iter().all(|(id, ..)| self.known.contains(id)) => {
                    let old = std::mem::replace(&mut e.tick, tick);
                    let found = (
                        e.made.clone(),
                        e.events.clone(),
                        e.runs.clone(),
                        e.inner.clone(),
                        e.print,
                    );
                    c.lru.remove(&old);
                    c.lru.insert(tick, key);
                    c.tick = tick;
                    Some(found)
                }
                _ => None,
            }
        };
        let Some((made, events, runs, inner, print)) = found else {
            self.misses += 1;
            return None;
        };
        let met_as_another =
            |(k, p, shared): &Use| *shared && self.seen.get(k).is_some_and(|(q, _)| q != p);
        if before.is_some_and(|(p, _)| p != print)
            || inner.iter().any(met_as_another)
            || !fits(&events)
        {
            self.misses += 1;
            return None;
        }
        for r in runs.iter() {
            if !self.runs.iter().any(|(id, ..)| *id == r.0) {
                self.runs.push(r.clone());
            }
        }
        self.saw((key, print, shared));
        for u in inner.iter() {
            self.saw(*u);
        }
        self.hits += 1;
        Some(((*made).clone(), events, before.is_some()))
    }

    /// Notes a result made or used in this run: its key, its facets
    /// ([`fingerprint`]) and whether it is shared.
    pub fn saw(&mut self, u: Use) {
        self.log.push(u);
        self.seen.entry(u.0).or_insert((u.1, u.2));
    }

    /// Keeps what was made under `key` since `mark` (`shared`: a module call
    /// the run shares), with what it told on the way, if it took `cost` or
    /// more (unknown: kept) or is shared; notes it only unless `wanted`.
    #[allow(clippy::too_many_arguments)]
    pub fn keep(
        &mut self,
        key: u128,
        shared: bool,
        made: &Option<Geom>,
        events: &[Event],
        mark: usize,
        files: Files,
        cost: Option<Duration>,
        wanted: bool,
    ) {
        let inner: Arc<[Use]> = self.log.get(mark..).unwrap_or_default().into();
        let print = fingerprint(made);
        self.saw((key, print, shared));
        // A shared one is kept however quickly made: a result kept that
        // shares it is used with it, its faces' names the same.
        let cheap = !shared && cost.is_some_and(|c| c < self.min_cost);
        if !wanted || cheap || self.pending.contains_key(&key) {
            return;
        }
        let bytes = 64
            + made.as_ref().map_or(0, geom_bytes)
            + events.iter().map(Event::bytes).sum::<usize>();
        let capacity = self.cache.lock().capacity;
        if self.pending_bytes + bytes > capacity {
            return;
        }
        self.pending_bytes += bytes;
        self.pending.insert(
            key,
            Pending {
                made: Arc::new(made.clone()),
                print,
                events: events.into(),
                inner,
                files,
                bytes,
            },
        );
    }

    /// Ends the run: hands what it kept, and the facets they need, to the
    /// cache (unless the cache was cleared meanwhile). `facets` is the run's
    /// table, `next_face` the next name of a face it would have given.
    pub fn finish(self, facets: &Facets, next_face: u64) {
        let mut c = self.cache.lock();
        c.stats.hits += self.hits;
        c.stats.misses += self.misses;
        let current = c.generation == self.generation;
        let last = current && c.next_facet == self.start + WINDOW;
        let last_face = current && c.next_face == self.face_start.saturating_add(FACE_WINDOW);
        let (start, own) = facets.own();
        if !current || self.pending.is_empty() || start != self.start || own.len() > WINDOW as usize
        {
            if last {
                c.next_facet = self.start;
            }
            if last_face {
                c.next_face = self.face_start;
            }
            return;
        }
        let own: Arc<[Facet]> = own.into();
        let own_bytes = own.len() * std::mem::size_of::<Facet>();
        let id = c.next_run;
        c.next_run += 1;
        c.runs.push((id, start, Arc::downgrade(&own), own_bytes));
        let mut runs = self.runs;
        runs.push((id, start, own.clone()));
        let runs: Arc<[Run]> = runs.into();
        if last {
            c.next_facet = start + own.len() as u32;
        }
        if last_face {
            c.next_face = next_face.max(self.face_start);
        }
        for (key, p) in self.pending {
            c.remove(key);
            c.tick += 1;
            let tick = c.tick;
            c.lru.insert(tick, key);
            c.bytes += p.bytes;
            c.stats.stored += 1;
            c.entries.insert(
                key,
                Entry {
                    made: p.made,
                    print: p.print,
                    events: p.events,
                    inner: p.inner,
                    runs: runs.clone(),
                    files: p.files,
                    bytes: p.bytes,
                    tick,
                },
            );
        }
        drop(own);
        c.evict();
    }
}

/// A result made or used in a run: its key, its facets, whether shared.
pub(crate) type Use = (u128, u64, bool);

/// Which facets a result's solid has, by their numbers: two results alike
/// but made apart have other ones, and so other names of faces.
pub(crate) fn fingerprint(made: &Option<Geom>) -> u64 {
    match made {
        Some(Geom::D3(s)) => {
            let mut h = H128::new(0xF1A6);
            h.u64(s.facets.len() as u64);
            for f in &s.facets {
                h.u64(u64::from(*f));
            }
            h.finish() as u64
        }
        Some(Geom::D2(_)) => 1,
        None => 0,
    }
}

/// About how many bytes a shape takes.
pub(crate) fn geom_bytes(g: &Geom) -> usize {
    match g {
        Geom::D3(s) => s.bytes(),
        Geom::D2(r) => 32 + r.loops.iter().map(|l| 24 + l.len() * 16).sum::<usize>(),
    }
}

/// A hash of 128 bits, quick, for keys: two lanes of different mixing.
#[derive(Clone, Debug)]
pub(crate) struct H128 {
    a: u64,
    b: u64,
}

impl H128 {
    pub fn new(tag: u64) -> H128 {
        H128 {
            a: 0x243F_6A88_85A3_08D3 ^ tag,
            b: 0x1319_8A2E_0370_7344 ^ tag.rotate_left(17),
        }
    }

    #[inline]
    pub fn u64(&mut self, x: u64) {
        self.a = (self.a.rotate_left(5) ^ x).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95);
        self.b = (self.b ^ x).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        self.b ^= self.b >> 29;
    }

    pub fn u128(&mut self, x: u128) {
        self.u64(x as u64);
        self.u64((x >> 64) as u64);
    }

    pub fn f64(&mut self, x: f64) {
        self.u64(x.to_bits());
    }

    pub fn bytes(&mut self, b: &[u8]) {
        self.u64(b.len() as u64);
        let mut chunks = b.chunks_exact(8);
        for c in &mut chunks {
            self.u64(u64::from_le_bytes(c.try_into().unwrap_or_default()));
        }
        let rest = chunks.remainder();
        if !rest.is_empty() {
            let mut w = [0u8; 8];
            w[..rest.len()].copy_from_slice(rest);
            self.u64(u64::from_le_bytes(w));
        }
    }

    pub fn str(&mut self, s: &str) {
        self.bytes(s.as_bytes());
    }

    pub fn finish(&self) -> u128 {
        fn mix(mut x: u64) -> u64 {
            x ^= x >> 33;
            x = x.wrapping_mul(0xff51_afd7_ed55_8ccd);
            x ^= x >> 33;
            x = x.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
            x ^ (x >> 33)
        }
        (u128::from(mix(self.a ^ self.b.rotate_left(31))) << 64) | u128::from(mix(self.b ^ self.a))
    }
}

/// A hash of bytes, for the contents of a file.
pub(crate) fn hash_bytes(b: &[u8]) -> u64 {
    let mut h = H128::new(0xF11E);
    h.bytes(b);
    h.finish() as u64
}
