//! The evaluation of a parsed file into shapes, by OpenSCAD's rules: in a
//! scope the assignments come first, in their order, a name assigned twice
//! taking the last value at the place of the first; functions and modules
//! are known in the whole scope; `$` names pass from caller to callee.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use opensolve_kernel::Op;

use crate::Error;
use crate::ast::{Arg, Elem, Expr, Func, Modifier, Module, Pos, Stmt};
use crate::builtins;
use crate::geom::{self, Builder, Geom, IDENTITY, Matrix, V2, V3, cos_deg, fragments, sin_deg};
use crate::parser::parse;
use crate::value::{self, MOST, Value, item, repr};

/// Deeper calls than this end the evaluation: a recursion without end.
const DEPTH: usize = 3000;
/// The most pieces a circle is cut into.
const PIECES: usize = 1000;
/// More notes than this are counted, not kept.
const NOTES: usize = 40;
/// More marks than this are dropped.
const MARKS: usize = 500;

/// A scope: its names, the scope it is written in (`parent`), and for a
/// call the scope it was called from (`caller`), where `$` names are found.
#[derive(Default)]
pub struct Frame {
    vars: RefCell<Vec<(Rc<str>, Value)>>,
    funcs: RefCell<Vec<(Rc<str>, Rc<Func>)>>,
    mods: RefCell<Vec<(Rc<str>, Rc<Module>)>>,
    parent: Option<Rc<Frame>>,
    caller: Option<Rc<Frame>>,
    /// A module's children and the scope they were written in.
    children: Option<(Rc<[Stmt]>, Rc<Frame>)>,
}

impl std::fmt::Debug for Frame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Frame")
    }
}

impl Frame {
    fn inner(parent: &Rc<Frame>) -> Rc<Frame> {
        Rc::new(Frame {
            parent: Some(parent.clone()),
            ..Frame::default()
        })
    }

    fn set(&self, name: &Rc<str>, v: Value) {
        let mut vars = self.vars.borrow_mut();
        match vars.iter_mut().find(|(n, _)| n == name) {
            Some(slot) => slot.1 = v,
            None => vars.push((name.clone(), v)),
        }
    }

    fn own(&self, name: &str) -> Option<Value> {
        self.vars
            .borrow()
            .iter()
            .find(|(n, _)| &**n == name)
            .map(|(_, v)| v.clone())
    }

    /// A name's value: `$` names by who called, others by where written.
    fn get(self: &Rc<Frame>, name: &str) -> Option<Value> {
        let dynamic = name.starts_with('$');
        let mut f = Some(self.clone());
        while let Some(x) = f {
            if let Some(v) = x.own(name) {
                return Some(v);
            }
            f = if dynamic {
                x.caller.clone().or_else(|| x.parent.clone())
            } else {
                x.parent.clone()
            };
        }
        None
    }

    fn func(self: &Rc<Frame>, name: &str) -> Option<(Rc<Func>, Rc<Frame>)> {
        let mut f = Some(self.clone());
        while let Some(x) = f {
            if let Some((_, d)) = x.funcs.borrow().iter().find(|(n, _)| &**n == name) {
                return Some((d.clone(), x.clone()));
            }
            f = x.parent.clone();
        }
        None
    }

    fn module(self: &Rc<Frame>, name: &str) -> Option<(Rc<Module>, Rc<Frame>)> {
        let mut f = Some(self.clone());
        while let Some(x) = f {
            if let Some((_, d)) = x.mods.borrow().iter().find(|(n, _)| &**n == name) {
                return Some((d.clone(), x.clone()));
            }
            f = x.parent.clone();
        }
        None
    }

    /// The children of the module being run here.
    fn children(self: &Rc<Frame>) -> Option<(Rc<[Stmt]>, Rc<Frame>)> {
        let mut f = Some(self.clone());
        while let Some(x) = f {
            if let Some(c) = &x.children {
                return Some(c.clone());
            }
            f = x.parent.clone();
        }
        None
    }
}

type R<T> = Result<T, Error>;

/// Evaluated arguments of a call.
struct Args {
    pos: Vec<Value>,
    named: Vec<(Rc<str>, Value)>,
}

impl Args {
    fn get(&self, name: &str, at: usize) -> Option<&Value> {
        self.named
            .iter()
            .find(|(n, _)| &**n == name)
            .map(|(_, v)| v)
            .or_else(|| self.pos.get(at))
            .filter(|v| !matches!(v, Value::Undef))
    }

    fn num(&self, name: &str, at: usize) -> Option<f64> {
        self.get(name, at).and_then(Value::num)
    }

    fn bool(&self, name: &str, at: usize) -> bool {
        self.get(name, at).is_some_and(Value::truthy)
    }
}

pub struct Interp {
    /// The files read: their paths and texts; file 0 is the main one.
    pub files: Vec<(PathBuf, String)>,
    sources: HashMap<PathBuf, String>,
    used: Vec<Rc<Frame>>,
    pub notes: Vec<String>,
    /// The notes again with where they are, for an editor: no place in
    /// their text, echoes too, all of them up to [`MARKS`].
    pub marks: Vec<crate::Note>,
    dropped: usize,
    pub geo: Builder,
    depth: usize,
    /// What a `!` made: the whole result.
    root: Option<Option<Geom>>,
    /// The modules being run, the innermost last: `parent_module`.
    modules: Vec<Rc<str>>,
    /// What `rands` draws from: seeded anew by a seed, else the same
    /// numbers in each run, so that a part is built the same each time.
    rng: builtins::Mt19937,
}

impl Interp {
    pub fn new(sources: HashMap<PathBuf, String>) -> Interp {
        Interp {
            files: Vec::new(),
            sources,
            used: Vec::new(),
            notes: Vec::new(),
            marks: Vec::new(),
            dropped: 0,
            geo: Builder::default(),
            depth: 0,
            root: None,
            modules: Vec::new(),
            rng: builtins::Mt19937::new(5489),
        }
    }

    pub fn note(&mut self, pos: Option<Pos>, text: impl Into<String>) {
        let text = text.into();
        self.mark(pos, &text, false);
        let text = match pos {
            Some(p) => format!("{}: {text}", self.place(p)),
            None => text,
        };
        self.keep(text);
    }

    /// What `echo` writes: a note without its place, a mark with it.
    fn echo(&mut self, pos: Pos, text: String) {
        let text = format!("ECHO: {text}");
        self.mark(Some(pos), &text, true);
        self.keep(text);
    }

