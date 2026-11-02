//! The files a client has open and those they include and use, and what
//! the language server answers about them: diagnostics, completion, hover,
//! definitions, symbols and signatures. Places are byte offsets; the
//! protocol and the app's editor turn them into their own.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};

use tree_sitter::{Node, Tree};

use crate::builtins::{BUILTINS, Builtin, BuiltinKind, builtin, parameter_doc, parameters};
use crate::index::{Def, DefKind, FileIndex, index, locals};
use crate::syntax::{self, Span};
use crate::text::{Lines, is_word, word_at};
use crate::{Diagnostic, Severity};

/// A file parsed.
#[derive(Debug)]
pub struct Parsed {
    pub text: String,
    pub tree: Tree,
    pub lines: Lines,
    pub index: FileIndex,
}

impl Parsed {
    pub fn new(text: String) -> Parsed {
        let tree = syntax::parse(&text);
        let lines = Lines::new(&text);
        let index = index(&tree, &text);
        Parsed {
            text,
            tree,
            lines,
            index,
        }
    }
}

/// Reads a file that is not open.
pub type Read = dyn Fn(&Path) -> Option<String> + Send + Sync;

/// What a completion offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CompletionKind {
    Module,
    Function,
    Variable,
    Parameter,
    Constant,
    Keyword,
    File,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Completion {
    pub label: String,
    pub kind: CompletionKind,
    /// The signature.
    pub detail: String,
    pub doc: String,
    /// What it replaces: the word typed so far.
    pub replace: Range<usize>,
    /// What takes its place: the label, or `d = ` for a parameter's name.
    pub insert: String,
}

/// What hover shows: Markdown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hover {
    pub range: Range<usize>,
    pub markdown: String,
    /// The signature alone, and the explanation alone, for an editor that
    /// shows them its own way.
    pub signature: String,
    pub doc: String,
}

/// A place in a file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Location {
    pub path: PathBuf,
    pub range: Range<usize>,
}

/// A module, function or variable at the top of a file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Symbol {
    pub name: String,
    pub kind: DefKind,
    pub detail: String,
    pub range: Range<usize>,
    pub name_range: Range<usize>,
}

/// The signature of the call around a place.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signature {
    pub label: String,
    pub parameters: Vec<String>,
    /// The parameter the place is at.
    pub active: usize,
    pub doc: String,
}

/// The open files, and how others are read.
pub struct Workspace {
    open: HashMap<PathBuf, Arc<Parsed>>,
    /// Files read from the disk, by their text.
    cache: Mutex<HashMap<PathBuf, Arc<Parsed>>>,
    read: Box<Read>,
}

impl std::fmt::Debug for Workspace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Workspace")
            .field("open", &self.open.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

impl Default for Workspace {
    fn default() -> Self {
        Workspace::new(Box::new(|p| std::fs::read_to_string(p).ok()))
    }
}

/// `a/b/../c` as `a/c`, without asking the disk.
pub fn plain(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            c => out.push(c),
        }
    }
    out
}

impl Workspace {
    /// A workspace whose files that are not open are read by `read`.
    pub fn new(read: Box<Read>) -> Workspace {
        Workspace {
            open: HashMap::new(),
            cache: Mutex::new(HashMap::new()),
            read,
        }
    }

    /// A file opened, or its text changed.
    pub fn set(&mut self, path: &Path, text: String) {
        self.open.insert(plain(path), Arc::new(Parsed::new(text)));
    }

    pub fn close(&mut self, path: &Path) {
        self.open.remove(&plain(path));
    }

    pub fn is_open(&self, path: &Path) -> bool {
        self.open.contains_key(&plain(path))
    }

    /// A file, open or read.
    pub fn file(&self, path: &Path) -> Option<Arc<Parsed>> {
        let path = plain(path);
        if let Some(p) = self.open.get(&path) {
            return Some(p.clone());
        }
        let text = (self.read)(&path)?;
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(p) = cache.get(&path)
            && p.text == text
        {
            return Some(p.clone());
        }
        let p = Arc::new(Parsed::new(text));
        cache.insert(path, p.clone());
        Some(p)
    }

