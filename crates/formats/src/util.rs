//! Shared helpers: paths, timestamps, file reading (incl. zstd), SQLite
//! access, and a conversation builder used by every importer.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, TimeZone, Utc};
use larrier_core::{
    lsf::{new_id, title_from_prompt, truncate_middle, MediaSource},
    AgentKind, FileDiff, Message, NativeLink, Origin, Part, PlanEntry, PlanStatus, Role, Session, SessionStatus, ToolCall,
    ToolKind, ToolStatus, Usage, WorkspaceRef,
};
use serde_json::Value;

use crate::NativeSessionInfo;

/// Tool outputs larger than this are trimmed (head + tail) on import.
pub const MAX_TOOL_OUTPUT: usize = 5_000;
/// String values inside tool inputs (file contents, patches) beyond this are cut.
const MAX_INPUT_STRING: usize = 800;
/// Tighter cap for outputs that are re-readable (file reads, searches).
const MAX_READ_OUTPUT: usize = 1_500;
/// Whole-file diff texts beyond this become a trimmed unified hunk.
const MAX_DIFF_TEXT: usize = 12_000;
const MAX_UNIFIED_LINES: usize = 300;
/// Base64 images larger than this are not embedded.
const MAX_IMAGE_B64: usize = 160_000;

/// Shrink an imported session for storage: native sessions stay resumable by
/// their agent, so LSF keeps what is needed to read and review them.
pub fn compact(s: &mut Session) {
    for m in s.messages.iter_mut() {
        for p in m.parts.iter_mut() {
            match p {
                Part::Tool(t) => {
                    if t.kind.is_mutation() && !t.diffs.is_empty() {
                        // The diff carries the content; keep only the addressing fields.
                        if let Value::Object(o) = &mut t.input {
                            for k in ["old_string", "new_string", "oldString", "newString", "old_str", "new_str", "content", "contents", "code_edit", "file_text", "edits", "patch", "input"] {
                                if o.contains_key(k) {
                                    o.insert(k.into(), Value::String("[see diff]".into()));
                                }
                            }
                        } else if t.input.is_string() {
                            t.input = Value::String("[see diff]".into());
                        }
                    }
                    compact_value(&mut t.input);
                    let cap = match t.kind {
                        ToolKind::Read | ToolKind::Search | ToolKind::Fetch | ToolKind::WebSearch | ToolKind::Mcp => MAX_READ_OUTPUT,
                        _ => MAX_TOOL_OUTPUT,
                    };
                    if let Some(o) = &t.output {
                        if o.len() > cap {
                            t.output = Some(truncate_middle(o, cap));
                        }
                    }
                    for d in t.diffs.iter_mut() {
                        compact_diff(d);
                    }
                }
                Part::Image { source: MediaSource::Base64 { data }, mime } if data.len() > MAX_IMAGE_B64 => {
                    let kb = data.len() * 3 / 4 / 1024;
                    *p = Part::File { path: "image".into(), mime: mime.clone(), text: Some(format!("[image omitted on import: {kb} KB]")) };
                }
                _ => {}
            }
        }
    }
}

fn compact_value(v: &mut Value) {
    match v {
        Value::String(s) if s.len() > MAX_INPUT_STRING => {
            let total = s.chars().count();
            let head: String = s.chars().take(MAX_INPUT_STRING).collect();
            *s = format!("{head}\n…[{} more chars]", total.saturating_sub(MAX_INPUT_STRING));
        }
        Value::Array(a) => a.iter_mut().for_each(compact_value),
        Value::Object(o) => o.values_mut().for_each(compact_value),
        _ => {}
    }
}

fn compact_diff(d: &mut FileDiff) {
    let big = d.old_text.as_ref().map(|t| t.len()).unwrap_or(0) + d.new_text.as_ref().map(|t| t.len()).unwrap_or(0) > MAX_DIFF_TEXT;
    if big {
        let old = d.old_text.take().unwrap_or_default();
        let new = d.new_text.take().unwrap_or_default();
        if d.unified.is_none() {
            d.unified = Some(hunk(&old, &new));
        }
    }
    if let Some(u) = &d.unified {
        let n = u.lines().count();
        if n > MAX_UNIFIED_LINES {
            let mut cut: String = u.lines().take(MAX_UNIFIED_LINES).collect::<Vec<_>>().join("\n");
            cut.push_str(&format!("\n@@ … {} more lines @@\n", n - MAX_UNIFIED_LINES));
            d.unified = Some(cut);
        }
    }
}