    fn mark(&mut self, pos: Option<Pos>, text: &str, echo: bool) {
        if self.marks.len() >= MARKS {
            return;
        }
        let p = pos.unwrap_or_default();
        let file = (p.file != 0)
            .then(|| self.files.get(p.file as usize).map(|f| f.0.clone()))
            .flatten();
        let mark = crate::Note {
            file,
            line: p.line,
            col: p.col,
            message: text.to_string(),
            echo,
        };
        if !self.marks.contains(&mark) {
            self.marks.push(mark);
        }
    }

    fn keep(&mut self, text: String) {
        if self.notes.contains(&text) {
            return;
        }
        if self.notes.len() < NOTES {
            self.notes.push(text);
        } else {
            self.dropped += 1;
        }
    }

    /// The notes, the last saying how many more there were.
    pub fn take_notes(&mut self) -> Vec<String> {
        let mut n = std::mem::take(&mut self.notes);
        if self.dropped > 0 {
            n.push(format!("and {} more", self.dropped));
        }
        n
    }

    /// `line 3` in the main file, `lib.scad, line 3` in another.
    fn place(&self, p: Pos) -> String {
        if p.file == 0 {
            return format!("line {}", p.line);
        }
        let name = self.files.get(p.file as usize).map_or(String::new(), |f| {
            f.0.file_name()
                .map_or(String::new(), |n| n.to_string_lossy().into_owned())
        });
        format!("{name}, line {}", p.line)
    }

    fn error(&self, p: Pos, message: impl Into<String>) -> Error {
        let mut e = Error::at(p.line, p.col, message);
        if p.file != 0 {
            e.file = self
                .files
                .get(p.file as usize)
                .map(|f| f.0.display().to_string());
        }
        e
    }

    /// Parses a file read before; errors carry its name.
    fn parse_file(&mut self, path: PathBuf) -> R<Vec<Stmt>> {
        let Some(text) = self.sources.get(&path).cloned() else {
            return Err(Error::at(
                0,
                0,
                format!("{} cannot be read", path.display()),
            ));
        };
        let index = self.files.len() as u32;
        self.files.push((path.clone(), text));
        let text = &self.files[index as usize].1;
        parse(text, index).map_err(|mut e| {
            if index != 0 {
                e.file = Some(path.display().to_string());
            }
            e
        })
    }

    /// A path in `include` or `use`, from the folder of the file it is in,
    /// else from the library folders: the first that was read.
    fn path_of(&self, from: Pos, path: &str) -> PathBuf {
        let dir = self
            .files
            .get(from.file as usize)
            .and_then(|f| f.0.parent())
            .map(PathBuf::from)
            .unwrap_or_default();
        let local = crate::plain(&dir.join(path));
        if self.sources.contains_key(&local) {
            return local;
        }
        crate::library::search()
            .iter()
            .map(|lib| crate::plain(&lib.join(path)))
            .find(|p| self.sources.contains_key(p))
            .unwrap_or(local)
    }

    /// The statements of a file with what it includes put in their place.
    pub fn load(&mut self, path: PathBuf, seen: &mut Vec<PathBuf>) -> R<Vec<Stmt>> {
        if seen.contains(&path) {
            return Ok(Vec::new());
        }
        seen.push(path.clone());
        let stmts = self.parse_file(path)?;
        let mut out = Vec::with_capacity(stmts.len());
        for s in stmts {
            match s {
                Stmt::Include(p, pos) => {
                    let file = self.path_of(pos, &p);
                    if !self.sources.contains_key(&file) {
                        return Err(
                            self.error(pos, format!("include <{p}>: the file cannot be read"))
                        );
                    }
                    out.extend(self.load(file, seen)?);
                }
                s => out.push(s),
            }
        }
        seen.pop();
        Ok(out)
    }

    /// The scope of the whole file, with OpenSCAD's constants and defaults.
    pub fn root() -> Rc<Frame> {
        let f = Rc::new(Frame::default());
        for (n, v) in [
            ("PI", Value::Num(std::f64::consts::PI)),
            ("$fn", Value::Num(0.0)),
            ("$fa", Value::Num(12.0)),
            ("$fs", Value::Num(2.0)),
            ("$t", Value::Num(0.0)),
            ("$preview", Value::Bool(false)),
            ("$children", Value::Num(0.0)),
        ] {
            f.set(&n.into(), v);
        }
        f
    }

    /// Runs the main file: what it makes, all of it as one.
    pub fn run(&mut self, stmts: &[Stmt], frame: &Rc<Frame>) -> R<Option<Geom>> {
        let made = self.exec(stmts, frame, None)?;
        if let Some(root) = self.root.take() {
            return Ok(root);
        }
        self.union(made, None)
    }

    /// `use <…>`: the file's modules and functions, its assignments in a
    /// scope of its own, nothing it makes.
    fn use_file(&mut self, path: &str, pos: Pos) -> R<()> {
        let file = self.path_of(pos, path);
        if !self.sources.contains_key(&file) {
            return Err(self.error(pos, format!("use <{path}>: the file cannot be read")));
        }
        if self.files.iter().any(|f| f.0 == file) {
            return Ok(());
        }
        let stmts = self.load(file, &mut Vec::new())?;
        let frame = Interp::root();
        let defs: Vec<Stmt> = stmts
            .into_iter()
            .filter(|s| {
                matches!(
                    s,
                    Stmt::Assign { .. } | Stmt::Module(..) | Stmt::Function(..) | Stmt::Use(..)
                )
            })
            .collect();
        self.exec(&defs, &frame, None)?;
        self.used.push(frame);
        Ok(())
    }

    fn find_func(&self, f: &Rc<Frame>, name: &str) -> Option<(Rc<Func>, Rc<Frame>)> {
        f.func(name)
            .or_else(|| self.used.iter().find_map(|u| u.func(name)))
    }

    fn find_module(&self, f: &Rc<Frame>, name: &str) -> Option<(Rc<Module>, Rc<Frame>)> {
        f.module(name)
            .or_else(|| self.used.iter().find_map(|u| u.module(name)))
    }

