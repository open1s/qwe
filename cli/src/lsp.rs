//! `pwe lsp` — a minimal, dependency-free Language Server over stdio.
//!
//! Speaks the LSP base protocol (JSON-RPC framed with `Content-Length`). It
//! implements full-document sync, **publishDiagnostics** on open/change/close
//! (via `lang::compile` + `lang::diagnose`), and **textDocument/formatting**
//! (via `lang::format_source`). No completion/hover/semantic tokens (yet).

use crate::json::Json;
use std::io::{BufRead, Write};

fn obj(fields: Vec<(&str, Json)>) -> Json {
    Json::Obj(
        fields
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect(),
    )
}

fn caps() -> Json {
    obj(vec![
        (
            "capabilities",
            obj(vec![
                ("textDocumentSync", Json::Num(1.0)),
                ("documentFormattingProvider", Json::Bool(true)),
            ]),
        ),
        ("serverInfo", obj(vec![("name", Json::Str("pwe".into()))])),
    ])
}

/// Runs the server until EOF / `exit`.
pub fn run<R: BufRead, W: Write>(mut input: R, out: &mut W) -> i32 {
    let mut docs: std::collections::HashMap<String, String> = Default::default();
    while let Some(body) = read_message(&mut input) {
        let Ok(msg) = crate::json::parse(&body) else {
            continue;
        };
        let method = msg
            .get("method")
            .and_then(Json::as_str)
            .unwrap_or("")
            .to_string();
        let id = msg.get("id").cloned();
        let params = msg.get("params");
        match method.as_str() {
            "initialize" => respond(out, id, caps()),
            "initialized" | "$/cancelRequest" | "$/setTrace" => {}
            "shutdown" => respond(out, id, Json::Null),
            "exit" => break,
            "textDocument/didOpen" | "textDocument/didChange" => {
                if let Some((uri, text)) = doc_change(&method, params) {
                    let diags = diagnostics(&text);
                    docs.insert(uri.clone(), text);
                    publish(out, &uri, diags);
                }
            }
            "textDocument/didClose" => {
                if let Some(uri) = doc_uri(params) {
                    docs.remove(&uri);
                    publish(out, &uri, Vec::new());
                }
            }
            "textDocument/formatting" => {
                let uri = doc_uri(params).unwrap_or_default();
                let text = docs.get(&uri).cloned().unwrap_or_default();
                let formatted = pwe_reference::lang::format_source(&text);
                let edits = if formatted == text {
                    Vec::new()
                } else {
                    vec![obj(vec![
                        ("range", full_range(&text)),
                        ("newText", Json::Str(formatted)),
                    ])]
                };
                respond(out, id, Json::Arr(edits));
            }
            other => {
                // Unknown request: reply null (never hang the client).
                if id.is_some() {
                    let _ = other;
                    respond(out, id, Json::Null);
                }
            }
        }
    }
    0
}

fn doc_uri(params: Option<&Json>) -> Option<String> {
    params?
        .get("textDocument")?
        .get("uri")?
        .as_str()
        .map(str::to_string)
}

fn doc_change(method: &str, params: Option<&Json>) -> Option<(String, String)> {
    let uri = doc_uri(params)?;
    let p = params?;
    let text = if method.ends_with("didOpen") {
        p.get("textDocument")?.get("text")?.as_str()?.to_string()
    } else {
        p.get("contentChanges")?
            .as_arr()?
            .first()?
            .get("text")?
            .as_str()?
            .to_string()
    };
    Some((uri, text))
}

/// Diagnostics for a document (errors as severity 1, warnings as 2).
fn diagnostics(text: &str) -> Vec<Json> {
    pwe_reference::lang::clear_diagnostics();
    let mut diags = Vec::new();
    match pwe_reference::lang::compile(text) {
        Ok(_) => {
            for d in pwe_reference::lang::take_diagnostics() {
                diags.push(diag_json(d.detail, &d.message, 0, text, 2, 0));
            }
        }
        Err(e) => {
            let recorded = pwe_reference::lang::take_diagnostics();
            let message = recorded
                .iter()
                .find(|d| d.detail == e.detail)
                .map(|d| d.message.clone())
                .unwrap_or_else(|| pwe_reference::lang::detail_name(e.detail).to_string());
            diags.push(diag_json(
                e.detail,
                &message,
                e.byte_offset as usize,
                text,
                1,
                0,
            ));
        }
    }
    let _ = pwe_reference::lang::take_diagnostics();
    diags
}

fn diag_json(
    detail: u32,
    message: &str,
    offset: usize,
    text: &str,
    severity: u32,
    _end: u32,
) -> Json {
    let (line, character) = pos_at(text, offset);
    obj(vec![
        (
            "range",
            obj(vec![
                (
                    "start",
                    obj(vec![
                        ("line", Json::Num(line as f64)),
                        ("character", Json::Num(character as f64)),
                    ]),
                ),
                (
                    "end",
                    obj(vec![
                        ("line", Json::Num(line as f64)),
                        ("character", Json::Num((character + 1) as f64)),
                    ]),
                ),
            ]),
        ),
        ("severity", Json::Num(severity as f64)),
        ("source", Json::Str("pwe".into())),
        ("code", Json::Num(detail as f64)),
        ("message", Json::Str(message.to_string())),
    ])
}

