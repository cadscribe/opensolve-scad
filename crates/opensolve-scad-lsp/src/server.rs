//! The Language Server Protocol over stdio: JSON-RPC messages with a
//! `Content-Length` header, positions in UTF-16 code units. Written on
//! `serde_json` alone, as the MCP server is, for the few messages a
//! language server needs.

use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::index::DefKind;
use crate::text::Lines;
use crate::workspace::{CompletionKind, Workspace};
use crate::{Diagnostic, Severity};

/// Reads one message; `None` at the end of the input.
pub fn read_message(input: &mut impl BufRead) -> io::Result<Option<Value>> {
    let mut length = None;
    loop {
        let mut line = String::new();
        if input.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        let line = line.trim_end();
        if line.is_empty() {
            if length.is_some() {
                break;
            }
            continue;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            length = value.trim().parse::<usize>().ok();
        }
    }
    let mut body = vec![0; length.unwrap_or(0)];
    input.read_exact(&mut body)?;
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// Writes one message with its header.
pub fn write_message(output: &mut impl Write, message: &Value) -> io::Result<()> {
    let body = message.to_string();
    write!(output, "Content-Length: {}\r\n\r\n{body}", body.len())?;
    output.flush()
}

/// Answers messages from `input` on `output` until `exit` or the end of
/// the input.
pub fn serve(input: &mut impl BufRead, output: &mut impl Write) -> io::Result<()> {
    let mut server = Server::default();
    while let Some(message) = read_message(input)? {
        for out in server.handle(&message) {
            write_message(output, &out)?;
        }
        if server.exit {
            break;
        }
    }
    Ok(())
}

/// The server's state between messages.
#[derive(Debug, Default)]
pub struct Server {
    pub workspace: Workspace,
    shutdown: bool,
    /// `exit` came.
    pub exit: bool,
}

impl Server {
    /// The messages to send for one received.
    pub fn handle(&mut self, message: &Value) -> Vec<Value> {
        let method = message["method"].as_str().unwrap_or_default();
        let params = &message["params"];
        let Some(id) = message.get("id").cloned() else {
            return self.notification(method, params);
        };
        if method.is_empty() {
            // An answer to a request of ours: we send none.
            return Vec::new();
        }
        let result = match method {
            "initialize" => Ok(initialize()),
            "shutdown" => {
                self.shutdown = true;
                Ok(Value::Null)
            }
            _ if self.shutdown => Err((-32600, "shut down".to_string())),
            "textDocument/completion" => Ok(self.completion(params)),
            "textDocument/hover" => Ok(self.hover(params)),
            "textDocument/definition" => Ok(self.definition(params)),
            "textDocument/documentSymbol" => Ok(self.symbols(params)),
            "textDocument/signatureHelp" => Ok(self.signature(params)),
            _ => Err((-32601, format!("{method} is not supported"))),
        };
        vec![match result {
            Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            Err((code, message)) => json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": code, "message": message },
            }),
        }]
    }

    fn notification(&mut self, method: &str, params: &Value) -> Vec<Value> {
        let path = || path_of(params["textDocument"]["uri"].as_str().unwrap_or_default());
        match method {
            "exit" => {
                self.exit = true;
                Vec::new()
            }
            "textDocument/didOpen" => {
                let Some(path) = path() else {
                    return Vec::new();
                };
                let text = params["textDocument"]["text"].as_str().unwrap_or_default();
                self.workspace.set(&path, text.to_string());
                vec![self.publish(&path, true)]
            }
            "textDocument/didChange" => {
                let Some(path) = path() else {
                    return Vec::new();
                };
                // Whole texts only (sync kind 1): the last change is the text.
                let changes = params["contentChanges"].as_array();
                let Some(text) = changes
                    .and_then(|c| c.last())
                    .and_then(|c| c["text"].as_str())
                else {
                    return Vec::new();
                };
                self.workspace.set(&path, text.to_string());
                vec![self.publish(&path, false)]
            }
            "textDocument/didSave" => {
                let Some(path) = path() else {
                    return Vec::new();
                };
                if let Some(text) = params["text"].as_str() {
                    self.workspace.set(&path, text.to_string());
                }
                vec![self.publish(&path, true)]
            }
            "textDocument/didClose" => {
                let Some(path) = path() else {
                    return Vec::new();
                };
                self.workspace.close(&path);
                vec![json!({
                    "jsonrpc": "2.0",
                    "method": "textDocument/publishDiagnostics",
                    "params": { "uri": uri_of(&path), "diagnostics": [] },
                })]
            }
            _ => Vec::new(),
        }
    }

    /// The diagnostics of a file: the syntax always, the evaluation when
    /// opened or saved (it builds the shapes, too slow for each key).
    fn publish(&self, path: &Path, evaluate: bool) -> Value {
        let found = if evaluate {
            self.workspace.diagnostics(path)
        } else {
            self.workspace.syntax_diagnostics(path)
        };
        let file = self.workspace.file(path);
        let (text, lines) = file
            .as_ref()
            .map_or(("", None), |f| (f.text.as_str(), Some(&f.lines)));
        let diagnostics: Vec<Value> = match lines {
            Some(lines) => found
                .iter()
                .map(|d| diagnostic_json(d, text, lines))
                .collect(),
            None => Vec::new(),
        };
        json!({
            "jsonrpc": "2.0",
            "method": "textDocument/publishDiagnostics",
            "params": { "uri": uri_of(path), "diagnostics": diagnostics },
        })
    }

    /// The file and byte offset of a request's position.
    fn place(&self, params: &Value) -> Option<(PathBuf, usize)> {
        let path = path_of(params["textDocument"]["uri"].as_str()?)?;
        let file = self.workspace.file(&path)?;
        let line = params["position"]["line"].as_u64()? as u32;
        let col = params["position"]["character"].as_u64()? as u32;
        let offset = file.lines.from_utf16(&file.text, line, col);
        Some((path, offset))
    }

    fn range_json(&self, path: &Path, range: &std::ops::Range<usize>) -> Value {
        match self.workspace.file(path) {
            Some(f) => range_json(&f.text, &f.lines, range),
            None => range_json("", &Lines::new(""), &(0..0)),
        }
    }

    fn completion(&self, params: &Value) -> Value {
        let Some((path, offset)) = self.place(params) else {
            return Value::Null;
        };
        let items: Vec<Value> = self
            .workspace
            .completion(&path, offset)
            .into_iter()
            .map(|c| {
                json!({
                    "label": c.label,
                    "kind": match c.kind {
                        CompletionKind::Module => 9,
                        CompletionKind::Function => 3,
                        CompletionKind::Variable => 6,
                        CompletionKind::Parameter => 6,
                        CompletionKind::Constant => 21,
                        CompletionKind::Keyword => 14,
                        CompletionKind::File => 17,
                    },
                    "detail": c.detail,
                    "documentation": { "kind": "markdown", "value": c.doc },
                    "textEdit": {
                        "range": self.range_json(&path, &c.replace),
                        "newText": c.label,
                    },
                })
            })
            .collect();
        json!({ "isIncomplete": false, "items": items })
    }

    fn hover(&self, params: &Value) -> Value {
        let Some((path, offset)) = self.place(params) else {
            return Value::Null;
        };
        match self.workspace.hover(&path, offset) {
            Some(h) => json!({
                "contents": { "kind": "markdown", "value": h.markdown },
                "range": self.range_json(&path, &h.range),
            }),
            None => Value::Null,
        }
    }

    fn definition(&self, params: &Value) -> Value {
        let Some((path, offset)) = self.place(params) else {
            return Value::Null;
        };
        match self.workspace.definition(&path, offset) {
            Some(l) => json!({
                "uri": uri_of(&l.path),
                "range": self.range_json(&l.path, &l.range),
            }),
            None => Value::Null,
        }
    }

    fn symbols(&self, params: &Value) -> Value {
        let Some(path) = params["textDocument"]["uri"].as_str().and_then(path_of) else {
            return Value::Null;
        };
        let symbols: Vec<Value> = self
            .workspace
            .symbols(&path)
            .into_iter()
            .map(|s| {
                json!({
                    "name": s.name,
                    "detail": s.detail,
                    "kind": match s.kind {
                        DefKind::Module => 6,
                        DefKind::Function => 12,
                        DefKind::Variable | DefKind::Parameter => 13,
                    },
                    "range": self.range_json(&path, &s.range),
                    "selectionRange": self.range_json(&path, &s.name_range),
                })
            })
            .collect();
        Value::Array(symbols)
    }

    fn signature(&self, params: &Value) -> Value {
        let Some((path, offset)) = self.place(params) else {
            return Value::Null;
        };
        match self.workspace.signature(&path, offset) {
            Some(s) => {
                let active = s.active.min(s.parameters.len().saturating_sub(1));
                json!({
                    "signatures": [{
                        "label": s.label,
                        "documentation": { "kind": "markdown", "value": s.doc },
                        "parameters": s.parameters.iter()
                            .map(|p| json!({ "label": p }))
                            .collect::<Vec<_>>(),
                    }],
                    "activeSignature": 0,
                    "activeParameter": active,
                })
            }
            None => Value::Null,
        }
    }
}