    /// The statements of a scope; with `only`, of the instantiations just
    /// those (as `children(i)` picks them). One shape per instantiation
    /// that makes one.
    fn exec(&mut self, stmts: &[Stmt], f: &Rc<Frame>, only: Option<&[usize]>) -> R<Vec<Geom>> {
        for s in stmts {
            match s {
                Stmt::Module(n, m) => f.mods.borrow_mut().push((n.clone(), m.clone())),
                Stmt::Function(n, d) => f.funcs.borrow_mut().push((n.clone(), d.clone())),
                Stmt::Use(p, pos) => self.use_file(p, *pos)?,
                _ => {}
            }
        }
        let mut order: Vec<(&Rc<str>, &Expr)> = Vec::new();
        for s in stmts {
            if let Stmt::Assign { name, value, .. } = s {
                match order.iter_mut().find(|o| o.0 == name) {
                    Some(o) => o.1 = value,
                    None => order.push((name, value)),
                }
            }
        }
        for (name, value) in order {
            let v = self.eval(value, f)?;
            f.set(name, v);
        }
        let mut out = Vec::new();
        let instances = stmts.iter().filter(|s| is_instance(s));
        for (k, s) in instances.enumerate() {
            if only.is_some_and(|o| !o.contains(&k)) {
                continue;
            }
            if let Some(g) = self.instance(s, f)? {
                out.push(g);
            }
        }
        Ok(out)
    }

    fn instance(&mut self, s: &Stmt, f: &Rc<Frame>) -> R<Option<Geom>> {
        let (made, modifier) = match s {
            Stmt::Call {
                modifier: Modifier::Disable | Modifier::Background,
                ..
            }
            | Stmt::If {
                modifier: Modifier::Disable | Modifier::Background,
                ..
            } => return Ok(None),
            Stmt::Call {
                name,
                args,
                children,
                modifier,
                pos,
            } => (self.call_module(name, args, children, *pos, f)?, *modifier),
            Stmt::If {
                cond,
                then,
                or,
                modifier,
            } => {
                let branch = if self.eval(cond, f)?.truthy() {
                    then
                } else {
                    or
                };
                let made = self.exec(branch, &Frame::inner(f), None)?;
                (self.union(made, None)?, *modifier)
            }
            Stmt::Block(b) => {
                let made = self.exec(b, &Frame::inner(f), None)?;
                (self.union(made, None)?, Modifier::None)
            }
            _ => return Ok(None),
        };
        if modifier == Modifier::Root && self.root.is_none() {
            self.root = Some(made.clone());
        }
        Ok(made)
    }

    /// Evaluates the arguments of a call; `$` names among them go to `into`.
    fn args(&mut self, args: &[Arg], f: &Rc<Frame>) -> R<Args> {
        let mut out = Args {
            pos: Vec::new(),
            named: Vec::new(),
        };
        for a in args {
            let v = match &a.value {
                Some(e) => self.eval(e, f)?,
                None => Value::Undef,
            };
            match &a.name {
                Some(n) => out.named.push((n.clone(), v)),
                None => out.pos.push(v),
            }
        }
        Ok(out)
    }

    /// A scope for the children of a built-in module, with the `$` names
    /// it was given.
    fn with_specials(f: &Rc<Frame>, a: &Args) -> Rc<Frame> {
        let inner = Frame::inner(f);
        for (n, v) in &a.named {
            if n.starts_with('$') {
                inner.set(n, v.clone());
            }
        }
        inner
    }

    /// A `$` value: given to the call, or of the scope.
    fn special(a: &Args, f: &Rc<Frame>, name: &str) -> f64 {
        a.named
            .iter()
            .find(|(n, _)| &**n == name)
            .map(|(_, v)| v.clone())
            .or_else(|| f.get(name))
            .and_then(|v| v.num())
            .unwrap_or(0.0)
    }

    /// OpenSCAD's number of pieces of a circle, at most [`PIECES`]: more
    /// would keep the app busy for minutes.
    fn fragments(&mut self, a: &Args, f: &Rc<Frame>, r: f64, pos: Pos) -> usize {
        let (fn_, fs, fa) = (
            Self::special(a, f, "$fn"),
            Self::special(a, f, "$fs"),
            Self::special(a, f, "$fa"),
        );
        let n = fragments(r, fn_, fs, fa);
        if n > PIECES {
            self.note(
                Some(pos),
                format!("{n} pieces of a circle are too many: {PIECES} made"),
            );
        }
        n.min(PIECES)
    }

    fn call_module(
        &mut self,
        name: &Rc<str>,
        args: &[Arg],
        children: &Rc<[Stmt]>,
        pos: Pos,
        f: &Rc<Frame>,
    ) -> R<Option<Geom>> {
        self.modules.push(name.clone());
        let made = self.module_call(name, args, children, pos, f);
        self.modules.pop();
        made
    }

