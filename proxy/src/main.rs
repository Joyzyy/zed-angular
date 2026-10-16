//! Sits between Zed and the Angular language server. While a completion
//! request is in flight and the user keeps typing word characters on the same
//! line, later completion requests reuse the in-flight answer instead of
//! queueing another full template compile (same as VS Code's suggest model).
//!
//! Usage: `angular-completion-proxy <command> [args...]`

use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Write};
use std::process::{self, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;

struct Pending {
    id: Value,
    line: u64,
    character: u64,
    waiters: Vec<(Value, u64)>,
    broken: bool,
}

type Inflight = Arc<Mutex<HashMap<String, Pending>>>;

enum Route {
    Forward,
    Drop,
    Reply(Value),
}

fn main() {
    let mut args = std::env::args().skip(1);
    let program = args
        .next()
        .expect("usage: angular-completion-proxy <command> [args...]");
    let mut child = Command::new(&program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| panic!("failed to start {program}: {e}"));

    let mut server_in = child.stdin.take().unwrap();
    let server_out = BufReader::new(child.stdout.take().unwrap());
    let inflight = Inflight::default();

    let state = inflight.clone();
    let server_thread = thread::spawn(move || {
        let mut reader = server_out;
        while let Ok(Some(mut msg)) = read_message(&mut reader) {
            let replies = on_server_message(&state, &mut msg);
            let mut out = io::stdout().lock();
            for reply in replies {
                write_message(&mut out, &reply).ok();
            }
            write_message(&mut out, &msg).ok();
        }
        let status = child.wait().ok().and_then(|s| s.code()).unwrap_or(1);
        process::exit(status);
    });

    let mut stdin = io::stdin().lock();
    while let Ok(Some(msg)) = read_message(&mut stdin) {
        let sent = match route_client_message(&inflight, &msg) {
            Route::Forward => write_message(&mut server_in, &msg),
            Route::Drop => Ok(()),
            Route::Reply(reply) => write_message(&mut io::stdout().lock(), &reply),
        };
        if sent.is_err() {
            break;
        }
    }
    drop(server_in);
    server_thread.join().ok();
}

fn route_client_message(inflight: &Inflight, msg: &Value) -> Route {
    let mut inflight = inflight.lock().unwrap();
    let uri = msg["params"]["textDocument"]["uri"].as_str().unwrap_or_default();

    match msg["method"].as_str() {
        Some("textDocument/completion") => {
            let line = msg["params"]["position"]["line"].as_u64().unwrap_or_default();
            let character = msg["params"]["position"]["character"].as_u64().unwrap_or_default();
            if let Some(pending) = inflight.get_mut(uri) {
                if !pending.broken && pending.line == line && character >= pending.character {
                    pending.waiters.push((msg["id"].clone(), character));
                    return Route::Drop;
                }
            }
            inflight.insert(
                uri.to_string(),
                Pending { id: msg["id"].clone(), line, character, waiters: Vec::new(), broken: false },
            );
        }
        // code actions would force or queue behind a compile while completions wait,
        // zed asks again on the next cursor move
        Some("textDocument/codeAction") if inflight.contains_key(uri) => {
            return Route::Reply(json!({ "jsonrpc": "2.0", "id": msg["id"], "result": [] }));
        }
        Some("textDocument/didChange") => {
            if let Some(pending) = inflight.get_mut(uri) {
                let changes = msg["params"]["contentChanges"].as_array();
                if !changes.is_some_and(|c| c.iter().all(|c| is_word_insert(c, pending))) {
                    pending.broken = true;
                }
            }
        }
        Some("$/cancelRequest") => {
            let id = &msg["params"]["id"];
            for pending in inflight.values_mut() {
                if &pending.id == id {
                    // the server can't abort a running compile, and waiters depend on its answer
                    if !pending.waiters.is_empty() {
                        return Route::Drop;
                    }
                    pending.broken = true;
                    break;
                }
                pending.waiters.retain(|(waiter, _)| waiter != id);
            }
        }
        _ => {}
    }
    Route::Forward
}

/// Answers for completion requests that were waiting on this response.
fn on_server_message(inflight: &Inflight, msg: &mut Value) -> Vec<Value> {
    if msg.get("method").is_some() || msg.get("id").is_none() {
        return Vec::new();
    }
    // the angular server sends cancellations without the required `message`
    if let Some(error) = msg.get_mut("error").and_then(Value::as_object_mut) {
        error.entry("message").or_insert_with(|| "Request cancelled".into());
    }

    let mut inflight = inflight.lock().unwrap();
    let Some(uri) = inflight
        .iter()
        .find(|(_, pending)| pending.id == msg["id"])
        .map(|(uri, _)| uri.clone())
    else {
        return Vec::new();
    };
    let pending = inflight.remove(&uri).unwrap();

    pending
        .waiters
        .into_iter()
        .map(|(id, character)| {
            let mut reply = msg.clone();
            reply["id"] = id;
            retarget(&mut reply["result"], pending.line, character);
            reply
        })
        .collect()
}

/// An insertion of word characters at or after the requested position.
fn is_word_insert(change: &Value, pending: &Pending) -> bool {
    let range = &change["range"];
    let text = change["text"].as_str().unwrap_or_default();
    range["start"]["line"].as_u64() == Some(pending.line)
        && range["end"]["line"].as_u64() == Some(pending.line)
        && range["start"]["character"].as_u64() >= Some(pending.character)
        && range["start"]["character"] == range["end"]["character"]
        && text.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Stretches each item's replace range to the waiter's cursor.
fn retarget(result: &mut Value, line: u64, character: u64) {
    let items = match result {
        Value::Array(items) => items,
        Value::Object(list) => match list.get_mut("items") {
            Some(Value::Array(items)) => items,
            _ => return,
        },
        _ => return,
    };
    for item in items {
        let Some(edit) = item.get_mut("textEdit") else { continue };
        for key in ["range", "insert", "replace"] {
            if let Some(range) = edit.get_mut(key) {
                if range["end"]["line"].as_u64() == Some(line) {
                    range["end"] = json!({ "line": line, "character": character });
                }
            }
        }
    }
}

fn read_message(reader: &mut impl BufRead) -> io::Result<Option<Value>> {
    let mut length = None;
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header)? == 0 {
            return Ok(None);
        }
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                length = value.trim().parse().ok();
            }
        }
    }
    let mut body = vec![0; length.ok_or_else(|| io::Error::other("missing Content-Length"))?];
    reader.read_exact(&mut body)?;
    Ok(Some(serde_json::from_slice(&body)?))
}

fn write_message(writer: &mut impl Write, msg: &Value) -> io::Result<()> {
    let body = serde_json::to_vec(msg)?;
    write!(writer, "Content-Length: {}\r\n\r\n", body.len())?;
    writer.write_all(&body)?;
    writer.flush()
}
