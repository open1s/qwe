//! `pwe lsp` — a Language Server over stdio.
//!
//! Speaks the LSP base protocol (JSON-RPC framed with `Content-Length`) using
//! `serde_json`. It implements full-document sync, **publishDiagnostics** on
//! open/change/close (via `lang::compile` + diagnostics), and
//! **textDocument/formatting** (via `lang::format_source`). No completion/hover/
//! semantic tokens (yet).

use serde_json::{json, Value};
use std::io::{BufRead, Write};

fn capabilities() -> Value {
    json!({
        "capabilities": {
            "textDocumentSync": 1,
            "documentFormattingProvider": true,
        },
        "serverInfo": { "name": "pwe" },
    })
}

/// Runs the server until EOF / `exit`.
pub fn run<R: BufRead, W: Write>(mut input: R, out: &mut W) -> i32 {
    let mut docs: std::collections::HashMap<String, String> = Default::default();
    while let Some(body) = read_message(&mut input) {
        let Ok(msg) = serde_json::from_str::<Value>(&body) else {
            continue;
        };
        let method = msg
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let id = msg.get("id").cloned();
        let params = msg.get("params");
        match method.as_str() {
            "initialize" => respond(out, id, capabilities()),
            "initialized" | "$/cancelRequest" | "$/setTrace" => {}
            "shutdown" => respond(out, id, Value::Null),
            "exit" => break,
            "textDocument/didOpen" | "textDocument/didChange" => {
                if let Some((uri, text)) = doc_change(&method, params) {
                    let diags = diagnostics(&uri, &text);
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
                let edits: Vec<Value> = if formatted == text {
                    Vec::new()
                } else {
                    vec![json!({ "range": full_range(&text), "newText": formatted })]
                };
                respond(out, id, Value::Array(edits));
            }
            _ => {
                // Unknown request: reply null (never hang the client).
                if id.is_some() {
                    respond(out, id, Value::Null);
                }
            }
        }
    }
    0
}

fn doc_uri(params: Option<&Value>) -> Option<String> {
    params?
        .get("textDocument")?
        .get("uri")?
        .as_str()
        .map(str::to_string)
}

fn doc_change(method: &str, params: Option<&Value>) -> Option<(String, String)> {
    let uri = doc_uri(params)?;
    let p = params?;
    let text = if method.ends_with("didOpen") {
        p.get("textDocument")?.get("text")?.as_str()?.to_string()
    } else {
        p.get("contentChanges")?
            .as_array()?
            .first()?
            .get("text")?
            .as_str()?
            .to_string()
    };
    Some((uri, text))
}

/// Diagnostics for a document (errors as severity 1, warnings as 2).
///
/// A document that uses `import` is compiled **from its file path** so modules
/// resolve (`load_program_sources`), like the CLI; otherwise it is compiled
/// in-memory.
fn diagnostics(uri: &str, text: &str) -> Vec<Value> {
    pwe_reference::lang::clear_diagnostics();
    let mut diags = Vec::new();
    let compiled: Result<(), pwe_api::Error> = if text.contains("import") {
        match uri_file_path(uri) {
            Some(path) => pwe_reference::lang::load_program_sources_with_root(
                std::path::Path::new(&path),
                text,
            )
            .and_then(|(parsed, _sources)| pwe_reference::lang::compile_program(parsed))
            .map(|_| ()),
            None => pwe_reference::lang::compile(text).map(|_| ()),
        }
    } else {
        pwe_reference::lang::compile(text).map(|_| ())
    };
    match compiled {
        Ok(_) => {
            for d in pwe_reference::lang::take_diagnostics() {
                diags.push(diag_json(d.detail, &d.message, 0, text, 2));
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
            ));
        }
    }
    let _ = pwe_reference::lang::take_diagnostics();
    diags
}

fn diag_json(detail: u32, message: &str, offset: usize, text: &str, severity: u32) -> Value {
    // Warnings record offset 0; locate the offending name from the message
    // (`` `name` ``) so the range points at it rather than at 0:0.
    let offset = if offset == 0 {
        backtick_name(message)
            .and_then(|name| find_ident(text, &name))
            .unwrap_or(0)
    } else {
        offset
    };
    let (line, character) = pos_at(text, offset);
    json!({
        "range": {
            "start": { "line": line, "character": character },
            "end": { "line": line, "character": character + 1 },
        },
        "severity": severity,
        "source": "pwe",
        "code": detail,
        "message": message,
    })
}

fn full_range(text: &str) -> Value {
    let (line, character) = pos_at(text, text.len());
    json!({
        "start": { "line": 0, "character": 0 },
        "end": { "line": line, "character": character },
    })
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
            // LSP `character` is a UTF-16 code unit offset.
            col += c.len_utf16();
        }
    }
    (line, col)
}

/// The first whole-token occurrence of `name` in **code** (an identifier
/// boundary before and after, and not inside a comment or string literal), so a
/// short name is not matched inside a longer word, a comment, or prose.
fn find_ident(text: &str, name: &str) -> Option<usize> {
    if name.is_empty() {
        return None;
    }
    let masked = mask_comments_and_strings(text);
    let nb = name.as_bytes();
    let is_ident = |c: u8| c.is_ascii_alphanumeric() || c == b'_' || c == b'.';
    let mut i = 0;
    while i + nb.len() <= masked.len() {
        if &masked[i..i + nb.len()] == nb {
            let before = i == 0 || !is_ident(masked[i - 1]);
            let after = i + nb.len() >= masked.len() || !is_ident(masked[i + nb.len()]);
            if before && after {
                return Some(i);
            }
        }
        i += 1;
    }
    None
}

