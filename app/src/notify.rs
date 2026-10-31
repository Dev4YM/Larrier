//! System integration: native notifications, the completion chime, the
//! system tray icon, window size / position restore and recovery of agents
//! that were still running when Larrier closed.
//!
//! Mirrors Cursor's Settings → General (Notifications, System Tray Icon,
//! Completion Sound, Window Restoration, Continue Interrupted Agents).

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use dioxus::desktop::{LogicalPosition, LogicalSize, WindowBuilder};
use dioxus::prelude::*;
use larrier_core::{Message, Part, Role, SessionStatus, Settings, Store, ToolStatus};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Focus + settings
// ---------------------------------------------------------------------------

static FOCUSED: AtomicBool = AtomicBool::new(true);
/// Set when a toast is clicked; the window tracker brings Larrier forward.
static ACTIVATE: AtomicBool = AtomicBool::new(false);

/// Record whether the Larrier window has keyboard focus (toasts only show
/// while it doesn't). [`track_window`] keeps this up to date.
pub fn set_focused(focused: bool) {
    FOCUSED.store(focused, Ordering::Relaxed);
}

pub fn is_focused() -> bool {
    FOCUSED.load(Ordering::Relaxed)
}

fn settings() -> Settings {
    crate::state::store().load_config().settings
}

// ---------------------------------------------------------------------------
// Agent events → toast + chime
// ---------------------------------------------------------------------------

/// An agent finished a turn (or needs input). `ok == false` for failures.
///
/// Plays the completion chime when `completion_sound` is on and shows a
/// native notification when `notify_on_finish` is on and the window is not
/// focused. Call from inside the Dioxus runtime (the chime goes through the
/// webview); the notification itself is shown on a background thread.
pub fn agent_event(title: &str, body: &str, ok: bool) {
    let s = settings();
    if s.completion_sound {
        play_sound(ok);
    }
    if s.notify_on_finish && !is_focused() {
        show_notification(title, body);
    }
}

/// Show a native notification now (regardless of focus / settings).
pub fn show_notification(title: &str, body: &str) {
    let (title, body) = (title.to_string(), body.to_string());
    let spawned = std::thread::Builder::new().name("larrier-toast".into()).spawn(move || {
        let mut n = notify_rust::Notification::new();
        n.summary(&title).body(&body).appname("Larrier");
        #[cfg(windows)]
        {
            n.app_id(windows_app_id());
            if let Some(icon) = icon_png_path() {
                n.image_path(&icon.to_string_lossy());
            }
        }
        match n.show() {
            #[cfg(windows)]
            Ok(handle) => {
                // Blocks this helper thread until the toast is clicked or
                // goes away; a click brings the Larrier window forward.
                let _ = handle.wait_for_response(|r: &notify_rust::NotificationResponse| {
                    if matches!(r, notify_rust::NotificationResponse::Default | notify_rust::NotificationResponse::Action(_)) {
                        ACTIVATE.store(true, Ordering::Relaxed);
                    }
                });
            }
            #[cfg(not(windows))]
            Ok(_) => {}
            Err(e) => tracing::warn!("notification failed: {e}"),
        }
    });
    if let Err(e) = spawned {
        tracing::warn!("could not start notification thread: {e}");
    }
}

/// AppUserModelID used for Windows toasts.
///
/// Unpackaged desktop apps can show toasts under their own name by
/// registering the AUMID in `HKCU\Software\Classes\AppUserModelId\<id>` with
/// a `DisplayName` (+ `IconUri`). No Start-menu shortcut is needed. If the
/// registration fails we fall back to PowerShell's well-known AUMID so
/// notifications still appear (attributed to "Windows PowerShell").
#[cfg(windows)]
pub fn windows_app_id() -> &'static str {
    pub const AUMID: &str = "Dev4YM.Larrier";
    const POWERSHELL: &str = "{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\\WindowsPowerShell\\v1.0\\powershell.exe";
    static ID: OnceLock<&'static str> = OnceLock::new();
    ID.get_or_init(|| {
        use std::os::windows::process::CommandExt;
        let key = format!("HKCU\\Software\\Classes\\AppUserModelId\\{AUMID}");
        let reg = |name: &str, value: &str| -> bool {
            std::process::Command::new("reg")
                .args(["add", &key, "/v", name, "/t", "REG_SZ", "/d", value, "/f"])
                .creation_flags(0x0800_0000)
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
        };
        if !reg("DisplayName", "Larrier") {
            tracing::warn!("could not register the Larrier notification id; using PowerShell's");
            return POWERSHELL;
        }
        if let Some(icon) = icon_png_path() {
            reg("IconUri", &icon.to_string_lossy());
        }
        reg("IconBackgroundColor", "FF26251E");
        AUMID
    })
}

