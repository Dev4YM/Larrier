//! Settings → Models: the model registry — every model Larrier knows with
//! its vendor, context window, output cap and prices, searchable and
//! filterable.

use dioxus::prelude::*;
use larrier_core::models::{self, money, tokens_short, ModelInfo, ModelStatus, Source, Vendor};

use crate::icons::{Icon, I};
use crate::logos::{model_logo, Logo};
use crate::state::use_app;

#[component]
pub fn ModelRegistry() -> Element {
    let app = use_app();
    let mut query = use_signal(String::new);
    let mut vendor = use_signal(|| None::<Vendor>);
    let mut show_old = use_signal(|| false);
    let mut limit = use_signal(|| 80usize);
    // Bumped after a refresh so the list re-reads the registry.
    let mut rev = use_signal(|| 0u32);
    let status = {
        let _ = rev();
        crate::models_sync::status()
    };
    let all: Vec<ModelInfo> = {
        let _ = rev();
        models::all()
    };
    let q = query.read().trim().to_lowercase();
    let vend = vendor();
    let mut vendors: Vec<(Vendor, usize)> = Vec::new();
    for m in all.iter() {
        match vendors.iter_mut().find(|(v, _)| *v == m.vendor) {
            Some(e) => e.1 += 1,
            None => vendors.push((m.vendor, 1)),
        }
    }
    // Best-known vendors first, "Other" last.
    vendors.sort_by_key(|(v, n)| (*v == Vendor::Other, Vendor::ALL.iter().position(|x| x == v).unwrap_or(99), std::cmp::Reverse(*n)));
    let shown: Vec<ModelInfo> = all
        .into_iter()
        .filter(|m| vend.map(|v| v == m.vendor).unwrap_or(true))
        .filter(|m| show_old() || m.status == ModelStatus::Active)
        // Anthropic's own catalog is complete; older third-party listings of
        // the same models only show with legacy models.
        .filter(|m| show_old() || m.vendor != Vendor::Anthropic || m.source == Source::Official)
        .filter(|m| q.is_empty() || m.name.to_lowercase().contains(&q) || m.id.contains(&q) || m.vendor.name().to_lowercase().contains(&q))
        .collect();
    let total = shown.len();
    let visible: Vec<ModelInfo> = shown.into_iter().take(limit()).collect();
    let updated = match (&status.fetched_at, status.refreshing) {
        (_, true) => "Updating…".to_string(),
        (Some(t), _) => match crate::state::rel_time(*t).as_str() {
            "now" => "Prices updated just now".to_string(),
            r if r.ends_with(['m', 'h', 'd']) && r.len() <= 4 => format!("Prices updated {r} ago"),
            r => format!("Prices updated {r}"),
        },
        (None, _) => "Live prices not downloaded yet".to_string(),
    };

    rsx! {
        div { class: "set-group-title", style: "display: flex; align-items: baseline; gap: 10px;",
            span { "Model registry" }
            span { class: "mr-meta", "{updated}" }
            div { class: "grow" }
            button { class: "ghost-btn sm", disabled: status.refreshing,
                onclick: move |_| {
                    spawn(async move {
                        let r = tokio::task::spawn_blocking(crate::models_sync::refresh_blocking).await;
                        match r {
                            Ok(Ok(n)) => app.toast(format!("Model registry updated · {n} models")),
                            Ok(Err(e)) => app.toast_err(format!("Couldn't update the model registry: {e}")),
                            Err(e) => app.toast_err(e.to_string()),
                        }
                        rev += 1;
                    });
                    rev += 1;
                },
                Icon { i: I::Refresh, size: 13 }
                "Refresh"
            }
        }
        div { class: "mr-note",
            "Anthropic models use Anthropic's published prices; other vendors come from the open LiteLLM registry. Prices are US dollars per million tokens."
        }
        div { class: "set-card mr-card",
            div { class: "mr-tools",
                div { class: "mr-search",
                    Icon { i: I::Search, size: 13 }
                    input { placeholder: "Search {total} models…", value: "{query}", oninput: move |e| { query.set(e.value()); limit.set(80); } }
                }
                label { class: "mr-old",
                    input { r#type: "checkbox", checked: show_old(), onchange: move |e| show_old.set(e.checked()) }
                    "Show legacy & deprecated"
                }
            }
            div { class: "mr-vendors",
                button { class: if vend.is_none() { "mr-chip on" } else { "mr-chip" }, onclick: move |_| vendor.set(None), "All" }
                for (v, n) in vendors.iter().copied().take(14) {
                    button { key: "{v.name()}", class: if vend == Some(v) { "mr-chip on" } else { "mr-chip" },
                        onclick: move |_| { vendor.set(if vend == Some(v) { None } else { Some(v) }); limit.set(80); },
                        "{v.name()}"
                        span { class: "n", "{n}" }
                    }
                }
            }
            div { class: "mr-table",
                div { class: "mr-row head",
                    span { class: "c-name", "Model" }
                    span { class: "c-num", "Context" }
                    span { class: "c-num", "Output" }
                    span { class: "c-num", "Input" }
                    span { class: "c-num", "Output" }
                    span { class: "c-num", "Cache read" }
                }
                for m in visible {
                    ModelRow { key: "{m.id}", m }
                }
                if total == 0 {
                    div { class: "mr-empty", "No models match." }
                }
            }
            if total > limit() {
                button { class: "load-earlier", onclick: move |_| limit.set(limit() + 200), "Show more ({total - limit()} left)" }
            }
        }
    }
}

#[component]
fn ModelRow(m: ModelInfo) -> Element {
    let logo = model_logo(&m.id);
    let p = m.price.clone();
    let dash = || "—".to_string();
    let tip = format!(
        "{}\n{} · {}{}",
        m.id,
        m.vendor.name(),
        if m.source == Source::Official { "official catalog" } else { "LiteLLM registry" },
        p.as_ref()
            .and_then(|p| p.cache_write)
            .map(|w| format!("\nCache write {}", money(w)))
            .unwrap_or_default()
    );
    rsx! {
        div { class: "mr-row", title: "{tip}",
            span { class: "c-name",
                span { class: "lg", if let Some(svg) = logo { Logo { svg, size: 14 } } else { Icon { i: I::Sparkle, size: 13, stroke: 1.5 } } }
                span { class: "nm ellipsis", "{m.name}" }
                if let Some(t) = m.tag.clone() { span { class: "mr-tag", "{t}" } }
                if m.status != ModelStatus::Active { span { class: "mr-tag old", "{m.status.label()}" } }
                span { class: "id ellipsis", "{m.id}" }
            }
            span { class: "c-num", {m.context.map(tokens_short).unwrap_or_else(dash)} }
            span { class: "c-num", {m.max_output.map(tokens_short).unwrap_or_else(dash)} }
            span { class: "c-num", {p.as_ref().map(|p| money(p.input)).unwrap_or_else(dash)} }
            span { class: "c-num", {p.as_ref().map(|p| money(p.output)).unwrap_or_else(dash)} }
            span { class: "c-num", {p.as_ref().and_then(|p| p.cache_read).map(money).unwrap_or_else(dash)} }
        }
    }
}