    /// The file an `include` or `use` in `from` names.
    fn import_path(from: &Path, path: &str) -> PathBuf {
        plain(&from.parent().unwrap_or(Path::new("")).join(path))
    }

    /// The highlighted pieces of a file.
    pub fn highlight(&self, path: &Path) -> Vec<Span> {
        self.file(path)
            .map_or_else(Vec::new, |p| syntax::highlight(&p.tree, &p.text))
    }

    /// The syntax errors of a file: at once, while typing.
    pub fn syntax_diagnostics(&self, path: &Path) -> Vec<Diagnostic> {
        let Some(p) = self.file(path) else {
            return Vec::new();
        };
        let mut out = syntax::errors(&p.tree, &p.text);
        // An `include` or `use` of a file that is not there.
        for i in &p.index.imports {
            if self.file(&Self::import_path(path, &i.path)).is_none() {
                out.push(Diagnostic {
                    range: i.range.clone(),
                    severity: Severity::Error,
                    message: format!("{} cannot be read", i.path),
                });
            }
        }
        out
    }

    /// What evaluating a file needs: its text and those of what it
    /// includes and uses, open ones as they are in the editor. Run it on
    /// another thread with [`Evaluation::run`].
    pub fn evaluation(&self, path: &Path) -> Evaluation {
        let path = plain(path);
        let read = |p: &Path| {
            self.file(p)
                .map(|f| f.text.as_bytes().to_vec())
                .ok_or_else(|| format!("{} cannot be read", p.display()))
        };
        let sources = opensolve_scad::files(&path, &read)
            .into_iter()
            .filter_map(|p| self.file(&p).map(|f| (p, f.text.clone())))
            .collect();
        Evaluation { path, sources }
    }

    /// The syntax errors and, when there are none, what evaluating the file
    /// finds. The evaluation builds the shapes too: on a large file, run
    /// [`Workspace::evaluation`] in the background instead.
    pub fn diagnostics(&self, path: &Path) -> Vec<Diagnostic> {
        let mut out = self.syntax_diagnostics(path);
        if out.is_empty() {
            out = self.evaluation(path).run();
        }
        out
    }

    /// Modules, functions and variables at the top of a file.
    pub fn symbols(&self, path: &Path) -> Vec<Symbol> {
        let Some(p) = self.file(path) else {
            return Vec::new();
        };
        p.index
            .defs
            .iter()
            .map(|d| Symbol {
                name: d.name.clone(),
                kind: d.kind,
                detail: d.signature.clone(),
                range: d.range.clone(),
                name_range: d.name_range.clone(),
            })
            .collect()
    }