/// The Larrier mark as a PNG on disk (notification / AUMID icon).
fn icon_png_path() -> Option<PathBuf> {
    static PATH: OnceLock<Option<PathBuf>> = OnceLock::new();
    PATH.get_or_init(|| {
        let p = crate::state::store().root().join("larrier-mark.png");
        if !p.exists() {
            let n = 128;
            std::fs::write(&p, png_rgba(n, n, &app_icon_rgba(n))).ok()?;
        }
        Some(p)
    })
    .clone()
}

// ---------------------------------------------------------------------------
// Completion sound
// ---------------------------------------------------------------------------

/// Two rising notes (E5 → A5), ~0.4 s, through WebAudio.
pub const CHIME_JS: &str = r#"(() => { try {
  const C = window.AudioContext || window.webkitAudioContext; if (!C) return;
  const ctx = window.__larrierAudio || (window.__larrierAudio = new C());
  if (ctx.state === 'suspended') ctx.resume();
  const now = ctx.currentTime + 0.02;
  [[659.25, 0], [880.0, 0.12]].forEach(([f, t]) => {
    const o = ctx.createOscillator(), g = ctx.createGain();
    o.type = 'sine'; o.frequency.value = f;
    g.gain.setValueAtTime(0.0001, now + t);
    g.gain.exponentialRampToValueAtTime(0.16, now + t + 0.015);
    g.gain.exponentialRampToValueAtTime(0.0001, now + t + 0.28);
    o.connect(g); g.connect(ctx.destination);
    o.start(now + t); o.stop(now + t + 0.32);
  });
} catch (e) {} })();"#;

/// Two falling notes for failures / attention.
pub const ALERT_JS: &str = r#"(() => { try {
  const C = window.AudioContext || window.webkitAudioContext; if (!C) return;
  const ctx = window.__larrierAudio || (window.__larrierAudio = new C());
  if (ctx.state === 'suspended') ctx.resume();
  const now = ctx.currentTime + 0.02;
  [[587.33, 0], [440.0, 0.14]].forEach(([f, t]) => {
    const o = ctx.createOscillator(), g = ctx.createGain();
    o.type = 'triangle'; o.frequency.value = f;
    g.gain.setValueAtTime(0.0001, now + t);
    g.gain.exponentialRampToValueAtTime(0.14, now + t + 0.015);
    g.gain.exponentialRampToValueAtTime(0.0001, now + t + 0.3);
    o.connect(g); g.connect(ctx.destination);
    o.start(now + t); o.stop(now + t + 0.34);
  });
} catch (e) {} })();"#;

/// JS that plays `custom` (an audio file on disk) or the built-in chime.
pub fn sound_js(custom: Option<&str>) -> String {
    if let Some(path) = custom.filter(|p| !p.trim().is_empty()) {
        let lower = path.to_ascii_lowercase();
        let mime = if lower.ends_with(".mp3") {
            "audio/mpeg"
        } else if lower.ends_with(".ogg") || lower.ends_with(".oga") {
            "audio/ogg"
        } else if lower.ends_with(".m4a") || lower.ends_with(".aac") {
            "audio/mp4"
        } else if lower.ends_with(".flac") {
            "audio/flac"
        } else {
            "audio/wav"
        };
        if let Ok(bytes) = std::fs::read(path) {
            if bytes.len() <= 8 * 1024 * 1024 {
                use base64::Engine;
                let data = base64::engine::general_purpose::STANDARD.encode(bytes);
                return format!("(() => {{ try {{ new Audio('data:{mime};base64,{data}').play().catch(() => {{}}); }} catch (e) {{}} }})();");
            }
        }
    }
    CHIME_JS.to_string()
}

/// Play the completion chime (or the failure tone when `ok` is false).
pub fn play_sound(ok: bool) {
    let _ = document::eval(if ok { CHIME_JS } else { ALERT_JS });
}

/// Play the completion chime.
pub fn play_completion_sound() {
    play_sound(true);
}

/// "Preview" button in settings: plays the built-in chime.
pub fn preview_sound() {
    let _ = document::eval(CHIME_JS);
}

/// Preview / play a custom sound file (falls back to the chime).
pub fn preview_sound_file(custom: Option<&str>) {
    let _ = document::eval(&sound_js(custom));
}

