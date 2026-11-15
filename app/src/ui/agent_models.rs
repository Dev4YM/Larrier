//! A harness's models (Settings → Harnesses → one agent): what the agent
//! itself is set up with (synced from its own settings), Larrier's defaults,
//! which models the picker shows, and models the user adds, on the agent's
//! own service or one of its API accounts.

use dioxus::prelude::*;
use larrier_agents::models::effort_label;
use larrier_agents::request::takes_effort;
use larrier_core::{AccountAuth, AgentKind, CustomModel};

use super::{Select, Toggle};
use crate::agent_models::{self as am, Choice};
use crate::icons::{Icon, I};
use crate::logos::{model_logo, Logo};
use crate::state::{rel_time, use_app};

fn add_model_hint(kind: &AgentKind) -> &'static str {
    match kind {
        AgentKind::Cursor => "An id from Cursor's model list. Cursor Agent only runs models your Cursor account offers.",
        AgentKind::ClaudeCode => "Any id Claude Code accepts, such as a dated model id or a model on your gateway.",
        AgentKind::Codex => "Any model id. Pick one of its API accounts to run it on your own provider.",
        AgentKind::OpenCode => "provider/model as OpenCode names it, or a model on one of its API accounts.",
        AgentKind::Droid => "A Droid model id, or a model on one of its API accounts (added as a Droid custom model per run).",
        AgentKind::Hermes => "A model id for Hermes's current provider, or a model on one of its API accounts.",
        _ => "A model id the agent accepts.",
    }
}

/// "Low – Max" for a model's efforts.
fn effort_span(e: &[String]) -> String {
    match (e.first(), e.last()) {
        (Some(a), Some(b)) if a != b => format!("{} – {}", effort_label(a), effort_label(b)),
        (Some(a), _) => effort_label(a),
        _ => String::new(),
    }
}