/// A single unified hunk around the changed region of two texts.
pub fn hunk(old: &str, new: &str) -> String {
    const CTX: usize = 3;
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    let mut pre = 0;
    while pre < a.len() && pre < b.len() && a[pre] == b[pre] {
        pre += 1;
    }
    let (mut ea, mut eb) = (a.len(), b.len());
    while ea > pre && eb > pre && a[ea - 1] == b[eb - 1] {
        ea -= 1;
        eb -= 1;
    }
    let start = pre.saturating_sub(CTX);
    let end_a = (ea + CTX).min(a.len());
    let end_b = (eb + CTX).min(b.len());
    let mut out = format!("@@ -{},{} +{},{} @@\n", start + 1, end_a - start, start + 1, end_b - start);
    for l in &a[start..pre] {
        out.push_str(&format!(" {l}\n"));
    }
    for l in &a[pre..ea] {
        out.push_str(&format!("-{l}\n"));
    }
    for l in &b[pre..eb] {
        out.push_str(&format!("+{l}\n"));
    }
    for l in &b[eb..end_b] {
        out.push_str(&format!(" {l}\n"));
    }
    out
}

/// Error used to signal "nothing worth importing" (empty session) so sync can
/// count it as skipped instead of failed.
#[derive(Debug)]
pub struct ImportSkip(pub String);

impl std::fmt::Display for ImportSkip {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "skipped: {}", self.0)
    }
}

impl std::error::Error for ImportSkip {}

pub fn skip(reason: impl Into<String>) -> anyhow::Error {
    anyhow::Error::new(ImportSkip(reason.into()))
}

// ---------------------------------------------------------------- paths

pub fn home() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

/// `$var` as a directory if set and non-empty.
pub fn env_dir(var: &str) -> Option<PathBuf> {
    std::env::var_os(var).filter(|v| !v.is_empty()).map(PathBuf::from)
}

pub fn file_info(path: &Path) -> (u64, i64) {
    match std::fs::metadata(path) {
        Ok(m) => (m.len(), m.modified().ok().map(system_time_ms).unwrap_or(0)),
        Err(_) => (0, 0),
    }
}

pub fn system_time_ms(t: std::time::SystemTime) -> i64 {
    t.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

pub fn info(agent: AgentKind, native_id: impl Into<String>, path: &Path) -> NativeSessionInfo {
    let (size, mtime_ms) = file_info(path);
    NativeSessionInfo {
        agent,
        native_id: native_id.into(),
        path: path.to_path_buf(),
        title: None,
        cwd: None,
        updated_at: ts_from_ms(mtime_ms),
        size,
        mtime_ms,
    }
}

// ---------------------------------------------------------------- time

pub fn parse_ts(s: &str) -> Option<DateTime<Utc>> {
    if let Ok(t) = DateTime::parse_from_rfc3339(s) {
        return Some(t.with_timezone(&Utc));
    }
    for fmt in ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%dT%H:%M:%S"] {
        if let Ok(n) = chrono::NaiveDateTime::parse_from_str(s, fmt) {
            return Some(Utc.from_utc_datetime(&n));
        }
    }
    None
}

pub fn ts_from_ms(ms: i64) -> Option<DateTime<Utc>> {
    if ms <= 0 {
        return None;
    }
    Utc.timestamp_millis_opt(ms).single()
}

pub fn ts_from_secs(secs: f64) -> Option<DateTime<Utc>> {
    if secs <= 0.0 {
        return None;
    }
    ts_from_ms((secs * 1000.0) as i64)
}

/// Accept ISO strings, epoch seconds or epoch millis.
pub fn ts_value(v: &Value) -> Option<DateTime<Utc>> {
    match v {
        Value::String(s) => parse_ts(s).or_else(|| s.parse::<f64>().ok().and_then(ts_number)),
        Value::Number(n) => n.as_f64().and_then(ts_number),
        _ => None,
    }
}

