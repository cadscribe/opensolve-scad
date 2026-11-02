//! The binary over stdio, as an editor runs it: initialize, open a file,
//! get its diagnostics, ask, shut down.

use std::io::{BufReader, Write};
use std::process::{Command, Stdio};

use opensolve_scad_lsp::server::{read_message, uri_of, write_message};
use serde_json::{Value, json};

#[test]
fn a_round_trip_over_stdio() {
    let dir = std::env::temp_dir().join(format!("opensolve-scad-lsp-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("part.scad");
    let text = "cube(1);\nwidget(2);\nsphere(;\n";
    std::fs::write(&file, text).unwrap();
    let uri = uri_of(&file);

    let mut child = Command::new(env!("CARGO_BIN_EXE_opensolve-scad-lsp"))
        .arg("--stdio")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    let mut send = |m: Value| write_message(&mut input, &m).unwrap();

    send(json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": { "processId": null, "rootUri": null, "capabilities": {} } }));
    let init = read_message(&mut output).unwrap().unwrap();
    assert_eq!(init["id"], 1);
    assert_eq!(init["result"]["capabilities"]["hoverProvider"], true);
    assert_eq!(init["result"]["serverInfo"]["name"], "opensolve-scad-lsp");
    send(json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }));

    send(
        json!({ "jsonrpc": "2.0", "method": "textDocument/didOpen", "params": {
        "textDocument": { "uri": uri, "languageId": "openscad", "version": 1, "text": text } } }),
    );
    let published = read_message(&mut output).unwrap().unwrap();
    assert_eq!(published["method"], "textDocument/publishDiagnostics");
    assert_eq!(published["params"]["uri"], uri.as_str());
    let diagnostics = published["params"]["diagnostics"].as_array().unwrap();
    assert!(!diagnostics.is_empty());
    assert_eq!(diagnostics[0]["severity"], 1);
    assert_eq!(diagnostics[0]["range"]["start"]["line"], 2);

    // Fixed: now the evaluation speaks, of the unknown module.
    let fixed = "cube(1);\nwidget(2);\n";
    send(
        json!({ "jsonrpc": "2.0", "method": "textDocument/didChange", "params": {
        "textDocument": { "uri": uri, "version": 2 },
        "contentChanges": [{ "text": fixed }] } }),
    );
    let published = read_message(&mut output).unwrap().unwrap();
    assert_eq!(published["params"]["diagnostics"], json!([]));
    send(
        json!({ "jsonrpc": "2.0", "method": "textDocument/didSave", "params": {
        "textDocument": { "uri": uri }, "text": fixed } }),
    );
    let published = read_message(&mut output).unwrap().unwrap();
    let d = &published["params"]["diagnostics"][0];
    assert_eq!(d["severity"], 2);
    assert_eq!(d["range"]["start"], json!({ "line": 1, "character": 0 }));
    assert_eq!(d["range"]["end"], json!({ "line": 1, "character": 6 }));

    send(
        json!({ "jsonrpc": "2.0", "id": 2, "method": "textDocument/hover", "params": {
        "textDocument": { "uri": uri }, "position": { "line": 0, "character": 1 } } }),
    );
    let hover = read_message(&mut output).unwrap().unwrap();
    assert_eq!(hover["id"], 2);
    let shown = hover["result"]["contents"]["value"].as_str().unwrap();
    assert!(shown.contains("cube(size"), "{shown}");

    send(json!({ "jsonrpc": "2.0", "id": 3, "method": "shutdown" }));
    let done = read_message(&mut output).unwrap().unwrap();
    assert_eq!(done["id"], 3);
    send(json!({ "jsonrpc": "2.0", "method": "exit" }));
    input.flush().unwrap();
    drop(input);
    let status = child.wait().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(status.success());
}
