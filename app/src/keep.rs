//! What Larrier keeps between visits and launches. Each chat: the message
//! being typed (text, objects, images and files), its follow-ups waiting
//! to be sent, its side panel and what its tabs hold (browser pages,
//! terminals, open files, the commit message, where it was scrolled to).
//! The window: the chat or page shown, its tabs and the chat beside it,
//! the way back and forward, the sidebar, a prompt being edited. The new
//! agent screen keeps its message too, with its picks (agent, model,
//! project); once left with something in it, it shows in the sidebar as a
//! draft.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{mpsc, Mutex, OnceLock};
use std::time::Duration;

use chrono::{DateTime, Utc};
use dioxus::prelude::*;
use larrier_core::{AgentKind, Part};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::state::{ChatTab, Draft, EditState, PanelTab, Queued, RightTab, Ui, View};
use crate::ui::mentions::{MentionChip, CLOSE, OPEN};

/// The new agent screen's key.
pub const HOME: &str = "new";
/// The key of what the window itself keeps (its sidebar's scroll).
pub const WINDOW: &str = "window";
/// The sidebar row of the new agent draft.
pub const DRAFT_ID: &str = "draft:new";

/// A composer's message as typed.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Input {
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub attachments: Vec<Part>,
    /// Objects in the text (only those still in it).
    #[serde(default)]
    pub mentions: Vec<MentionChip>,
}

impl Input {
    pub fn new(text: String, attachments: Vec<Part>, mentions: Vec<MentionChip>) -> Input {
        let mentions = mentions.into_iter().filter(|c| text.contains(&c.marked())).collect();
        Input { text, attachments, mentions }
    }

    pub fn is_empty(&self) -> bool {
        self.text.trim().is_empty() && self.attachments.is_empty()
    }

    /// One line for the sidebar: the text with its objects as their labels.
    fn title(&self) -> String {
        let flat: String = self.text.chars().filter(|c| !matches!(*c, OPEN | CLOSE)).collect();
        let line = flat.split_whitespace().collect::<Vec<_>>().join(" ");
        if !line.is_empty() {
            return line;
        }
        match self.attachments.len() {
            1 => match &self.attachments[0] {
                Part::Image { .. } => "Image".into(),
                Part::File { path, .. } => std::path::Path::new(path).file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_else(|| path.clone()),
                _ => "Attachment".into(),
            },
            n => format!("{n} attachments"),
        }
    }
}

/// A chat's side panel.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Panel {
    pub open: bool,
    pub tab: RightTab,
    pub tabs: Vec<PanelTab>,
}

impl Panel {
    pub fn of(ui: &Ui) -> Panel {
        Panel { open: ui.right_open, tab: ui.right_tab, tabs: ui.right_tabs.clone() }
    }

    /// A chat that has none of its own yet takes the panel's tabs as they
    /// are, closed: a new chat never opens the panel or changes its tab.
    pub fn fresh(ui: &Ui) -> Panel {
        Panel { open: false, tab: ui.right_tab, tabs: ui.right_tabs.clone() }
    }

    /// Open the panel on a tool, as a regular tab.
    pub fn show(&mut self, tab: RightTab) {
        self.open = true;
        crate::state::show_tab(&mut self.tabs, tab, true);
        self.tab = tab;
    }

    pub fn apply(self, ui: &mut Ui) {
        ui.right_open = self.open;
        ui.right_tab = self.tab;
        ui.right_tabs = self.tabs;
    }
}

/// Where the window was: what it showed and how its sidebar was.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Layout {
    #[serde(default)]
    pub view: Option<View>,
    /// Subagent tabs beside their chats.
    #[serde(default)]
    pub tabs: Vec<ChatTab>,
    /// The chat shown side by side.
    #[serde(default)]
    pub split: Option<String>,
    #[serde(default)]
    pub history: Vec<View>,
    #[serde(default)]
    pub hist_pos: usize,
    #[serde(default = "yes")]
    pub sidebar_open: bool,
    #[serde(default)]
    pub sidebar_query: String,
    /// Sidebar lists showing all their chats ("More").
    #[serde(default)]
    pub expanded: BTreeSet<String>,
    /// The agent Settings → Models was on.
    #[serde(default)]
    pub models_agent: Option<AgentKind>,
    /// A prompt being edited in place.
    #[serde(default)]
    pub editing: Option<EditState>,
}