fn full_range(text: &str) -> Json {
    let (line, character) = pos_at(text, text.len());
    obj(vec![
        (
            "start",
            obj(vec![
                ("line", Json::Num(0.0)),
                ("character", Json::Num(0.0)),
            ]),
        ),
        (
            "end",
            obj(vec![
                ("line", Json::Num(line as f64)),
                ("character", Json::Num(character as f64)),
            ]),
        ),
    ])
}

/// Byte offset -> (0-based line, 0-based character). ASCII-exact; for
/// non-ASCII, `character` counts code points (adequate for PWE source).
fn pos_at(text: &str, offset: usize) -> (usize, usize) {
    let mut line = 0usize;
    let mut col = 0usize;
    for (i, c) in text.char_indices() {
        if i >= offset {
            break;
        }
        if c == '\n' {
            line += 1;
            col = 0;
        } else {
            col += 1;
        }
    }
    (line, col)
}

fn respond<W: Write>(out: &mut W, id: Option<Json>, result: Json) {
    let id = id.unwrap_or(Json::Null);
    write_message(
        out,
        &obj(vec![
            ("jsonrpc", Json::Str("2.0".into())),
            ("id", id),
            ("result", result),
        ]),
    );
}

fn publish<W: Write>(out: &mut W, uri: &str, diags: Vec<Json>) {
    write_message(
        out,
        &obj(vec![
            ("jsonrpc", Json::Str("2.0".into())),
            (
                "method",
                Json::Str("textDocument/publishDiagnostics".into()),
            ),
            (
                "params",
                obj(vec![
                    ("uri", Json::Str(uri.to_string())),
                    ("diagnostics", Json::Arr(diags)),
                ]),
            ),
        ]),
    );
}

fn read_message<R: BufRead>(input: &mut R) -> Option<String> {
    let mut len = 0usize;
    let mut line = String::new();
    loop {
        line.clear();
        match input.read_line(&mut line) {
            Ok(0) => return None,
            Ok(_) => {}
            Err(_) => return None,
        }
        let t = line.trim_end_matches(['\r', '\n']);
        if t.is_empty() {
            break;
        }
        if let Some(v) = t.strip_prefix("Content-Length:") {
            len = v.trim().parse().unwrap_or(0);
        }
    }
    if len == 0 {
        return None;
    }
    let mut buf = vec![0u8; len];
    input.read_exact(&mut buf).ok()?;
    String::from_utf8(buf).ok()
}

fn write_message<W: Write>(out: &mut W, v: &Json) {
    let body = v.to_json();
    let _ = write!(out, "Content-Length: {}\r\n\r\n{}", body.len(), body);
    let _ = out.flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(v: &Json) -> String {
        let b = v.to_json();
        format!("Content-Length: {}\r\n\r\n{}", b.len(), b)
    }

    fn req(id: i64, method: &str, params: Json) -> String {
        frame(&obj(vec![
            ("jsonrpc", Json::Str("2.0".into())),
            ("id", Json::Num(id as f64)),
            ("method", Json::Str(method.into())),
            ("params", params),
        ]))
    }

    #[test]
    fn initialize_returns_capabilities() {
        let mut input = req(1, "initialize", obj(vec![]));
        input.push_str(&req(2, "shutdown", obj(vec![])));
        input.push_str(&frame(&obj(vec![
            ("jsonrpc", Json::Str("2.0".into())),
            ("method", Json::Str("exit".into())),
        ])));
        let mut out = Vec::new();
        assert_eq!(run(std::io::Cursor::new(input), &mut out), 0);
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("documentFormattingProvider"));
        assert!(s.contains("\"id\":2"));
    }

    #[test]
    fn did_open_publishes_diagnostics_and_formatting_edits() {
        let bad = "world { entity e { state=(x=1.0) } }\nsystems { update { on=e; dt=1.0 x = nope(1.0) } }\n";
        let open = req(
            1,
            "textDocument/didOpen",
            obj(vec![(
                "textDocument",
                obj(vec![
                    ("uri", Json::Str("file:///t.pwe".into())),
                    ("text", Json::Str(bad.into())),
                ]),
            )]),
        );
        let fmt = req(
            2,
            "textDocument/formatting",
            obj(vec![(
                "textDocument",
                obj(vec![("uri", Json::Str("file:///t.pwe".into()))]),
            )]),
        );
        let mut input = open + &fmt;
        input.push_str(&req(3, "shutdown", obj(vec![])));
        input.push_str(&frame(&obj(vec![
            ("jsonrpc", Json::Str("2.0".into())),
            ("method", Json::Str("exit".into())),
        ])));
        let mut out = Vec::new();
        run(std::io::Cursor::new(input), &mut out);
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("publishDiagnostics"), "{s}");
        assert!(s.contains("nope"), "{s}");
        assert!(s.contains("\"id\":2"), "{s}");
    }
}