// ---------------------------------------------------------------------------
// Tray icon
// ---------------------------------------------------------------------------

pub const TRAY_SHOW: &str = "larrier.tray.show";
pub const TRAY_NEW_AGENT: &str = "larrier.tray.new-agent";
pub const TRAY_QUIT: &str = "larrier.tray.quit";

/// Create the tray icon (Show Larrier / New Agent / Quit). Must run inside
/// a component (it provides the tray handle as context); call once.
/// Left-clicking the icon shows the window (Dioxus default).
pub fn init_tray() {
    use dioxus::desktop::trayicon::menu::{Menu, MenuItem, PredefinedMenuItem};
    use dioxus::desktop::trayicon::{init_tray_icon, Icon};
    let menu = Menu::new();
    let added = menu.append_items(&[
        &MenuItem::with_id(TRAY_SHOW, "Show Larrier", true, None),
        &MenuItem::with_id(TRAY_NEW_AGENT, "New Agent", true, None),
        &PredefinedMenuItem::separator(),
        &MenuItem::with_id(TRAY_QUIT, "Quit Larrier", true, None),
    ]);
    if let Err(e) = added {
        tracing::warn!("tray menu: {e}");
        return;
    }
    let icon = Icon::from_rgba(app_icon_rgba(32), 32, 32).ok();
    // The builder panics if the OS refuses a tray icon; never take the app down for it.
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        init_tray_icon(menu, icon);
    }))
    .is_err()
    {
        tracing::warn!("system tray icon unavailable");
    }
}

/// Tray hook: creates the icon when `tray_icon` is on and handles its menu.
/// Call unconditionally from the root component.
pub fn use_tray(on_new_agent: impl FnMut() + 'static) {
    let app = crate::state::use_app();
    let mut on_new_agent = on_new_agent;
    use_hook(|| {
        if settings().tray_icon {
            init_tray();
        }
    });
    dioxus::desktop::use_tray_menu_event_handler(move |event| {
        let w = dioxus::desktop::window();
        match event.id.0.as_str() {
            TRAY_SHOW => bring_to_front(),
            TRAY_NEW_AGENT => {
                bring_to_front();
                on_new_agent();
            }
            TRAY_QUIT => {
                app.install_on_exit();
                w.close()
            }
            _ => {}
        }
    });
}

/// Show, un-minimize and focus the Larrier window.
pub fn bring_to_front() {
    let w = dioxus::desktop::window();
    w.set_visible(true);
    w.set_minimized(false);
    w.set_focus();
}

// ---------------------------------------------------------------------------
// Window state
// ---------------------------------------------------------------------------

/// Saved window bounds in logical pixels.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct WindowState {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    #[serde(default)]
    pub maximized: bool,
}

/// A monitor's work area in logical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MonitorRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

const MIN_W: f64 = 880.0;
const MIN_H: f64 = 560.0;

fn window_state_path() -> PathBuf {
    crate::state::store().root().join("window.json")
}

pub fn load_window_state() -> Option<WindowState> {
    let text = std::fs::read_to_string(window_state_path()).ok()?;
    let s: WindowState = serde_json::from_str(&text).ok()?;
    let finite = [s.x, s.y, s.width, s.height].iter().all(|v| v.is_finite());
    (finite && s.width >= 200.0 && s.height >= 150.0).then_some(s)
}

pub fn save_window_state(s: &WindowState) {
    if let Ok(t) = serde_json::to_string_pretty(s) {
        if let Err(e) = larrier_core::store::atomic_write(&window_state_path(), t.as_bytes()) {
            tracing::warn!("saving window state: {e}");
        }
    }
}

/// Apply the saved size / position / maximized state (when "Window
/// Restoration" is on). Off-screen positions are corrected by
/// [`track_window`] once monitors are known.
pub fn apply_to_builder(builder: WindowBuilder) -> WindowBuilder {
    if !settings().restore_window {
        return builder;
    }
    let Some(s) = load_window_state() else { return builder };
    builder
        .with_inner_size(LogicalSize::new(s.width.max(MIN_W), s.height.max(MIN_H)))
        .with_position(LogicalPosition::new(s.x, s.y))
        .with_maximized(s.maximized)
}