    /// The top-level names `path` sees, by OpenSCAD's rules: its own, all of
    /// what it includes, the modules and functions of what it uses; each
    /// with the file it is in, the nearest first.
    pub fn visible(&self, path: &Path) -> Vec<(Def, PathBuf)> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        self.gather(&plain(path), true, &mut seen, &mut out);
        out
    }

    /// The names of `path`: all of them, or (for `use`) its modules and
    /// functions and those of what it includes.
    fn gather(
        &self,
        path: &Path,
        all: bool,
        seen: &mut HashSet<(PathBuf, bool)>,
        out: &mut Vec<(Def, PathBuf)>,
    ) {
        if !seen.insert((path.to_path_buf(), all)) {
            return;
        }
        let Some(p) = self.file(path) else {
            return;
        };
        for d in &p.index.defs {
            if all || matches!(d.kind, DefKind::Module | DefKind::Function) {
                out.push((d.clone(), path.to_path_buf()));
            }
        }
        for i in &p.index.imports {
            let file = Self::import_path(path, &i.path);
            if i.include {
                self.gather(&file, all, seen, out);
            } else if all {
                self.gather(&file, false, seen, out);
            }
        }
    }

    /// The name at `offset`, the node it is in, and what is wanted of it.
    fn name_at<'t>(p: &'t Parsed, offset: usize) -> Option<(Node<'t>, Want)> {
        let text = &p.text;
        let range = word_at(text, offset);
        if range.is_empty() {
            return None;
        }
        let mut node = p
            .tree
            .root_node()
            .descendant_for_byte_range(range.start, range.end)?;
        if node.kind() == "special_variable" {
            // `$fn` as a whole.
        } else if node.kind() != "identifier" {
            return None;
        } else if let Some(parent) = node.parent()
            && parent.kind() == "special_variable"
        {
            node = parent;
        }
        let want = match node.parent() {
            Some(parent) if parent.kind() == "module_call" => Want::Module,
            Some(parent) if parent.kind() == "module_item" => Want::Module,
            Some(parent) if parent.kind() == "function_item" => Want::Function,
            Some(parent) if parent.kind() == "function_call" => Want::Function,
            _ => Want::Variable,
        };
        Some((node, want))
    }

    /// The definition of the name at `offset` in `path`: a local one, then
    /// the file's own, then what it includes and uses.
    fn resolve(&self, path: &Path, offset: usize) -> Option<(Resolved, Range<usize>)> {
        let p = self.file(path)?;
        let (node, want) = Self::name_at(&p, offset)?;
        let name = &p.text[node.byte_range()];
        let range = node.byte_range();
        let fits = |d: &Def| d.name == name && want.fits(d.kind);
        if want != Want::Module
            && let Some(d) = locals(&p.tree, &p.text, offset)
                .into_iter()
                .find(|d| fits(d))
        {
            return Some((Resolved::User(d, plain(path)), range));
        }
        if let Some((d, file)) = self.visible(path).into_iter().find(|(d, _)| fits(d)) {
            return Some((Resolved::User(d, file), range));
        }
        // A module defined in a block around it.
        if want == Want::Module
            && let Some(d) = locals(&p.tree, &p.text, offset)
                .into_iter()
                .find(|d| fits(d))
        {
            return Some((Resolved::User(d, plain(path)), range));
        }
        let b = builtin(name).filter(|b| want.fits_builtin(b.kind))?;
        Some((Resolved::Builtin(b), range))
    }

    /// Where the name at `offset` is defined; the file an `include` or
    /// `use` names when at its path.
    pub fn definition(&self, path: &Path, offset: usize) -> Option<Location> {
        let p = self.file(path)?;
        if let Some(i) = p.index.imports.iter().find(|i| i.range.contains(&offset)) {
            let file = Self::import_path(path, &i.path);
            return self.file(&file).map(|_| Location {
                path: file,
                range: 0..0,
            });
        }
        match self.resolve(path, offset)?.0 {
            Resolved::User(d, file) => Some(Location {
                path: file,
                range: d.name_range,
            }),
            Resolved::Builtin(_) => None,
        }
    }

    /// The signature and explanation of the name at `offset`.
    pub fn hover(&self, path: &Path, offset: usize) -> Option<Hover> {
        let (resolved, range) = self.resolve(path, offset)?;
        let (signature, doc) = match resolved {
            Resolved::User(d, file) => {
                let mut doc = d.doc;
                if file != plain(path)
                    && let Some(name) = file.file_name()
                {
                    let from = format!("*{}*", name.to_string_lossy());
                    doc = if doc.is_empty() {
                        from
                    } else {
                        format!("{doc}\n\n{from}")
                    };
                }
                (d.signature, doc)
            }
            Resolved::Builtin(b) => (b.signature.to_string(), b.doc.to_string()),
        };
        let markdown = if doc.is_empty() {
            format!("```openscad\n{signature}\n```")
        } else {
            format!("```openscad\n{signature}\n```\n\n{doc}")
        };
        Some(Hover {
            range,
            markdown,
            signature,
            doc,
        })
    }

    /// What may be written at `offset`: a file in `include <…>` and
    /// `use <…>`, else the names the place sees and OpenSCAD's own, those
    /// that begin as the word typed so far.
    pub fn completion(&self, path: &Path, offset: usize) -> Vec<Completion> {
        let Some(p) = self.file(path) else {
            return Vec::new();
        };
        let text = &p.text;
        let offset = crate::text::floor(text, offset.min(text.len()));
        let line_start = text[..offset].rfind('\n').map_or(0, |i| i + 1);
        if let Some(files) = self.complete_path(path, &text[line_start..offset], offset) {
            return files;
        }
        let start = text[..offset]
            .char_indices()
            .rev()
            .take_while(|(_, c)| is_word(*c))
            .last()
            .map_or(offset, |(i, _)| i);
        let typed = &text[start..offset];
        let replace = start..offset;
        let fits = |name: &str| name.to_lowercase().starts_with(&typed.to_lowercase());
        let mut out: Vec<Completion> = Vec::new();
        let mut names = HashSet::new();
        // In a call's arguments: its parameters' names where an argument
        // starts, and no modules, which are no values.
        let call = call_at(text, start);
        let mut add = |c: Completion| {
            let module = c.kind == CompletionKind::Module && call.is_some();
            if !module && fits(&c.label) && names.insert((c.label.clone(), c.kind)) {
                out.push(c);
            }
        };
        if let Some(call) = call.as_ref().filter(|c| c.at_name) {
            for (name, detail, doc) in self.call_parameters(path, text, call) {
                add(Completion {
                    insert: format!("{name} = "),
                    label: name,
                    kind: CompletionKind::Parameter,
                    detail,
                    doc,
                    replace: replace.clone(),
                });
            }
        }
        let defs = locals(&p.tree, text, offset)
            .into_iter()
            .map(|d| (d, plain(path)))
            .chain(self.visible(path));
        for (d, _) in defs {
            // Not the name being typed.
            if d.name_range.end == offset {
                continue;
            }
            add(Completion {
                label: d.name.clone(),
                kind: match d.kind {
                    DefKind::Module => CompletionKind::Module,
                    DefKind::Function => CompletionKind::Function,
                    DefKind::Variable => CompletionKind::Variable,
                    DefKind::Parameter => CompletionKind::Parameter,
                },
                insert: d.name.clone(),
                detail: d.signature,
                doc: d.doc,
                replace: replace.clone(),
            });
        }
        for b in BUILTINS {
            add(builtin_completion(b, replace.clone()));
        }
        out
    }

    /// The files and folders for the path being typed in `include <…`.
    fn complete_path(&self, path: &Path, line: &str, offset: usize) -> Option<Vec<Completion>> {
        let open = line.rfind('<')?;
        let head = line[..open].trim_end();
        if !(head.ends_with("include") || head.ends_with("use")) || line[open..].contains('>') {
            return None;
        }
        let typed = &line[open + 1..];
        let (dir_part, name_part) = match typed.rfind('/') {
            Some(i) => (&typed[..=i], &typed[i + 1..]),
            None => ("", typed),
        };
        let base = path.parent().unwrap_or(Path::new("")).join(dir_part);
        let mut out = Vec::new();
        let entries = std::fs::read_dir(&base).ok()?;
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let is_dir = e.file_type().is_ok_and(|t| t.is_dir());
            let scad = Path::new(&name)
                .extension()
                .is_some_and(|x| x.eq_ignore_ascii_case("scad"));
            if name.starts_with('.') || !(is_dir || scad) || !name.starts_with(name_part) {
                continue;
            }
            let label = if is_dir { format!("{name}/") } else { name };
            out.push(Completion {
                insert: label.clone(),
                label,
                kind: CompletionKind::File,
                detail: String::new(),
                doc: String::new(),
                replace: offset - name_part.len()..offset,
            });
        }
        out.sort_by(|a, b| a.label.cmp(&b.label));
        Some(out)
    }

    /// The signature of the call whose arguments `offset` is in.
    pub fn signature(&self, path: &Path, offset: usize) -> Option<Signature> {
        let p = self.file(path)?;
        let mut node = p
            .tree
            .root_node()
            .descendant_for_byte_range(offset, offset)?;
        let args = loop {
            if node.kind() == "arguments" && node.start_byte() < offset && offset <= node.end_byte()
            {
                // Not behind the closing parenthesis.
                let closed = node
                    .child(node.child_count().saturating_sub(1))
                    .is_some_and(|c| c.kind() == ")" && c.end_byte() <= offset);
                if !closed {
                    break node;
                }
            }
            node = node.parent()?;
        };
        let call = args.parent()?;
        let name_node = call.child_by_field_name("name").or_else(|| call.child(0))?;
        let name_end = name_node.end_byte();
        let mut cursor = args.walk();
        let active = args
            .children(&mut cursor)
            .filter(|c| c.kind() == "," && c.end_byte() <= offset)
            .count();
        let (label, doc) = match self.resolve(path, name_end) {
            Some((Resolved::User(d, _), _)) => (
                d.signature
                    .trim_start_matches("module ")
                    .trim_start_matches("function ")
                    .to_string(),
                d.doc,
            ),
            Some((Resolved::Builtin(b), _)) => (b.signature.to_string(), b.doc.to_string()),
            None => {
                let name = &p.text[name_node.byte_range()];
                let b = builtin(name)?;
                (b.signature.to_string(), b.doc.to_string())
            }
        };
        Some(Signature {
            parameters: parameters(&label),
            label,
            active,
            doc,
        })
    }
}

