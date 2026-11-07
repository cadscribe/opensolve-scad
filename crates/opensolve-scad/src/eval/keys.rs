//! The keys of results kept across runs (see [`crate::store`]): a hash of
//! all a result depends on, by content, so that the next run, with its
//! own frames and values, finds it again when nothing it depends on
//! changed.
//!
//! What a module call depends on: the module, the values of its arguments,
//! the `$` variables it sees, its children with the values they read where
//! they are written, and the colour and cutting state of the run (as the
//! keys within a run, [`Interp::call_key`]). What the module is: its code
//! (the syntax, places included, since notes carry them), the values of
//! the variables it reads from where it is defined, and, the same way, the
//! functions and modules it calls, by name as they are found from there,
//! and so on: a graph of definitions ([`Interp::node`]). Its hash is
//! written as a tree: a definition met again on the way down is a
//! reference by how far up it is, so a definition whose references all
//! stay within it hashes the same wherever it is met, and is worked out
//! once a run.
//!
//! What a built-in operation depends on: its name and arguments, the
//! `$fn`, `$fa`, `$fs` it sees, and its children by content (corners,
//! triangles, the planes and faces of their facets as they are numbered
//! from the first, whether the boolean library's mesh is kept).
//!
//! A result whose making drew numbers from `rands` (which depend on all
//! drawn before, and change those drawn after) is not kept; nor a call
//! that includes or uses a file in its body.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

use crate::ast::{Arg, Elem, Expr, Func, Module, Pos, Stmt};
use crate::builtins;
use crate::geom::Geom;
use crate::store::{Event, Files, H128, Session};
use crate::value::Value;

use super::{Args, Frame, Fx, Interp};

/// What a definition or a list of statements reads from outside: the
/// names of variables, functions and modules, each once in the order met;
/// whether it reads its children (`children()`, `$children`); whether it
/// cannot be keyed; a hash of its code.
#[derive(Debug, Default)]
pub(crate) struct Names {
    pub hash: u128,
    pub vars: Vec<Arc<str>>,
    pub funcs: Vec<Arc<str>>,
    pub mods: Vec<Arc<str>>,
    pub children: bool,
    pub opaque: bool,
    pub file: u32,
}

