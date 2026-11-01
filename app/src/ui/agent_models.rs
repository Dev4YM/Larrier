//! Settings → Models for one agent: what the agent itself is set up with
//! (synced from its own settings), Larrier's defaults, which models the
//! picker shows, models the user adds, and their own API endpoints where the
//! agent can take one.

use dioxus::prelude::*;
use larrier_agents::models::effort_label;
use larrier_agents::request::{endpoint_for_all, endpoint_kinds, takes_effort};
use larrier_core::lsf::new_id;
use larrier_core::{secrets, AgentEndpoint, AgentKind, CustomModel, EndpointKind};

use super::{Select, Toggle};
use crate::agent_models::{self as am, Choice};
use crate::icons::{Icon, I};
use crate::logos::{model_logo, Logo};
use crate::state::{rel_time, use_app};

/// How Larrier hands an agent your endpoint, in a sentence.
fn endpoint_how(kind: &AgentKind) -> &'static str {
    match kind {
        AgentKind::ClaudeCode => "Runs started in Larrier get ANTHROPIC_BASE_URL and your key. In these runs your key is used instead of your Claude subscription. Claude Code's own settings aren't changed.",
        AgentKind::Codex => "Each run gets a Codex provider of its own (-c model_providers…) with your key in its environment. ~/.codex/config.toml isn't changed.",
        AgentKind::OpenCode => "Each run gets a provider of its own through OPENCODE_CONFIG_CONTENT, with your key in its environment. Your opencode.json isn't changed.",
        AgentKind::Droid => "Each run gets your model as a Droid custom model through a settings file Larrier writes for that run (droid --settings). ~/.factory/settings.json isn't changed.",
        AgentKind::Hermes => "Runs use --provider with your base URL and key in the environment. Hermes's config.yaml isn't changed.",
        _ => "",
    }
}

/// What the agent can't do with your own keys, when it can't take them.
fn endpoint_limit(kind: &AgentKind) -> String {
    match kind {
        AgentKind::Cursor => "Cursor Agent runs every model through your Cursor account. Your own OpenAI or Anthropic keys, a custom OpenAI base URL and custom models are Cursor app features. Its CLI doesn't take them, so Larrier can't apply them to runs. What you've set up in the Cursor app is shown above.".into(),
        AgentKind::Gemini => "Gemini CLI uses your Google login or GEMINI_API_KEY. Add a key under Accounts to run it on your own key.".into(),
        _ => format!("{} can't be pointed at your own endpoint from Larrier. Use Accounts for its API key or login.", kind.display_name()),
    }
}