#[component]
pub fn AgentModelsPanel(kind: AgentKind) -> Element {
    let app = use_app();
    let name = kind.display_name().to_string();
    let native = app.native_models.read().get(&kind.id()).cloned();
    let refreshing = app.models_refreshing.read().contains(&kind.id());
    let ac = app.config.read().agent(&kind);
    let all = am::choices(&app, &kind);
    let mut query = crate::keep::use_kept(crate::keep::WINDOW, "models.search", String::new);
    let mut show_all = crate::keep::use_kept(crate::keep::WINDOW, "models.all", || false);
    let mut limit = use_signal(|| 60usize);
    // Re-read the agent's files when its page opens.
    let k0 = kind.clone();
    use_effect(use_reactive!(|k0| app.refresh_agent_models(Some(k0.clone()), false)));

    let shown_n = all.iter().filter(|c| c.shown).count();
    let q = query.read().trim().to_lowercase();
    let list: Vec<Choice> = all
        .iter()
        .filter(|c| show_all() || c.shown || !q.is_empty())
        .filter(|c| q.is_empty() || c.name.to_lowercase().contains(&q) || c.id.to_lowercase().contains(&q))
        .cloned()
        .collect();
    let total = list.len();

    // Defaults: Larrier's, else the agent's own.
    let r = am::resolve(&app, &kind, None, None);
    let agent_default = native.as_ref().and_then(|n| n.default_model.clone());
    let agent_default_name = agent_default.as_deref().map(|m| {
        let fam = native.as_ref().and_then(|n| n.model(m)).map(|x| x.id.clone()).unwrap_or(m.to_string());
        am::short_name(&app, &kind, &fam)
    });
    let def_family = ac.default_model.as_deref().map(|m| native.as_ref().and_then(|n| n.model(m)).map(|x| x.id.clone()).unwrap_or(m.to_string()));
    let mut model_opts: Vec<(String, String)> = vec![(String::new(), match &agent_default_name {
        Some(n) => format!("{name}'s default · {n}"),
        None => format!("{name}'s default"),
    })];
    for c in all.iter().filter(|c| c.shown || def_family.as_deref() == Some(c.id.as_str())) {
        model_opts.push((c.id.clone(), c.name.clone()));
    }
    let fam_for_effort = def_family.clone().or_else(|| r.family.clone()).or_else(|| r.model.clone());
    let effort_choices: Vec<String> = fam_for_effort.as_deref().and_then(|f| all.iter().find(|c| c.id == f)).map(|c| c.efforts.clone()).unwrap_or_default();
    let cursor_like = native.as_ref().map(|n| n.models.iter().any(|m| !m.variants.is_empty())).unwrap_or(false);
    let effort_value = if cursor_like { ac.default_model.as_deref().and_then(|m| native.as_ref().and_then(|n| n.variant(m))).and_then(|(_, v)| v.and_then(|v| v.effort.clone())).unwrap_or_default() } else { ac.default_effort.clone().unwrap_or_default() };
    let agent_effort = native.as_ref().and_then(|n| n.default_effort.clone());
    let mut effort_opts: Vec<(String, String)> = vec![(String::new(), match &agent_effort {
        Some(e) => format!("{name}'s default · {}", effort_label(e)),
        None => format!("{name}'s default"),
    })];
    effort_opts.extend(effort_choices.iter().map(|e| (e.clone(), effort_label(e))));
    let show_effort = (takes_effort(&kind) || cursor_like) && !effort_choices.is_empty();

    let sources = native.as_ref().map(|n| n.sources.clone()).unwrap_or_default();
    let updated = native.as_ref().and_then(|n| n.fetched_at).map(|t| match rel_time(t).as_str() {
        "now" => "Read just now".to_string(),
        r => format!("Read {r} ago"),
    });

    rsx! {
        // ---------------------------------------------------------- synced
        div { class: "set-group-title",
            span { "From {name}'s settings" }
            div { class: "grow" }
            button { class: "ghost-btn sm", disabled: refreshing, title: "Read the agent's settings and model list again",
                onclick: { let k = kind.clone(); move |_| app.refresh_agent_models(Some(k.clone()), true) },
                if refreshing { super::loaders::Orbit { size: 12 } } else { Icon { i: I::Refresh, size: 13 } }
                if refreshing { "Reading…" } else { "Refresh" }
            }
        }
        div { class: "set-card",
            div { class: "set-row",
                div { class: "lbl",
                    div { class: "t",
                        "Default "
                        span { class: "faint", style: "font-weight: 400",
                            {agent_default_name.clone().unwrap_or_else(|| "not set".into())}
                            if let Some(e) = &agent_effort { " · {effort_label(e)}" }
                        }
                    }
                    div { class: "d",
                        {updated.clone().unwrap_or_else(|| "Not read yet".into())}
                        if !sources.is_empty() { " · " span { class: "code-inline", title: "{sources.join(\"\\n\")}", {sources.len().to_string()} if sources.len() == 1 { " file" } else { " files" } } }
                        " · {all.iter().filter(|c| !c.added).count()} models"
                    }
                }
            }
            if let Some(err) = native.as_ref().and_then(|n| n.error.clone()) {
                div { class: "set-row",
                    span { class: "faint", Icon { i: I::Alert, size: 14 } }
                    div { class: "lbl", div { class: "d", "{err}" } }
                }
            }
            for p in native.as_ref().map(|n| n.providers.clone()).unwrap_or_default() {
                div { key: "{p.name}", class: "set-row",
                    span { class: "faint", Icon { i: if p.base_url.is_some() { I::Server } else { I::Key }, size: 14 } }
                    div { class: "lbl",
                        div { class: "t", "{p.name}" }
                        div { class: "d",
                            {p.base_url.clone().unwrap_or_else(|| "Provider's own API".into())}
                            if !p.models.is_empty() { " · {p.models.len()} models" }
                        }
                    }
                    if let Some(on) = p.active { span { class: if on { "status-pill ok" } else { "status-pill missing" }, if on { "On" } else { "Off" } } }
                    span { class: if p.has_key { "status-pill ok" } else { "status-pill missing" }, if p.has_key { "Key set" } else { "No key" } }
                }
            }
            for n in native.as_ref().map(|n| n.notes.clone()).unwrap_or_default() {
                div { key: "{n}", class: "set-row",
                    span { class: "faint", Icon { i: I::Info, size: 14 } }
                    div { class: "lbl", div { class: "d", "{n}" } }
                }
            }
        }

        // ---------------------------------------------------------- defaults
        div { class: "set-group-title", "Defaults in Larrier" }
        div { class: "set-card",
            div { class: "set-row",
                div { class: "lbl",
                    div { class: "t", "Model" }
                    div { class: "d", "New sessions start with this. \"{name}'s default\" follows its own setting as it changes." }
                }
                Select { value: def_family.clone().unwrap_or_default(), options: model_opts, wide: true,
                    onchange: {
                        let k = kind.clone();
                        let eff = ac.default_effort.clone();
                        move |v: String| {
                            if v.is_empty() {
                                app.update_config(|c| c.agent_mut(&k).default_model = None);
                            } else {
                                let (m, e) = am::compose(&app, &k, &v, eff.as_deref(), false);
                                app.update_config(|c| { let a = c.agent_mut(&k); a.default_model = Some(m); if e.is_none() && cursor_like { a.default_effort = None; } });
                            }
                        }
                    },
                }
            }
            if show_effort {
                div { class: "set-row",
                    div { class: "lbl",
                        div { class: "t", "Reasoning effort" }
                        div { class: "d", "How hard the model thinks before answering. Higher is slower and uses more of your plan." }
                    }
                    Select { value: effort_value, options: effort_opts, wide: true,
                        onchange: {
                            let k = kind.clone();
                            let fam = fam_for_effort.clone();
                            let has_default = ac.default_model.is_some();
                            move |v: String| {
                                let e = (!v.is_empty()).then_some(v);
                                match (&fam, cursor_like) {
                                    // Cursor: the effort picks a variant of the default model.
                                    (Some(f), true) => {
                                        let (m, _) = am::compose(&app, &k, f, e.as_deref(), false);
                                        app.update_config(|c| c.agent_mut(&k).default_model = if e.is_none() && !has_default { None } else { Some(m) });
                                    }
                                    _ => app.update_config(|c| c.agent_mut(&k).default_effort = e),
                                }
                            }
                        },
                    }
                }
            }
        }

        // ---------------------------------------------------------- models
        div { class: "set-group-title",
            span { "Models in the picker" }
            span { class: "faint", style: "font-weight: 400", "{shown_n} of {all.len()}" }
        }
        div { class: "set-card mr-card",
            div { class: "mr-tools",
                div { class: "mr-search",
                    Icon { i: I::Search, size: 13 }
                    input { placeholder: "Search {all.len()} models…", value: "{query}", oninput: move |e| { query.set(e.value()); limit.set(60); } }
                }
                div { class: "seg-mini",
                    button { class: if !show_all() { "on" } else { "" }, onclick: move |_| show_all.set(false), "In picker" }
                    button { class: if show_all() { "on" } else { "" }, onclick: move |_| show_all.set(true), "All" }
                }
            }
            div { class: "am-list",
                for c in list.iter().take(limit()).cloned() {
                    ModelRow { key: "{c.id}", kind: kind.clone(), c }
                }
                if total == 0 {
                    div { class: "mr-empty", if q.is_empty() { "No models yet. Refresh, or add one below." } else { "No models match." } }
                }
            }
            if total > limit() {
                button { class: "load-earlier", onclick: move |_| limit.set(limit() + 200), "Show more ({total - limit()} left)" }
            }
        }

        AddModel { kind: kind.clone() }
    }
}