fn yes() -> bool {
    true
}

/// The new agent draft's row in the sidebar.
#[derive(Clone, Debug, PartialEq)]
pub struct HomeRow {
    pub title: String,
    pub at: DateTime<Utc>,
}

/// Shown in the sidebar while the new agent screen holds a message it was
/// left with.
pub static HOME_ROW: GlobalSignal<Option<HomeRow>> = Signal::global(|| None);

#[derive(Default, Serialize, Deserialize)]
struct Kept {
    #[serde(default)]
    inputs: HashMap<String, Input>,
    #[serde(default)]
    panels: HashMap<String, Panel>,
    /// The new agent screen's picks, with its message.
    #[serde(default)]
    draft: Option<Draft>,
    /// When the new agent screen was left with its message.
    #[serde(default)]
    draft_at: Option<DateTime<Utc>>,
    #[serde(default)]
    layout: Option<Layout>,
    /// Follow-ups waiting to be sent, by chat.
    #[serde(default)]
    queues: HashMap<String, Vec<Queued>>,
    /// What each chat's tabs hold, by chat and name (see [`value`]).
    #[serde(default)]
    chats: HashMap<String, BTreeMap<String, serde_json::Value>>,
}

fn kept() -> &'static Mutex<Kept> {
    static KEPT: OnceLock<Mutex<Kept>> = OnceLock::new();
    KEPT.get_or_init(|| {
        let k = std::fs::read_to_string(file()).ok().and_then(|t| serde_json::from_str::<Kept>(&t).ok()).unwrap_or_default();
        Mutex::new(k)
    })
}

fn file() -> std::path::PathBuf {
    larrier_core::store::data_root().join("kept.json")
}

fn write_now() {
    let text = match kept().lock() {
        Ok(k) => serde_json::to_string(&*k).unwrap_or_default(),
        Err(_) => return,
    };
    let path = file();
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, text).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

/// Write soon: once nothing has changed for a moment.
fn save() {
    static TX: OnceLock<Mutex<mpsc::Sender<()>>> = OnceLock::new();
    let tx = TX.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<()>();
        std::thread::Builder::new()
            .name("keep".into())
            .spawn(move || {
                while rx.recv().is_ok() {
                    while rx.recv_timeout(Duration::from_millis(400)).is_ok() {}
                    write_now();
                }
            })
            .ok();
        Mutex::new(tx)
    });
    if let Ok(tx) = tx.lock() {
        let _ = tx.send(());
    }
}

/// Write what's waiting at once (Larrier is closing).
pub fn flush() {
    write_now();
}

/// Read what was kept; the new agent draft shows in the sidebar if there
/// was one. Returns the new agent screen's picks, kept with its message.
pub fn load() -> Option<Draft> {
    let k = kept().lock().ok()?;
    let input = k.inputs.get(HOME).filter(|i| !i.is_empty())?;
    *HOME_ROW.write() = Some(HomeRow { title: input.title(), at: k.draft_at.unwrap_or_else(Utc::now) });
    k.draft.clone()
}

/// The key of a composer: its chat, or the new agent screen.
pub fn key(session: Option<&str>) -> String {
    session.unwrap_or(HOME).to_string()
}

pub fn input(key: &str) -> Input {
    kept().lock().ok().and_then(|k| k.inputs.get(key).cloned()).unwrap_or_default()
}

pub fn set_input(key: &str, input: Input) {
    let empty = input.is_empty();
    {
        let Ok(mut k) = kept().lock() else { return };
        if empty {
            if k.inputs.remove(key).is_none() {
                return;
            }
        } else {
            if k.inputs.get(key) == Some(&input) {
                return;
            }
            k.inputs.insert(key.to_string(), input);
        }
    }
    // Sent or cleared: no draft left to show.
    if empty && key == HOME && HOME_ROW.peek().is_some() {
        *HOME_ROW.write() = None;
    }
    save();
}