/// The call whose arguments a place is in, found in the text: the tree of
/// a call still being typed is broken.
#[derive(Debug)]
struct Call {
    /// Its name.
    name: Range<usize>,
    /// Its arguments before the place.
    args: Range<usize>,
    /// The place starts an argument: a parameter's name may go there.
    at_name: bool,
}

/// The call whose open parenthesis is before `offset`, if any.
fn call_at(text: &str, offset: usize) -> Option<Call> {
    let mut depth = 0usize;
    let mut arg_start = None;
    for (i, c) in text[..offset].char_indices().rev() {
        match c {
            ')' | ']' | '}' => depth += 1,
            '[' | '{' if depth > 0 => depth -= 1,
            '(' if depth > 0 => depth -= 1,
            ',' if depth == 0 => {
                arg_start.get_or_insert(i + 1);
            }
            '(' => {
                let before = text[..i].trim_end();
                let name_start = before
                    .char_indices()
                    .rev()
                    .take_while(|(_, c)| is_word(*c))
                    .last()
                    .map_or(before.len(), |(j, _)| j);
                let name = name_start..before.len();
                let word = &text[name.clone()];
                let keyword = matches!(word, "if" | "for" | "let" | "each" | "assert" | "echo");
                if word.is_empty() || keyword || word.starts_with(|c: char| c.is_ascii_digit()) {
                    return None;
                }
                let from = arg_start.unwrap_or(i + 1);
                return Some(Call {
                    name,
                    args: i + 1..offset,
                    at_name: text[from..offset].trim().is_empty(),
                });
            }
            ';' | '[' | '{' => return None,
            _ => {}
        }
    }
    None
}