    fn module_call(
        &mut self,
        name: &Rc<str>,
        args: &[Arg],
        children: &Rc<[Stmt]>,
        pos: Pos,
        f: &Rc<Frame>,
    ) -> R<Option<Geom>> {
        if let Some((def, home)) = self.find_module(f, name) {
            return self.user_module(&def, &home, args, children, pos, f);
        }
        // These bind names, not arguments.
        match &**name {
            "for" | "intersection_for" => {
                let mut made = Vec::new();
                self.each(args, 0, f, &mut |me, inner| {
                    let m = me.exec(children, inner, None)?;
                    made.push(me.union(m, Some(pos))?);
                    Ok(())
                })?;
                return if &**name == "for" {
                    self.union(made.into_iter().flatten().collect(), Some(pos))
                } else if made.iter().any(Option::is_none) {
                    Ok(None)
                } else {
                    self.fold(made.into_iter().flatten().collect(), Op::Intersect, pos)
                };
            }
            "let" => {
                let inner = self.lets(args, f)?;
                let made = self.exec(children, &inner, None)?;
                return self.union(made, Some(pos));
            }
            _ => {}
        }
        let a = self.args(args, f)?;
        let inner = Self::with_specials(f, &a);
        let kids = |me: &mut Self| me.exec(children, &inner, None);
        let geo = match &**name {
            "cube" => {
                let size = match a.get("size", 0) {
                    Some(Value::Num(s)) => [*s; 3],
                    Some(v) => v.point().unwrap_or([1.0; 3]),
                    None => [1.0; 3],
                };
                self.geo.cube(size, a.bool("center", 1)).map(Geom::D3)
            }
            "sphere" => {
                let r = a
                    .num("r", 0)
                    .or_else(|| a.num("d", usize::MAX).map(|d| d / 2.0))
                    .unwrap_or(1.0);
                let n = self.fragments(&a, f, r, pos);
                self.geo.sphere(r, n).map(Geom::D3)
            }
            "cylinder" => {
                let h = a.num("h", 0).unwrap_or(1.0);
                let r = a
                    .num("r", usize::MAX)
                    .or_else(|| a.num("d", usize::MAX).map(|d| d / 2.0));
                let r1 = a
                    .num("r1", 1)
                    .or_else(|| a.num("d1", usize::MAX).map(|d| d / 2.0))
                    .or(r)
                    .unwrap_or(1.0);
                let r2 = a
                    .num("r2", 2)
                    .or_else(|| a.num("d2", usize::MAX).map(|d| d / 2.0))
                    .or(r)
                    .unwrap_or(1.0);
                let n = self.fragments(&a, f, r1.max(r2), pos);
                self.geo
                    .cylinder(h, r1, r2, a.bool("center", 3), n)
                    .map(Geom::D3)
            }
            "polyhedron" => {
                let points = points3(a.get("points", 0));
                let faces = a.get("faces", 1).or_else(|| a.get("triangles", usize::MAX));
                let faces: Vec<Vec<usize>> = faces
                    .map(|v| v.items().unwrap_or_default())
                    .unwrap_or_default()
                    .iter()
                    .map(|face| {
                        face.nums()
                            .unwrap_or_default()
                            .iter()
                            .map(|i| *i as usize)
                            .collect()
                    })
                    .collect();
                match self.geo.polyhedron(&points, &faces) {
                    Ok(s) => Some(Geom::D3(s)),
                    Err(e) => {
                        self.note(Some(pos), e);
                        None
                    }
                }
            }
            "square" => {
                let size = match a.get("size", 0) {
                    Some(Value::Num(s)) => [*s; 2],
                    Some(v) => v.point().map_or([1.0; 2], |p| [p[0], p[1]]),
                    None => [1.0; 2],
                };
                self.geo.square(size, a.bool("center", 1)).map(Geom::D2)
            }
            "circle" => {
                let r = a
                    .num("r", 0)
                    .or_else(|| a.num("d", usize::MAX).map(|d| d / 2.0))
                    .unwrap_or(1.0);
                let n = self.fragments(&a, f, r, pos);
                self.geo.circle(r, n).map(Geom::D2)
            }
            "polygon" => {
                let points: Vec<V2> = points3(a.get("points", 0))
                    .iter()
                    .map(|p| [p[0], p[1]])
                    .collect();
                let paths: Vec<Vec<V2>> = match a.get("paths", 1) {
                    Some(v) => v
                        .items()
                        .unwrap_or_default()
                        .iter()
                        .map(|path| {
                            path.nums()
                                .unwrap_or_default()
                                .iter()
                                .filter_map(|i| points.get(*i as usize).copied())
                                .collect()
                        })
                        .collect(),
                    None => vec![points],
                };
                match self.geo.polygon(&paths) {
                    Ok(s) => s.map(Geom::D2),
                    Err(e) => {
                        self.note(Some(pos), e);
                        None
                    }
                }
            }
            "translate" | "rotate" | "scale" | "mirror" | "multmatrix" => {
                let m = matrix(name, &a);
                let made = kids(self)?;
                match m {
                    Some(m) => {
                        let one = self.union(made, Some(pos))?;
                        one.map(|g| self.transform(g, &m))
                    }
                    None => {
                        self.note(Some(pos), format!("{name}: its argument is not understood"));
                        self.union(made, Some(pos))?
                    }
                }
            }
            "resize" => {
                let made = kids(self)?;
                let one = self.union(made, Some(pos))?;
                one.map(|g| self.resize(g, &a))
            }
            "union" | "group" | "render" | "color" => {
                let made = kids(self)?;
                self.union(made, Some(pos))?
            }
            "difference" | "intersection" => {
                let made = kids(self)?;
                let op = if &**name == "difference" {
                    Op::Subtract
                } else {
                    Op::Intersect
                };
                self.fold(made, op, pos)?
            }
            "hull" => {
                let made = kids(self)?;
                self.hull(made, pos)
            }
            "linear_extrude" | "rotate_extrude" => {
                let made = kids(self)?;
                let shape = self.union(made, Some(pos))?;
                self.extrude(name, shape, &a, f, pos)?
            }
            "children" => self.children(&a, f, pos)?,
            "echo" => {
                let text = echo_text(&a);
                self.echo(pos, text);
                let made = kids(self)?;
                self.union(made, Some(pos))?
            }
            "assert" => {
                self.check(&a, pos, args)?;
                let made = kids(self)?;
                self.union(made, Some(pos))?
            }
            "minkowski" => {
                let made = kids(self)?;
                self.minkowski(made, pos)?
            }
            "offset" => {
                let made = kids(self)?;
                let shape = self.union(made, Some(pos))?;
                self.offset(shape, &a, f, pos)?
            }
            "projection" | "text" | "import" | "surface" | "roof" | "fill" => {
                self.note(Some(pos), format!("not supported: {name}"));
                None
            }
            _ => {
                self.note(Some(pos), format!("unknown module {name}"));
                None
            }
        };
        Ok(geo)
    }

    fn user_module(
        &mut self,
        def: &Rc<Module>,
        home: &Rc<Frame>,
        args: &[Arg],
        children: &Rc<[Stmt]>,
        pos: Pos,
        f: &Rc<Frame>,
    ) -> R<Option<Geom>> {
        let a = self.args(args, f)?;
        let frame = Rc::new(Frame {
            parent: Some(home.clone()),
            caller: Some(f.clone()),
            children: Some((children.clone(), f.clone())),
            ..Frame::default()
        });
        let count = children.iter().filter(|s| is_instance(s)).count();
        frame.set(&"$children".into(), Value::Num(count as f64));
        self.bind(&def.params, a, &frame)?;
        self.enter(pos)?;
        let made = self.exec(&def.body, &frame, None);
        self.depth -= 1;
        self.union(made?, Some(pos))
    }

    fn enter(&mut self, pos: Pos) -> R<()> {
        self.depth += 1;
        if self.depth > DEPTH {
            self.depth -= 1;
            return Err(self.error(pos, "calls go too deep: a recursion without end?"));
        }
        Ok(())
    }