#[component]
fn ModelRow(kind: AgentKind, c: Choice) -> Element {
    let app = use_app();
    let logo = model_logo(&c.id);
    let efforts = effort_span(&c.efforts);
    let mut badges: Vec<(String, &'static str)> = Vec::new();
    if c.added {
        badges.push((c.endpoint.clone().map(|e| format!("via {e}")).unwrap_or_else(|| "Added".into()), "mr-tag"));
    } else if c.custom {
        badges.push(("Custom".into(), "mr-tag"));
    }
    if !c.featured && !c.added {
        badges.push(("Not in its picker".into(), "mr-tag old"));
    }
    let tip = {
        let mut t = c.id.clone();
        if let Some(g) = &c.group {
            t.push_str(&format!("\n{g}"));
        }
        if !efforts.is_empty() {
            t.push_str(&format!("\nEffort: {}", c.efforts.iter().map(|e| effort_label(e)).collect::<Vec<_>>().join(", ")));
        }
        t
    };
    let (k1, k2, id1, id2) = (kind.clone(), kind.clone(), c.id.clone(), c.id.clone());
    let featured = c.featured;
    rsx! {
        div { class: if c.shown { "am-row" } else { "am-row off" }, title: "{tip}",
            span { class: "lg", if let Some(svg) = logo { Logo { svg, size: 14 } } else { Icon { i: I::Sparkle, size: 13, stroke: 1.5 } } }
            span { class: "nm ellipsis", "{c.name}" }
            for (b, cls) in badges { span { key: "{b}", class: "{cls}", "{b}" } }
            span { class: "id ellipsis", "{c.id}" }
            span { class: "eff", "{efforts}" }
            span { class: "ctx", {c.context.map(larrier_core::models::tokens_short).unwrap_or_default()} }
            if c.added {
                button { class: "icon-btn", title: "Remove this model",
                    onclick: move |_| app.update_config(|cfg| { let a = cfg.agent_mut(&k1); a.custom_models.retain(|m| m.id != id1); a.extra_models.retain(|m| *m != id1); }),
                    Icon { i: I::Trash, size: 13 }
                }
            } else {
                Toggle { on: c.shown,
                    onchange: move |on: bool| app.update_config(|cfg| {
                        let a = cfg.agent_mut(&k2);
                        // Featured models are on unless hidden; the rest are off unless shown.
                        if featured {
                            a.hidden_models.retain(|m| *m != id2);
                            if !on { a.hidden_models.push(id2.clone()); }
                        } else {
                            a.shown_models.retain(|m| *m != id2);
                            if on { a.shown_models.push(id2.clone()); }
                        }
                    }),
                }
            }
        }
    }
}

#[component]
fn AddModel(kind: AgentKind) -> Element {
    let app = use_app();
    let mut id = use_signal(String::new);
    let mut label = use_signal(String::new);
    let mut via = use_signal(String::new);
    // Its API accounts that Larrier points it at can serve a model.
    let mut ep_opts: Vec<(String, String)> = vec![(String::new(), "Whatever the chat runs as".to_string())];
    ep_opts.extend(app.config.read().accounts_for(&kind).into_iter().filter_map(|a| match &a.auth {
        AccountAuth::Api(x) if x.env_var.is_none() => Some((a.id.clone(), format!("{} · {}", a.label, x.api.label()))),
        _ => None,
    }));
    let can_add = !id.read().trim().is_empty();
    let k = kind.clone();
    let add = move |_| {
        let mid = id.read().trim().to_string();
        if mid.is_empty() {
            return;
        }
        let name = Some(label.read().trim().to_string()).filter(|s| !s.is_empty());
        let account = Some(via.read().clone()).filter(|s| !s.is_empty());
        app.update_config(|c| {
            let a = c.agent_mut(&k);
            a.custom_models.retain(|m| m.id != mid);
            a.custom_models.push(CustomModel { id: mid.clone(), name: name.clone(), account: account.clone() });
        });
        id.set(String::new());
        label.set(String::new());
        app.toast(format!("Added {mid}"));
    };
    rsx! {
        div { class: "set-group-title", "Add a model" }
        div { class: "set-card",
            div { class: "set-row am-add",
                input { class: "input", placeholder: "Model id", value: "{id}", oninput: move |e| id.set(e.value()) }
                input { class: "input", placeholder: "Name (optional)", value: "{label}", oninput: move |e| label.set(e.value()) }
                if ep_opts.len() > 1 {
                    Select { value: via(), options: ep_opts, onchange: move |v: String| via.set(v) }
                }
                button { class: "primary-btn", disabled: !can_add, onclick: add, Icon { i: I::Plus, size: 12 } "Add" }
            }
            div { class: "set-row", style: "min-height: 0; padding-top: 0;",
                div { class: "lbl", div { class: "d", "{add_model_hint(&kind)}" } }
            }
        }
    }
}