fn add_model_hint(kind: &AgentKind) -> &'static str {
    match kind {
        AgentKind::Cursor => "An id from Cursor's model list. Cursor Agent only runs models your Cursor account offers.",
        AgentKind::ClaudeCode => "Any id Claude Code accepts, such as a dated model id or a model on your gateway.",
        AgentKind::Codex => "Any model id. Pick one of your endpoints to run it on your own provider.",
        AgentKind::OpenCode => "provider/model as OpenCode names it, or a model on one of your endpoints.",
        AgentKind::Droid => "A Droid model id, or a model on one of your endpoints (added as a Droid custom model per run).",
        AgentKind::Hermes => "A model id for Hermes's current provider, or a model on one of your endpoints.",
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

    let kinds = endpoint_kinds(&kind);
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

        // ---------------------------------------------------------- endpoints
        div { class: "set-group-title", "Your API keys & endpoints" }
        if kinds.is_empty() {
            div { class: "set-card",
                div { class: "set-row",
                    span { class: "faint", Icon { i: I::Info, size: 14 } }
                    div { class: "lbl", div { class: "d", "{endpoint_limit(&kind)}" } }
                }
            }
        } else {
            div { class: "set-card",
                div { class: "set-row",
                    span { class: "faint", Icon { i: I::Info, size: 14 } }
                    div { class: "lbl", div { class: "d", "{endpoint_how(&kind)}" } }
                }
                for ep in ac.endpoints.clone() {
                    EndpointRow { key: "{ep.id}", kind: kind.clone(), ep }
                }
                AddEndpoint { kind: kind.clone() }
            }
        }
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
    let ac = app.config.read().agent(&kind);
    let mut id = use_signal(String::new);
    let mut label = use_signal(String::new);
    let mut via = use_signal(String::new);
    let mut ep_opts: Vec<(String, String)> = vec![(String::new(), format!("{}'s own service", kind.display_name()))];
    ep_opts.extend(ac.endpoints.iter().map(|e| (e.id.clone(), format!("{} · {}", e.name, e.kind.label()))));
    let can_add = !id.read().trim().is_empty();
    let k = kind.clone();
    let add = move |_| {
        let mid = id.read().trim().to_string();
        if mid.is_empty() {
            return;
        }
        let name = Some(label.read().trim().to_string()).filter(|s| !s.is_empty());
        let endpoint = Some(via.read().clone()).filter(|s| !s.is_empty());
        app.update_config(|c| {
            let a = c.agent_mut(&k);
            a.custom_models.retain(|m| m.id != mid);
            a.custom_models.push(CustomModel { id: mid.clone(), name: name.clone(), endpoint: endpoint.clone() });
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

#[component]
fn EndpointRow(kind: AgentKind, ep: AgentEndpoint) -> Element {
    let app = use_app();
    let saved = ep.secret_ref.as_deref().and_then(secrets::get).map(|k| secrets::mask(&k));
    let mut key = use_signal(String::new);
    let (k1, k2, k3, k4, k5, k6) = (kind.clone(), kind.clone(), kind.clone(), kind.clone(), kind.clone(), kind.clone());
    let (i1, i2, i3, i4, i5, i6) = (ep.id.clone(), ep.id.clone(), ep.id.clone(), ep.id.clone(), ep.id.clone(), ep.id.clone());
    let for_all = endpoint_for_all(&kind);
    let with = move |k: AgentKind, id: String, f: Box<dyn FnOnce(&mut AgentEndpoint)>| {
        app.update_config(move |c| {
            if let Some(e) = c.agent_mut(&k).endpoints.iter_mut().find(|e| e.id == id) {
                f(e);
            }
        })
    };
    rsx! {
        div { class: "am-ep",
            div { class: "set-row",
                span { class: "faint", Icon { i: I::Server, size: 14 } }
                div { class: "lbl am-ep-fields",
                    input { class: "input", value: "{ep.name}", title: "Name",
                        onchange: move |e| { let v = e.value(); with(k1.clone(), i1.clone(), Box::new(move |x| x.name = v)) } }
                    span { class: "status-pill", "{ep.kind.label()}" }
                    input { class: "input wide", value: "{ep.base_url}", title: "Base URL",
                        onchange: move |e| { let v = e.value(); with(k2.clone(), i2.clone(), Box::new(move |x| x.base_url = v)) } }
                }
                Toggle { on: ep.enabled, onchange: move |v: bool| with(k3.clone(), i3.clone(), Box::new(move |x| x.enabled = v)) }
                button { class: "icon-btn", title: "Remove this endpoint",
                    onclick: move |_| {
                        let id = i4.clone();
                        app.update_config(|c| {
                            let a = c.agent_mut(&k4);
                            if let Some(e) = a.endpoints.iter().find(|e| e.id == id) { if let Some(r) = &e.secret_ref { secrets::delete(r); } }
                            a.endpoints.retain(|e| e.id != id);
                            for m in a.custom_models.iter_mut().filter(|m| m.endpoint.as_deref() == Some(id.as_str())) { m.endpoint = None; }
                        });
                    },
                    Icon { i: I::Trash, size: 13 }
                }
            }
            div { class: "set-row am-ep-sub",
                div { class: "row", style: "gap: 6px; flex: 1; min-width: 0;",
                    input { class: "input", style: "width: 220px", r#type: "password", placeholder: if saved.is_some() { "Replace key" } else { "Paste API key" }, value: "{key}", oninput: move |e| key.set(e.value()) }
                    button { class: "ghost-btn", disabled: key().trim().is_empty(),
                        onclick: move |_| {
                            let r = format!("endpoint:{}", i5);
                            match secrets::set(&r, key().trim()) {
                                Ok(()) => { let r2 = r.clone(); with(k5.clone(), i5.clone(), Box::new(move |x| x.secret_ref = Some(r2))); key.set(String::new()); app.toast("Key saved to your keychain"); }
                                Err(e) => app.toast_err(e.to_string()),
                            }
                        },
                        "Save key"
                    }
                    span { class: "faint", style: "font-size: 11px;", {saved.clone().map(|m| format!("Saved: {m}")).unwrap_or_else(|| "No key yet".into())} }
                }
                if for_all {
                    label { class: "am-check",
                        input { r#type: "checkbox", checked: ep.use_for_all, onchange: move |e| { let v = e.checked(); with(k6.clone(), i6.clone(), Box::new(move |x| x.use_for_all = v)) } }
                        "Use for every model"
                    }
                }
            }
        }
    }
}

#[component]
fn AddEndpoint(kind: AgentKind) -> Element {
    let app = use_app();
    let kinds = endpoint_kinds(&kind);
    let first = kinds.first().copied().unwrap_or(EndpointKind::OpenAiCompatible);
    let mut ek = use_signal(|| first);
    let mut url = use_signal(|| first.default_url().to_string());
    let mut nm = use_signal(String::new);
    let opts: Vec<(String, String)> = kinds.iter().map(|k| (k.id().to_string(), k.label().to_string())).collect();
    let kk = kind.clone();
    rsx! {
        div { class: "set-row am-add",
            Select { value: ek().id().to_string(), options: opts,
                onchange: move |v: String| {
                    if let Some(k) = EndpointKind::ALL.iter().find(|k| k.id() == v) {
                        // Follow the kind's usual URL unless one was typed.
                        if EndpointKind::ALL.iter().any(|x| x.default_url() == url.read().as_str()) || url.read().is_empty() {
                            url.set(k.default_url().to_string());
                        }
                        ek.set(*k);
                    }
                },
            }
            input { class: "input", placeholder: "Name", value: "{nm}", oninput: move |e| nm.set(e.value()) }
            input { class: "input wide", placeholder: "Base URL", value: "{url}", oninput: move |e| url.set(e.value()) }
            button { class: "primary-btn", disabled: url.read().trim().is_empty(),
                onclick: move |_| {
                    let kind_now = ek();
                    let name = Some(nm.read().trim().to_string()).filter(|s| !s.is_empty()).unwrap_or_else(|| kind_now.label().to_string());
                    let ep = AgentEndpoint { id: new_id()[24..].to_string(), name, kind: kind_now, base_url: url.read().trim().to_string(), secret_ref: None, enabled: true, use_for_all: false };
                    app.update_config(|c| c.agent_mut(&kk).endpoints.push(ep.clone()));
                    nm.set(String::new());
                    app.toast("Endpoint added. Paste its key to use it.");
                },
                Icon { i: I::Plus, size: 12 }
                "Add endpoint"
            }
        }
    }
}