    /// Parameters bound in `frame`: by position, by name, else their
    /// defaults; `$` names given go along.
    fn bind(&mut self, params: &[Arg], a: Args, frame: &Rc<Frame>) -> R<()> {
        let mut pos = a.pos.into_iter();
        for p in params {
            let Some(name) = &p.name else { continue };
            let given = a
                .named
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, v)| v.clone());
            let next = pos.next();
            let v = match (given, next) {
                (Some(v), _) => v,
                (None, Some(v)) => v,
                (None, None) => match &p.value {
                    Some(e) => self.eval(e, frame)?,
                    None => Value::Undef,
                },
            };
            frame.set(name, v);
        }
        for (n, v) in a.named {
            if n.starts_with('$') {
                frame.set(&n, v);
            }
        }
        Ok(())
    }

    /// `children()`, `children(i)`, `children([i, j])`.
    fn children(&mut self, a: &Args, f: &Rc<Frame>, pos: Pos) -> R<Option<Geom>> {
        let Some((stmts, home)) = f.children() else {
            return Ok(None);
        };
        let only: Option<Vec<usize>> = a.get("index", 0).map(|v| {
            v.items()
                .unwrap_or_default()
                .iter()
                .filter_map(Value::num)
                .filter(|i| *i >= 0.0)
                .map(|i| i as usize)
                .collect()
        });
        let frame = Rc::new(Frame {
            parent: Some(home),
            caller: Some(f.clone()),
            ..Frame::default()
        });
        let made = self.exec(&stmts, &frame, only.as_deref())?;
        self.union(made, Some(pos))
    }

    /// Runs `body` for each value of each variable of a `for`, nested.
    fn each(
        &mut self,
        vars: &[Arg],
        k: usize,
        f: &Rc<Frame>,
        body: &mut dyn FnMut(&mut Self, &Rc<Frame>) -> R<()>,
    ) -> R<()> {
        let Some(var) = vars.get(k) else {
            return body(self, f);
        };
        let (Some(name), Some(e)) = (&var.name, &var.value) else {
            return self.each(vars, k + 1, f, body);
        };
        let over = self.eval(e, f)?;
        let items = over.items().map_err(|m| Error::at(0, 0, m))?;
        for v in items {
            let inner = Frame::inner(f);
            inner.set(name, v);
            self.each(vars, k + 1, &inner, body)?;
        }
        Ok(())
    }

    fn check(&mut self, a: &Args, pos: Pos, args: &[Arg]) -> R<()> {
        if a.pos.first().is_some_and(Value::truthy) {
            return Ok(());
        }
        let what = match a.pos.get(1) {
            Some(v) => v.to_string(),
            None => args
                .first()
                .and_then(|a| a.value.as_ref())
                .map_or_else(String::new, |e| format!("{e} is false")),
        };
        Err(self.error(pos, format!("assertion failed: {what}")))
    }

    /// Several shapes as one: the 3D ones joined, or else the 2D ones.
    fn union(&mut self, made: Vec<Geom>, pos: Option<Pos>) -> R<Option<Geom>> {
        if made.len() <= 1 {
            return Ok(made.into_iter().next());
        }
        let (flat, solid): (Vec<Geom>, Vec<Geom>) =
            made.into_iter().partition(|g| matches!(g, Geom::D2(_)));
        if !flat.is_empty() && !solid.is_empty() {
            self.note(pos, "2D and 3D shapes together: the 2D ones are left out");
        }
        let (list, is_flat) = if solid.is_empty() {
            (flat, true)
        } else {
            (solid, false)
        };
        let solids = list
            .into_iter()
            .map(|g| {
                let (Geom::D2(s) | Geom::D3(s)) = g;
                s
            })
            .collect();
        let out = self.join(solids, is_flat, pos)?;
        Ok(out.map(|s| if is_flat { Geom::D2(s) } else { Geom::D3(s) }))
    }

    /// Solids joined in pairs, the pairs again, and so on: the meshes the
    /// booleans take stay small, and those apart are only put together.
    fn join(
        &mut self,
        mut list: Vec<opensolve_kernel::Solid>,
        flat: bool,
        pos: Option<Pos>,
    ) -> R<Option<opensolve_kernel::Solid>> {
        while list.len() > 1 {
            let mut next = Vec::with_capacity(list.len().div_ceil(2));
            let mut it = list.into_iter();
            while let Some(a) = it.next() {
                next.push(match it.next() {
                    Some(b) => self.boolean(&a, &b, Op::Add, flat, pos)?,
                    None => a,
                });
            }
            list = next;
        }
        Ok(list.pop())
    }

    fn boolean(
        &mut self,
        a: &opensolve_kernel::Solid,
        b: &opensolve_kernel::Solid,
        op: Op,
        flat: bool,
        pos: Option<Pos>,
    ) -> R<opensolve_kernel::Solid> {
        self.geo
            .boolean(a, b, op, flat)
            .map_err(|e| self.error(pos.unwrap_or_default(), e))
    }

    /// The first shape and the others by `op`, those of the other dimension
    /// left out.
    fn fold(&mut self, made: Vec<Geom>, op: Op, pos: Pos) -> R<Option<Geom>> {
        let mut it = made.into_iter();
        let Some(first) = it.next() else {
            return Ok(None);
        };
        let flat = matches!(first, Geom::D2(_));
        let (Geom::D2(mut acc) | Geom::D3(mut acc)) = first;
        let mut others = Vec::new();
        for g in it {
            match (g, flat) {
                (Geom::D2(s), true) | (Geom::D3(s), false) => others.push(s),
                _ => self.note(
                    Some(pos),
                    "2D and 3D shapes together: the others are left out",
                ),
            }
        }
        if op == Op::Subtract {
            // All that is cut away at once: much faster than one by one.
            if let Some(cut) = self.join(others, flat, Some(pos))? {
                acc = self.boolean(&acc, &cut, op, flat, Some(pos))?;
            }
        } else {
            for s in &others {
                acc = self.boolean(&acc, s, op, flat, Some(pos))?;
            }
        }
        Ok(Some(if flat { Geom::D2(acc) } else { Geom::D3(acc) }))
    }

    fn transform(&mut self, g: Geom, m: &Matrix) -> Geom {
        match g {
            Geom::D3(s) => Geom::D3(self.geo.transformed(&s, m)),
            Geom::D2(s) => Geom::D2(self.geo.transformed_2d(&s, m)),
        }
    }

    /// `resize(newsize, auto)`: scaled to the size asked, a 0 kept or, with
    /// `auto`, scaled as the others.
    fn resize(&mut self, g: Geom, a: &Args) -> Geom {
        let (Geom::D2(s) | Geom::D3(s)) = &g;
        let Some((lo, hi)) = s.bounds() else {
            return g;
        };
        let want = a
            .get("newsize", 0)
            .and_then(Value::nums)
            .unwrap_or_default();
        let auto: Vec<bool> = match a.get("auto", 1) {
            Some(Value::List(v)) => v.iter().map(Value::truthy).collect(),
            Some(v) => vec![v.truthy(); 3],
            None => vec![false; 3],
        };
        let dims = if matches!(g, Geom::D2(_)) { 2 } else { 3 };
        let mut k = [1.0; 3];
        let mut uniform = None;
        for i in 0..dims {
            let size = hi[i] - lo[i];
            if let Some(&w) = want.get(i).filter(|w| **w > 0.0)
                && size > 1e-12
            {
                k[i] = w / size;
                uniform = Some(uniform.map_or(k[i], |u: f64| u.max(k[i])));
            }
        }
        for (i, ki) in k.iter_mut().enumerate().take(dims) {
            let asked = want.get(i).is_some_and(|w| *w > 0.0);
            if !asked && auto.get(i).copied().unwrap_or(false) {
                *ki = uniform.unwrap_or(1.0);
            }
        }
        let m = [
            [k[0], 0.0, 0.0, 0.0],
            [0.0, k[1], 0.0, 0.0],
            [0.0, 0.0, k[2], 0.0],
        ];
        self.transform(g, &m)
    }

    /// `minkowski()`: the sum of its children, one after the other.
    fn minkowski(&mut self, made: Vec<Geom>, pos: Pos) -> R<Option<Geom>> {
        let mut it = made.into_iter();
        let Some(mut acc) = it.next() else {
            return Ok(None);
        };
        for g in it {
            let sum = match (&acc, &g) {
                (Geom::D3(a), Geom::D3(b)) => self.geo.minkowski(a, b).map(|s| s.map(Geom::D3)),
                (Geom::D2(a), Geom::D2(b)) => self.geo.minkowski_2d(a, b).map(|s| s.map(Geom::D2)),
                _ => {
                    self.note(Some(pos), "minkowski: 2D and 3D shapes together");
                    continue;
                }
            };
            match sum {
                Ok(Some(s)) => acc = s,
                Ok(None) => return Ok(None),
                Err(e) => return Err(self.error(pos, e)),
            }
        }
        Ok(Some(acc))
    }

    /// `offset(r)`, `offset(delta, chamfer)` of a 2D shape.
    fn offset(
        &mut self,
        shape: Option<Geom>,
        a: &Args,
        f: &Rc<Frame>,
        pos: Pos,
    ) -> R<Option<Geom>> {
        let slab = match shape {
            None => return Ok(None),
            Some(Geom::D3(_)) => {
                self.note(Some(pos), "offset: needs a 2D shape");
                return Ok(None);
            }
            Some(Geom::D2(s)) => s,
        };
        let (d, join) = match (a.num("r", 0), a.num("delta", usize::MAX)) {
            (Some(r), _) => {
                let n = self.fragments(a, f, r.abs(), pos);
                (r, geom::Join::Round(n))
            }
            (None, Some(d)) if a.bool("chamfer", usize::MAX) => (d, geom::Join::Chamfer),
            (None, Some(d)) => (d, geom::Join::Miter),
            (None, None) => (1.0, geom::Join::Miter),
        };
        match self.geo.offset(&slab, d, join) {
            Ok(s) => Ok(s.map(Geom::D2)),
            Err(e) => Err(self.error(pos, e)),
        }
    }

    fn hull(&mut self, made: Vec<Geom>, pos: Pos) -> Option<Geom> {
        let solid = made.iter().any(|g| matches!(g, Geom::D3(_)));
        let mut points: Vec<V3> = Vec::new();
        for g in &made {
            match (g, solid) {
                (Geom::D3(s), true) => points.extend(&s.points),
                (Geom::D2(s), false) => points.extend(s.points.iter().filter(|p| p[2] > 0.5)),
                _ => self.note(Some(pos), "hull: 2D and 3D shapes together"),
            }
        }
        if solid {
            self.geo.hull(&points).map(Geom::D3)
        } else {
            let flat: Vec<V2> = points.iter().map(|p| [p[0], p[1]]).collect();
            self.geo.hull_2d(&flat).map(Geom::D2)
        }
    }

    fn extrude(
        &mut self,
        name: &str,
        shape: Option<Geom>,
        a: &Args,
        f: &Rc<Frame>,
        pos: Pos,
    ) -> R<Option<Geom>> {
        let slab = match shape {
            None => return Ok(None),
            Some(Geom::D3(_)) => {
                self.note(Some(pos), format!("{name}: needs a 2D shape"));
                return Ok(None);
            }
            Some(Geom::D2(s)) => s,
        };
        let far = slab
            .points
            .iter()
            .map(|p| p[0].hypot(p[1]))
            .fold(0.0, f64::max);
        if name == "rotate_extrude" {
            let angle = a.num("angle", 0).unwrap_or(360.0).clamp(-360.0, 360.0);
            if angle == 0.0 {
                return Ok(None);
            }
            let max_x = slab.points.iter().map(|p| p[0].abs()).fold(0.0, f64::max);
            let n = self.fragments(a, f, max_x, pos) as f64 * angle.abs() / 360.0;
            let n = n.ceil().max(1.0) as usize;
            return match self.geo.rotate_extrude(&slab, angle, n) {
                Ok(s) => Ok(s.map(Geom::D3)),
                Err(e) => {
                    self.note(Some(pos), e);
                    Ok(None)
                }
            };
        }
        let height = a.num("height", 0).unwrap_or(100.0);
        let twist = a.num("twist", 3).unwrap_or(0.0);
        let scale = match a.get("scale", 5) {
            Some(Value::Num(s)) => [*s; 2],
            Some(v) => v.point().map_or([1.0; 2], |p| [p[0], p[1]]),
            None => [1.0; 2],
        };
        // A top shrunk to a point keeps a trace of it: a closed mesh.
        let scale = scale.map(|s| if s.abs() < 1e-9 { 1e-9 } else { s });
        let slices = match a.num("slices", 4) {
            Some(s) => s.max(1.0) as usize,
            None if twist != 0.0 => {
                let n = self.fragments(a, f, far, pos) as f64 * twist.abs() / 360.0;
                n.ceil().max(1.0) as usize
            }
            None => 1,
        };
        let center = a.bool("center", 1);
        Ok(self
            .geo
            .linear_extrude(&slab, height, center, twist, slices.min(10_000), scale)
            .map(Geom::D3))
    }

    pub fn eval(&mut self, e: &Expr, f: &Rc<Frame>) -> R<Value> {
        Ok(match e {
            Expr::Num(x) => Value::Num(*x),
            Expr::Str(s) => Value::Str(s.clone()),
            Expr::Bool(b) => Value::Bool(*b),
            Expr::Undef => Value::Undef,
            Expr::Var(n, pos) => match f.get(n) {
                Some(v) => v,
                None if &**n == "$parent_modules" => Value::Num(self.modules.len() as f64),
                None => {
                    if !n.starts_with('$') {
                        self.note(Some(*pos), format!("unknown variable {n}"));
                    }
                    Value::Undef
                }
            },
            Expr::List(elems) => {
                let mut out = Vec::new();
                for el in elems {
                    self.elem(el, f, &mut out)?;
                }
                Value::list(out)
            }
            Expr::Range(b, s, e) => {
                let b = self.eval(b, f)?.num();
                let step = match s {
                    Some(s) => self.eval(s, f)?.num(),
                    None => None,
                };
                let e = self.eval(e, f)?.num();
                match (b, s.is_some(), step, e) {
                    // `[9:0]` runs from 0 to 9, as OpenSCAD still has it.
                    (Some(b), false, _, Some(e)) if b > e => Value::Range(e, 1.0, b),
                    (Some(b), false, _, Some(e)) => Value::Range(b, 1.0, e),
                    (Some(b), true, Some(s), Some(e)) => Value::Range(b, s, e),
                    _ => Value::Undef,
                }
            }
            Expr::Unary(op, x) => value::unary(op, &self.eval(x, f)?),
            Expr::Binary(op, a, b) => match *op {
                "&&" => {
                    let a = self.eval(a, f)?;
                    Value::Bool(a.truthy() && self.eval(b, f)?.truthy())
                }
                "||" => {
                    let a = self.eval(a, f)?;
                    Value::Bool(a.truthy() || self.eval(b, f)?.truthy())
                }
                op => {
                    let a = self.eval(a, f)?;
                    value::binary(op, &a, &self.eval(b, f)?)
                }
            },
            Expr::Ternary(c, y, n) => {
                if self.eval(c, f)?.truthy() {
                    self.eval(y, f)?
                } else {
                    self.eval(n, f)?
                }
            }
            Expr::Call(callee, args, pos) => self.call(callee, args, *pos, f)?,
            Expr::Index(v, i) => {
                let v = self.eval(v, f)?;
                match self.eval(i, f)? {
                    Value::Num(i) if i >= 0.0 => item(&v, i as usize),
                    _ => Value::Undef,
                }
            }
            Expr::Member(v, m) => {
                let v = self.eval(v, f)?;
                match &**m {
                    "x" => item(&v, 0),
                    "y" => item(&v, 1),
                    "z" => item(&v, 2),
                    _ => Value::Undef,
                }
            }
            Expr::Let(args, body) => {
                let inner = self.lets(args, f)?;
                self.eval(body, &inner)?
            }
            Expr::Echo(args, body, pos) => {
                let a = self.args(args, f)?;
                self.echo(*pos, echo_text(&a));
                self.eval(body, f)?
            }
            Expr::Assert(args, body, pos) => {
                let a = self.args(args, f)?;
                self.check(&a, *pos, args)?;
                self.eval(body, f)?
            }
            Expr::Lambda(func) => Value::Func(func.clone(), f.clone()),
        })
    }

    /// A scope with `let`'s assignments, each seeing those before it.
    fn lets(&mut self, args: &[Arg], f: &Rc<Frame>) -> R<Rc<Frame>> {
        let inner = Frame::inner(f);
        for a in args {
            if let (Some(n), Some(e)) = (&a.name, &a.value) {
                let v = self.eval(e, &inner)?;
                inner.set(n, v);
            }
        }
        Ok(inner)
    }

    fn call(&mut self, callee: &Expr, args: &[Arg], pos: Pos, f: &Rc<Frame>) -> R<Value> {
        // Functions have names of their own: a variable of the same name
        // is only called when there is no function of it.
        if let Expr::Var(name, _) = callee {
            if let Some((def, home)) = self.find_func(f, name) {
                return self.call_func(&def, &home, args, pos, f);
            }
            if builtins::is_builtin(name) {
                let a = self.args(args, f)?;
                return Ok(self.builtin(name, a));
            }
            if f.get(name).is_none() {
                self.args(args, f)?;
                self.note(Some(pos), format!("unknown function {name}"));
                return Ok(Value::Undef);
            }
        }
        match self.eval(callee, f)? {
            Value::Func(def, home) => self.call_func(&def, &home, args, pos, f),
            v => {
                self.note(
                    Some(pos),
                    format!("a {} is called as a function", v.type_name()),
                );
                Ok(Value::Undef)
            }
        }
    }

    /// A built-in function; those that know the evaluation here.
    fn builtin(&mut self, name: &str, a: Args) -> Value {
        if name == "parent_module" {
            let n = a.num("n", 0).unwrap_or(0.0);
            let depth = self.modules.len();
            return if n >= 0.0 && (n as usize) < depth {
                Value::Str(self.modules[depth - 1 - n as usize].clone())
            } else {
                Value::Undef
            };
        }
        let list: Vec<(Option<Rc<str>>, Value)> = a
            .pos
            .into_iter()
            .map(|v| (None, v))
            .chain(a.named.into_iter().map(|(n, v)| (Some(n), v)))
            .collect();
        if name == "rands" {
            return builtins::rands(&list, &mut self.rng);
        }
        builtins::call(name, &list).unwrap_or_default()
    }

    fn call_func(
        &mut self,
        def: &Rc<Func>,
        home: &Rc<Frame>,
        args: &[Arg],
        pos: Pos,
        f: &Rc<Frame>,
    ) -> R<Value> {
        let a = self.args(args, f)?;
        let frame = Rc::new(Frame {
            parent: Some(home.clone()),
            caller: Some(f.clone()),
            ..Frame::default()
        });
        self.bind(&def.params, a, &frame)?;
        self.enter(pos)?;
        let v = self.eval(&def.body, &frame);
        self.depth -= 1;
        v
    }

    /// The values an element of a list adds.
    fn elem(&mut self, el: &Elem, f: &Rc<Frame>, out: &mut Vec<Value>) -> R<()> {
        if out.len() > MOST {
            return Err(Error::at(0, 0, "a list of more than a million values"));
        }
        match el {
            Elem::Expr(e) => out.push(self.eval(e, f)?),
            Elem::For(vars, body) => {
                self.each(vars, 0, f, &mut |me, inner| me.elem(body, inner, out))?;
            }
            Elem::ForC(init, cond, next, body) => {
                let mut frame = self.lets(init, f)?;
                let mut n = 0;
                while self.eval(cond, &frame)?.truthy() {
                    self.elem(body, &frame, out)?;
                    // The next values one after the other, each seeing
                    // those before it; what `next` does not set stays.
                    let step = Frame::inner(f);
                    for (name, v) in frame.vars.borrow().iter() {
                        step.set(name, v.clone());
                    }
                    for a in next {
                        if let (Some(name), Some(e)) = (&a.name, &a.value) {
                            let v = self.eval(e, &step)?;
                            step.set(name, v);
                        }
                    }
                    frame = step;
                    n += 1;
                    if n > MOST {
                        return Err(Error::at(0, 0, "a for loop of more than a million steps"));
                    }
                }
            }
            Elem::If(c, then, or) => {
                if self.eval(c, f)?.truthy() {
                    self.elem(then, f, out)?;
                } else if let Some(or) = or {
                    self.elem(or, f, out)?;
                }
            }
            Elem::Let(args, body) => {
                let inner = self.lets(args, f)?;
                self.elem(body, &inner, out)?;
            }
            Elem::Each(body) => {
                let mut inner = Vec::new();
                self.elem(body, f, &mut inner)?;
                for v in inner {
                    match v {
                        Value::List(_) | Value::Range(..) | Value::Str(_) => {
                            out.extend(v.items().map_err(|m| Error::at(0, 0, m))?);
                        }
                        v => out.push(v),
                    }
                }
            }
        }
        Ok(())
    }
}

