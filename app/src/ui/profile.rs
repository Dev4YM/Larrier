//! Settings → Profile: who's using Larrier (name, photo, role, about) and
//! their work with AI over the year: a calendar of every day's tokens,
//! cost, prompts or sessions, by harness and model.

use std::collections::{BTreeMap, HashMap, HashSet};

use chrono::{Datelike, Duration, Local, NaiveDate};
use dioxus::prelude::*;
use larrier_core::{lsf::MediaSource, AgentKind, Part, SessionSummary};

use super::usage::stats::{self, date_of, day_num, SessionStats};
use super::usage::{fmt_cost, fmt_int, fmt_tokens};
use crate::icons::{Icon, I};
use crate::logos::{agent_logo, Logo};
use crate::state::{display_name, image_src, use_app};

/// The user's photo, or their initial.
#[component]
pub fn Avatar(#[props(default = 30)] size: u32) -> Element {
    let app = use_app();
    let (name, photo) = {
        let cfg = app.config.read();
        (display_name(&cfg), cfg.profile.photo.clone())
    };
    let src = photo.and_then(|p| {
        let mime = mime_of(&p);
        image_src(&Part::Image { mime: Some(mime.into()), source: MediaSource::Path { path: p } })
    });
    let initial = name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();
    let font = (size as f64 * 0.43).round();
    rsx! {
        div { class: "avatar", style: "width: {size}px; height: {size}px; font-size: {font}px;",
            if let Some(s) = src { img { src: "{s}", alt: "", draggable: "false" } } else { "{initial}" }
        }
    }
}

fn mime_of(path: &str) -> &'static str {
    match std::path::Path::new(path).extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref() {
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        _ => "image/png",
    }
}

/// Where the photo is kept: Larrier's data folder.
fn photo_dir() -> std::path::PathBuf {
    larrier_core::store::data_root().join("profile")
}

/// Keep a picked photo in `dir` (the old one there goes).
fn keep_photo(from: &std::path::Path, old: Option<&str>, dir: &std::path::Path) -> Result<String, String> {
    let ext = from.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).filter(|e| ["png", "jpg", "jpeg", "gif", "webp"].contains(&e.as_str())).ok_or("Pick a PNG, JPEG, GIF or WebP picture")?;
    let meta = std::fs::metadata(from).map_err(|e| e.to_string())?;
    if meta.len() > 8 * 1024 * 1024 {
        return Err("That picture is over 8 MB".into());
    }
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let to = dir.join(format!("photo-{}.{ext}", larrier_core::lsf::new_id()));
    std::fs::copy(from, &to).map_err(|e| e.to_string())?;
    if let Some(old) = old.map(std::path::Path::new).filter(|o| o.starts_with(dir)) {
        let _ = std::fs::remove_file(old);
    }
    Ok(to.to_string_lossy().to_string())
}

#[derive(Clone, Copy, PartialEq)]
enum Metric {
    Tokens,
    Cost,
    Prompts,
    Sessions,
}

impl Metric {
    const ALL: [Metric; 4] = [Metric::Tokens, Metric::Cost, Metric::Prompts, Metric::Sessions];

    fn label(self) -> &'static str {
        match self {
            Metric::Tokens => "Tokens",
            Metric::Cost => "Cost",
            Metric::Prompts => "Prompts",
            Metric::Sessions => "Sessions",
        }
    }
}

/// One day's (or one harness's share of a day's) work.
#[derive(Clone, Default, PartialEq)]
struct Tally {
    tokens: u64,
    cost: f64,
    prompts: u32,
    sessions: u32,
}

impl Tally {
    fn add(&mut self, o: &Tally) {
        self.tokens += o.tokens;
        self.cost += o.cost;
        self.prompts += o.prompts;
        self.sessions += o.sessions;
    }

    fn get(&self, m: Metric) -> f64 {
        match m {
            Metric::Tokens => self.tokens as f64,
            Metric::Cost => self.cost,
            Metric::Prompts => self.prompts as f64,
            Metric::Sessions => self.sessions as f64,
        }
    }