/// The new agent screen was left: what it holds shows in the sidebar.
pub fn list_home() {
    let title = {
        let Ok(mut k) = kept().lock() else { return };
        let Some(input) = k.inputs.get(HOME).filter(|i| !i.is_empty()) else { return };
        let title = input.title();
        k.draft_at = Some(Utc::now());
        title
    };
    let row = HomeRow { title, at: Utc::now() };
    if HOME_ROW.peek().as_ref().map(|r| &r.title) != Some(&row.title) {
        *HOME_ROW.write() = Some(row);
    }
    save();
}

/// Throw the new agent draft away (from its sidebar row).
pub fn discard_home() {
    set_input(HOME, Input::default());
}

pub fn set_draft(draft: &Draft) {
    {
        let Ok(mut k) = kept().lock() else { return };
        if k.draft.as_ref() == Some(draft) {
            return;
        }
        k.draft = Some(draft.clone());
    }
    save();
}

pub fn panel(key: &str) -> Option<Panel> {
    kept().lock().ok().and_then(|k| k.panels.get(key).cloned())
}

pub fn set_panel(key: &str, panel: Panel) {
    {
        let Ok(mut k) = kept().lock() else { return };
        if k.panels.get(key) == Some(&panel) {
            return;
        }
        k.panels.insert(key.to_string(), panel);
    }
    save();
}

/// A chat was deleted: nothing of it is kept.
pub fn forget(key: &str) {
    {
        let Ok(mut k) = kept().lock() else { return };
        let had = k.inputs.remove(key).is_some() | k.panels.remove(key).is_some() | k.queues.remove(key).is_some() | k.chats.remove(key).is_some();
        if !had {
            return;
        }
    }
    save();
}

pub fn layout() -> Option<Layout> {
    kept().lock().ok().and_then(|k| k.layout.clone())
}

pub fn set_layout(layout: &Layout) {
    {
        let Ok(mut k) = kept().lock() else { return };
        if k.layout.as_ref() == Some(layout) {
            return;
        }
        k.layout = Some(layout.clone());
    }
    save();
}

/// Follow-ups that were waiting to be sent when Larrier closed.
pub fn queues() -> HashMap<String, Vec<Queued>> {
    kept().lock().ok().map(|k| k.queues.clone()).unwrap_or_default()
}

pub fn set_queues(queues: &HashMap<String, Vec<Queued>>) {
    {
        let Ok(mut k) = kept().lock() else { return };
        let now: HashMap<String, Vec<Queued>> = queues.iter().filter(|(_, v)| !v.is_empty()).map(|(id, v)| (id.clone(), v.clone())).collect();
        if k.queues == now {
            return;
        }
        k.queues = now;
    }
    save();
}

/// Something a chat's tab holds (`name`: "browser", "terminals", …).
pub fn value<T: DeserializeOwned>(key: &str, name: &str) -> Option<T> {
    let v = kept().lock().ok()?.chats.get(key)?.get(name)?.clone();
    serde_json::from_value(v).ok()
}

/// Keep `v` as `name` of chat `key`; an empty value (nothing typed, no
/// tabs) isn't kept.
pub fn set_value<T: Serialize>(key: &str, name: &str, v: &T) {
    let Ok(v) = serde_json::to_value(v) else { return };
    let empty = match &v {
        serde_json::Value::Null => true,
        serde_json::Value::String(s) => s.is_empty(),
        serde_json::Value::Array(a) => a.is_empty(),
        serde_json::Value::Object(o) => o.is_empty(),
        _ => false,
    };
    {
        let Ok(mut k) = kept().lock() else { return };
        if empty {
            let Some(m) = k.chats.get_mut(key) else { return };
            if m.remove(name).is_none() {
                return;
            }
            if m.is_empty() {
                k.chats.remove(key);
            }
        } else {
            let m = k.chats.entry(key.to_string()).or_default();
            if m.get(name) == Some(&v) {
                return;
            }
            m.insert(name.to_string(), v);
        }
    }
    save();
}

