//! Automations page: scheduled and on-commit agent runs, Cursor style
//! (header + New Automation, hero, Mine / Runs table, template gallery).

use chrono::{Datelike, Local, Timelike};
use dioxus::prelude::*;
use larrier_core::automation::{templates, Automation, Template, Trigger, CATEGORIES};
use larrier_core::AgentKind;

use super::{AgentGlyph, Toggle};
use crate::icons::{Icon, Lucide, I};
use crate::state::{rel_time, use_app, App};

const CSS: &str = r#"
.au-page { padding: 56px 32px 80px; display: flex; justify-content: center; }
.au-inner { width: 100%; max-width: 1080px; display: flex; flex-direction: column; }
.au-head { display: flex; align-items: flex-start; gap: 16px; margin-bottom: 26px; }
.au-head h1 { font-size: 20px; font-weight: 500; margin: 0 0 6px; line-height: 1.3; letter-spacing: -0.2px; }
.au-head .sub { font-size: 14px; color: var(--fg-2); }
.au-black { height: 34px; padding: 0 16px; border-radius: 17px; background: var(--fg); color: var(--pane, #fff); font-size: 14px; font-weight: 500; display: inline-flex; align-items: center; gap: 6px; white-space: nowrap; }
.au-black:hover { opacity: .88; }
.au-black.wide { width: 100%; justify-content: center; height: 36px; }
.au-label { font-size: 13px; color: var(--fg-2); margin: 0 0 10px 2px; }
.au-hero { display: grid; grid-template-columns: 1fr 1fr; border: 1px solid var(--border); border-radius: 14px; overflow: hidden; background: var(--card-2); margin-bottom: 44px; min-height: 300px; }
.au-hero-art { background: radial-gradient(120% 90% at 0% 0%, #3a4a5a 0%, #15171a 55%, #0d0d0c 100%); padding: 26px 0 0 26px; position: relative; overflow: hidden; }
.au-tl { background: #121211; border: 1px solid rgba(255,255,255,.08); border-right: none; border-bottom: none; border-radius: 12px 0 0 0; height: 100%; padding: 16px 18px; color: #e9e7e0; display: flex; flex-direction: column; gap: 10px; }
.au-tl .h { font-size: 11px; color: rgba(233,231,224,.5); text-transform: uppercase; letter-spacing: .04em; }
.au-tl-row { display: flex; align-items: center; gap: 10px; padding: 9px 10px; border-radius: 9px; border: 1px solid rgba(255,255,255,.07); background: rgba(255,255,255,.03); font-size: 12.5px; }
.au-tl-row .w { margin-left: auto; color: rgba(233,231,224,.5); font-size: 11.5px; white-space: nowrap; }
.au-tl-row .nm { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; min-width: 0; }
.au-tag { font-size: 10.5px; padding: 1px 7px; border-radius: 6px; background: rgba(255,255,255,.08); color: rgba(233,231,224,.7); white-space: nowrap; }
.au-hero-body { padding: 34px 30px 28px; display: flex; flex-direction: column; gap: 6px; background: linear-gradient(180deg, color-mix(in srgb, var(--green) 7%, transparent), transparent 40%); }
.au-hero-body h2 { font-size: 22px; font-weight: 500; text-align: center; margin: 4px 0 14px; letter-spacing: -0.3px; }
.au-feat { display: flex; align-items: center; gap: 14px; padding: 10px 12px; border-radius: 10px; cursor: pointer; text-align: left; width: 100%; }
.au-feat:hover { background: var(--hover); }
.au-feat .ic { width: 40px; height: 40px; border-radius: 50%; display: flex; align-items: center; justify-content: center; flex-shrink: 0; }
.au-feat .t { font-size: 14px; }
.au-feat .d { font-size: 13px; color: var(--fg-2); }
.au-bar { display: flex; align-items: center; gap: 6px; margin-bottom: 10px; }
.au-tab { height: 32px; padding: 0 13px; border-radius: 16px; font-size: 14px; color: var(--fg-2); }
.au-tab:hover { color: var(--fg); }
.au-tab.on { background: var(--chip); color: var(--fg); }
.au-search { width: 210px; height: 32px; border-radius: 8px; border: 1px solid var(--border); background: var(--card-2); padding: 0 10px; font-size: 13px; outline: none; color: var(--fg); }
.au-table { border: 1px solid var(--border); border-radius: 12px; background: var(--card-2); margin-bottom: 44px; overflow: hidden; }
.au-tr { display: grid; grid-template-columns: minmax(0, 2.2fr) minmax(0, 1.3fr) minmax(0, 1.5fr) minmax(0, 1.6fr) 148px; align-items: center; gap: 12px; padding: 0 18px; min-height: 54px; font-size: 14px; }
.au-tr + .au-tr { border-top: 1px solid var(--border); }
.au-tr.head { min-height: 44px; font-size: 13px; color: var(--fg-2); }
.au-tr .nm { font-size: 14px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.au-tr .pr { font-size: 12px; color: var(--fg-3); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.au-tr .cell { display: flex; align-items: center; gap: 7px; min-width: 0; font-size: 13px; }
.au-tr .cell .s { color: var(--fg-3); font-size: 12px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.au-tr .acts { display: flex; align-items: center; justify-content: flex-end; gap: 2px; }
.au-tr.run { cursor: pointer; }
.au-tr.run:hover { background: var(--hover); }
.au-dot { width: 7px; height: 7px; border-radius: 50%; background: var(--fg-4); flex-shrink: 0; }
.au-dot.on { background: var(--green); }
.au-dot.busy { background: var(--blue); animation: au-pulse 1.2s ease-in-out infinite; }
@keyframes au-pulse { 50% { opacity: .35; } }
.au-empty { padding: 34px 20px; text-align: center; color: var(--fg-3); font-size: 13px; line-height: 20px; }
.au-cats { display: flex; gap: 4px; flex-wrap: wrap; margin-bottom: 14px; }
.au-grid { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); gap: 14px; }
@media (max-width: 1100px) { .au-grid { grid-template-columns: repeat(2, minmax(0, 1fr)); } .au-hero { grid-template-columns: 1fr; } .au-hero-art { display: none; } }
.au-card { border: 1px solid var(--border); border-radius: 12px; background: var(--card-2); padding: 18px 18px 16px; display: flex; flex-direction: column; gap: 6px; min-height: 186px; position: relative; }
.au-card:hover { border-color: var(--border-2); }
.au-card .ic { height: 32px; display: flex; align-items: center; margin-bottom: 14px; }
.au-card .t { font-size: 15px; }
.au-card .d { font-size: 13.5px; color: var(--fg-2); line-height: 20px; overflow: hidden; display: -webkit-box; -webkit-line-clamp: 2; -webkit-box-orient: vertical; }
.au-card .flow { margin-top: auto; padding-top: 12px; display: flex; align-items: center; gap: 7px; font-size: 12.5px; color: var(--fg-2); }
.au-use { position: absolute; top: 14px; right: 14px; height: 30px; padding: 0 13px; border-radius: 15px; border: 1px solid var(--border); background: var(--card-2); font-size: 13px; }
.au-use:hover { background: var(--hover); }
.au-modal { width: min(640px, 94vw); }
.au-form { display: grid; grid-template-columns: 110px 1fr; gap: 12px 14px; align-items: center; padding: 6px 18px 16px; font-size: 13px; }
.au-form .k { color: var(--fg-2); font-size: 12.5px; }
.au-form .top { align-self: start; padding-top: 6px; }
.au-form textarea { min-height: 140px; resize: vertical; border: 1px solid var(--border); border-radius: 8px; padding: 8px 10px; background: var(--input); outline: none; font: inherit; font-size: 13px; line-height: 19px; color: var(--fg); }
.au-days { display: flex; gap: 4px; }
.au-day { width: 30px; height: 26px; border-radius: 6px; border: 1px solid var(--border); font-size: 12px; color: var(--fg-2); }
.au-day.on { background: var(--fg); color: var(--pane, #fff); border-color: var(--fg); }
.au-hint { grid-column: 2; font-size: 12px; color: var(--fg-3); margin-top: -6px; }
.au-err { color: var(--red); font-size: 12px; margin-right: auto; }
"#;

#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Mine,
    Runs,
}

#[component]
pub fn AutomationsView() -> Element {
    let app = use_app();
    let mut tab = use_signal(|| Tab::Mine);
    let mut query = use_signal(String::new);
    let mut category = use_signal(|| "Popular");
    let mut editing: Signal<Option<(Automation, bool)>> = use_signal(|| None);

    let autos = app.config.read().automations.clone();
    let all_templates = templates();
    let default_agent = app.config.read().settings.default_agent.clone();
    let first_ws = app.config.read().workspaces.first().map(|w| w.path.clone());
    let q = query.read().to_lowercase();
    let shown: Vec<Automation> = autos
        .iter()
        .filter(|a| q.is_empty() || a.name.to_lowercase().contains(&q) || a.prompt.to_lowercase().contains(&q))
        .cloned()
        .collect();

    let new_from = move |t: Option<Template>| {
        let mut a = match &t {
            Some(t) => t.instantiate(default_agent.clone()),
            None => Automation::new("", default_agent.clone(), "", Trigger::Daily { hour: 9, minute: 0, weekdays: vec![0, 1, 2, 3, 4] }),
        };
        a.workspace = first_ws.clone();
        editing.set(Some((a, true)));
    };
    let hero: Vec<Template> = all_templates.iter().filter(|t| t.popular).take(3).cloned().collect();
    let cards: Vec<Template> = all_templates.iter().filter(|t| t.in_category(category())).cloned().collect();

    rsx! {
        style { {CSS} }
        div { class: "au-page",
            div { class: "au-inner",
                div { class: "au-head",
                    div { class: "grow",
                        h1 { "Automations" }
                        div { class: "sub", "Run agents on a schedule or when your repo changes — locally, with any agent." }
                    }
                    button { class: "au-black", onclick: { let mut n = new_from.clone(); move |_| n(None) }, "New Automation" }
                }

                div { class: "au-label", "From Larrier" }
                div { class: "au-hero",
                    div { class: "au-hero-art", Upcoming { autos: autos.clone() } }
                    div { class: "au-hero-body",
                        h2 { "Put your agents on autopilot" }
                        for t in hero {
                            {
                                let t2 = t.clone();
                                let mut n = new_from.clone();
                                rsx! {
                                    button { key: "{t.id}", class: "au-feat", onclick: move |_| n(Some(t2.clone())),
                                        span { class: "ic", style: "background: color-mix(in srgb, {t.color} 14%, transparent); color: {t.color};",
                                            Lucide { name: t.icon.to_string(), size: 19, stroke: 1.9 }
                                        }
                                        span { class: "col",
                                            span { class: "t", "{t.title}" }
                                            span { class: "d", "{t.description}" }
                                        }
                                    }
                                }
                            }
                        }
                        div { style: "margin-top: 14px;",
                            button { class: "au-black wide", onclick: { let mut n = new_from.clone(); let first = all_templates.first().cloned(); move |_| n(first.clone()) },
                                "Get Started" Icon { i: I::ArrowRight, size: 15, stroke: 2.0 }
                            }
                        }
                    }
                }

                div { class: "au-bar",
                    button { class: if tab() == Tab::Mine { "au-tab on" } else { "au-tab" }, onclick: move |_| tab.set(Tab::Mine), "Mine" }
                    button { class: if tab() == Tab::Runs { "au-tab on" } else { "au-tab" }, onclick: move |_| tab.set(Tab::Runs), "Runs" }
                    div { class: "grow" }
                    input { class: "au-search", placeholder: "Search…", value: "{query}", oninput: move |e| query.set(e.value()) }
                }
                if tab() == Tab::Mine {
                    div { class: "au-table",
                        div { class: "au-tr head",
                            span { "Name" }
                            span { "Agent" }
                            span { "Trigger" }
                            span { "Status" }
                            span {}
                        }
                        if shown.is_empty() {
                            div { class: "au-empty",
                                if autos.is_empty() {
                                    "No automations yet. Create one, or start from a template below — runs happen on this PC with the agents you already use."
                                } else {
                                    "No automations match your search."
                                }
                            }
                        }
                        for a in shown {
                            AutoRow { key: "{a.id}", a: a.clone(), onedit: move |a: Automation| editing.set(Some((a, false))) }
                        }
                    }
                } else {
                    RunsTable { query: q.clone() }
                }

                div { class: "au-cats",
                    for c in CATEGORIES.iter().copied() {
                        button { key: "{c}", class: if category() == c { "au-tab on" } else { "au-tab" }, onclick: move |_| category.set(c), "{c}" }
                    }
                }
                div { class: "au-grid",
                    for t in cards {
                        TemplateCard { key: "{t.id}", id: t.id, onuse: { let mut n = new_from.clone(); move |t: Template| n(Some(t)) } }
                    }
                }
            }
        }
        if let Some((a, is_new)) = editing() {
            Editor { key: "{a.id}", initial: a, is_new, onclose: move |_| editing.set(None) }
        }
    }
}

/// Next scheduled runs, shown in the hero's dark panel.
#[component]
fn Upcoming(autos: Vec<Automation>) -> Element {
    let now = Local::now();
    let mut next: Vec<(String, String, String)> = autos
        .iter()
        .filter(|a| a.enabled)
        .map(|a| {
            let when = match a.next_due(now) {
                Some(t) => when_label(t),
                None => a.trigger.describe(),
            };
            (a.name.clone(), when, a.agent.display_name())
        })
        .collect();
    next.truncate(4);
    let samples = [
        ("Find critical bugs", "Weekdays 09:00", "Claude Code"),
        ("Summarize changes daily", "Today 17:30", "Codex"),
        ("Check build health", "On new commits", "Cursor Agent"),
    ];
    rsx! {
        div { class: "au-tl",
            div { class: "h", if next.is_empty() { "What an automation looks like" } else { "Up next" } }
            if next.is_empty() {
                for (n, w, ag) in samples {
                    div { key: "{n}", class: "au-tl-row",
                        span { style: "color: #4cb88a; display: flex;", Icon { i: I::CircleCheck, size: 14 } }
                        span { class: "nm", "{n}" }
                        span { class: "au-tag", "{ag}" }
                        span { class: "w", "{w}" }
                    }
                }
            } else {
                for (i, (n, w, ag)) in next.into_iter().enumerate() {
                    div { key: "{i}", class: "au-tl-row",
                        span { style: "color: #7ba7d8; display: flex;", Icon { i: I::History, size: 14 } }
                        span { class: "nm", "{n}" }
                        span { class: "au-tag", "{ag}" }
                        span { class: "w", "{w}" }
                    }
                }
            }
        }
    }
}

/// "Today 09:00", "Tomorrow 09:00", "Mon 09:00", "Mar 4 09:00".
fn when_label(t: chrono::DateTime<Local>) -> String {
    let today = Local::now().date_naive();
    let d = t.date_naive();
    let hm = format!("{:02}:{:02}", t.hour(), t.minute());
    let days = (d - today).num_days();
    if days <= 0 {
        format!("Today {hm}")
    } else if days == 1 {
        format!("Tomorrow {hm}")
    } else if days < 7 {
        format!("{} {hm}", ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"][t.weekday().num_days_from_monday() as usize])
    } else {
        format!("{} {hm}", t.format("%b %-d"))
    }
}

#[component]
fn AutoRow(a: Automation, onedit: EventHandler<Automation>) -> Element {
    let app = use_app();
    let busy = a.last_session_id.as_deref().map(|s| app.is_running(s)).unwrap_or(false);
    let next = a.next_due(Local::now()).filter(|_| a.enabled).map(when_label);
    let mut status = if busy {
        "Running".to_string()
    } else if a.enabled {
        "Active".to_string()
    } else {
        "Paused".to_string()
    };
    let detail = match (&a.last_run, &a.last_status, &next) {
        (_, _, Some(n)) if !busy => format!("Next {n}"),
        (Some(t), Some(s), _) if !busy => format!("{} · {}", rel_time(*t), s),
        (Some(t), _, _) => format!("Last {}", rel_time(*t)),
        _ => String::new(),
    };
    if a.runs > 0 && !busy && !a.enabled {
        status = format!("Paused · {} run{}", a.runs, if a.runs == 1 { "" } else { "s" });
    }
    let dot = if busy { "au-dot busy" } else if a.enabled { "au-dot on" } else { "au-dot" };
    let ws = a.workspace.as_ref().map(|w| std::path::Path::new(w).file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or(w.clone()));
    let (id1, id2, id3) = (a.id.clone(), a.id.clone(), a.id.clone());
    let last = a.last_session_id.clone();
    let a2 = a.clone();
    let enabled = a.enabled;
    rsx! {
        div { class: "au-tr",
            div { class: "col", style: "min-width: 0;",
                span { class: "nm", title: "{a.prompt}", "{a.name}" }
                span { class: "pr",
                    if let Some(w) = ws { "{w} · " }
                    "{larrier_core::lsf::preview_line(&a.prompt, 90)}"
                }
            }
            div { class: "cell",
                AgentGlyph { kind: a.agent.clone() }
                span { class: "ellipsis", "{a.agent.display_name()}" }
            }
            div { class: "cell", span { class: "ellipsis", "{a.trigger.describe()}" } }
            div { class: "cell", style: "flex-direction: column; align-items: flex-start; gap: 1px;",
                span { class: "row", style: "gap: 7px;", span { class: "{dot}" } "{status}" }
                if !detail.is_empty() { span { class: "s", "{detail}" } }
            }
            div { class: "acts",
                if let Some(sid) = last {
                    button { class: "icon-btn", title: "Open last run", onclick: move |_| app.open_session(&sid), Icon { i: I::External, size: 14 } }
                }
                button { class: "icon-btn", title: "Run now", disabled: busy, onclick: move |_| crate::automations::run_now(app, &id1), Icon { i: I::Play, size: 13 } }
                button { class: "icon-btn", title: "Edit", onclick: move |_| onedit.call(a2.clone()), Icon { i: I::Pencil, size: 14 } }
                span { style: "padding: 0 4px; display: flex;", title: if enabled { "Pause" } else { "Resume" },
                    Toggle { on: enabled, onchange: move |v: bool| set_enabled(app, &id2, v) }
                }
                button { class: "icon-btn", title: "Delete",
                    onclick: move |_| {
                        let id = id3.clone();
                        app.update_config(move |c| c.automations.retain(|x| x.id != id));
                        app.toast("Automation deleted");
                    },
                    Icon { i: I::Trash, size: 14 }
                }
            }
        }
    }
}

fn set_enabled(app: App, id: &str, on: bool) {
    let id = id.to_string();
    app.update_config(move |c| {
        if let Some(x) = c.automations.iter_mut().find(|x| x.id == id) {
            x.enabled = on;
            if on && matches!(x.trigger, Trigger::Interval { .. } | Trigger::Daily { .. }) {
                // Resume from now instead of firing every missed slot at once.
                x.last_run = Some(chrono::Utc::now());
            }
        }
    });
}

/// Sessions started by automations ("⟳ " titles), newest first.
#[component]
fn RunsTable(query: String) -> Element {
    let app = use_app();
    let runs: Vec<larrier_core::SessionSummary> = app
        .sessions
        .read()
        .iter()
        .filter(|s| s.title.starts_with('⟳'))
        .filter(|s| query.is_empty() || s.title.to_lowercase().contains(&query))
        .take(50)
        .cloned()
        .collect();
    rsx! {
        div { class: "au-table",
            div { class: "au-tr head",
                span { "Run" }
                span { "Agent" }
                span { "Started" }
                span { "Result" }
                span {}
            }
            if runs.is_empty() {
                div { class: "au-empty", "No runs yet. Runs appear here — and in the sidebar — as soon as an automation fires." }
            }
            for s in runs {
                {
                    let id = s.id.clone();
                    let busy = app.is_running(&s.id);
                    rsx! {
                        div { key: "{s.id}", class: "au-tr run", onclick: move |_| app.open_session(&id),
                            div { class: "col", style: "min-width: 0;",
                                span { class: "nm", "{s.title.trim_start_matches('⟳').trim()}" }
                                span { class: "pr", "{s.preview}" }
                            }
                            div { class: "cell", AgentGlyph { kind: s.agent.clone() } span { class: "ellipsis", "{s.agent.display_name()}" } }
                            div { class: "cell", span { "{rel_time(s.created_at)}" } }
                            div { class: "cell",
                                span { class: if busy { "au-dot busy" } else { "au-dot on" } }
                                if busy { "Running" } else { "{s.status.label()}" }
                                if s.added + s.removed > 0 {
                                    span { class: "s", span { class: "add", "+{s.added}" } " " span { class: "del", "-{s.removed}" } }
                                }
                            }
                            div { class: "acts", Icon { i: I::ChevronRight, size: 14 } }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn TemplateCard(id: &'static str, onuse: EventHandler<Template>) -> Element {
    let Some(t) = templates().into_iter().find(|t| t.id == id) else { return rsx! {} };
    let trig_icon = match t.trigger {
        Trigger::OnGitChange { .. } => "git-commit-horizontal",
        Trigger::Manual => "hand",
        _ => "clock",
    };
    let t2 = t.clone();
    rsx! {
        div { class: "au-card",
            button { class: "au-use", onclick: move |_| onuse.call(t2.clone()), "Use" }
            div { class: "ic", style: "color: {t.color};", Lucide { name: t.icon.to_string(), size: 26, stroke: 1.9 } }
            div { class: "t", "{t.title}" }
            div { class: "d", "{t.description}" }
            div { class: "flow",
                Lucide { name: trig_icon.to_string(), size: 14 }
                span { "{t.trigger.badge()}" }
                Icon { i: I::ArrowRight, size: 12 }
                span { "{t.action}" }
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Daily,
    Interval,
    Git,
    Manual,
}

#[component]
fn Editor(initial: Automation, is_new: bool, onclose: EventHandler<()>) -> Element {
    let app = use_app();
    let mut a = use_signal(|| initial.clone());
    let mut error = use_signal(String::new);
    let kind = use_signal(|| match initial.trigger {
        Trigger::Daily { .. } => Kind::Daily,
        Trigger::Interval { .. } => Kind::Interval,
        Trigger::OnGitChange { .. } => Kind::Git,
        Trigger::Manual => Kind::Manual,
    });
    // Per-kind values survive switching between kinds in the editor.
    let (h0, m0, d0) = match &initial.trigger {
        Trigger::Daily { hour, minute, weekdays } => (*hour, *minute, weekdays.clone()),
        _ => (9, 0, vec![0, 1, 2, 3, 4]),
    };
    let mut time = use_signal(|| format!("{h0:02}:{m0:02}"));
    let mut days = use_signal(|| d0);
    let mut minutes = use_signal(|| match initial.trigger {
        Trigger::Interval { minutes } => minutes,
        _ => 60,
    });
    let mut branch = use_signal(|| match &initial.trigger {
        Trigger::OnGitChange { branch } => branch.clone().unwrap_or_default(),
        _ => String::new(),
    });

    let workspaces = app.config.read().workspaces.clone();
    let customs = app.config.read().custom_agents.clone();
    let cur = a.read().clone();
    let models: Vec<String> = cur.agent.spec().models.iter().map(|s| s.to_string()).collect();

    let save = move |run: bool| {
        let mut v = a.read().clone();
        v.name = v.name.trim().to_string();
        if v.name.is_empty() {
            return error.set("Give the automation a name".into());
        }
        if v.prompt.trim().is_empty() {
            return error.set("Write what the agent should do".into());
        }
        v.trigger = match kind() {
            Kind::Daily => {
                let (h, m) = time().split_once(':').map(|(h, m)| (h.parse().unwrap_or(9), m.parse().unwrap_or(0))).unwrap_or((9, 0));
                Trigger::Daily { hour: h, minute: m, weekdays: days() }
            }
            Kind::Interval => Trigger::Interval { minutes: minutes().max(5) },
            Kind::Git => {
                if v.workspace.is_none() {
                    return error.set("On-commit automations need a workspace".into());
                }
                let b = branch().trim().to_string();
                Trigger::OnGitChange { branch: (!b.is_empty()).then_some(b) }
            }
            Kind::Manual => Trigger::Manual,
        };
        if v.workspace.is_none() {
            v.worktree = false;
        }
        let id = v.id.clone();
        app.update_config(move |c| match c.automations.iter().position(|x| x.id == v.id) {
            Some(i) => c.automations[i] = v,
            None => c.automations.push(v),
        });
        app.toast(if is_new { "Automation created" } else { "Automation saved" });
        onclose.call(());
        if run {
            crate::automations::run_now(app, &id);
        }
    };
    let mut save2 = save.clone();
    let mut save3 = save.clone();

    let seg = move |k: Kind, label: &'static str| {
        rsx! {
            button {
                class: if kind() == k { "on" } else { "" },
                onclick: move |_| {
                    let mut kd = kind;
                    kd.set(k);
                },
                "{label}"
            }
        }
    };

    rsx! {
        div { class: "overlay", onclick: move |_| onclose.call(()),
            div { class: "modal au-modal", onclick: move |e| e.stop_propagation(),
                div { class: "modal-head", if is_new { "New automation" } else { "Edit automation" } }
                div { class: "au-form",
                    span { class: "k", "Name" }
                    input { class: "input full", autofocus: true, placeholder: "Nightly bug hunt", value: "{cur.name}",
                        oninput: move |e| a.write().name = e.value() }

                    span { class: "k", "Agent" }
                    div { class: "row",
                        AgentGlyph { kind: cur.agent.clone() }
                        select { class: "input grow",
                            onchange: move |e| { let mut w = a.write(); w.agent = AgentKind::from_id(&e.value()); w.model = None; },
                            for s in larrier_core::agent::runnable() {
                                option { key: "{s.kind.id()}", value: "{s.kind.id()}", selected: s.kind == cur.agent, "{s.name}" }
                            }
                            for c in customs {
                                option { key: "{c.id}", value: "custom:{c.id}", selected: AgentKind::Custom(c.id.clone()) == cur.agent, "{c.name}" }
                            }
                        }
                    }

                    span { class: "k", "Model" }
                    div {
                        input { class: "input full", list: "au-models", placeholder: "Agent default",
                            value: "{cur.model.clone().unwrap_or_default()}",
                            oninput: move |e| { let v = e.value(); a.write().model = (!v.trim().is_empty()).then_some(v); } }
                        datalist { id: "au-models",
                            for m in models { option { key: "{m}", value: "{m}" } }
                        }
                    }

                    span { class: "k", "Workspace" }
                    select { class: "input full",
                        onchange: move |e| { let v = e.value(); a.write().workspace = (!v.is_empty()).then_some(v); },
                        option { value: "", selected: cur.workspace.is_none(), "No folder (chat only)" }
                        for w in workspaces {
                            option { key: "{w.id}", value: "{w.path}", selected: cur.workspace.as_deref() == Some(w.path.as_str()), "{w.name} — {w.path}" }
                        }
                    }

                    if cur.workspace.is_some() {
                        span { class: "k", "Isolation" }
                        div { class: "row",
                            Toggle { on: cur.worktree, onchange: move |v: bool| a.write().worktree = v }
                            span { class: "faint", "Run each time in a fresh git worktree" }
                        }
                    }

                    span { class: "k", "Trigger" }
                    div { class: "seg",
                        {seg(Kind::Daily, "Schedule")}
                        {seg(Kind::Interval, "Interval")}
                        {seg(Kind::Git, "On commit")}
                        {seg(Kind::Manual, "Manual")}
                    }
                    match kind() {
                        Kind::Daily => rsx! {
                            span { class: "k", "At" }
                            div { class: "row", style: "gap: 10px; flex-wrap: wrap;",
                                input { class: "input", r#type: "time", value: "{time}", oninput: move |e| time.set(e.value()) }
                                div { class: "au-days",
                                    for (i, l) in ["M", "T", "W", "T", "F", "S", "S"].iter().enumerate() {
                                        button {
                                            key: "{i}",
                                            class: if days.read().contains(&(i as u8)) { "au-day on" } else { "au-day" },
                                            onclick: move |_| {
                                                let mut d = days.write();
                                                if let Some(p) = d.iter().position(|x| *x == i as u8) { d.remove(p); } else { d.push(i as u8); d.sort_unstable(); }
                                            },
                                            "{l}"
                                        }
                                    }
                                }
                            }
                            span { class: "au-hint", if days.read().is_empty() { "Every day" } else { "Only on the selected days" } }
                        },
                        Kind::Interval => rsx! {
                            span { class: "k", "Every" }
                            select { class: "input",
                                onchange: move |e| minutes.set(e.value().parse().unwrap_or(60)),
                                for (m, l) in [(15u32, "15 minutes"), (30, "30 minutes"), (60, "hour"), (120, "2 hours"), (240, "4 hours"), (720, "12 hours"), (1440, "24 hours")] {
                                    option { key: "{m}", value: "{m}", selected: minutes() == m, "{l}" }
                                }
                            }
                        },
                        Kind::Git => rsx! {
                            span { class: "k", "Branch" }
                            input { class: "input full", placeholder: "Any branch", value: "{branch}", oninput: move |e| branch.set(e.value()) }
                            span { class: "au-hint", "Runs when the workspace's HEAD moves to a new commit (checked every 30 seconds)." }
                        },
                        Kind::Manual => rsx! {
                            span { class: "k" }
                            span { class: "faint", "Runs only when you press Run now." }
                        },
                    }

                    span { class: "k top", "Instructions" }
                    textarea { placeholder: "What should the agent do each time it runs?", value: "{cur.prompt}",
                        oninput: move |e| a.write().prompt = e.value() }
                }
                div { class: "modal-foot",
                    if !error.read().is_empty() { span { class: "au-err", "{error}" } }
                    button { class: "ghost-btn", onclick: move |_| onclose.call(()), "Cancel" }
                    button { class: "ghost-btn", onclick: move |_| save2(true), Icon { i: I::Play, size: 12 } "Save & run now" }
                    button { class: "primary-btn", onclick: move |_| save3(false), if is_new { "Create" } else { "Save" } }
                }
            }
        }
    }
}