    fn fmt(&self, m: Metric) -> String {
        match m {
            Metric::Tokens => format!("{} tokens", fmt_tokens(self.tokens)),
            Metric::Cost => fmt_cost(self.cost),
            Metric::Prompts => format!("{} prompt{}", fmt_int(self.prompts as u64), if self.prompts == 1 { "" } else { "s" }),
            Metric::Sessions => format!("{} session{}", fmt_int(self.sessions as u64), if self.sessions == 1 { "" } else { "s" }),
        }
    }
}

#[derive(Clone, Default, PartialEq)]
struct Day {
    all: Tally,
    by: BTreeMap<String, (AgentKind, Tally)>,
}

/// Every day's work, by harness, and tokens by model.
#[derive(Clone, Default, PartialEq)]
struct Calendar {
    days: HashMap<i32, Day>,
    models: HashMap<String, u64>,
    first: Option<i32>,
}

fn build(list: &[SessionSummary], map: &HashMap<String, SessionStats>, only: Option<&AgentKind>) -> Calendar {
    let mut c = Calendar::default();
    for s in list {
        if only.is_some_and(|a| *a != s.agent) {
            continue;
        }
        let Some(st) = map.get(&s.id) else { continue };
        let mut touched: HashSet<i32> = HashSet::new();
        let put = |day: i32, t: Tally, c: &mut Calendar| {
            let d = c.days.entry(day).or_default();
            d.all.add(&t);
            d.by.entry(s.agent.id().to_string()).or_insert_with(|| (s.agent.clone(), Tally::default())).1.add(&t);
            c.first = Some(c.first.map_or(day, |f| f.min(day)));
        };
        for u in &st.uses {
            put(u.day, Tally { tokens: u.input + u.output, cost: u.cost, ..Default::default() }, &mut c);
            *c.models.entry(u.model.clone()).or_default() += u.input + u.output;
            touched.insert(u.day);
        }
        for (day, _, _, n) in &st.prompts {
            put(*day, Tally { prompts: *n, ..Default::default() }, &mut c);
            touched.insert(*day);
        }
        for day in touched {
            put(day, Tally { sessions: 1, ..Default::default() }, &mut c);
        }
    }
    c
}

/// Runs of days with any work: (longest, the one reaching today or yesterday).
fn streaks(c: &Calendar, today: i32) -> (u32, u32) {
    let mut days: Vec<i32> = c.days.iter().filter(|(_, d)| d.all.prompts > 0 || d.all.tokens > 0).map(|(k, _)| *k).collect();
    days.sort_unstable();
    let (mut best, mut run, mut prev) = (0u32, 0u32, None::<i32>);
    for d in &days {
        run = if prev == Some(d - 1) { run + 1 } else { 1 };
        best = best.max(run);
        prev = Some(*d);
    }
    let set: HashSet<i32> = days.into_iter().collect();
    let mut cur = 0;
    let mut d = if set.contains(&today) { today } else { today - 1 };
    while set.contains(&d) {
        cur += 1;
        d -= 1;
    }
    (best, cur)
}