impl Workspace {
    /// The parameters of `call` not given by name yet: their names, how
    /// the signature writes them (`h = 1`, its default) and what they mean.
    fn call_parameters(
        &self,
        path: &Path,
        text: &str,
        call: &Call,
    ) -> Vec<(String, String, String)> {
        let name = &text[call.name.clone()];
        let signature = match self.resolve(path, call.name.end) {
            Some((Resolved::User(d, _), _)) => Some(d.signature),
            Some((Resolved::Builtin(b), _)) => Some(b.signature.to_string()),
            None => builtin(name).map(|b| b.signature.to_string()).or_else(|| {
                self.visible(path)
                    .into_iter()
                    .map(|(d, _)| d)
                    .find(|d| d.name == name && d.kind != DefKind::Variable)
                    .map(|d| d.signature)
            }),
        };
        let given = &text[call.args.clone()];
        parameters(&signature.unwrap_or_default())
            .into_iter()
            .filter_map(|p| {
                let n = p.split('=').next().unwrap_or("").trim().to_string();
                let named = given
                    .split(',')
                    .any(|a| a.split('=').next().is_some_and(|x| x.trim() == n) && a.contains('='));
                let doc = parameter_doc(name, &n).unwrap_or_default().to_string();
                (!n.is_empty() && !named).then_some((n, p, doc))
            })
            .collect()
    }
}