/// Keep a window on screen: if less than 120×80 px of it overlaps any
/// monitor, center it on the first monitor; never larger than that monitor.
pub fn clamp_to_monitors(s: WindowState, monitors: &[MonitorRect]) -> WindowState {
    let Some(primary) = monitors.first().copied() else { return s };
    let overlap = |m: &MonitorRect| {
        let w = (s.x + s.width).min(m.x + m.width) - s.x.max(m.x);
        let h = (s.y + s.height).min(m.y + m.height) - s.y.max(m.y);
        w >= 120.0 && h >= 80.0
    };
    // Monitor that contains most of the window's top edge (where you grab it).
    let home = monitors
        .iter()
        .find(|m| {
            let cx = s.x + s.width / 2.0;
            s.y >= m.y - 1.0 && s.y < m.y + m.height - 40.0 && cx >= m.x && cx < m.x + m.width
        })
        .copied();
    let mut out = s;
    match home.or_else(|| monitors.iter().find(|m| overlap(m)).copied()) {
        Some(m) => {
            out.width = out.width.min(m.width);
            out.height = out.height.min(m.height);
            // Keep the title bar reachable.
            out.y = out.y.clamp(m.y, m.y + m.height - 40.0);
            out.x = out.x.clamp(m.x - out.width + 120.0, m.x + m.width - 120.0);
        }
        None => {
            out.width = out.width.min(primary.width);
            out.height = out.height.min(primary.height);
            out.x = primary.x + (primary.width - out.width) / 2.0;
            out.y = primary.y + (primary.height - out.height) / 2.0;
        }
    }
    out
}

struct Tracker {
    state: Option<WindowState>,
    dirty: bool,
    last_save: Instant,
}

static TRACKER: Mutex<Option<Tracker>> = Mutex::new(None);

/// Read the window's current bounds (normal bounds are kept while
/// maximized / minimized so restoring un-maximizes to the right size).
fn record(force_save: bool) {
    let w = dioxus::desktop::window();
    let mut guard = TRACKER.lock().unwrap_or_else(|p| p.into_inner());
    let t = guard.get_or_insert_with(|| Tracker { state: load_window_state(), dirty: false, last_save: Instant::now() });
    if !w.is_minimized() {
        let maximized = w.is_maximized();
        let scale = w.scale_factor();
        let mut s = t.state.unwrap_or(WindowState { x: 80.0, y: 60.0, width: 1440.0, height: 900.0, maximized });
        if !maximized {
            if let Ok(pos) = w.outer_position() {
                let size = w.inner_size();
                s.x = pos.x as f64 / scale;
                s.y = pos.y as f64 / scale;
                s.width = size.width as f64 / scale;
                s.height = size.height as f64 / scale;
            }
        }
        s.maximized = maximized;
        if t.state != Some(s) {
            t.state = Some(s);
            t.dirty = true;
        }
    }
    if t.dirty && (force_save || t.last_save.elapsed() > Duration::from_millis(1500)) {
        if let Some(s) = t.state {
            save_window_state(&s);
        }
        t.dirty = false;
        t.last_save = Instant::now();
    }
}

/// Hook: tracks focus (for notifications), saves the window's size and
/// position (debounced, and on close), moves an off-screen window back onto
/// a monitor at startup, and brings the window forward when a toast is
/// clicked. Call unconditionally from the root component.
pub fn track_window() {
    let app = crate::state::use_app();
    use dioxus::desktop::tao::event::{Event, WindowEvent};
    let wid = use_hook(|| {
        let w = dioxus::desktop::window();
        let monitors: Vec<MonitorRect> = w
            .available_monitors()
            .map(|m| {
                let sf = m.scale_factor();
                MonitorRect {
                    x: m.position().x as f64 / sf,
                    y: m.position().y as f64 / sf,
                    width: m.size().width as f64 / sf,
                    height: m.size().height as f64 / sf,
                }
            })
            .collect();
        if !w.is_maximized() {
            if let Ok(pos) = w.outer_position() {
                let sf = w.scale_factor();
                let size = w.inner_size();
                let cur = WindowState {
                    x: pos.x as f64 / sf,
                    y: pos.y as f64 / sf,
                    width: size.width as f64 / sf,
                    height: size.height as f64 / sf,
                    maximized: false,
                };
                let fixed = clamp_to_monitors(cur, &monitors);
                if fixed != cur {
                    w.set_outer_position(LogicalPosition::new(fixed.x, fixed.y));
                    w.set_inner_size(LogicalSize::new(fixed.width, fixed.height));
                }
            }
        }
        // Toast clicks arrive on a background thread; poll for them here.
        spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(350)).await;
                if ACTIVATE.swap(false, Ordering::Relaxed) {
                    bring_to_front();
                }
            }
        });
        w.id()
    });
    dioxus::desktop::use_wry_event_handler(move |event, _| {
        if let Event::WindowEvent { window_id, event, .. } = event {
            if *window_id != wid {
                return;
            }
            match event {
                WindowEvent::Focused(f) => {
                    set_focused(*f);
                    if !*f {
                        record(true);
                    }
                }
                WindowEvent::Moved(_) | WindowEvent::Resized(_) => record(false),
                WindowEvent::CloseRequested | WindowEvent::Destroyed => {
                    record(true);
                    crate::state::flush_saves();
                    if matches!(event, WindowEvent::CloseRequested) {
                        app.install_on_exit();
                    }
                }
                _ => {}
            }
        }
    });
}