#[component]
pub fn ProfileView() -> Element {
    let mut app = use_app();
    let mut loaded: Signal<Option<std::sync::Arc<HashMap<String, SessionStats>>>> = use_signal(|| None);
    let mut metric = use_signal(|| Metric::Tokens);
    let mut harness = use_signal(|| "all".to_string());
    // None: the last twelve months; Some(year): that year.
    let mut year = use_signal(|| None::<i32>);
    let mut hover = use_signal(|| None::<(i32, usize, usize)>);

    // Read every session's usage once (cached on disk, like Plan & Usage).
    let key = {
        let l = app.sessions.read();
        (l.len(), l.iter().map(|s| s.updated_at.timestamp()).max().unwrap_or(0) / 20)
    };
    use_effect(use_reactive!(|key| {
        let _ = key;
        let list = app.sessions.peek().clone();
        spawn(async move {
            if let Ok(map) = tokio::task::spawn_blocking(move || stats::load_all(&list)).await {
                loaded.set(Some(map));
            }
        });
    }));

    let cfg = app.config.read().clone();
    let p = cfg.profile.clone();
    let name = display_name(&cfg);
    let list = app.sessions.read().clone();
    let today_date = Local::now().date_naive();
    let today = day_num(today_date);
    let map = loaded.read().clone();
    let only: Option<AgentKind> = list.iter().map(|s| s.agent.clone()).find(|a| a.id() == harness());
    let cal = map.as_ref().map(|m| build(&list, m, only.as_ref())).unwrap_or_default();
    let all_cal = if only.is_some() { map.as_ref().map(|m| build(&list, m, None)).unwrap_or_default() } else { cal.clone() };
    let harnesses: Vec<(String, String)> = {
        let mut seen: Vec<(String, String)> = Vec::new();
        for d in all_cal.days.values() {
            for (id, (k, _)) in &d.by {
                if !seen.iter().any(|(i, _)| i == id) {
                    seen.push((id.clone(), k.display_name().to_string()));
                }
            }
        }
        seen.sort_by(|a, b| a.1.cmp(&b.1));
        std::iter::once(("all".to_string(), "All harnesses".to_string())).chain(seen).collect()
    };
    let first_year = all_cal.first.map(|d| date_of(d).year()).unwrap_or(today_date.year());
    let years: Vec<i32> = (first_year..=today_date.year()).rev().collect();

    // The weeks shown, Monday first.
    let (start, end) = match year() {
        Some(y) => (NaiveDate::from_ymd_opt(y, 1, 1).unwrap_or(today_date), NaiveDate::from_ymd_opt(y, 12, 31).unwrap_or(today_date).min(today_date)),
        None => (today_date - Duration::days(364), today_date),
    };
    let grid_start = start - Duration::days(start.weekday().num_days_from_monday() as i64);
    let grid_end = match year() {
        Some(y) => NaiveDate::from_ymd_opt(y, 12, 31).unwrap_or(end),
        None => end,
    };
    let weeks = ((grid_end - grid_start).num_days() / 7 + 1) as usize;
    let m = metric();
    let in_range = |d: i32| d >= day_num(start) && d <= day_num(end);
    let mut values: Vec<f64> = cal.days.iter().filter(|(d, _)| in_range(**d)).map(|(_, v)| v.all.get(m)).filter(|v| *v > 0.0).collect();
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let q = |f: f64| values.get(((values.len() as f64 - 1.0) * f).round().max(0.0) as usize).copied().unwrap_or(0.0);
    let (q1, q2, q3) = (q(0.25), q(0.5), q(0.75));
    let level = |v: f64| if v <= 0.0 { 0 } else if v <= q1 { 1 } else if v <= q2 { 2 } else if v <= q3 { 3 } else { 4 };

    // Totals for the days shown.
    let mut total = Tally::default();
    let mut sessions_seen: HashSet<String> = HashSet::new();
    let mut active = 0u32;
    let mut busiest: Option<(i32, f64)> = None;
    let mut by_harness: BTreeMap<String, (AgentKind, Tally)> = BTreeMap::new();
    for (d, day) in cal.days.iter().filter(|(d, _)| in_range(**d)) {
        let mut t = day.all.clone();
        t.sessions = 0;
        total.add(&t);
        if day.all.prompts > 0 || day.all.tokens > 0 {
            active += 1;
        }
        let v = day.all.get(m);
        if v > 0.0 && busiest.map_or(true, |(_, b)| v > b) {
            busiest = Some((*d, v));
        }
        for (id, (k, t)) in &day.by {
            by_harness.entry(id.clone()).or_insert_with(|| (k.clone(), Tally::default())).1.add(t);
        }
    }
    if let Some(m) = &map {
        for s in &list {
            if only.as_ref().is_some_and(|a| *a != s.agent) {
                continue;
            }
            if m.get(&s.id).is_some_and(|st| st.uses.iter().any(|u| in_range(u.day)) || st.prompts.iter().any(|p| in_range(p.0))) {
                sessions_seen.insert(s.id.clone());
            }
        }
    }
    total.sessions = sessions_seen.len() as u32;
    let (longest, current) = streaks(&cal, today);
    let mut harness_rows: Vec<(AgentKind, Tally)> = by_harness.into_values().collect();
    harness_rows.sort_by(|a, b| b.1.get(m).partial_cmp(&a.1.get(m)).unwrap_or(std::cmp::Ordering::Equal));
    let harness_max = harness_rows.first().map(|r| r.1.get(m)).unwrap_or(0.0).max(1e-9);
    let mut models: Vec<(String, u64)> = cal.models.iter().map(|(k, v)| (k.clone(), *v)).filter(|(k, v)| *v > 0 && !k.is_empty()).collect();
    models.sort_by(|a, b| b.1.cmp(&a.1));
    models.truncate(5);
    let model_max = models.first().map(|r| r.1).unwrap_or(1).max(1) as f64;
    let since = list.iter().map(|s| s.created_at).min().map(|t| t.with_timezone(&Local).format("%B %Y").to_string());

    // Month names over the weeks they start in.
    let mut months: Vec<(usize, String)> = Vec::new();
    for w in 0..weeks {
        let d = grid_start + Duration::days(w as i64 * 7);
        let label = d.format("%b").to_string();
        let first_of = (0..7).map(|i| d + Duration::days(i)).find(|x| x.day() == 1 && *x >= start && *x <= grid_end);
        if w == 0 || first_of.is_some() {
            let shown = first_of.map(|x| x.format("%b").to_string()).unwrap_or(label);
            if months.last().map(|(_, l)| l != &shown).unwrap_or(true) {
                months.push((w, shown));
            }
        }
    }
    let busy = map.is_none();

    let pick_photo = move |_| {
        spawn(async move {
            let Some(f) = rfd::AsyncFileDialog::new().add_filter("Pictures", &["png", "jpg", "jpeg", "gif", "webp"]).pick_file().await else { return };
            let old = app.config.peek().profile.photo.clone();
            match keep_photo(f.path(), old.as_deref(), &photo_dir()) {
                Ok(path) => app.update_config(|c| c.profile.photo = Some(path)),
                Err(e) => app.toast_err(e),
            }
        });
    };
    let field = move |get: fn(&mut larrier_core::Profile) -> &mut Option<String>, v: String| {
        let mut app = app;
        *get(&mut app.config.write().profile) = Some(v).filter(|s| !s.trim().is_empty());
    };

    rsx! {
        style { {CSS} }
        h1 { "Profile" }
        div { class: "sub", "How you show up in Larrier, and your work with AI over time." }

        div { class: "set-card pf-card",
            div { class: "pf-photo",
                button { class: "pf-avatar", title: "Change your photo", onclick: pick_photo,
                    Avatar { size: 84 }
                    span { class: "pf-cam", Icon { i: I::Camera, size: 16 } }
                }
                if p.photo.is_some() {
                    button { class: "pf-link", onclick: move |_| {
                            let old = app.config.peek().profile.photo.clone();
                            if let Some(o) = old.filter(|o| std::path::Path::new(o).starts_with(photo_dir())) {
                                let _ = std::fs::remove_file(o);
                            }
                            app.update_config(|c| c.profile.photo = None);
                        },
                        "Remove"
                    }
                } else {
                    button { class: "pf-link", onclick: pick_photo, "Upload a photo" }
                }
            }
            div { class: "pf-fields",
                label { class: "pf-f",
                    span { "Name" }
                    input { class: "input", placeholder: "{crate::state::user_name()}", value: "{p.name.clone().unwrap_or_default()}",
                        oninput: move |e| field(|p| &mut p.name, e.value()), onchange: move |_| app.save_config() }
                }
                label { class: "pf-f",
                    span { "Role" }
                    input { class: "input", placeholder: "What you do", value: "{p.role.clone().unwrap_or_default()}",
                        oninput: move |e| field(|p| &mut p.role, e.value()), onchange: move |_| app.save_config() }
                }
                label { class: "pf-f",
                    span { "Location" }
                    input { class: "input", placeholder: "City, country", value: "{p.location.clone().unwrap_or_default()}",
                        oninput: move |e| field(|p| &mut p.location, e.value()), onchange: move |_| app.save_config() }
                }
                label { class: "pf-f",
                    span { "Website" }
                    input { class: "input", placeholder: "https://…", value: "{p.website.clone().unwrap_or_default()}", spellcheck: "false",
                        oninput: move |e| field(|p| &mut p.website, e.value()), onchange: move |_| app.save_config() }
                }
                label { class: "pf-f wide",
                    span { "About" }
                    textarea { class: "input", rows: "3", dir: "auto", placeholder: "A few words about yourself and what you build", value: "{p.about}",
                        oninput: move |e| app.config.write().profile.about = e.value(), onchange: move |_| app.save_config() }
                }
            }
        }
        div { class: "pf-who",
            span { class: "pf-name", "{name}" }
            if let Some(r) = p.role.clone() { span { class: "pf-meta", Icon { i: I::Briefcase, size: 12 } "{r}" } }
            if let Some(l) = p.location.clone() { span { class: "pf-meta", Icon { i: I::MapPin, size: 12 } "{l}" } }
            if let Some(w) = p.website.clone() {
                button { class: "pf-meta pf-web", onclick: move |_| { let _ = open::that(&w); }, Icon { i: I::Link, size: 12 } {p.website.clone().unwrap_or_default().trim_start_matches("https://").trim_start_matches("http://").to_string()} }
            }
            if let Some(s) = since { span { class: "pf-meta", Icon { i: I::CalendarDays, size: 12 } "With Larrier since {s}" } }
        }

        div { class: "pf-head",
            h3 { "Your AI work" }
            div { class: "grow" }
            crate::ui::Select { value: harness(), options: harnesses.clone(), onchange: move |v: String| harness.set(v) }
            div { class: "seg",
                for mm in Metric::ALL {
                    button { key: "{mm.label()}", class: if mm == m { "on" } else { "" }, onclick: move |_| metric.set(mm), "{mm.label()}" }
                }
            }
        }
        div { class: "pf-kpis",
            Kpi { label: "Tokens", value: fmt_tokens(total.tokens) }
            Kpi { label: "Cost", value: fmt_cost(total.cost) }
            Kpi { label: "Prompts", value: fmt_int(total.prompts as u64) }
            Kpi { label: "Sessions", value: fmt_int(total.sessions as u64) }
            Kpi { label: "Active days", value: fmt_int(active as u64) }
            Kpi { label: "Current streak", value: format!("{current} day{}", if current == 1 { "" } else { "s" }), icon: Some(I::Flame) }
            Kpi { label: "Longest streak", value: format!("{longest} day{}", if longest == 1 { "" } else { "s" }), icon: Some(I::Trophy) }
        }
        div { class: "set-card pf-cal-card",
            div { class: "pf-years",
                button { class: if year().is_none() { "on" } else { "" }, onclick: move |_| year.set(None), "Last 12 months" }
                for y in years.clone() {
                    button { key: "{y}", class: if year() == Some(y) { "on" } else { "" }, onclick: move |_| year.set(Some(y)), "{y}" }
                }
            }
            if busy {
                div { class: "pf-empty", super::loaders::Orbit { size: 13 } "Reading your sessions…" }
            } else {
                div { class: "pf-cal", style: "--weeks: {weeks};",
                    div { class: "pf-months",
                        for (w, label) in months.clone() {
                            span { key: "{w}", style: "grid-column: {w + 1};", "{label}" }
                        }
                    }
                    div { class: "pf-wd", span { "Mon" } span {} span { "Wed" } span {} span { "Fri" } span {} span {} }
                    div { class: "pf-grid", onmouseleave: move |_| hover.set(None),
                        for w in 0..weeks {
                            for r in 0..7usize {
                                {
                                    let date = grid_start + Duration::days((w * 7 + r) as i64);
                                    let d = day_num(date);
                                    let shown = date >= start && date <= end;
                                    let v = cal.days.get(&d).map(|x| x.all.get(m)).unwrap_or(0.0);
                                    let lv = if shown { level(v) } else { -1 };
                                    let cls = if lv < 0 { "pf-cell out".to_string() } else { format!("pf-cell l{lv}") };
                                    rsx! {
                                        div { key: "{d}", class: "{cls}", style: "grid-column: {w + 1}; grid-row: {r + 1};",
                                            onmouseenter: move |_| if shown { hover.set(Some((d, w, r))) },
                                        }
                                    }
                                }
                            }
                        }
                        if let Some((d, w, r)) = hover() {
                            {
                                let day = cal.days.get(&d).cloned().unwrap_or_default();
                                let mut parts: Vec<(AgentKind, Tally)> = day.by.values().cloned().collect();
                                parts.sort_by(|a, b| b.1.get(m).partial_cmp(&a.1.get(m)).unwrap_or(std::cmp::Ordering::Equal));
                                let left = (w as f64 + 0.5) / weeks as f64 * 100.0;
                                let flip = w as f64 > weeks as f64 * 0.7;
                                rsx! {
                                    div { class: if flip { "pf-tip flip" } else { "pf-tip" }, style: "left: {left:.2}%; top: calc({r} * (100% / 7));",
                                        div { class: "t", {date_of(d).format("%A, %B %-d, %Y").to_string()} }
                                        if day.all == Tally::default() {
                                            div { class: "none", "No AI work" }
                                        } else {
                                            div { class: "big", {day.all.fmt(m)} }
                                            div { class: "row", {Metric::ALL.iter().filter(|x| **x != m).map(|x| day.all.fmt(*x)).collect::<Vec<_>>().join(" · ")} }
                                            for (k, t) in parts.into_iter().take(4) {
                                                div { key: "{k.id()}", class: "h",
                                                    span { class: "lg", if let Some(svg) = agent_logo(&k) { Logo { svg, size: 12 } } }
                                                    span { class: "nm", "{k.display_name()}" }
                                                    span { class: "v", {t.fmt(m)} }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                div { class: "pf-legend",
                    if let Some((d, v)) = busiest {
                        span { class: "pf-busiest", "Busiest: {date_of(d).format(\"%b %-d\")} · " {Tally { tokens: v as u64, cost: v, prompts: v as u32, sessions: v as u32 }.fmt(m)} }
                    }
                    div { class: "grow" }
                    "Less"
                    for l in 0..5 { span { key: "{l}", class: "pf-cell l{l}" } }
                    "More"
                }
            }
        }
        div { class: "pf-split",
            div { class: "set-card pf-list",
                div { class: "pf-list-head", "By harness" }
                if harness_rows.is_empty() { div { class: "pf-empty", "Nothing in this period" } }
                for (k, t) in harness_rows.clone() {
                    div { key: "{k.id()}", class: "pf-bar",
                        span { class: "lg", if let Some(svg) = agent_logo(&k) { Logo { svg, size: 14 } } }
                        span { class: "nm", "{k.display_name()}" }
                        div { class: "track", div { class: "fill", style: "width: {t.get(m) / harness_max * 100.0:.1}%;" } }
                        span { class: "v", {t.fmt(m)} }
                    }
                }
            }
            div { class: "set-card pf-list",
                div { class: "pf-list-head", "Top models · tokens" }
                if models.is_empty() { div { class: "pf-empty", "Nothing in this period" } }
                for (name, n) in models.clone() {
                    div { key: "{name}", class: "pf-bar",
                        span { class: "nm", title: "{name}", "{name}" }
                        div { class: "track", div { class: "fill", style: "width: {n as f64 / model_max * 100.0:.1}%;" } }
                        span { class: "v", "{fmt_tokens(n)}" }
                    }
                }
            }
        }
    }
}

#[component]
fn Kpi(label: &'static str, value: String, #[props(default)] icon: Option<I>) -> Element {
    rsx! {
        div { class: "pf-kpi",
            span { class: "k", if let Some(i) = icon { Icon { i, size: 12 } } "{label}" }
            span { class: "v", "{value}" }
        }
    }
}

const CSS: &str = r#"
.pf-card { display: flex; gap: 22px; padding: 18px; align-items: flex-start; }
.pf-photo { display: flex; flex-direction: column; align-items: center; gap: 8px; flex-shrink: 0; }
.pf-avatar { position: relative; border-radius: 50%; }
.pf-avatar .avatar img, .avatar img { width: 100%; height: 100%; object-fit: cover; border-radius: 50%; display: block; }
.pf-cam { position: absolute; inset: 0; border-radius: 50%; display: flex; align-items: center; justify-content: center; color: #fff; background: rgba(0, 0, 0, .38); opacity: 0; transition: opacity .15s ease; }
.pf-avatar:hover .pf-cam { opacity: 1; }
.pf-link { font-size: 12px; color: var(--fg-3); }
.pf-link:hover { color: var(--fg); }
.pf-fields { flex: 1; min-width: 0; display: grid; grid-template-columns: 1fr 1fr; gap: 10px 14px; }
.pf-f { display: flex; flex-direction: column; gap: 4px; min-width: 0; font-size: 12px; color: var(--fg-3); }
.pf-f.wide { grid-column: 1 / -1; }
.pf-f .input { width: 100%; color: var(--fg); }
.pf-f textarea.input { height: auto; min-height: 64px; resize: vertical; padding: 8px 10px; line-height: 18px; font: inherit; font-size: 13px; color: var(--fg); }
.pf-who { display: flex; flex-wrap: wrap; align-items: center; gap: 6px 14px; margin: 12px 2px 0; }
.pf-name { font-size: 15px; font-weight: 600; color: var(--fg); }
.pf-meta { display: inline-flex; align-items: center; gap: 5px; font-size: 12.5px; color: var(--fg-3); }
.pf-web:hover { color: var(--fg); text-decoration: underline; }
.pf-head { display: flex; align-items: center; gap: 10px; margin: 26px 0 10px; }
.pf-head h3 { margin: 0; font-size: 14px; }
.pf-head .seg { display: inline-flex; gap: 2px; padding: 2px; border-radius: 8px; background: var(--chip); }
.pf-head .seg button { height: 24px; padding: 0 10px; border-radius: 6px; font-size: 12px; color: var(--fg-2); }
.pf-head .seg button.on { background: var(--card); color: var(--fg); box-shadow: 0 1px 2px rgba(0, 0, 0, .08); }
.pf-kpis { display: grid; grid-template-columns: repeat(4, minmax(0, 1fr)); gap: 8px; margin-bottom: 10px; }
.pf-kpi { display: flex; flex-direction: column; gap: 3px; padding: 10px 12px; border-radius: 10px; border: 1px solid var(--border); background: var(--card); }
.pf-kpi .k { display: inline-flex; align-items: center; gap: 5px; font-size: 11.5px; color: var(--fg-3); }
.pf-kpi .v { font-size: 16px; font-weight: 600; color: var(--fg); font-variant-numeric: tabular-nums; }
.pf-cal-card { padding: 14px 16px 12px; }
.pf-years { display: flex; flex-wrap: wrap; gap: 4px; margin-bottom: 12px; }
.pf-years button { height: 24px; padding: 0 10px; border-radius: 6px; font-size: 12px; color: var(--fg-3); }
.pf-years button:hover { color: var(--fg); background: var(--hover); }
.pf-years button.on { color: var(--fg); background: var(--selected); }
.pf-cal { display: grid; grid-template-columns: 28px minmax(0, 1fr); grid-template-rows: auto auto; gap: 4px 6px; }
.pf-months { grid-column: 2; display: grid; grid-template-columns: repeat(var(--weeks), minmax(0, 1fr)); font-size: 10.5px; color: var(--fg-3); height: 14px; overflow: visible; }
.pf-months span { white-space: nowrap; }
.pf-wd { grid-column: 1; grid-row: 2; display: grid; grid-template-rows: repeat(7, 1fr); font-size: 10px; color: var(--fg-3); align-items: center; }
.pf-grid { grid-column: 2; grid-row: 2; position: relative; display: grid; grid-template-columns: repeat(var(--weeks), minmax(0, 1fr)); grid-template-rows: repeat(7, auto); gap: 3px; min-width: 0; }
.pf-cell { aspect-ratio: 1; min-width: 0; border-radius: 2px; background: var(--chip); }
.pf-cell.out { background: transparent; }
.pf-cell.l1 { background: color-mix(in srgb, var(--green) 28%, var(--chip)); }
.pf-cell.l2 { background: color-mix(in srgb, var(--green) 50%, var(--chip)); }
.pf-cell.l3 { background: color-mix(in srgb, var(--green) 74%, var(--chip)); }
.pf-cell.l4 { background: var(--green); }
.pf-grid .pf-cell:not(.out):hover { outline: 1.5px solid var(--fg-2); outline-offset: 1px; }
.pf-tip { position: absolute; z-index: 5; transform: translate(-50%, calc(-100% - 8px)); width: max-content; max-width: 260px; padding: 9px 11px; border-radius: 10px; background: var(--card-2); border: 1px solid var(--border);
  box-shadow: 0 8px 24px rgba(0, 0, 0, .14); pointer-events: none; font-size: 12px; color: var(--fg-2); display: flex; flex-direction: column; gap: 3px; }
.pf-tip.flip { transform: translate(calc(-100% + 8px), calc(-100% - 8px)); }
.pf-tip .t { font-weight: 600; color: var(--fg); }
.pf-tip .big { font-size: 13px; color: var(--fg); }
.pf-tip .row, .pf-tip .none { color: var(--fg-3); }
.pf-tip .h { display: flex; align-items: center; gap: 6px; }
.pf-tip .h .nm { flex: 1; }
.pf-tip .h .v { color: var(--fg-3); font-variant-numeric: tabular-nums; }
.pf-legend { display: flex; align-items: center; gap: 4px; margin-top: 10px; font-size: 11px; color: var(--fg-3); }
.pf-legend .pf-cell { width: 11px; height: 11px; aspect-ratio: auto; display: inline-block; }
.pf-busiest { color: var(--fg-3); }
.pf-split { display: grid; grid-template-columns: 1fr 1fr; gap: 10px; margin-top: 10px; }
.pf-list { padding: 12px 14px; display: flex; flex-direction: column; gap: 8px; }
.pf-list-head { font-size: 12px; font-weight: 600; color: var(--fg-2); margin-bottom: 2px; }
.pf-bar { display: grid; grid-template-columns: auto minmax(70px, 120px) 1fr auto; align-items: center; gap: 8px; font-size: 12.5px; }
.pf-bar:not(:has(.lg)) { grid-template-columns: minmax(90px, 150px) 1fr auto; }
.pf-bar .nm { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; color: var(--fg); }
.pf-bar .track { height: 6px; border-radius: 3px; background: var(--chip); overflow: hidden; }
.pf-bar .fill { height: 100%; border-radius: 3px; background: var(--green); }
.pf-bar .v { color: var(--fg-3); font-variant-numeric: tabular-nums; font-size: 12px; }
.pf-empty { display: flex; align-items: center; gap: 8px; padding: 10px 0; color: var(--fg-3); font-size: 12px; }
@media (max-width: 760px) { .pf-fields, .pf-split { grid-template-columns: 1fr; } .pf-card { flex-direction: column; align-items: center; } .pf-kpis { grid-template-columns: repeat(2, minmax(0, 1fr)); } }
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use super::stats::Use;

    fn session(agent: AgentKind) -> SessionSummary {
        larrier_core::Session::new(agent, None).summary()
    }

    #[test]
    fn days_add_up_by_harness_and_streaks_count() {
        let (a, b) = (session(AgentKind::ClaudeCode), session(AgentKind::Codex));
        let today = day_num(chrono::Local::now().date_naive());
        let mut map = HashMap::new();
        map.insert(a.id.clone(), SessionStats {
            uses: vec![Use { day: today, model: "opus".into(), input: 100, output: 50, cost: 0.5, ..Default::default() }, Use { day: today - 1, model: "opus".into(), input: 10, output: 0, ..Default::default() }],
            prompts: vec![(today, 10, 0, 3)],
            ..Default::default()
        });
        map.insert(b.id.clone(), SessionStats { uses: vec![Use { day: today, model: "gpt".into(), input: 40, output: 10, ..Default::default() }], prompts: vec![(today - 5, 9, 0, 1)], ..Default::default() });
        let list = vec![a, b];
        let c = build(&list, &map, None);
        let d = &c.days[&today];
        assert_eq!((d.all.tokens, d.all.prompts, d.all.sessions), (200, 3, 2));
        assert!((d.all.cost - 0.5).abs() < 1e-9);
        assert_eq!(d.by.len(), 2);
        assert_eq!(c.models["opus"], 160);
        // Today and yesterday in a row; the lone day five back is its own run.
        assert_eq!(streaks(&c, today), (2, 2));
        // One harness only.
        let codex = build(&list, &map, Some(&AgentKind::Codex));
        assert_eq!(codex.days[&today].all.tokens, 50);
        assert_eq!(codex.first, Some(today - 5));
    }

    #[test]
    fn a_photo_is_kept_in_larriers_folder() {
        let root = std::env::temp_dir().join(format!("larrier-photo-{}", std::process::id()));
        let dir = root.join("profile");
        std::fs::create_dir_all(&root).unwrap();
        let pic = root.join("me.PNG");
        std::fs::write(&pic, b"\x89PNG fake").unwrap();
        let kept = keep_photo(&pic, None, &dir).unwrap();
        assert!(kept.starts_with(&dir.to_string_lossy().to_string()) && kept.ends_with(".png"), "{kept}");
        let again = keep_photo(&pic, Some(&kept), &dir).unwrap();
        assert!(!std::path::Path::new(&kept).exists(), "the old photo goes");
        assert!(std::path::Path::new(&again).exists());
        assert!(keep_photo(&root.join("notes.txt"), None, &dir).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }
}