/// What the server can do.
fn initialize() -> Value {
    json!({
        "capabilities": {
            "positionEncoding": "utf-16",
            "textDocumentSync": {
                "openClose": true,
                "change": 1,
                "save": { "includeText": true },
            },
            "completionProvider": { "triggerCharacters": ["$", "<", "/"] },
            "hoverProvider": true,
            "definitionProvider": true,
            "documentSymbolProvider": true,
            "signatureHelpProvider": { "triggerCharacters": ["(", ","] },
        },
        "serverInfo": { "name": "opensolve-scad-lsp", "version": env!("CARGO_PKG_VERSION") },
    })
}

fn position_json(text: &str, lines: &Lines, offset: usize) -> Value {
    let (line, character) = lines.utf16(text, offset);
    json!({ "line": line, "character": character })
}

fn range_json(text: &str, lines: &Lines, range: &std::ops::Range<usize>) -> Value {
    json!({
        "start": position_json(text, lines, range.start),
        "end": position_json(text, lines, range.end),
    })
}

fn diagnostic_json(d: &Diagnostic, text: &str, lines: &Lines) -> Value {
    json!({
        "range": range_json(text, lines, &d.range),
        "severity": match d.severity {
            Severity::Error => 1,
            Severity::Warning => 2,
            Severity::Info => 3,
        },
        "source": "opensolve-scad",
        "message": d.message,
    })
}

