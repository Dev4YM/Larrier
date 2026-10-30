//! A Chrome DevTools Protocol client: one websocket to the browser, pages
//! reached through flattened sessions, on a thread of its own.

use std::collections::HashMap;
use std::net::TcpStream;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

/// Something the browser reported: `session` names the page it came from.
#[derive(Debug, Clone)]
pub struct Event {
    pub method: String,
    pub params: Value,
    pub session: Option<String>,
}

type Waiter = Sender<Result<Value, String>>;

pub struct Cdp {
    out: Sender<String>,
    pending: Arc<Mutex<HashMap<u64, Waiter>>>,
    next: AtomicU64,
    pub events: Mutex<Receiver<Event>>,
}

impl Cdp {
    /// Connect to the browser's websocket (`ws://127.0.0.1:<port>/devtools/browser/<id>`).
    pub fn connect(url: &str) -> Result<Cdp, String> {
        let (mut ws, _) = tungstenite::connect(url).map_err(|e| format!("couldn't reach the browser: {e}"))?;
        if let MaybeTlsStream::Plain(s) = ws.get_mut() {
            let _ = s.set_read_timeout(Some(Duration::from_millis(15)));
            let _ = s.set_nodelay(true);
        }
        let (out_tx, out_rx) = mpsc::channel::<String>();
        let (ev_tx, ev_rx) = mpsc::channel::<Event>();
        let pending: Arc<Mutex<HashMap<u64, Waiter>>> = Arc::default();
        let p = pending.clone();
        std::thread::Builder::new().name("cdp".into()).spawn(move || pump(ws, out_rx, ev_tx, p)).map_err(|e| e.to_string())?;
        Ok(Cdp { out: out_tx, pending, next: AtomicU64::new(1), events: Mutex::new(ev_rx) })
    }

    /// Send a command and wait for its result.
    pub fn call(&self, method: &str, params: Value, session: Option<&str>) -> Result<Value, String> {
        self.call_for(method, params, session, Duration::from_secs(30))
    }

    pub fn call_for(&self, method: &str, params: Value, session: Option<&str>, wait: Duration) -> Result<Value, String> {
        let id = self.next.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = mpsc::channel();
        self.pending.lock().unwrap().insert(id, tx);
        let mut msg = json!({"id": id, "method": method, "params": params});
        if let Some(s) = session {
            msg["sessionId"] = json!(s);
        }
        self.out.send(msg.to_string()).map_err(|_| "the browser connection closed".to_string())?;
        match rx.recv_timeout(wait) {
            Ok(r) => r,
            Err(_) => {
                self.pending.lock().unwrap().remove(&id);
                Err(format!("{method} timed out"))
            }
        }
    }

    /// Send a command without waiting (acks, input that needs no answer).
    pub fn send(&self, method: &str, params: Value, session: Option<&str>) {
        let id = self.next.fetch_add(1, Ordering::SeqCst);
        let mut msg = json!({"id": id, "method": method, "params": params});
        if let Some(s) = session {
            msg["sessionId"] = json!(s);
        }
        let _ = self.out.send(msg.to_string());
    }

    /// Events that arrived so far.
    pub fn drain(&self) -> Vec<Event> {
        let rx = self.events.lock().unwrap();
        let mut out = Vec::new();
        while let Ok(e) = rx.try_recv() {
            out.push(e);
        }
        out
    }

    /// Wait for the next event, up to `wait`.
    pub fn next_event(&self, wait: Duration) -> Option<Event> {
        self.events.lock().unwrap().recv_timeout(wait).ok()
    }
}

fn pump(mut ws: WebSocket<MaybeTlsStream<TcpStream>>, out: Receiver<String>, events: Sender<Event>, pending: Arc<Mutex<HashMap<u64, Waiter>>>) {
    let fail_all = |why: &str| {
        for (_, w) in pending.lock().unwrap().drain() {
            let _ = w.send(Err(why.to_string()));
        }
    };
    loop {
        loop {
            match out.try_recv() {
                Ok(m) => {
                    if ws.send(Message::text(m)).is_err() {
                        return fail_all("the browser connection closed");
                    }
                }
                Err(mpsc::TryRecvError::Empty) => break,
                // Every handle is gone: close quietly.
                Err(mpsc::TryRecvError::Disconnected) => {
                    let _ = ws.close(None);
                    return;
                }
            }
        }
        match ws.read() {
            Ok(Message::Text(t)) => {
                let Ok(v) = serde_json::from_str::<Value>(t.as_str()) else { continue };
                // A dialog (alert, confirm, beforeunload) blocks the page and
                // every command sent to it: accept it at once.
                if v["method"] == "Page.javascriptDialogOpening" {
                    let mut ok = json!({"id": 0, "method": "Page.handleJavaScriptDialog", "params": {"accept": true}});
                    if let Some(s) = v["sessionId"].as_str() {
                        ok["sessionId"] = json!(s);
                    }
                    let _ = ws.send(Message::text(ok.to_string()));
                }
                match v["id"].as_u64() {
                    Some(id) => {
                        if let Some(w) = pending.lock().unwrap().remove(&id) {
                            let r = match v.get("error") {
                                Some(e) => Err(e["message"].as_str().unwrap_or("CDP error").to_string()),
                                None => Ok(v["result"].clone()),
                            };
                            let _ = w.send(r);
                        }
                    }
                    None => {
                        let e = Event {
                            method: v["method"].as_str().unwrap_or("").to_string(),
                            params: v["params"].clone(),
                            session: v["sessionId"].as_str().map(String::from),
                        };
                        if events.send(e).is_err() {
                            // Nobody listens for events; responses still matter.
                        }
                    }
                }
            }
            Ok(Message::Close(_)) => return fail_all("the browser closed"),
            Ok(_) => {}
            Err(tungstenite::Error::Io(e)) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
            Err(_) => return fail_all("the browser connection closed"),
        }
    }
}