/// A definition worked out, kept alive while its address is a key.
pub(crate) enum Held {
    M(#[allow(dead_code)] Arc<Module>),
    F(#[allow(dead_code)] Arc<Func>),
    S(#[allow(dead_code)] Arc<[Stmt]>),
}

/// A definition to key, with the scope it is defined in.
#[derive(Clone)]
enum Def {
    M(Arc<Module>),
    F(Arc<Func>),
}

impl Def {
    fn ptr(&self) -> usize {
        match self {
            Def::M(m) => Arc::as_ptr(m) as usize,
            Def::F(f) => Arc::as_ptr(f) as usize,
        }
    }
}

/// The key of a definition and all it reaches: its hash, the highest
/// place on the way down that it refers back to (`usize::MAX`: none, it
/// stands alone), and the files of its code.
#[derive(Clone, Copy, Debug)]
pub(crate) struct NodeKey {
    hash: u128,
    low: usize,
    files: Files,
}

const MISSING: u64 = 0xA1;
const BUILTIN: u64 = 0xA2;
const LAMBDA: u64 = 0xA3;
const BACK: u64 = 0xA4;

/// Walks the syntax of a definition: its hash and what it reads from
/// outside. Names bound inside (parameters, assignments, `let`, `for`,
/// local functions and modules) are not read from outside; variables,
/// functions and modules are told apart, as OpenSCAD does.
#[derive(Default)]
struct Walk {
    h: Option<H128>,
    out: Names,
    seen_v: HashSet<Arc<str>>,
    seen_f: HashSet<Arc<str>>,
    seen_m: HashSet<Arc<str>>,
    bound_v: Vec<Arc<str>>,
    bound_f: Vec<Arc<str>>,
    bound_m: Vec<Arc<str>>,
    /// Inside a module defined in the one walked: its children are its
    /// own.
    nested: usize,
}

impl Walk {
    fn new() -> Walk {
        Walk {
            h: Some(H128::new(0x5C0DE)),
            ..Walk::default()
        }
    }

    fn h(&mut self) -> &mut H128 {
        self.h.get_or_insert_with(|| H128::new(0))
    }

    fn tag(&mut self, t: u64) {
        self.h().u64(t);
    }

    fn name(&mut self, n: &str) {
        self.h().str(n);
    }

    fn pos(&mut self, p: Pos) {
        if self.out.file == 0 {
            self.out.file = p.file;
        }
        let h = self.h();
        h.u64(u64::from(p.file));
        h.u64(u64::from(p.line) << 32 | u64::from(p.col));
    }

    fn mark(&self) -> (usize, usize, usize) {
        (self.bound_v.len(), self.bound_f.len(), self.bound_m.len())
    }

    fn back(&mut self, m: (usize, usize, usize)) {
        self.bound_v.truncate(m.0);
        self.bound_f.truncate(m.1);
        self.bound_m.truncate(m.2);
    }

    fn var(&mut self, n: &Arc<str>) {
        if n.starts_with('$') {
            if &**n == "$children" && self.nested == 0 {
                self.out.children = true;
            }
            return;
        }
        if !self.bound_v.contains(n) && self.seen_v.insert(n.clone()) {
            self.out.vars.push(n.clone());
        }
    }

    fn func(&mut self, n: &Arc<str>) {
        if !self.bound_f.contains(n) && self.seen_f.insert(n.clone()) {
            self.out.funcs.push(n.clone());
        }
    }

    fn module(&mut self, n: &Arc<str>) {
        if !self.bound_m.contains(n) && self.seen_m.insert(n.clone()) {
            self.out.mods.push(n.clone());
        }
    }

    fn args(&mut self, args: &[Arg]) {
        self.tag(args.len() as u64);
        for a in args {
            match &a.name {
                Some(n) => {
                    self.tag(1);
                    self.name(n);
                }
                None => self.tag(0),
            }
            match &a.value {
                Some(v) => {
                    self.tag(1);
                    self.expr(v);
                }
                None => self.tag(0),
            }
        }
    }

    /// `let`'s assignments, each seeing those before.
    fn lets(&mut self, args: &[Arg]) {
        self.tag(args.len() as u64);
        for a in args {
            if let Some(v) = &a.value {
                self.expr(v);
            }
            if let Some(n) = &a.name {
                self.name(n);
                self.bound_v.push(n.clone());
            }
        }
    }

    /// Parameters with their defaults, bound in order as a call binds
    /// them: a default sees the parameters before it, and reads one of its
    /// own name or a later one from outside (`x = x`).
    fn params(&mut self, params: &[Arg]) {
        self.tag(params.len() as u64);
        for p in params {
            match &p.value {
                Some(v) => {
                    self.tag(1);
                    self.expr(v);
                }
                None => self.tag(0),
            }
            if let Some(n) = &p.name {
                self.name(n);
                self.bound_v.push(n.clone());
            }
        }
    }

    fn func_def(&mut self, d: &Func) {
        let m = self.mark();
        self.params(&d.params);
        self.expr(&d.body);
        self.back(m);
    }

    fn module_def(&mut self, d: &Module) {
        let m = self.mark();
        self.params(&d.params);
        self.block(&d.body);
        self.back(m);
    }

    fn expr(&mut self, e: &Expr) {
        match e {
            Expr::Num(x) => {
                self.tag(1);
                self.h().f64(*x);
            }
            Expr::Str(s) => {
                self.tag(2);
                self.name(s);
            }
            Expr::Bool(b) => self.tag(3 + u64::from(*b)),
            Expr::Undef => self.tag(5),
            Expr::Var(n, p) => {
                self.tag(6);
                self.name(n);
                self.pos(*p);
                self.var(n);
            }
            Expr::List(elems) => {
                self.tag(7);
                self.tag(elems.len() as u64);
                for el in elems {
                    self.elem(el);
                }
            }
            Expr::Range(b, s, e) => {
                self.tag(8);
                self.expr(b);
                match s {
                    Some(s) => self.expr(s),
                    None => self.tag(0),
                }
                self.expr(e);
            }
            Expr::Unary(op, x) => {
                self.tag(9);
                self.name(op);
                self.expr(x);
            }
            Expr::Binary(op, a, b) => {
                self.tag(10);
                self.name(op);
                self.expr(a);
                self.expr(b);
            }
            Expr::Ternary(c, y, n) => {
                self.tag(11);
                self.expr(c);
                self.expr(y);
                self.expr(n);
            }
            Expr::Call(callee, args, p) => {
                self.tag(12);
                self.pos(*p);
                match &**callee {
                    Expr::Var(n, vp) => {
                        self.tag(1);
                        self.name(n);
                        self.pos(*vp);
                        // A function of the name first, else a variable
                        // that holds one: both are looked up.
                        self.func(n);
                    }
                    c => {
                        self.tag(2);
                        self.expr(c);
                    }
                }
                self.args(args);
            }
            Expr::Index(v, i) => {
                self.tag(13);
                self.expr(v);
                self.expr(i);
            }
            Expr::Member(v, m) => {
                self.tag(14);
                self.expr(v);
                self.name(m);
            }
            Expr::Let(args, body) => {
                self.tag(15);
                let m = self.mark();
                self.lets(args);
                self.expr(body);
                self.back(m);
            }
            Expr::Echo(args, body, p) | Expr::Assert(args, body, p) => {
                self.tag(if matches!(e, Expr::Echo(..)) { 16 } else { 17 });
                self.pos(*p);
                self.args(args);
                self.expr(body);
            }
            Expr::Lambda(f) => {
                self.tag(18);
                self.func_def(f);
            }
        }
    }

    fn elem(&mut self, el: &Elem) {
        match el {
            Elem::Expr(e) => {
                self.tag(30);
                self.expr(e);
            }
            Elem::For(args, body) => {
                self.tag(31);
                let m = self.mark();
                self.lets(args);
                self.elem(body);
                self.back(m);
            }
            Elem::ForC(init, cond, next, body) => {
                self.tag(32);
                let m = self.mark();
                self.lets(init);
                self.expr(cond);
                self.lets(next);
                self.elem(body);
                self.back(m);
            }
            Elem::If(c, then, or) => {
                self.tag(33);
                self.expr(c);
                self.elem(then);
                match or {
                    Some(o) => self.elem(o),
                    None => self.tag(0),
                }
            }
            Elem::Let(args, body) => {
                self.tag(34);
                let m = self.mark();
                self.lets(args);
                self.elem(body);
                self.back(m);
            }
            Elem::Each(body) => {
                self.tag(35);
                self.elem(body);
            }
        }
    }

    /// Statements in a scope of their own, as they run: its functions and
    /// modules known in all of it; its assignments in order, each name at
    /// the place of its first with its last value, each value seeing the
    /// names assigned before it (a later one is read from outside); then
    /// the rest, seeing all.
    fn block(&mut self, stmts: &[Stmt]) {
        let m = self.mark();
        self.tag(40);
        self.tag(stmts.len() as u64);
        for s in stmts {
            match s {
                Stmt::Module(n, _) => self.bound_m.push(n.clone()),
                Stmt::Function(n, _) => self.bound_f.push(n.clone()),
                _ => {}
            }
        }
        let mut order: Vec<(&Arc<str>, &Stmt)> = Vec::new();
        for s in stmts {
            if let Stmt::Assign { name, .. } = s {
                match order.iter_mut().find(|(n, _)| *n == name) {
                    Some(o) => o.1 = s,
                    None => order.push((name, s)),
                }
            }
        }
        for (name, s) in order {
            self.stmt(s);
            self.bound_v.push(name.clone());
        }
        for s in stmts {
            if !matches!(s, Stmt::Assign { .. }) {
                self.stmt(s);
            }
        }
        self.back(m);
    }

    fn stmt(&mut self, s: &Stmt) {
        match s {
            Stmt::Assign {
                name, value, pos, ..
            } => {
                self.tag(41);
                self.name(name);
                self.pos(*pos);
                self.expr(value);
            }
            Stmt::Module(n, d) => {
                self.tag(42);
                self.name(n);
                self.nested += 1;
                self.module_def(d);
                self.nested -= 1;
            }
            Stmt::Function(n, d) => {
                self.tag(43);
                self.name(n);
                self.func_def(d);
            }
            Stmt::Call {
                name,
                args,
                children,
                modifier,
                pos,
            } => {
                self.tag(44);
                self.name(name);
                self.tag(*modifier as u64);
                self.pos(*pos);
                match &**name {
                    "for" | "intersection_for" | "let" => {
                        let m = self.mark();
                        self.lets(args);
                        self.block(children);
                        self.back(m);
                        return;
                    }
                    "children" if !self.bound_m.contains(name) => {
                        if self.nested == 0 {
                            self.out.children = true;
                        }
                    }
                    _ => self.module(name),
                }
                self.args(args);
                self.block(children);
            }
            Stmt::If { cond, then, or, .. } => {
                self.tag(45);
                self.expr(cond);
                self.block(then);
                self.block(or);
            }
            Stmt::Block(b) => {
                self.tag(46);
                self.block(b);
            }
            Stmt::Include(..) | Stmt::Use(..) => self.out.opaque = true,
        }
    }

    fn done(mut self) -> Names {
        self.out.hash = self.h().finish();
        self.out
    }
}

fn analyse_module(m: &Module) -> Names {
    let mut w = Walk::new();
    w.tag(1);
    w.module_def(m);
    w.done()
}

fn analyse_func(f: &Func) -> Names {
    let mut w = Walk::new();
    w.tag(2);
    w.func_def(f);
    w.done()
}

fn analyse_stmts(s: &[Stmt]) -> Names {
    let mut w = Walk::new();
    w.tag(3);
    w.block(s);
    w.done()
}

impl Interp {
    fn names(&mut self, d: &Def) -> Rc<Names> {
        let ptr = d.ptr();
        if let Some((_, n)) = self.analyses.get(&ptr) {
            return n.clone();
        }
        let (held, n) = match d {
            Def::M(m) => (Held::M(m.clone()), analyse_module(m)),
            Def::F(f) => (Held::F(f.clone()), analyse_func(f)),
        };
        let n = Rc::new(n);
        self.analyses.insert(ptr, (held, n.clone()));
        n
    }

    fn stmts_names(&mut self, s: &Arc<[Stmt]>) -> Rc<Names> {
        let ptr = s.as_ptr() as usize;
        if let Some((_, n)) = self.analyses.get(&ptr) {
            return n.clone();
        }
        let n = Rc::new(analyse_stmts(s));
        self.analyses.insert(ptr, (Held::S(s.clone()), n.clone()));
        n
    }

    /// The key of definition `d` defined in `home`, met with `stack` (the
    /// definitions on the way down to it) above it; `None` if it cannot be
    /// keyed.
    fn node(
        &mut self,
        d: &Def,
        home: &Rc<Frame>,
        stack: &mut Vec<(usize, usize)>,
    ) -> Option<NodeKey> {
        let id = (d.ptr(), Rc::as_ptr(home) as usize);
        if let Some(at) = stack.iter().rposition(|x| *x == id) {
            let mut h = H128::new(BACK);
            h.u64((stack.len() - 1 - at) as u64);
            return Some(NodeKey {
                hash: h.finish(),
                low: at,
                files: Files::default(),
            });
        }
        if let Some(k) = self.nodes.get(&id) {
            return Some(*k);
        }
        let names = self.names(d);
        if names.opaque {
            return None;
        }
        let me = stack.len();
        stack.push(id);
        let made = self.reads(&names, home, stack);
        stack.pop();
        let (hash, mut low, mut files) = made?;
        files.add(names.file);
        if low >= me {
            low = usize::MAX;
        }
        let k = NodeKey { hash, low, files };
        // Alone, and in a scope that lasts the run: the same wherever met.
        if low == usize::MAX && home.parent.is_none() {
            self.nodes.insert(id, k);
        }
        Some(k)
    }

    /// The hash of `names`' code and of all it reads from `home`, the
    /// highest place on the way down it refers back to, and its files.
    fn reads(
        &mut self,
        names: &Names,
        home: &Rc<Frame>,
        stack: &mut Vec<(usize, usize)>,
    ) -> Option<(u128, usize, Files)> {
        let mut h = H128::new(0x40DE);
        h.u128(names.hash);
        let mut acc = (usize::MAX, Files::default());
        for v in &names.vars {
            match home.get(v) {
                Some(val) => self.value_hash(&val, &mut h, stack, &mut acc)?,
                None => h.u64(MISSING),
            }
        }
        h.u64(0xF0);
        for n in &names.funcs {
            h.str(n);
            if let Some((d, hh)) = self.find_func(home, n) {
                let k = self.node(&Def::F(d), &hh, stack)?;
                join(&mut h, &mut acc, k);
            } else if builtins::is_builtin(n) {
                // `rands` too: a result that drew numbers is not kept (see
                // `Interp::draws`).
                h.u64(BUILTIN);
            } else {
                match home.get(n) {
                    Some(val) => self.value_hash(&val, &mut h, stack, &mut acc)?,
                    None => h.u64(MISSING),
                }
            }
        }
        h.u64(0xF1);
        for n in &names.mods {
            h.str(n);
            if let Some((d, hh)) = self.find_module(home, n) {
                let k = self.node(&Def::M(d), &hh, stack)?;
                join(&mut h, &mut acc, k);
            } else {
                h.u64(BUILTIN);
                if &**n == "text" {
                    for f in &self.font_files {
                        h.str(&f.to_string_lossy());
                    }
                }
            }
        }
        Some((h.finish(), acc.0, acc.1))
    }

    /// Writes a value into `h`: a function literal made in a file's scope
    /// as its definition's key (what it refers back to and its files go
    /// into `acc`); `None` for one made elsewhere, which cannot be keyed.
    fn value_hash(
        &mut self,
        v: &Value,
        h: &mut H128,
        stack: &mut Vec<(usize, usize)>,
        acc: &mut (usize, Files),
    ) -> Option<()> {
        match v {
            Value::Undef => h.u64(0),
            Value::Bool(b) => h.u64(1 + u64::from(*b)),
            Value::Num(x) => {
                h.u64(3);
                h.f64(*x);
            }
            Value::Str(s) => {
                h.u64(4);
                h.str(s);
            }
            Value::List(l) => {
                h.u64(5);
                h.u64(l.len() as u64);
                for x in l.iter() {
                    self.value_hash(x, h, stack, acc)?;
                }
            }
            Value::Range(a, b, c) => {
                h.u64(6);
                h.f64(*a);
                h.f64(*b);
                h.f64(*c);
            }
            // What it reads where it was made, as a definition's.
            Value::Func(d, frame) => {
                h.u64(LAMBDA);
                let k = self.node(&Def::F(d.clone()), frame, stack)?;
                join(h, acc, k);
            }
        }
        Some(())
    }

    /// A value's hash on its own (`None`: it cannot be keyed).
    fn value_alone(&mut self, v: &Value, h: &mut H128, files: &mut Files) -> Option<()> {
        let mut acc = (usize::MAX, Files::default());
        self.value_hash(v, h, &mut Vec::new(), &mut acc)?;
        files.join(acc.1);
        Some(())
    }

    /// The key of statements run in `f`: their code, what they read from
    /// there, and the children they hand on, if they do.
    fn stmts_key(
        &mut self,
        stmts: &Arc<[Stmt]>,
        f: &Rc<Frame>,
        depth: usize,
    ) -> Option<(u128, Files)> {
        if depth > 64 {
            return None;
        }
        let names = self.stmts_names(stmts);
        if names.opaque {
            return None;
        }
        let (hash, _, mut files) = self.reads(&names, f, &mut Vec::new())?;
        files.add(names.file);
        let mut h = H128::new(0xC41D);
        h.u128(hash);
        if names.children {
            match f.get("$children") {
                Some(v) => {
                    self.value_alone(&v, &mut h, &mut files)?;
                }
                None => h.u64(MISSING),
            }
            match f.children() {
                Some((s, home)) => {
                    let (k, fl) = self.stmts_key(&s, &home, depth + 1)?;
                    h.u128(k);
                    files.join(fl);
                }
                None => h.u64(MISSING),
            }
        }
        Some((h.finish(), files))
    }

    /// The key of a call of module `def` (found in `home`) with arguments
    /// `a` and `children` from `f`, across runs: `None` if it cannot be
    /// keyed.
    pub(super) fn module_key(
        &mut self,
        def: &Arc<Module>,
        home: &Rc<Frame>,
        a: &Args,
        children: &Arc<[Stmt]>,
        count: usize,
        f: &Rc<Frame>,
    ) -> Option<(u128, Files)> {
        let node = self.node(&Def::M(def.clone()), home, &mut Vec::new())?;
        let mut files = node.files;
        let mut h = H128::new(0xCA11);
        h.u128(node.hash);
        for (v, before) in a.pos.iter().zip(&a.before) {
            h.u64(*before as u64);
            self.value_alone(v, &mut h, &mut files)?;
        }
        h.u64(0xF2);
        for (n, v) in &a.named {
            h.str(n);
            self.value_alone(v, &mut h, &mut files)?;
        }
        h.u64(0xF3);
        // The `$` names as the call sees them: the nearest of each name.
        let mut seen: Vec<Arc<str>> = Vec::new();
        let mut x: Option<&Rc<Frame>> = Some(f);
        let mut steps = 0;
        while let Some(fr) = x {
            steps += 1;
            if steps > 200 {
                return None;
            }
            let vars: Vec<(Arc<str>, Value)> = fr
                .vars
                .borrow()
                .list
                .iter()
                .filter(|(n, _)| n.starts_with('$') && &**n != "$children" && !seen.contains(n))
                .map(|(n, v)| (n.clone(), v.clone()))
                .collect();
            for (n, v) in vars {
                h.str(&n);
                self.value_alone(&v, &mut h, &mut files)?;
                seen.push(n);
            }
            x = fr.caller.as_ref().or(fr.parent.as_ref());
        }
        h.u64(0xF4);
        h.u64(count as u64);
        if count > 0 {
            let (k, fl) = self.stmts_key(children, f, 0)?;
            h.u128(k);
            files.join(fl);
        }
        if self.parents_matter {
            for m in &self.modules {
                h.str(m);
            }
        }
        h.u64(u64::from(self.modules.len() > 1));
        h.u64(self.paint_state());
        Some((h.finish(), files))
    }

    /// The colours made and whether inside a colour or cutting, as far as
    /// they choose what a module makes (see [`Interp::call_key`]).
    fn paint_state(&self) -> u64 {
        match self.paint {
            crate::Paint::All => u64::from(self.painted),
            crate::Paint::Uncoloured => {
                0x10 | u64::from(self.painted) | (u64::from(self.cutting) << 1)
            }
            crate::Paint::Only(c) => {
                let mut h = H128::new(0xC010);
                for x in c {
                    h.f64(x);
                }
                (h.finish() as u64 & !0xff)
                    | 0x20
                    | u64::from(self.painted)
                    | (u64::from(self.cutting) << 1)
            }
        }
    }

    /// Writes a shape's contents into `h`.
    pub(super) fn geom_hash(&self, g: &Geom, h: &mut H128) {
        match g {
            Geom::D2(r) => {
                h.u64(2);
                h.u64(r.loops.len() as u64);
                for l in &r.loops {
                    h.u64(l.len() as u64);
                    for p in l {
                        h.f64(p[0]);
                        h.f64(p[1]);
                    }
                }
            }
            Geom::D3(s) => {
                h.u64(3);
                h.u64(u64::from(s.has_mesh()));
                h.u64(s.points.len() as u64);
                for p in &s.points {
                    h.f64(p[0]);
                    h.f64(p[1]);
                    h.f64(p[2]);
                }
                h.u64(s.tris.len() as u64);
                for t in &s.tris {
                    h.u64(u64::from(t[0]) | u64::from(t[1]) << 21 | u64::from(t[2]) << 42);
                }
                // The facets as numbered from the first met, each with its
                // plane and its face, numbered so too.
                let mut facets: HashMap<u32, u32, Fx> = HashMap::default();
                let mut faces: HashMap<u64, u32, Fx> = HashMap::default();
                for &f in &s.facets {
                    let n = facets.len() as u32;
                    let at = *facets.entry(f).or_insert(n);
                    if at == n {
                        let x = self.geo.facets.get(f);
                        let m = faces.len() as u32;
                        let face = *faces.entry(x.face.0).or_insert(m);
                        h.u64(0xFAC);
                        h.f64(x.normal[0]);
                        h.f64(x.normal[1]);
                        h.f64(x.normal[2]);
                        h.f64(x.offset);
                        h.u64(u64::from(face));
                    }
                    h.u64(u64::from(at));
                }
            }
        }
    }

    /// Whether shapes are big enough for their operation to be kept.
    pub(super) fn worth(made: &[&Geom]) -> bool {
        let size: usize = made
            .iter()
            .map(|g| match g {
                Geom::D3(s) => s.tris.len(),
                Geom::D2(r) => r.loops.iter().map(Vec::len).sum(),
            })
            .sum();
        size >= MIN_SIZE
    }

    /// The key of operation `name` on `made` with arguments `a` and the
    /// `$fn`, `$fa`, `$fs` of `f`; `None` without a cache, for small shapes
    /// or arguments that cannot be keyed.
    pub(super) fn op_key(
        &mut self,
        name: &str,
        made: &[&Geom],
        a: Option<&Args>,
        f: Option<&Rc<Frame>>,
    ) -> Option<u128> {
        self.session.as_ref()?;
        if !made.is_empty() && !Self::worth(made) {
            return None;
        }
        let mut h = H128::new(0x0B);
        h.str(name);
        let mut files = Files::default();
        if let Some(a) = a {
            for (v, before) in a.pos.iter().zip(&a.before) {
                h.u64(*before as u64);
                self.value_alone(v, &mut h, &mut files)?;
            }
            h.u64(0xF2);
            for (n, v) in &a.named {
                h.str(n);
                self.value_alone(v, &mut h, &mut files)?;
            }
        }
        if let (Some(a), Some(f)) = (a, f) {
            for s in ["$fn", "$fa", "$fs"] {
                h.f64(Self::special(a, f, s));
            }
        }
        h.u64(made.len() as u64);
        for g in made {
            self.geom_hash(g, &mut h);
        }
        Some(h.finish())
    }

    /// The result kept under `key`, its notes told again (at `pos` if
    /// given, where an operation's notes are); `None` if there is none or
    /// a file it imported changed.
    pub(super) fn recall(&mut self, key: u128, pos: Option<Option<Pos>>) -> Option<Option<Geom>> {
        let (made, events) = self.session.as_mut()?.recall(key)?;
        // What it imported must be as it was.
        for e in events.iter() {
            if let Event::Read(path, hash) = e
                && self.content_hash(path).unwrap_or(0) != *hash
            {
                if let Some(s) = self.session.as_mut() {
                    s.forget(key);
                }
                return None;
            }
        }
        self.replay(&events, pos);
        Some(made)
    }

    /// Tells what a result told when it was made: as it was then, or at
    /// `pos` for an operation's notes.
    pub(super) fn replay(&mut self, events: &[Event], pos: Option<Option<Pos>>) {
        for e in events {
            match e {
                Event::Note(p, text) => self.note(pos.unwrap_or(*p), text.clone()),
                Event::Echo(p, text) => self.echo(*p, text.clone()),
                Event::Colour(c) => self.met_colour(*c),
                Event::Read(..) => self.told(e.clone()),
                Event::Batch(k, s) => {
                    self.geo.remember(*k, s.clone());
                    self.told(e.clone());
                }
            }
        }
    }

    /// Keeps a result made under `key`, with what was told since `mark`,
    /// unless numbers were drawn since `draws`.
    pub(super) fn keep_result(
        &mut self,
        key: u128,
        made: &Option<Geom>,
        mark: Mark,
        files: Files,
        cost: Option<std::time::Duration>,
    ) {
        // Not kept, but made: not used again in this run.
        let lost = self.lost || self.geo.log.is_none();
        let cost = if self.root.is_some() || self.draws != mark.draws || lost {
            Some(std::time::Duration::ZERO)
        } else {
            cost
        };
        let keys = mark.keys;
        let events = self.told_since(mark);
        if let Some(s) = self.session.as_mut() {
            s.keep(key, made, &events, keys, files, cost);
        }
    }

    /// Where the making of a result begins: what was told, drawn, kept
    /// and batched so far.
    pub(super) fn begin_making(&self) -> Mark {
        Mark {
            events: self.events.len(),
            draws: self.draws,
            keys: self.session.as_ref().map_or(0, Session::mark),
            batches: self.geo.log.as_ref().map_or(0, Vec::len),
        }
    }

    /// What was told since `mark`, with the batches made.
    pub(super) fn told_since(&self, mark: Mark) -> Vec<Event> {
        let mut out = self.events.get(mark.events..).unwrap_or_default().to_vec();
        if let Some(log) = &self.geo.log {
            out.extend(
                log.get(mark.batches..)
                    .unwrap_or_default()
                    .iter()
                    .map(|(k, s)| Event::Batch(*k, s.clone())),
            );
        }
        out
    }

    /// A hash of the contents of a file imported, as read now (once a run).
    pub(super) fn content_hash(&mut self, path: &std::path::Path) -> Option<u64> {
        if let Some(h) = self.read_hashes.get(path) {
            return *h;
        }
        let h = match self.blobs.get(path) {
            Some(b) => Some(crate::store::hash_bytes(b)),
            None => super::read_file(path)
                .ok()
                .map(|b| crate::store::hash_bytes(&b)),
        };
        self.read_hashes.insert(path.to_path_buf(), h);
        h
    }
}

/// Where the making of a result began (see [`Interp::begin_making`]).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Mark {
    events: usize,
    pub draws: u64,
    keys: usize,
    batches: usize,
}

fn join(h: &mut H128, acc: &mut (usize, Files), k: NodeKey) {
    h.u128(k.hash);
    acc.0 = acc.0.min(k.low);
    acc.1.join(k.files);
}

/// Shapes smaller than this (triangles, or corners of 2D shapes) are made
/// again rather than looked up.
const MIN_SIZE: usize = 256;