/// Replaces comment/string bytes with spaces (same byte length, so offsets are
/// preserved) so identifier search ignores comments and literals.
fn mask_comments_and_strings(text: &str) -> Vec<u8> {
    let bytes = text.as_bytes();
    let mut out = bytes.to_vec();
    let (mut i, mut in_string, mut in_comment) = (0usize, false, false);
    while i < bytes.len() {
        let c = bytes[i];
        if in_comment {
            if c == b'\n' {
                in_comment = false;
            } else {
                out[i] = b' ';
            }
        } else if in_string {
            out[i] = b' ';
            if c == b'"' {
                in_string = false;
            }
        } else if c == b'"' {
            out[i] = b' ';
            in_string = true;
        } else if c == b'#' || (c == b'/' && bytes.get(i + 1) == Some(&b'/')) {
            out[i] = b' ';
            in_comment = true;
        }
        i += 1;
    }
    out
}

/// The first `` `name` `` in a diagnostic message, if any.
fn backtick_name(message: &str) -> Option<String> {
    let start = message.find('`')? + 1;
    let end = message[start..].find('`')? + start;
    (end > start).then(|| message[start..end].to_string())
}

/// `file:///path` -> `/path` (basic; `%`-decoding omitted).
fn uri_file_path(uri: &str) -> Option<String> {
    let rest = uri.strip_prefix("file://")?;
    if rest.is_empty() {
        return None;
    }
    // `file://host/path` or `file:///path`: drop an optional authority.
    if let Some(after) = rest.strip_prefix('/') {
        Some(format!("/{after}"))
    } else {
        rest.split_once('/').map(|(_, p)| format!("/{p}"))
    }
}

fn respond<W: Write>(out: &mut W, id: Option<Value>, result: Value) {
    let id = id.unwrap_or(Value::Null);
    write_message(
        out,
        &json!({ "jsonrpc": "2.0", "id": id, "result": result }),
    );
}

fn publish<W: Write>(out: &mut W, uri: &str, diags: Vec<Value>) {
    write_message(
        out,
        &json!({
            "jsonrpc": "2.0",
            "method": "textDocument/publishDiagnostics",
            "params": { "uri": uri, "diagnostics": diags },
        }),
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

fn write_message<W: Write>(out: &mut W, v: &Value) {
    let body = v.to_string();
    let _ = write!(out, "Content-Length: {}\r\n\r\n{}", body.len(), body);
    let _ = out.flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(v: &Value) -> String {
        let b = v.to_string();
        format!("Content-Length: {}\r\n\r\n{}", b.len(), b)
    }

    fn req(id: i64, method: &str, params: Value) -> String {
        frame(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))
    }

    #[test]
    fn initialize_returns_capabilities() {
        let mut input = req(1, "initialize", json!({}));
        input.push_str(&req(2, "shutdown", json!({})));
        input.push_str(&frame(&json!({ "jsonrpc": "2.0", "method": "exit" })));
        let mut out = Vec::new();
        assert_eq!(run(std::io::Cursor::new(input), &mut out), 0);
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("documentFormattingProvider"));
        assert!(s.contains("\"id\":2"));
    }

    #[test]
    fn position_helpers() {
        // UTF-16 code units: an emoji is a surrogate pair (2), then "x".
        assert_eq!(pos_at("a\u{1f600}x", "a\u{1f600}".len()), (0, 3));
        assert_eq!(
            backtick_name("unknown identifier `ghost` — reads 0.0").as_deref(),
            Some("ghost")
        );
        assert_eq!(
            uri_file_path("file:///tmp/t.pwe").as_deref(),
            Some("/tmp/t.pwe")
        );
        assert_eq!(uri_file_path("untitled:Untitled-1"), None);
    }

    #[test]
    fn warning_range_points_at_the_name() {
        let d = diag_json(
            85,
            "unknown identifier `ghost` — reads 0.0",
            0,
            "x = ghost + 1",
            2,
        );
        // `ghost` starts at character 4 on line 0.
        assert_eq!(d["range"]["start"]["character"], 4);
    }

    #[test]
    fn did_open_publishes_diagnostics_and_formatting_edits() {
        let bad = "world { entity e { state=(x=1.0) } }\nsystems { update { on=e; dt=1.0 x = nope(1.0) } }\n";
        let open = req(
            1,
            "textDocument/didOpen",
            json!({ "textDocument": { "uri": "file:///t.pwe", "text": bad } }),
        );
        let fmt = req(
            2,
            "textDocument/formatting",
            json!({ "textDocument": { "uri": "file:///t.pwe" } }),
        );
        let mut input = open + &fmt;
        input.push_str(&req(3, "shutdown", json!({})));
        input.push_str(&frame(&json!({ "jsonrpc": "2.0", "method": "exit" })));
        let mut out = Vec::new();
        run(std::io::Cursor::new(input), &mut out);
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("publishDiagnostics"), "{s}");
        assert!(s.contains("nope"), "{s}");
        assert!(s.contains("\"id\":2"), "{s}");
    }
}