// ---------------------------------------------------------------------------
// Interrupted agents
// ---------------------------------------------------------------------------

/// Prompt to send when the user chooses to continue an interrupted agent.
pub const CONTINUE_PROMPT: &str = "Larrier was closed while you were working. Continue where you left off.";

const INTERRUPTED_NOTICE: &str = "Interrupted when Larrier closed";

/// Sessions left `Running` / `NeedsInput` by a previous run (crash or
/// close). Call at startup, before any new turn starts.
pub fn interrupted_sessions() -> Vec<String> {
    interrupted_in(crate::state::store())
}

pub fn interrupted_in(store: &Store) -> Vec<String> {
    store
        .list()
        .into_iter()
        .filter(|s| matches!(s.status, SessionStatus::Running | SessionStatus::NeedsInput))
        .map(|s| s.id)
        .collect()
}

/// Mark sessions as interrupted: status → Error, unfinished tools →
/// Cancelled, and a system notice. Keeps their position in the sidebar.
/// Returns how many sessions changed. Refresh the session list afterwards.
pub fn mark_interrupted(ids: &[String]) -> usize {
    mark_interrupted_in(crate::state::store(), ids)
}

pub fn mark_interrupted_in(store: &Store, ids: &[String]) -> usize {
    let mut changed = 0;
    for id in ids {
        let Ok(mut s) = store.load(id) else { continue };
        if !matches!(s.status, SessionStatus::Running | SessionStatus::NeedsInput) {
            continue;
        }
        let updated = s.updated_at;
        for m in s.messages.iter_mut() {
            for t in m.tool_calls_mut() {
                if matches!(t.status, ToolStatus::Running | ToolStatus::Pending | ToolStatus::Approval) {
                    t.status = ToolStatus::Cancelled;
                }
            }
        }
        s.push(Message::new(Role::System, vec![Part::Notice { text: INTERRUPTED_NOTICE.into() }]));
        s.status = SessionStatus::Error;
        s.unread = true;
        s.updated_at = updated;
        if store.save(&s).is_ok() {
            changed += 1;
        }
    }
    changed
}

// ---------------------------------------------------------------------------
// Icon helpers
// ---------------------------------------------------------------------------

/// The Larrier mark (dark rounded square with a light "L") as RGBA pixels.
pub fn app_icon_rgba(n: u32) -> Vec<u8> {
    crate::logo::mark_rgba(n)
}