/// The path of a `file:` URI.
pub fn path_of(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    // `file://host/…` is not a local file; `file:///…` is.
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    let mut bytes = Vec::with_capacity(rest.len());
    let mut it = rest.bytes();
    while let Some(b) = it.next() {
        if b == b'%' {
            let hex = [it.next()?, it.next()?];
            let hex = std::str::from_utf8(&hex).ok()?;
            bytes.push(u8::from_str_radix(hex, 16).ok()?);
        } else {
            bytes.push(b);
        }
    }
    let path = String::from_utf8(bytes).ok()?;
    // `/C:/…` on Windows.
    let path = match path.as_bytes() {
        [b'/', d, b':', ..] if d.is_ascii_alphabetic() => path[1..].to_string(),
        _ => path,
    };
    Some(PathBuf::from(path))
}

/// The `file:` URI of a path.
pub fn uri_of(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    let mut out = String::from("file://");
    if !text.starts_with('/') {
        out.push('/');
    }
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || b"/-_.~:".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uris_and_paths_both_ways() {
        let p = Path::new("/tmp/a b/ü.scad");
        let uri = uri_of(p);
        assert_eq!(uri, "file:///tmp/a%20b/%C3%BC.scad");
        assert_eq!(path_of(&uri).unwrap(), p);
        assert_eq!(
            path_of("file:///C:/x/y.scad").unwrap(),
            PathBuf::from("C:/x/y.scad")
        );
    }
}