fn ts_number(n: f64) -> Option<DateTime<Utc>> {
    if n > 1e12 {
        ts_from_ms(n as i64)
    } else {
        ts_from_secs(n)
    }
}

// ---------------------------------------------------------------- files

fn is_zst(path: &Path) -> bool {
    path.extension().map(|e| e.eq_ignore_ascii_case("zst")).unwrap_or(false)
}

/// Read a whole text file, transparently decompressing `.zst`.
pub fn read_text(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    if is_zst(path) {
        let mut dec = ruzstd::decoding::StreamingDecoder::new(&bytes[..]).map_err(|e| anyhow!("zstd: {e}"))?;
        let mut out = Vec::new();
        dec.read_to_end(&mut out)?;
        return Ok(String::from_utf8_lossy(&out).into_owned());
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// Parse JSON lines, skipping blanks and malformed lines.
pub fn json_lines(text: &str) -> Vec<Value> {
    text.lines().filter(|l| !l.trim().is_empty()).filter_map(|l| serde_json::from_str(l).ok()).collect()
}

/// First `n` non-empty lines of a (possibly compressed) file.
pub fn head_lines(path: &Path, n: usize) -> Vec<String> {
    if is_zst(path) {
        return read_text(path).map(|t| t.lines().filter(|l| !l.trim().is_empty()).take(n).map(String::from).collect()).unwrap_or_default();
    }
    let Ok(f) = std::fs::File::open(path) else { return Vec::new() };
    BufReader::new(f).lines().map_while(|l| l.ok()).filter(|l| !l.trim().is_empty()).take(n).collect()
}

/// Last lines of a file (reads at most `max_bytes` from the end).
pub fn tail_lines(path: &Path, max_bytes: u64) -> Vec<String> {
    if is_zst(path) {
        return read_text(path).map(|t| t.lines().rev().take(64).map(String::from).collect::<Vec<_>>().into_iter().rev().collect()).unwrap_or_default();
    }
    let Ok(mut f) = std::fs::File::open(path) else { return Vec::new() };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let start = len.saturating_sub(max_bytes);
    if f.seek(SeekFrom::Start(start)).is_err() {
        return Vec::new();
    }
    let mut buf = Vec::new();
    if f.read_to_end(&mut buf).is_err() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&buf);
    let mut lines: Vec<String> = text.lines().map(String::from).collect();
    if start > 0 && !lines.is_empty() {
        lines.remove(0); // partial line
    }
    lines
}

// ---------------------------------------------------------------- sqlite

/// Open a SQLite database read-only. Tries a normal read-only open first (it
/// sees WAL content) and falls back to `immutable=1` when the file is locked.
pub fn open_sqlite(path: &Path) -> Result<rusqlite::Connection> {
    use rusqlite::OpenFlags;
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let p = path.to_string_lossy().replace('\\', "/");
    let prefix = if p.starts_with('/') { "file://" } else { "file:///" };
    let ro = format!("{prefix}{}?mode=ro", uri_escape(&p));
    if let Ok(c) = rusqlite::Connection::open_with_flags(&ro, flags) {
        let _ = c.busy_timeout(std::time::Duration::from_millis(1500));
        if c.query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get::<_, i64>(0)).is_ok() {
            return Ok(c);
        }
    }
    let imm = format!("{prefix}{}?mode=ro&immutable=1", uri_escape(&p));
    rusqlite::Connection::open_with_flags(&imm, flags).with_context(|| format!("opening {}", path.display()))
}

fn uri_escape(p: &str) -> String {
    p.replace('%', "%25").replace('#', "%23").replace('?', "%3F").replace(' ', "%20")
}

pub fn table_columns(con: &rusqlite::Connection, table: &str) -> Vec<String> {
    let Ok(mut st) = con.prepare(&format!("PRAGMA table_info({table})")) else { return Vec::new() };
    st.query_map([], |r| r.get::<_, String>(1)).map(|it| it.flatten().collect()).unwrap_or_default()
}

pub fn has_table(con: &rusqlite::Connection, table: &str) -> bool {
    con.query_row("SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?1", [table], |r| r.get::<_, i64>(0)).map(|n| n > 0).unwrap_or(false)
}