/// Minimal PNG encoder (RGBA8, uncompressed deflate blocks).
pub fn png_rgba(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    fn crc32(data: &[u8]) -> u32 {
        let mut c = 0xFFFF_FFFFu32;
        for &b in data {
            c ^= b as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            }
        }
        !c
    }
    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut body = Vec::with_capacity(4 + data.len());
        body.extend_from_slice(kind);
        body.extend_from_slice(data);
        out.extend_from_slice(&body);
        out.extend_from_slice(&crc32(&body).to_be_bytes());
    }
    // Raw scanlines with filter byte 0.
    let stride = width as usize * 4;
    let mut raw = Vec::with_capacity((stride + 1) * height as usize);
    for row in rgba.chunks(stride).take(height as usize) {
        raw.push(0);
        raw.extend_from_slice(row);
    }
    // zlib stream of stored blocks.
    let mut z = vec![0x78, 0x01];
    let blocks: Vec<&[u8]> = raw.chunks(65_535).collect();
    for (i, b) in blocks.iter().enumerate() {
        z.push(if i + 1 == blocks.len() { 1 } else { 0 });
        let len = b.len() as u16;
        z.extend_from_slice(&len.to_le_bytes());
        z.extend_from_slice(&(!len).to_le_bytes());
        z.extend_from_slice(b);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &x in &raw {
        a = (a + x as u32) % 65_521;
        b = (b + a) % 65_521;
    }
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());

    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use larrier_core::{AgentKind, Session, ToolCall};

    const MON: MonitorRect = MonitorRect { x: 0.0, y: 0.0, width: 1920.0, height: 1040.0 };
    const RIGHT: MonitorRect = MonitorRect { x: 1920.0, y: 0.0, width: 2560.0, height: 1400.0 };

    #[test]
    fn on_screen_windows_are_kept() {
        let s = WindowState { x: 100.0, y: 80.0, width: 1200.0, height: 800.0, maximized: false };
        assert_eq!(clamp_to_monitors(s, &[MON]), s);
        let on_second = WindowState { x: 2200.0, y: 100.0, width: 1400.0, height: 900.0, maximized: true };
        assert_eq!(clamp_to_monitors(on_second, &[MON, RIGHT]), on_second);
    }

    #[test]
    fn off_screen_windows_are_centered_on_the_first_monitor() {
        // Saved on a monitor that is no longer connected.
        let s = WindowState { x: 4000.0, y: 200.0, width: 1200.0, height: 800.0, maximized: false };
        let c = clamp_to_monitors(s, &[MON]);
        assert_eq!((c.x, c.y), (360.0, 120.0));
        assert_eq!((c.width, c.height), (1200.0, 800.0));
        assert!(c.maximized == s.maximized);
    }

    #[test]
    fn oversized_and_title_bar_hidden_windows_are_fixed() {
        let big = WindowState { x: 0.0, y: 0.0, width: 5000.0, height: 3000.0, maximized: false };
        let c = clamp_to_monitors(big, &[MON]);
        assert!(c.width <= MON.width && c.height <= MON.height);
        // Title bar above the top edge → pulled back down.
        let high = WindowState { x: 200.0, y: -300.0, width: 1000.0, height: 700.0, maximized: false };
        let c = clamp_to_monitors(high, &[MON]);
        assert!(c.y >= 0.0);
        // No monitors known → unchanged.
        assert_eq!(clamp_to_monitors(high, &[]), high);
    }

    #[test]
    fn interrupted_sessions_are_found_and_marked() {
        let dir = std::env::temp_dir().join(format!("larrier-notify-{}", larrier_core::lsf::new_id()));
        let store = Store::open(&dir).unwrap();
        let mk = |status: SessionStatus, running_tool: bool| {
            let mut s = Session::new(AgentKind::ClaudeCode, None);
            s.push(Message::user_text("hi"));
            if running_tool {
                let mut m = Message::assistant(AgentKind::ClaudeCode, None);
                m.parts.push(Part::Tool(ToolCall::new("t1", "Bash", serde_json::json!({"command": "sleep 100"}))));
                s.push(m);
            }
            s.status = status;
            store.save(&s).unwrap();
            s
        };
        let running = mk(SessionStatus::Running, true);
        let needs = mk(SessionStatus::NeedsInput, false);
        let _done = mk(SessionStatus::Done, false);
        let mut ids = interrupted_in(&store);
        ids.sort();
        let mut want = vec![running.id.clone(), needs.id.clone()];
        want.sort();
        assert_eq!(ids, want);

        assert_eq!(mark_interrupted_in(&store, &ids), 2);
        let back = store.load(&running.id).unwrap();
        assert_eq!(back.status, SessionStatus::Error);
        assert_eq!(back.updated_at, running.updated_at);
        assert!(back.messages.iter().flat_map(|m| m.tool_calls()).all(|t| t.status == ToolStatus::Cancelled));
        assert!(matches!(back.messages.last().unwrap().parts[0], Part::Notice { ref text } if text == INTERRUPTED_NOTICE));
        assert!(interrupted_in(&store).is_empty());
        // Idempotent.
        assert_eq!(mark_interrupted_in(&store, &ids), 0);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn png_is_well_formed() {
        let png = png_rgba(32, 32, &app_icon_rgba(32));
        assert_eq!(&png[..8], &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
        // IEND chunk: length 0, "IEND", CRC AE426082.
        assert_eq!(&png[png.len() - 12..], &[0, 0, 0, 0, b'I', b'E', b'N', b'D', 0xAE, 0x42, 0x60, 0x82]);
        let icon = app_icon_rgba(64);
        assert_eq!(icon.len(), 64 * 64 * 4);
        // Corners transparent, center opaque.
        assert_eq!(icon[3], 0);
        assert_eq!(icon[((32 * 64 + 32) * 4 + 3) as usize], 255);
    }
}
