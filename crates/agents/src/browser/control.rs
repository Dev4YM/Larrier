//! Asking Larrier about its browser tabs, from another process (the agents'
//! tool server): Larrier listens on a local port named, with a token, in a
//! file only this user can read.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::time::Duration;

use serde_json::{json, Value};

pub fn file() -> PathBuf {
    super::view_dir().join("larrier-control")
}

/// Send one request (`{"cmd": …}`) and wait for the answer.
pub fn request(mut cmd: Value) -> Result<Value, String> {
    let text = std::fs::read_to_string(file()).map_err(|_| "Larrier isn't running, so its browser isn't either.".to_string())?;
    let mut lines = text.lines();
    let port = lines.next().unwrap_or("").trim();
    let token = lines.next().unwrap_or("").trim();
    let addr = format!("127.0.0.1:{port}").parse().map_err(|_| "Larrier's browser address is unreadable.".to_string())?;
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_secs(2)).map_err(|_| "Larrier isn't running, so its browser isn't either.".to_string())?;
    let _ = s.set_read_timeout(Some(Duration::from_secs(30)));
    cmd["token"] = json!(token);
    s.write_all(format!("{cmd}\n").as_bytes()).map_err(|e| e.to_string())?;
    let mut line = String::new();
    BufReader::new(s).read_line(&mut line).map_err(|e| e.to_string())?;
    let v: Value = serde_json::from_str(line.trim()).map_err(|_| "Larrier gave no answer.".to_string())?;
    match v.get("error").and_then(|e| e.as_str()) {
        Some(e) => Err(e.to_string()),
        None => Ok(v),
    }
}