/// State of a component that's kept for chat `key` as `name`: it starts
/// as it was left (even in a past launch) and is kept as it changes.
pub fn use_kept<T>(key: &str, name: &'static str, init: impl FnOnce() -> T) -> Signal<T>
where
    T: Serialize + DeserializeOwned + Clone + PartialEq + 'static,
{
    let k = key.to_string();
    let sig = use_signal(|| value::<T>(&k, name).unwrap_or_else(init));
    let k = key.to_string();
    use_effect(move || set_value(&k, name, &*sig.read()));
    sig
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_draft_reads_as_one_line_with_its_objects() {
        let chip = crate::ui::mentions::MentionChip::page("Docs", "https://example.com", "body");
        let text = format!("look at\n  {} please", chip.marked());
        let i = Input::new(text, Vec::new(), vec![chip.clone(), crate::ui::mentions::MentionChip::page("Gone", "https://x.y", "")]);
        assert_eq!(i.mentions.len(), 1, "objects no longer in the text are dropped");
        assert!(i.title().starts_with("look at @"), "{}", i.title());
        let pic = Input { attachments: vec![Part::Image { mime: None, source: larrier_core::lsf::MediaSource::Path { path: "a.png".into() } }], ..Default::default() };
        assert!(!pic.is_empty());
        assert_eq!(pic.title(), "Image");
        assert!(Input { text: "  \n".into(), ..Default::default() }.is_empty());
        let back: Input = serde_json::from_str(&serde_json::to_string(&i).unwrap()).unwrap();
        assert_eq!(back, i);
    }

    #[test]
    fn where_the_window_was_reads_back() {
        let l = Layout {
            view: Some(View::Subagent { session: "s1".into(), tool: "t1".into() }),
            tabs: vec![ChatTab { view: View::Subagent { session: "s1".into(), tool: "t1".into() }, preview: true }],
            split: Some("s2".into()),
            history: vec![View::Home, View::Settings(crate::state::SettingsTab::Models), View::Agent("s1".into())],
            hist_pos: 2,
            sidebar_open: false,
            sidebar_query: "fix".into(),
            expanded: ["p1".to_string()].into_iter().collect(),
            models_agent: Some(AgentKind::Codex),
            editing: None,
        };
        let back: Layout = serde_json::from_str(&serde_json::to_string(&l).unwrap()).unwrap();
        assert_eq!(back, l);
        // What an older Larrier kept still reads (with the sidebar open).
        let old: Kept = serde_json::from_str(r#"{"inputs":{},"panels":{}}"#).unwrap();
        assert!(old.layout.is_none() && old.chats.is_empty());
        let none: Layout = serde_json::from_str("{}").unwrap();
        assert!(none.sidebar_open);
    }

    #[test]
    fn an_agents_browser_opens_as_a_regular_tab() {
        let tab = |t, preview| PanelTab { tab: t, preview };
        // The preview tab stays, and the browser is added beside it.
        let mut p = Panel { open: false, tab: RightTab::Files, tabs: vec![tab(RightTab::Changes, false), tab(RightTab::Files, true)] };
        p.show(RightTab::Browser);
        assert_eq!((p.open, p.tab), (true, RightTab::Browser));
        assert_eq!(p.tabs, [tab(RightTab::Changes, false), tab(RightTab::Files, true), tab(RightTab::Browser, false)]);
        // A browser already shown as a preview is kept.
        let mut p = Panel { open: true, tab: RightTab::Changes, tabs: vec![tab(RightTab::Changes, false), tab(RightTab::Browser, true)] };
        p.show(RightTab::Browser);
        assert_eq!(p.tabs, [tab(RightTab::Changes, false), tab(RightTab::Browser, false)]);
    }
}