fn is_instance(s: &Stmt) -> bool {
    matches!(s, Stmt::Call { .. } | Stmt::If { .. } | Stmt::Block(_))
}

/// Points of a list of vectors of 2 or 3 numbers.
fn points3(v: Option<&Value>) -> Vec<V3> {
    v.map(|v| v.items().unwrap_or_default())
        .unwrap_or_default()
        .iter()
        .filter_map(Value::point)
        .collect()
}

/// `a = 1, 2` as echo shows its arguments.
fn echo_text(a: &Args) -> String {
    let named = a.named.iter().map(|(n, v)| format!("{n} = {}", repr(v)));
    a.pos
        .iter()
        .map(repr)
        .chain(named)
        .collect::<Vec<_>>()
        .join(", ")
}

/// The matrix of a transformation module, `None` if its argument makes none.
fn matrix(name: &str, a: &Args) -> Option<Matrix> {
    let scaled = |k: V3| -> Matrix {
        [
            [k[0], 0.0, 0.0, 0.0],
            [0.0, k[1], 0.0, 0.0],
            [0.0, 0.0, k[2], 0.0],
        ]
    };
    match name {
        "translate" => {
            let v = a.get("v", 0)?.point()?;
            Some([
                [1.0, 0.0, 0.0, v[0]],
                [0.0, 1.0, 0.0, v[1]],
                [0.0, 0.0, 1.0, v[2]],
            ])
        }
        "scale" => match a.get("v", 0)? {
            Value::Num(k) => Some(scaled([*k; 3])),
            v => {
                let n = v.nums()?;
                Some(scaled([
                    *n.first()?,
                    *n.get(1)?,
                    n.get(2).copied().unwrap_or(1.0),
                ]))
            }
        },
        "mirror" => {
            let n = a.get("v", 0)?.point()?;
            let l = n[0] * n[0] + n[1] * n[1] + n[2] * n[2];
            if l < 1e-24 {
                return Some(IDENTITY);
            }
            Some(std::array::from_fn(|i| {
                std::array::from_fn(|j| {
                    let id = if i == j { 1.0 } else { 0.0 };
                    if j == 3 {
                        0.0
                    } else {
                        id - 2.0 * n[i] * n[j] / l
                    }
                })
            }))
        }
        "multmatrix" => {
            let rows = a.get("m", 0)?.items().ok()?;
            let mut m = IDENTITY;
            for (i, row) in rows.iter().take(3).enumerate() {
                for (j, x) in row.nums()?.iter().take(4).enumerate() {
                    m[i][j] = *x;
                }
            }
            Some(m)
        }
        "rotate" => rotation(a),
        _ => None,
    }
}