// ---------------------------------------------------------------- json

pub fn s<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(|x| x.as_str())
}

pub fn string(v: &Value, key: &str) -> Option<String> {
    s(v, key).map(|x| x.to_string())
}

/// Parse a JSON string value if it holds JSON, else wrap the raw string.
pub fn json_or_string(raw: &str) -> Value {
    let t = raw.trim();
    if (t.starts_with('{') || t.starts_with('[')) && t.len() > 1 {
        if let Ok(v) = serde_json::from_str(t) {
            return v;
        }
    }
    Value::String(raw.to_string())
}

/// Flatten text-ish content (`string`, `[{type:text,text}]`, `{text}`).
/// Image blocks inside message or tool-result content (Anthropic `image`,
/// OpenAI `input_image` / `image_url`).
pub fn image_parts(v: &Value) -> Vec<Part> {
    let Value::Array(items) = v else { return Vec::new() };
    items
        .iter()
        .filter_map(|i| match i.get("type").and_then(|t| t.as_str()) {
            Some("image") => {
                let src = &i["source"];
                let mime = src.get("media_type").and_then(|m| m.as_str()).map(String::from);
                image_part(mime, src.get("data").and_then(|d| d.as_str()), src.get("url").and_then(|u| u.as_str()))
            }
            Some("input_image") | Some("image_url") => {
                let url = i.get("image_url").and_then(|u| u.as_str().or_else(|| u.get("url").and_then(|x| x.as_str())));
                image_part(None, None, url)
            }
            _ => None,
        })
        .collect()
}