fn builtin_completion(b: &Builtin, replace: Range<usize>) -> Completion {
    Completion {
        label: b.name.to_string(),
        insert: b.name.to_string(),
        kind: match b.kind {
            BuiltinKind::Module => CompletionKind::Module,
            BuiltinKind::Function => CompletionKind::Function,
            BuiltinKind::Variable => CompletionKind::Constant,
            BuiltinKind::Keyword => CompletionKind::Keyword,
        },
        detail: b.signature.to_string(),
        doc: b.doc.to_string(),
        replace,
    }
}

/// What a name at a place wants to be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Want {
    Module,
    Function,
    Variable,
}

impl Want {
    fn fits(self, kind: DefKind) -> bool {
        match self {
            Want::Module => kind == DefKind::Module,
            // A variable may hold a function literal.
            Want::Function => kind != DefKind::Module,
            Want::Variable => matches!(kind, DefKind::Variable | DefKind::Parameter),
        }
    }

    fn fits_builtin(self, kind: BuiltinKind) -> bool {
        match self {
            Want::Module => matches!(kind, BuiltinKind::Module | BuiltinKind::Keyword),
            Want::Function => matches!(kind, BuiltinKind::Function | BuiltinKind::Keyword),
            Want::Variable => matches!(kind, BuiltinKind::Variable | BuiltinKind::Keyword),
        }
    }
}

enum Resolved {
    User(Def, PathBuf),
    Builtin(&'static Builtin),
}

/// A file to evaluate, with the texts it needs: can be sent to another
/// thread.
#[derive(Clone, Debug)]
pub struct Evaluation {
    pub path: PathBuf,
    pub sources: HashMap<PathBuf, String>,
}

impl Evaluation {
    /// Evaluates the file as OpenSolve builds it, and gives what it finds
    /// in this file: unknown names, what is not supported, failed
    /// assertions, `echo` output.
    pub fn run(self) -> Vec<Diagnostic> {
        let Some(main) = self.sources.get(&self.path).cloned() else {
            return Vec::new();
        };
        let read = |p: &Path| {
            self.sources
                .get(p)
                .map(|t| t.as_bytes().to_vec())
                .ok_or_else(|| format!("{} cannot be read", p.display()))
        };
        let found = opensolve_scad::diagnose(&self.path, &read, &[]);
        let lines = Lines::new(&main);
        let here = |file: Option<&Path>| file.is_none_or(|f| plain(f) == self.path);
        let range = |line: u32, col: u32| {
            if line == 0 {
                return 0..0;
            }
            let at = lines.from_chars(&main, line, col);
            let word = word_at(&main, at);
            if word.start == at && !word.is_empty() {
                word
            } else {
                let end = lines.line_range(&main, line as usize - 1).end;
                at..end.max(at)
            }
        };
        let mut out = Vec::new();
        for n in found.notes {
            if !here(n.file.as_deref()) {
                continue;
            }
            let severity = if n.echo {
                Severity::Info
            } else {
                Severity::Warning
            };
            out.push(Diagnostic {
                range: range(n.line, n.col),
                severity,
                message: n.message,
            });
        }
        if let Some(e) = found.error {
            let file = e.file.as_deref().map(Path::new);
            let range = if here(file) {
                range(e.line, e.col)
            } else {
                // In a file it includes: marked at the top.
                0..0
            };
            out.push(Diagnostic {
                range,
                severity: Severity::Error,
                message: match &e.file {
                    Some(f) if !here(file) => format!("{f}: {}", e.message),
                    _ => e.message,
                },
            });
        }
        out
    }
}