/// `rotate(a)` about Z, `rotate([x, y, z])` about X, then Y, then Z, and
/// `rotate(a, v)` about an axis.
fn rotation(a: &Args) -> Option<Matrix> {
    let about = |axis: usize, deg: f64| -> Matrix {
        let (s, c) = (sin_deg(deg), cos_deg(deg));
        let (i, j) = ((axis + 1) % 3, (axis + 2) % 3);
        let mut m = IDENTITY;
        m[i][i] = c;
        m[i][j] = -s;
        m[j][i] = s;
        m[j][j] = c;
        m
    };
    match (a.get("a", 0)?, a.get("v", 1)) {
        (Value::Num(deg), Some(v)) => {
            // As OpenSCAD's angle_axis_degrees: dividing by the length
            // as little as may be.
            let k = v.point()?;
            let m = k[0] * k[0] + k[1] * k[1] + k[2] * k[2];
            if m <= 0.0 {
                return Some(IDENTITY);
            }
            let (s, c) = (sin_deg(*deg), cos_deg(*deg));
            let cv = k.map(|x| x * ((1.0 - c) / m));
            let l = m.sqrt();
            let us = k.map(|x| x / l * s);
            Some([
                [
                    cv[0] * k[0] + c,
                    cv[1] * k[0] - us[2],
                    cv[2] * k[0] + us[1],
                    0.0,
                ],
                [
                    cv[0] * k[1] + us[2],
                    cv[1] * k[1] + c,
                    cv[2] * k[1] - us[0],
                    0.0,
                ],
                [
                    cv[0] * k[2] - us[1],
                    cv[1] * k[2] + us[0],
                    cv[2] * k[2] + c,
                    0.0,
                ],
            ])
        }
        (Value::Num(deg), None) => Some(about(2, *deg)),
        (v, _) => {
            let n = v.nums()?;
            let angle = |i: usize| n.get(i).copied().unwrap_or(0.0);
            let m = geom::compose(&about(1, angle(1)), &about(0, angle(0)));
            Some(geom::compose(&about(2, angle(2)), &m))
        }
    }
}