/// `[Image: source: C:\…\shot.png]` references (images pasted into a prompt
/// and saved to disk) → image parts for files that exist; the references
/// are removed from the text.
pub fn image_refs(text: &str) -> (String, Vec<Part>) {
    let mut out = String::with_capacity(text.len());
    let mut parts = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find("[Image: source: ") {
        let after = &rest[i + 16..];
        let Some(end) = after.find(']') else { break };
        let path = after[..end].trim();
        out.push_str(&rest[..i]);
        if std::path::Path::new(path).is_file() {
            let lower = path.to_lowercase();
            let mime = [("png", "image/png"), ("jpg", "image/jpeg"), ("jpeg", "image/jpeg"), ("gif", "image/gif"), ("webp", "image/webp")]
                .iter()
                .find(|(e, _)| lower.ends_with(&format!(".{e}")))
                .map(|(_, m)| m.to_string());
            parts.push(Part::Image { mime, source: MediaSource::Path { path: path.to_string() } });
        } else {
            out.push_str(&rest[i..i + 16 + end + 1]);
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    (out.trim().to_string(), parts)
}

pub fn content_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(items) => items
            .iter()
            .filter_map(|i| match i {
                Value::String(s) => Some(s.clone()),
                Value::Object(_) => match i.get("type").and_then(|t| t.as_str()) {
                    Some("text") | Some("input_text") | Some("output_text") | None => i.get("text").and_then(|t| t.as_str()).map(String::from),
                    Some("image") | Some("input_image") | Some("image_url") => Some("[image]".into()),
                    _ => None,
                },
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Object(_) => v.get("text").and_then(|t| t.as_str()).map(String::from).unwrap_or_default(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

pub fn trim_output(s: &str) -> String {
    if s.len() > MAX_TOOL_OUTPUT {
        truncate_middle(s, MAX_TOOL_OUTPUT)
    } else {
        s.to_string()
    }
}

/// Parse a todo list in the common shapes (`{todos:[{content,status}]}`,
/// `{plan:[{step,status}]}`, `[{text, completed}]`).
pub fn plan_from(v: &Value) -> Option<Vec<PlanEntry>> {
    let list = v
        .get("todos")
        .or_else(|| v.get("plan"))
        .or_else(|| v.get("items"))
        .unwrap_or(v);
    let arr = list.as_array()?;
    let entries: Vec<PlanEntry> = arr
        .iter()
        .filter_map(|t| {
            let content = t
                .get("content")
                .or_else(|| t.get("step"))
                .or_else(|| t.get("text"))
                .or_else(|| t.get("title"))
                .and_then(|c| c.as_str())?
                .to_string();
            let status = if t.get("completed").and_then(|c| c.as_bool()).unwrap_or(false) {
                PlanStatus::Completed
            } else {
                match t.get("status").and_then(|s| s.as_str()).unwrap_or("").to_ascii_lowercase().as_str() {
                    "completed" | "done" | "complete" | "todo_status_completed" => PlanStatus::Completed,
                    "in_progress" | "in-progress" | "active" | "inprogress" | "todo_status_in_progress" => PlanStatus::InProgress,
                    "cancelled" | "canceled" => PlanStatus::Cancelled,
                    _ => PlanStatus::Pending,
                }
            };
            Some(PlanEntry { content, status, priority: t.get("priority").and_then(|p| p.as_str()).map(String::from) })
        })
        .collect();
    if entries.is_empty() {
        None
    } else {
        Some(entries)
    }
}

// ---------------------------------------------------------------- builder

/// Incrementally builds LSF messages. Consecutive assistant content between
/// two user prompts is gathered into a single assistant message.
pub struct Conv {
    pub agent: AgentKind,
    pub messages: Vec<Message>,
    tools: HashMap<String, (usize, usize)>,
    pub model: Option<String>,
    pub first_ts: Option<DateTime<Utc>>,
    pub last_ts: Option<DateTime<Utc>>,
}

impl Conv {
    pub fn new(agent: AgentKind) -> Self {
        Conv { agent, messages: Vec::new(), tools: HashMap::new(), model: None, first_ts: None, last_ts: None }
    }

    fn stamp(&mut self, ts: Option<DateTime<Utc>>) -> DateTime<Utc> {
        if let Some(t) = ts {
            if self.first_ts.is_none() || Some(t) < self.first_ts {
                self.first_ts = Some(t);
            }
            if self.last_ts.is_none() || Some(t) > self.last_ts {
                self.last_ts = Some(t);
            }
            t
        } else {
            self.last_ts.or(self.first_ts).unwrap_or_else(Utc::now)
        }
    }

    pub fn user_parts(&mut self, parts: Vec<Part>, ts: Option<DateTime<Utc>>) {
        let parts: Vec<Part> = parts.into_iter().filter(|p| !matches!(p, Part::Text { text } if text.trim().is_empty())).collect();
        if parts.is_empty() {
            return;
        }
        let t = self.stamp(ts);
        let mut m = Message::new(Role::User, parts);
        m.created_at = t;
        m.parent_id = self.messages.last().map(|x| x.id.clone());
        self.messages.push(m);
    }

    pub fn user(&mut self, text: impl Into<String>, ts: Option<DateTime<Utc>>) {
        self.user_parts(vec![Part::Text { text: text.into() }], ts);
    }

    pub fn notice(&mut self, text: impl Into<String>, ts: Option<DateTime<Utc>>) {
        let t = self.stamp(ts);
        let mut m = Message::new(Role::System, vec![Part::Notice { text: text.into() }]);
        m.created_at = t;
        m.parent_id = self.messages.last().map(|x| x.id.clone());
        self.messages.push(m);
    }

    /// The assistant message currently being built (created if needed).
    pub fn assistant(&mut self, ts: Option<DateTime<Utc>>, model: Option<&str>) -> &mut Message {
        let t = self.stamp(ts);
        if let Some(m) = model.filter(|m| !m.is_empty() && !m.starts_with('<')) {
            self.model = Some(m.to_string());
        }
        let need_new = self.messages.last().map(|m| m.role != Role::Assistant).unwrap_or(true);
        if need_new {
            let mut m = Message::assistant(self.agent.clone(), self.model.clone());
            m.created_at = t;
            m.parent_id = self.messages.last().map(|x| x.id.clone());
            self.messages.push(m);
        }
        let m = self.messages.last_mut().unwrap();
        if m.model.is_none() {
            m.model = self.model.clone();
        }
        m
    }

    pub fn text(&mut self, text: &str, ts: Option<DateTime<Utc>>, model: Option<&str>) {
        if text.trim().is_empty() {
            return;
        }
        let m = self.assistant(ts, model);
        match m.parts.last_mut() {
            Some(Part::Text { text: prev }) => {
                if !prev.ends_with('\n') {
                    prev.push_str("\n\n");
                }
                prev.push_str(text);
            }
            _ => m.parts.push(Part::Text { text: text.to_string() }),
        }
    }

    pub fn reasoning(&mut self, text: &str, duration_ms: Option<u64>, ts: Option<DateTime<Utc>>, model: Option<&str>) {
        if text.trim().is_empty() {
            return;
        }
        let m = self.assistant(ts, model);
        m.parts.push(Part::Reasoning { text: text.to_string(), duration_ms });
    }

    pub fn tool(&mut self, mut tool: ToolCall, ts: Option<DateTime<Utc>>, model: Option<&str>) {
        tool.infer_diffs();
        if tool.kind == ToolKind::Todo {
            if let Some(entries) = plan_from(&tool.input) {
                let m = self.assistant(ts, model);
                m.parts.push(Part::Plan { entries });
            }
        }
        if tool.id.is_empty() {
            tool.id = new_id();
        }
        let id = tool.id.clone();
        let m = self.assistant(ts, model);
        m.parts.push(Part::Tool(tool));
        let mi = self.messages.len() - 1;
        let pi = self.messages[mi].parts.len() - 1;
        self.tools.insert(id, (mi, pi));
    }

    pub fn has_tool(&self, id: &str) -> bool {
        self.tools.contains_key(id)
    }

    /// Attach a result to a previously seen tool call. Returns false when the
    /// call is unknown.
    /// Attach images a tool returned (screenshots, image reads, MCP content).
    pub fn tool_images(&mut self, id: &str, images: Vec<Part>) {
        if images.is_empty() {
            return;
        }
        let Some(&(mi, pi)) = self.tools.get(id) else { return };
        if let Some(Part::Tool(t)) = self.messages.get_mut(mi).and_then(|m| m.parts.get_mut(pi)) {
            t.images.extend(images);
        }
    }

    pub fn tool_result(&mut self, id: &str, output: Option<String>, is_error: bool, diffs: Vec<FileDiff>) -> bool {
        let Some(&(mi, pi)) = self.tools.get(id) else { return false };
        if let Some(Part::Tool(t)) = self.messages.get_mut(mi).and_then(|m| m.parts.get_mut(pi)) {
            if let Some(o) = output {
                t.output = Some(trim_output(&o));
            }
            t.is_error = t.is_error || is_error;
            t.status = if t.is_error { ToolStatus::Failed } else { ToolStatus::Completed };
            if !diffs.is_empty() {
                t.diffs = diffs;
                for d in &t.diffs {
                    if !t.locations.contains(&d.path) {
                        t.locations.push(d.path.clone());
                    }
                }
            }
            return true;
        }
        false
    }

    pub fn error(&mut self, text: &str, ts: Option<DateTime<Utc>>) {
        let m = self.assistant(ts, None);
        m.parts.push(Part::Error { message: text.to_string() });
    }

    pub fn add_usage(&mut self, usage: &Usage) {
        if usage.is_empty() {
            return;
        }
        if let Some(m) = self.messages.iter_mut().rev().find(|m| m.role == Role::Assistant) {
            let mut u = m.usage.clone().unwrap_or_default();
            u.add(usage);
            m.usage = Some(u);
        }
    }

    pub fn has_user(&self) -> bool {
        self.messages.iter().any(|m| m.role == Role::User && !m.is_empty())
    }

    pub fn first_user_text(&self) -> Option<String> {
        self.messages.iter().find(|m| m.role == Role::User).map(|m| m.text()).filter(|t| !t.trim().is_empty())
    }
}

/// Everything known about a native session besides its messages.
#[derive(Default)]
pub struct Meta {
    pub native_id: String,
    pub path: Option<PathBuf>,
    pub title: Option<String>,
    pub cwd: Option<String>,
    pub branch: Option<String>,
    pub remote: Option<String>,
    pub created: Option<DateTime<Utc>>,
    pub updated: Option<DateTime<Utc>>,
    pub version: Option<String>,
    /// Session-level usage when the source only reports totals.
    pub usage: Option<Usage>,
    pub archived: bool,
    /// Identifier passed to the agent's resume flag when it differs from `native_id`.
    pub resume_path: Option<String>,
}

/// Turn a built conversation into an LSF session.
pub fn finish(conv: Conv, origin: AgentKind, meta: Meta) -> Result<Session> {
    finish_as(conv, origin.clone(), origin, meta)
}

/// Like [`finish`] but lets the driving agent differ from the origin (chat
/// exports are continued with the built-in Chat agent).
pub fn finish_as(mut conv: Conv, origin: AgentKind, agent: AgentKind, meta: Meta) -> Result<Session> {
    // Close any tool without a result: imported history is final.
    for m in conv.messages.iter_mut() {
        for t in m.tool_calls_mut() {
            if matches!(t.status, ToolStatus::Running | ToolStatus::Pending | ToolStatus::Approval) {
                t.status = ToolStatus::Completed;
            }
        }
    }
    conv.messages.retain(|m| !(m.role == Role::Assistant && m.parts.is_empty()));
    if !conv.has_user() {
        return Err(skip("no user messages"));
    }
    let workspace = meta.cwd.as_ref().filter(|c| !c.trim().is_empty()).map(|c| {
        let mut w = WorkspaceRef::from_path(c.clone());
        w.branch = meta.branch.clone().filter(|b| !b.is_empty() && b != "HEAD");
        w.remote = meta.remote.clone();
        w
    });
    let mut s = Session::new(agent, workspace);
    let first_user = conv.first_user_text();
    s.title = meta
        .title
        .clone()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .or_else(|| first_user.map(|t| title_from_prompt(&t)))
        .unwrap_or_else(|| "Untitled".into());
    s.extra.insert("import_title".into(), Value::String(s.title.clone()));
    let now = Utc::now();
    s.created_at = meta.created.or(conv.first_ts).or(conv.messages.first().map(|m| m.created_at)).unwrap_or(now);
    s.updated_at = meta.updated.or(conv.last_ts).or(conv.messages.last().map(|m| m.created_at)).unwrap_or(s.created_at);
    if s.updated_at < s.created_at {
        s.updated_at = s.created_at;
    }
    s.model = conv.model.clone();
    s.status = SessionStatus::Done;
    s.unread = false;
    s.archived = meta.archived;
    let path_str = meta.path.as_ref().map(|p| p.to_string_lossy().to_string());
    s.origin = Origin {
        agent: origin.clone(),
        native_id: Some(meta.native_id.clone()),
        path: path_str.clone(),
        imported_at: Some(now),
        agent_version: meta.version.clone(),
    };
    s.messages = conv.messages;
    compact(&mut s);
    if s.agent == origin {
        s.native.push(NativeLink {
            agent: origin,
            id: meta.native_id.clone(),
            path: meta.resume_path.clone().or(path_str),
            synced_messages: s.messages.len(),
            account_id: None,
        });
    }
    s.recompute_usage();
    if s.usage.is_empty() {
        if let Some(u) = meta.usage {
            s.usage = u;
        }
    }
    Ok(s)
}

// ---------------------------------------------------------------- export helpers

/// Text-only rendering of an assistant message for agents that cannot take
/// foreign tool calls: text, plus a compact summary of tool activity.
pub fn flatten_assistant(m: &Message) -> String {
    let mut out = String::new();
    for p in &m.parts {
        match p {
            Part::Text { text } => {
                if !out.is_empty() && !out.ends_with("\n\n") {
                    out.push_str(if out.ends_with('\n') { "\n" } else { "\n\n" });
                }
                out.push_str(text.trim_end());
            }
            Part::Tool(t) => {
                if !out.is_empty() && !out.ends_with('\n') {
                    out.push('\n');
                }
                let title = t.title.clone().unwrap_or_else(|| t.name.clone());
                let (a, r) = t.added_removed();
                out.push_str(&format!("> [{}] {}", t.name, title));
                if a + r > 0 {
                    out.push_str(&format!(" (+{a} -{r})"));
                }
                if t.is_error {
                    out.push_str(" — failed");
                }
                out.push('\n');
                if let Some(o) = &t.output {
                    let o = truncate_middle(o.trim(), 400);
                    if !o.is_empty() && !t.kind.is_mutation() {
                        for l in o.lines().take(8) {
                            out.push_str(&format!(">   {l}\n"));
                        }
                    }
                }
            }
            Part::Plan { entries } => {
                for e in entries {
                    let mark = if e.status == PlanStatus::Completed { "x" } else { " " };
                    out.push_str(&format!("- [{mark}] {}\n", e.content));
                }
            }
            Part::Error { message } => out.push_str(&format!("\n[error] {message}\n")),
            _ => {}
        }
    }
    out.trim().to_string()
}

pub fn flatten_user(m: &Message) -> String {
    let mut out = m.text();
    for p in &m.parts {
        match p {
            Part::File { path, .. } => out.push_str(&format!("\n@{path}")),
            Part::Image { source: MediaSource::Path { path }, .. } => out.push_str(&format!("\n@{path}")),
            _ => {}
        }
    }
    out.trim().to_string()
}

/// User/assistant turns with consecutive same-role messages merged, starting
/// with a user turn. System notices are dropped.
pub fn turns(session: &Session) -> Vec<(Role, String, DateTime<Utc>, Option<String>)> {
    let mut out: Vec<(Role, String, DateTime<Utc>, Option<String>)> = Vec::new();
    for m in &session.messages {
        let (role, text) = match m.role {
            Role::User => (Role::User, flatten_user(m)),
            Role::Assistant => (Role::Assistant, flatten_assistant(m)),
            Role::System => continue,
        };
        if text.is_empty() {
            continue;
        }
        if let Some(last) = out.last_mut() {
            if last.0 == role {
                last.1.push_str("\n\n");
                last.1.push_str(&text);
                continue;
            }
        }
        if out.is_empty() && role == Role::Assistant {
            out.push((Role::User, "(continued conversation)".into(), m.created_at, None));
        }
        out.push((role, text, m.created_at, m.model.clone()));
    }
    out
}

pub fn session_cwd(session: &Session) -> String {
    session
        .cwd()
        .or_else(|| std::env::current_dir().ok().map(|p| p.to_string_lossy().to_string()))
        .unwrap_or_else(|| home().to_string_lossy().to_string())
}

pub fn write_new_file(path: &Path, content: &str) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    larrier_core::store::atomic_write(path, content.as_bytes())
}

pub fn image_part(mime: Option<String>, data: Option<&str>, url: Option<&str>) -> Option<Part> {
    if let Some(d) = data.filter(|d| !d.is_empty()) {
        return Some(Part::Image { mime, source: MediaSource::Base64 { data: d.to_string() } });
    }
    url.filter(|u| !u.is_empty()).map(|u| {
        let source = if let Some(rest) = u.strip_prefix("data:") {
            let data = rest.split_once(',').map(|(_, d)| d.to_string()).unwrap_or_default();
            MediaSource::Base64 { data }
        } else if u.starts_with("http") {
            MediaSource::Url { url: u.to_string() }
        } else {
            MediaSource::Path { path: u.trim_start_matches("file://").to_string() }
        };
        Part::Image { mime, source }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hunk_and_compact() {
        let old = (0..100).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
        let new = old.replace("line 50", "changed 50");
        let h = hunk(&old, &new);
        assert!(h.starts_with("@@ -48,7 +48,7 @@"), "{h}");
        assert_eq!(h.lines().filter(|l| l.starts_with('-')).count(), 1);
        let mut s = Session::new(AgentKind::ClaudeCode, None);
        let mut t = ToolCall::new("t", "Write", serde_json::json!({"file_path": "a", "content": "x".repeat(50_000)}));
        t.diffs.push(FileDiff::from_texts("a", None, Some("y\n".repeat(20_000))));
        t.output = Some("z".repeat(50_000));
        s.messages.push(Message::new(Role::Assistant, vec![Part::Tool(t)]));
        compact(&mut s);
        let Part::Tool(t) = &s.messages[0].parts[0] else { panic!() };
        assert!(t.input["content"].as_str().unwrap().len() < 3_000);
        assert!(t.output.as_ref().unwrap().len() < 6_000);
        assert!(t.diffs[0].new_text.is_none() && t.diffs[0].unified.is_some());
        assert_eq!(t.diffs[0].added, 20_000);
    }
}
