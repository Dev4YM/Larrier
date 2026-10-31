//! The door the agents' browser tools knock on (`larrier_agents::browser::
//! control`): a local port, named with a token in a file only this user can
//! read, answering for Larrier's browser tabs.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::time::Duration;

use dioxus::core::spawn_forever;
use dioxus::prelude::*;
use serde_json::{json, Value};

use crate::browser_host as host;

type Request = (Value, std::sync::mpsc::Sender<Value>);

/// Start listening (once, from the UI: requests are answered there).
pub fn start() {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Request>();
    spawn_forever(async move {
        while let Some((req, reply)) = rx.recv().await {
            let _ = reply.send(answer(&req));
        }
    });
    std::thread::Builder::new()
        .name("browser-control".into())
        .spawn(move || {
            let Ok(listener) = TcpListener::bind("127.0.0.1:0") else { return };
            let Ok(addr) = listener.local_addr() else { return };
            let token = uuid::Uuid::new_v4().simple().to_string();
            let file = larrier_agents::browser::control::file();
            if let Some(dir) = file.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if std::fs::write(&file, format!("{}\n{token}\n", addr.port())).is_err() {
                return;
            }
            for stream in listener.incoming().flatten() {
                let (tx, token) = (tx.clone(), token.clone());
                std::thread::spawn(move || {
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
                    let mut line = String::new();
                    let Ok(reader) = stream.try_clone() else { return };
                    if BufReader::new(reader).read_line(&mut line).is_err() {
                        return;
                    }
                    let req: Value = serde_json::from_str(line.trim()).unwrap_or(Value::Null);
                    let out = if req["token"].as_str() != Some(token.as_str()) {
                        json!({"error": "not allowed"})
                    } else {
                        let (rtx, rrx) = std::sync::mpsc::channel();
                        let _ = tx.send((req, rtx));
                        rrx.recv_timeout(Duration::from_secs(30)).unwrap_or_else(|_| json!({"error": "Larrier didn't answer"}))
                    };
                    let mut s = stream;
                    let _ = s.write_all(format!("{out}\n").as_bytes());
                });
            }
        })
        .ok();
}

/// One request, on the UI thread.
fn answer(req: &Value) -> Value {
    let st = host::STATE.peek().clone();
    match req["cmd"].as_str().unwrap_or("") {
        "tabs" => json!({"tabs": st.tabs.iter().map(|t| json!({"id": t.id, "url": t.url, "title": t.title, "active": Some(t.id) == st.active})).collect::<Vec<_>>()}),
        "new_tab" => match host::new_tab(req["url"].as_str().unwrap_or("about:blank")) {
            Some(id) => json!({"id": id}),
            None => json!({"error": st.error.unwrap_or_else(|| "Larrier couldn't open a tab".into())}),
        },
        // A tab to work in, if there's none yet.
        "ensure" => {
            if st.tabs.is_empty() {
                host::new_tab("about:blank");
            }
            json!({"ok": true})
        }
        "close_tab" => {
            host::close_tab(req["id"].as_u64().unwrap_or(0));
            json!({"ok": true})
        }
        "activate" => {
            host::activate(req["id"].as_u64().unwrap_or(0));
            json!({"ok": true})
        }
        other => json!({"error": format!("unknown request {other}")}),
    }
}
