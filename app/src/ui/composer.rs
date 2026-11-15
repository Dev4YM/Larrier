//! The prompt composer: a pill with a "+" menu, the input, one agent +
//! model picker (brand logos) and send / stop, with the branch and
//! environment pickers underneath.

use std::sync::{Mutex, OnceLock};

use dioxus::html::HasFileData;
use dioxus::prelude::*;
use larrier_agents::harness::{HarnessItem, ItemKind};
use larrier_core::{lsf::short_path, AgentKind, BusySend, Part, Protocol};

use super::mentions::MentionChip;
use super::{Dropdown, MenuItem};
use crate::icons::{Icon, I};
use crate::logos::{agent_logo, model_logo, Logo};
use crate::state::{use_app, App, SettingsTab};

/// Model choices of Larrier Chat (each API account's models, as
/// `account/model`) and custom agents.
fn listed_models(app: &App, agent: &AgentKind) -> Option<Vec<String>> {
    let cfg = app.config.read();
    if agent.spec().protocol == Protocol::HttpChat {
        return Some(
            cfg.accounts_for(agent)
                .into_iter()
                .filter_map(|a| match &a.auth {
                    larrier_core::AccountAuth::Api(api) => Some(api.models.iter().map(move |m| format!("{}/{}", a.id, m))),
                    _ => None,
                })
                .flatten()
                .collect(),
        );
    }
    match agent {
        AgentKind::Custom(id) => Some(cfg.custom_agent(id).map(|c| c.models.clone()).unwrap_or_default()),
        _ => None,
    }
}

/// One option of an agent mode group, as the composer shows it.
#[derive(Clone, PartialEq)]
pub struct ModeOptView {
    pub id: &'static str,
    pub label: &'static str,
    pub description: &'static str,
}

/// An agent mode group ("Mode", "Permissions", …) with the chosen option and
/// the one used when nothing is picked.
#[derive(Clone, PartialEq)]
pub struct ModeGroupView {
    pub label: &'static str,
    pub options: Vec<ModeOptView>,
    pub chosen: usize,
    pub default: usize,
}

/// The agent's own mode groups (Claude Code permission modes, Codex plan +
/// permissions, …) with the current picks resolved against the agent's
/// default and the global autonomy level.
pub fn mode_view(app: &App, agent: &AgentKind, picks: [Option<String>; 2]) -> Vec<ModeGroupView> {
    let spec = agent.spec();
    let cfg = app.config.read();
    let ac = cfg.agent(agent);
    let defaults = [ac.default_mode.clone(), ac.default_permission.clone()];
    let run_mode = cfg.settings.run_mode;
    spec.mode_groups
        .iter()
        .enumerate()
        .map(|(gi, g)| {
            let pos = |id: Option<&str>| id.and_then(|id| g.options.iter().position(|o| o.id == id)).unwrap_or(0);
            let default = pos(spec.mode_option(gi, None, defaults.get(gi).and_then(|d| d.as_deref()), run_mode).map(|o| o.id));
            let chosen = pos(spec.mode_option(gi, picks.get(gi).and_then(|p| p.as_deref()), defaults.get(gi).and_then(|d| d.as_deref()), run_mode).map(|o| o.id));
            ModeGroupView {
                label: g.label,
                options: g.options.iter().map(|o| ModeOptView { id: o.id, label: o.label, description: o.description }).collect(),
                chosen,
                default,
            }
        })
        .collect()
}

/// The value stored for picking option `oi` of a group: `None` for the
/// group's default, so the session keeps following the agent's settings.
fn pick_value(g: &ModeGroupView, oi: usize) -> Option<String> {
    if oi == g.default {
        None
    } else {
        g.options.get(oi).map(|o| o.id.to_string())
    }
}

/// The plan-like option of the first group (Plan, or Droid's Spec).
fn plan_option(agent: &AgentKind) -> Option<&'static str> {
    agent.spec().mode_groups.first().and_then(|g| g.find("plan")).map(|o| o.id)
}

/// Model and effort for the composer button ("Opus 5.5", "Extra high"):
/// the session's picks, else the defaults — the agent's own when Larrier
/// has none, so it reads what will really run.
fn model_display(app: &App, agent: &AgentKind, model: &Option<String>, effort: &Option<String>) -> (String, String) {
    use crate::agent_models as am;
    if agent.spec().protocol == Protocol::HttpChat {
        let name = match model {
            Some(m) => larrier_core::models::lookup(m.split_once('/').map(|(_, b)| b).unwrap_or(m)).map(|i| i.short_name()).unwrap_or_else(|| larrier_core::models::pretty_name(m)),
            None => "Auto".into(),
        };
        return (name, String::new());
    }
    let r = am::resolve(app, agent, model.as_deref(), effort.as_deref());
    let name = match (&r.family, &r.model) {
        (Some(f), Some(m)) if f != m => am::short_name(app, agent, f),
        (_, Some(m)) => am::short_name(app, agent, m),
        _ => "Auto".into(),
    };
    let mut extra: Vec<String> = Vec::new();
    if let (Some(ctx), Some(fam)) = (&r.context, r.family.as_deref()) {
        let params = am::choices(app, agent).into_iter().find(|c| c.id == fam).and_then(|c| c.params);
        if let Some(p) = params.filter(|p| p.default_context.as_deref() != Some(ctx.as_str())) {
            extra.push(p.contexts.iter().find(|(v, _)| v == ctx).map(|(_, l)| l.clone()).unwrap_or_else(|| ctx.to_uppercase()));
        }
    }
    extra.extend(r.effort.as_deref().map(larrier_agents::models::effort_label));
    if r.fast {
        extra.push("Fast".into());
    }
    (name, extra.join(" · "))
}

/// How a model choice reads in the picker: (label, note, "1M · $4 / $20",
/// tooltip).
fn model_row_text(m: &Option<String>) -> (String, String, String, String) {
    use larrier_core::models::{self, money, tokens_short};
    let Some(id) = m.as_deref() else {
        return ("Auto".into(), "Agent's default".into(), String::new(), "Let the agent use its default model".into());
    };
    let info = models::lookup(id);
    let meta = info
        .as_ref()
        .map(|i| {
            let mut b = Vec::new();
            if let Some(c) = i.context {
                b.push(tokens_short(c));
            }
            if let Some(p) = &i.price {
                b.push(p.short());
            }
            b.join(" · ")
        })
        .unwrap_or_default();
    let (label, note) = match models::alias(id) {
        Some((_, l)) => (l.to_string(), info.as_ref().map(|i| format!("latest · {}", i.short_name())).unwrap_or_else(|| "latest".into())),
        None => (models::display_name(id), info.as_ref().and_then(|i| i.tag.clone()).unwrap_or_default()),
    };
    let tip = match &info {
        Some(i) => {
            let mut t = format!("{id}\n{}", i.vendor.name());
            if let Some(c) = i.context {
                t.push_str(&format!(" · {} context", tokens_short(c)));
            }
            if let Some(o) = i.max_output {
                t.push_str(&format!(" · {} output", tokens_short(o)));
            }
            if let Some(p) = &i.price {
                t.push_str(&format!("\nInput {} · Output {} per 1M tokens", money(p.input), money(p.output)));
                if let Some(c) = p.cache_read {
                    t.push_str(&format!(" · Cache read {}", money(c)));
                }
            }
            t
        }
        None => id.to_string(),
    };
    (label, note, meta, tip)
}

const TIPS: &[&str] = &[
    "Type @ to mention a file, / for commands",
    "Continue any session with another agent from the model picker",
    "Ctrl+K searches every agent and chat you have",
    "Import ChatGPT and Claude.ai exports from File → Import File",
    "Multitask runs one prompt on several agents in parallel",
    "Right-click anything for Larrier's menu",
    "Every session is a portable .lsf file — export it from the … menu",
];

/// What the composer needs from its session (memoized so a streaming
/// reply doesn't re-render the composer).
#[derive(Clone, PartialEq)]
struct SessInfo {
    agent: AgentKind,
    model: Option<String>,
    mode: Option<String>,
    permission: Option<String>,
    effort: Option<String>,
    worktree: bool,
    /// The worktree it runs in, when that was there before the chat.
    existing: Option<String>,
    usage: larrier_core::Usage,
    /// Context in use and the window, as last reported.
    context: Option<(u64, Option<u64>)>,
    cost: Option<f64>,
    messages: usize,
    cwd: Option<String>,
    account: Option<String>,
    run_as: Option<String>,
    /// Nothing sent yet: who the chat runs as can still change.
    fresh: bool,
}

#[component]
pub fn Composer(
    session_id: Option<String>,
    #[props(default = false)] home: bool,
    /// DOM id of the text box (a split view has two composers).
    #[props(default = "composer-input")] input_id: &'static str,
) -> Element {
    let mut app = use_app();
    // What was typed here last time (this chat's, or the new agent
    // screen's), saved as it changes; the box is keyed by its chat.
    let keep_key = crate::keep::key(session_id.as_deref());
    let kept = use_hook({
        let k = keep_key.clone();
        move || crate::keep::input(&k)
    });
    let mut text = use_signal(|| kept.text.clone());
    // Bumped whenever Larrier itself changes the text (not the user typing):
    // the box takes those writes, and drops older copies of what was typed
    // that reach it after the user moved on (they'd throw the caret to the end).
    let put_gen = use_signal(|| 0u64);
    let attachments: Signal<Vec<Part>> = use_signal(|| kept.attachments.clone());
    // The card open above the box (the queue, running agents or terminals),
    // kept with the chat; the queued row being dragged and where it would
    // land, and the queued message being edited with the draft it set aside.
    let mut dock = super::use_kept(session_id.as_deref().unwrap_or_default(), "dock", || None::<super::tasks::DockCard>);
    let mut q_drag = use_signal(|| None::<usize>);
    let mut q_over = use_signal(|| None::<usize>);
    let q_edit = use_signal(|| None::<(u64, String, Vec<Part>, Vec<MentionChip>)>);
    let mut send_menu = use_signal(|| false);
    // Files, folders, chats, the diff, skills and subagents the text refers
    // to: each sits in the text as a marked token (see `mentions`).
    let mut mentions: Signal<Vec<MentionChip>> = use_signal(|| kept.mentions.clone());
    // While a queued message is being edited, the message set aside is
    // what's kept.
    use_effect(move || {
        let input = match &*q_edit.read() {
            Some((_, t, a, m)) => crate::keep::Input::new(t.clone(), a.clone(), m.clone()),
            None => crate::keep::Input::new(text.read().clone(), attachments.read().clone(), mentions.read().clone()),
        };
        crate::keep::set_input(&keep_key, input);
    });
    let plus_menu = use_signal(|| false);
    let mut model_menu = use_signal(|| false);
    let env_menu = use_signal(|| false);
    let mut ws_menu = use_signal(|| false);
    let mut pj_query = use_signal(String::new);
    let mut multi_menu = use_signal(|| false);
    let mut model_query = use_signal(String::new);
    let mut multi: Signal<Vec<AgentKind>> = use_signal(Vec::new);
    let tip_idx = use_hook(|| (chrono::Utc::now().timestamp() as usize) % TIPS.len());
    let mut hl = use_signal(|| 0usize);
    let mut dismissed = use_signal(|| None::<String>);
    let mut dragging = use_signal(|| false);
    let mut overflow = use_signal(|| false);
    // Only the newest width measurement counts (older ones can land late).
    let mut measure_seq = use_signal(|| 0u64);

    let primary = input_id == "composer-input";
    // Pick up attachments produced elsewhere (paste, drop, menus).
    let inbox_len = crate::state::use_ui(|u| u.attach_inbox.len())();
    use_effect(use_reactive!(|inbox_len| {
        if inbox_len > 0 && primary {
            let mut app = app;
            let items = std::mem::take(&mut app.ui.write().attach_inbox);
            let mut at = attachments;
            at.write().extend(items);
        }
    }));
    // Objects dropped in for this composer (a dragged browser page), and
    // their pictures.
    let chip_len = crate::state::use_ui(|u| u.chip_inbox.len())();
    let sid_chip = session_id.clone();
    use_effect(use_reactive!(|chip_len| {
        if chip_len == 0 {
            return;
        }
        let mut app = app;
        let mut text = text;
        let mine: Vec<(MentionChip, Option<larrier_core::Part>)> = {
            let mut ui = app.ui.write();
            let (m, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut ui.chip_inbox).into_iter().partition(|(sid, _, _)| *sid == sid_chip);
            ui.chip_inbox = rest;
            m.into_iter().map(|(_, c, p)| (c, p)).collect()
        };
        let mut add: Vec<String> = Vec::new();
        for (chip, picture) in mine {
            let chip = super::mentions::unique(&mentions.read(), chip);
            add.push(chip.marked());
            mentions.write().push(chip);
            if let Some(p) = picture {
                let mut at = attachments;
                at.write().push(p);
            }
        }
        if !add.is_empty() {
            let cur = text.read().clone();
            let sep = if cur.is_empty() || cur.ends_with(' ') || cur.ends_with('\n') { "" } else { " " };
            text.set(format!("{cur}{sep}{} ", add.join(" "))); bump(put_gen);
            app.ui.write().focus_composer += 1;
        }
    }));
    // Text dropped in for this composer (a dragged chat added as context).
    let compose_len = crate::state::use_ui(|u| u.compose_inbox.len())();
    let sid_in = session_id.clone();
    use_effect(use_reactive!(|compose_len| {
        if compose_len == 0 {
            return;
        }
        let mut app = app;
        let mut text = text;
        let mine: Vec<String> = {
            let mut ui = app.ui.write();
            let (m, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut ui.compose_inbox).into_iter().partition(|(sid, _)| *sid == sid_in);
            ui.compose_inbox = rest;
            m.into_iter().map(|(_, t)| t).collect()
        };
        if !mine.is_empty() {
            let mut add: Vec<String> = Vec::new();
            for t in mine {
                let found = t
                    .trim()
                    .strip_prefix("@agent:")
                    .and_then(|short| app.sessions.read().iter().find(|s| s.id.ends_with(short)).map(MentionChip::agent));
                match found {
                    Some(chip) => {
                        let have = mentions.read().iter().find(|c| c.kind == chip.kind && c.value == chip.value).cloned();
                        let chip = have.unwrap_or_else(|| super::mentions::unique(&mentions.read(), chip));
                        if !text.read().contains(&chip.marked()) {
                            add.push(chip.marked());
                            mentions.write().push(chip);
                        }
                    }
                    None => add.push(t),
                }
            }
            if !add.is_empty() {
                let cur = text.read().clone();
                let sep = if cur.is_empty() || cur.ends_with(' ') || cur.ends_with('\n') { "" } else { " " };
                text.set(format!("{cur}{sep}{} ", add.join(" "))); bump(put_gen);
            }
            document::eval(&format!("setTimeout(() => document.getElementById('{input_id}')?.focus(), 30)"));
        }
    }));

    // A message taken back from the agent: back in this chat's box as it was
    // (its objects, images and files), ahead of whatever was being typed.
    let restore_len = crate::state::use_ui(|u| u.restore_inbox.len())();
    let sid_restore = session_id.clone();
    use_effect(use_reactive!(|restore_len| {
        if restore_len == 0 {
            return;
        }
        let Some(sid) = sid_restore.clone() else { return };
        let (mut app, mut text, mut mentions, mut at) = (app, text, mentions, attachments);
        let mine: Vec<crate::state::Queued> = {
            let mut ui = app.ui.write();
            let (m, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut ui.restore_inbox).into_iter().partition(|(s, _)| *s == sid);
            ui.restore_inbox = rest;
            m.into_iter().map(|(_, q)| q).collect()
        };
        for q in mine {
            let (t, chips) = super::mentions::to_edit(&q.text, &app.sessions.peek());
            let typed = text.peek().clone();
            text.set(if typed.trim().is_empty() { t.clone() } else { format!("{t}\n{typed}") }); bump(put_gen);
            for c in chips {
                if !mentions.peek().iter().any(|m| m.marked() == c.marked()) {
                    mentions.write().push(c);
                }
            }
            let mut all = q.attachments.clone();
            all.extend(at.peek().iter().cloned());
            at.set(all);
            let end = t.encode_utf16().count();
            document::eval(&format!("setTimeout(() => {{ const el = document.getElementById('{input_id}'); if (el) {{ el.focus(); el.setSelectionRange({end}, {end}); }} }}, 40)"));
        }
    }));

    // Focus when asked (Ctrl+L, new agent, opening a session).
    let focus = crate::state::use_ui(|u| u.focus_composer)();
    use_effect(use_reactive!(|focus| {
        let _ = focus;
        if primary {
            document::eval("setTimeout(() => document.getElementById('composer-input')?.focus(), 30)");
        }
    }));

    let sid_memo = session_id.clone();
    let sess = use_memo(move || {
        let id = sid_memo.as_ref()?;
        app.with_session(id, |s| SessInfo {
            agent: s.agent.clone(),
            model: s.model.clone(),
            mode: s.mode.clone(),
            permission: s.permission.clone(),
            effort: s.effort.clone(),
            worktree: matches!(s.environment, larrier_core::Environment::Worktree { .. }),
            existing: match &s.environment {
                larrier_core::Environment::Worktree { path, branch, .. } if !branch.starts_with("larrier/") => Some(path.clone()),
                _ => None,
            },
            usage: s.usage.clone(),
            context: super::chat::context_of(s),
            cost: super::chat::cost_of(s),
            messages: s.messages.len(),
            cwd: s.cwd(),
            account: s.account_id.clone(),
            run_as: s.run_as.clone(),
            fresh: s.messages.is_empty(),
        })
    });

    // Workspace file list for @-mentions (indexed in the background).
    let file_root: Option<String> = match &session_id {
        Some(_) => sess.read().as_ref().and_then(|s| s.cwd.clone()),
        None => app.draft.read().workspace.clone(),
    };
    let fr2 = file_root.clone();
    let file_index = use_resource(use_reactive!(|fr2| async move {
        let Some(root) = fr2 else { return Vec::new() };
        tokio::task::spawn_blocking(move || list_files(&root, 30_000)).await.unwrap_or_default()
    }));
    let fr3 = file_root.clone();
    let branch = use_resource(use_reactive!(|fr3| async move {
        let root = fr3?;
        tokio::task::spawn_blocking(move || larrier_agents::git::current_branch(std::path::Path::new(&root))).await.ok().flatten()
    }));

    // Current selections: from the session, or the new-agent draft.
    let (agent, model, mode, worktree, existing, usage) = match &session_id {
        Some(_) => match sess.read().clone() {
            Some(s) => (s.agent, s.model, s.mode, s.worktree, s.existing, Some(s.usage)),
            None => return rsx! {},
        },
        None => {
            let d = app.draft.read();
            (d.agent.clone(), d.model.clone(), d.mode.clone(), d.worktree, d.existing_worktree.clone(), None)
        }
    };
    // Writing its reply; only background work left (commands, subagents)
    // counts as finished: a message then goes straight in.
    let running = session_id.as_ref().map(|id| app.replying(id)).unwrap_or(false);
    // Only its subagents are working: the agent itself waits on them, so
    // the box offers to send (into the turn), not to stop.
    let delegating = session_id.as_ref().is_some_and(|id| app.delegating.read().contains(id));
    let permission = match &session_id {
        Some(_) => sess.read().as_ref().and_then(|s| s.permission.clone()),
        None => app.draft.read().permission.clone(),
    };
    let effort = match &session_id {
        Some(_) => sess.read().as_ref().and_then(|s| s.effort.clone()),
        None => app.draft.read().effort.clone(),
    };
    // One of the agent's own agents the chat runs as (Claude Code's
    // `--agent`), chosen from @ before the first message.
    let (run_as, fresh) = match &session_id {
        Some(_) => sess.read().as_ref().map(|s| (s.run_as.clone(), s.fresh)).unwrap_or((None, false)),
        None => (app.draft.read().run_as.clone().filter(|_| agent == AgentKind::ClaudeCode), true),
    };
    // The agent to run as sits in the text as an object until the chat starts.
    let run_as_key = run_as.clone().filter(|_| fresh);
    let agent_ra = agent.clone();
    use_effect(use_reactive!(|run_as_key| {
        let (mut text, mut mentions) = (text, mentions);
        let Some(name) = run_as_key else { return };
        let chip = MentionChip::run_as(&name, &agent_ra);
        if text.peek().contains(&chip.marked()) {
            return;
        }
        let rest = text.peek().clone();
        mentions.write().retain(|c| c.kind != super::mentions::MentionKind::RunAs);
        text.set(format!("{} {}", chip.marked(), rest.trim_start())); bump(put_gen);
        mentions.write().push(chip);
    }));
    // The agent's own modes, e.g. Claude Code Auto / Manual / Plan …
    let groups = mode_view(&app, &agent, [mode.clone(), permission.clone()]);
    let plan_id = plan_option(&agent);
    let in_plan = plan_id.is_some() && groups.first().map(|g| Some(g.options[g.chosen].id) == plan_id).unwrap_or(false);
    let ctrl_enter = app.config.read().settings.send_with_ctrl_enter;
    let busy = app.config.read().settings.busy_send;
    // Running in the agent's own app: replies go there.
    let elsewhere = session_id.as_ref().map(|id| app.running_elsewhere(id)).unwrap_or(false);
    let placeholder = if home {
        "Plan, Build, / for commands, @ for context".to_string()
    } else if elsewhere {
        format!("Running in {} — reply there, or once it finishes", agent.display_name())
    } else {
        "Send follow-up".to_string()
    };
    let atts = attachments.read().clone();
    let chips_now = mentions.read().clone();
    let can_send = !elsewhere && (!text.read().trim().is_empty() || !atts.is_empty());
    let detected = app.detected.read().clone();
    let multi_now = multi.read().clone();
    // An empty box shows its placeholder in full: when that needs more than
    // the one-line box (a long notice, a narrow window) the box is two-line.
    // Measured again whenever the composer's width changes.
    let mut ph_tall = use_signal(|| false);
    let mut ph_task = use_signal(|| None::<dioxus::core::Task>);
    let ph = placeholder.clone();
    use_effect(use_reactive!(|ph| {
        let _ = &ph;
        let js = format!(
            r#"const el = document.getElementById('{input_id}'); if (!el) return;
            const c = window.__lmc || (window.__lmc = document.createElement('canvas').getContext('2d'));
            let last = null;
            const check = () => {{
              if (!el.isConnected) return;
              const box = el.closest('.composer');
              if (box && !box.classList.contains('tall')) el.dataset.cw = el.clientWidth;
              const w = parseFloat(el.dataset.cw || el.clientWidth);
              const cs = getComputedStyle(el);
              c.font = cs.fontStyle + ' ' + cs.fontWeight + ' ' + cs.fontSize + ' ' + cs.fontFamily;
              const over = w > 0 && c.measureText(el.placeholder || '').width > w - 14;
              if (over !== last) {{ last = over; dioxus.send(over); }}
            }};
            check();
            if (el.__phObs) el.__phObs.disconnect();
            el.__phObs = new ResizeObserver(() => requestAnimationFrame(check));
            el.__phObs.observe(el.closest('.composer-wrap') || el);
            await new Promise(() => {{}});"#
        );
        if let Some(t) = ph_task.write().take() {
            t.cancel();
        }
        let task = spawn(async move {
            let mut e = document::eval(&js);
            while let Ok(over) = e.recv::<bool>().await {
                if *ph_tall.peek() != over {
                    ph_tall.set(over);
                }
            }
        });
        ph_task.set(Some(task));
    }));
    // The box grows while what's in it needs more than a line; once it's
    // sent (queued, steered, cleared) it's one line again.
    let tall = home || (overflow() && !text.read().is_empty()) || (ph_tall() && text.read().is_empty()) || text.read().contains('\n') || !atts.is_empty();
    use_effect(move || {
        if text.read().is_empty() && *overflow.peek() {
            overflow.set(false);
        }
    });
    // The harness's logo: "Cursor Agent · Opus 5.5" is Cursor, whoever made the model.
    let model_logo_svg = agent_logo(&agent);
    let (model_text, effort_text) = model_display(&app, &agent, &model, &effort);

    // "@" mentions and "/" commands: one suggestion list, keyboard navigable.
    let text_now = text.read().clone();
    let is_dismissed = dismissed.read().as_deref() == Some(text_now.as_str());
    let suggs: Vec<Sugg> = if is_dismissed {
        Vec::new()
    } else {
        suggestions(&app, &text_now, file_index.read().as_deref().unwrap_or(&[]), home, &agent, session_id.as_deref(), fresh)
    };
    let hl_i = if suggs.is_empty() { 0 } else { hl().min(suggs.len() - 1) };
    let queued: Vec<crate::state::Queued> = session_id.as_ref().and_then(|id| app.queue.read().get(id).cloned()).unwrap_or_default();
    // What the agent is waiting on (a question, an approval, a plan), with
    // how many more wait behind it.
    let request = session_id.as_ref().and_then(|id| app.permissions.read().get(id).and_then(|v| Some((v.first()?.clone(), v.len() - 1))));

    // Copy-able helpers so every menu row and key handler can hold one.
    // Store a pick for mode group 0 (Session.mode) or 1 (Session.permission).
    let set_pick = move |group: usize, v: Option<String>, sid: Option<String>| {
        let mut app = app;
        match (&sid, group) {
            (Some(id), 0) => app.set_session_mode(id, v),
            (Some(id), _) => app.set_session_permission(id, v),
            (None, 0) => app.draft.write().mode = v,
            (None, _) => app.draft.write().permission = v,
        }
    };
    let set_mode = move |m: Option<String>, sid: Option<String>| set_pick(0, m, sid);
    let plan_k = plan_id;
    let agent_k: &'static larrier_core::AgentSpec = agent.spec();
    let run_slash = move |cmd: &str, sid: Option<String>| {
        let mut app = app;
        let mut text = text;
        text.set(String::new()); bump(put_gen);
        match cmd {
            "/plan" => if let Some(p) = plan_k { set_mode(Some(p.to_string()), sid) },
            "/agent" => set_mode(None, sid),
            "/ask" => if agent_k.mode_groups.first().and_then(|g| g.find("ask")).is_some() { set_mode(Some("ask".into()), sid) },
            "/new" => app.new_agent(),
            "/fork" => if let Some(id) = &sid { app.fork(id, None) },
            "/export" => if let Some(id) = &sid { app.export(id, larrier_formats::ExportFormat::LsfJsonl) },
            "/terminal" => if let Some(id) = &sid { app.open_in_terminal(id) },
            "/worktree" => { let mut d = app.draft.write(); d.worktree = true; d.existing_worktree = None; }
            "/local" => { let mut d = app.draft.write(); d.worktree = false; d.existing_worktree = None; }
            "/settings" => app.open_settings(SettingsTab::General),
            "/usage" => app.open_settings(SettingsTab::Usage),
            "/mcp" => {
                app.ui.write().customize_focus = Some((Some(larrier_agents::harness::ItemKind::Mcp), Some(agent_k.kind.clone())));
                app.navigate(crate::state::View::Customize);
            }
            _ => {}
        }
    };
    // Who a chat that hasn't started runs as.
    let set_run_as = move |name: Option<String>, sid: Option<String>| {
        let mut app = app;
        match &sid {
            Some(id) => app.update_session(id, |s| s.run_as = name),
            None => app.draft.write().run_as = name,
        }
    };
    // Apply a suggestion: the typed `@…` / `/…` becomes an object in the
    // text, or a Larrier command runs.
    let apply = move |sg: Sugg, sid: Option<String>| {
        let mut text = text;
        let mentions = mentions;
        // The text without the `@…` / `/…` being typed.
        let typed_cut = |t: &str| t.rfind(|c: char| c.is_whitespace() || c == super::mentions::CLOSE).map(|i| i + t[i..].chars().next().map(|c| c.len_utf8()).unwrap_or(1)).unwrap_or(0);
        let put = move |chip: MentionChip| {
            let (mut text, mut mentions) = (text, mentions);
            let t = text.read().clone();
            let cut = typed_cut(&t);
            let have = mentions.read().iter().find(|c| c.kind == chip.kind && c.value == chip.value).cloned();
            let chip = have.unwrap_or_else(|| super::mentions::unique(&mentions.read(), chip));
            text.set(format!("{}{} ", &t[..cut], chip.marked())); bump(put_gen);
            // Objects whose tokens are gone from the text are dropped.
            let now = text.read().clone();
            mentions.write().retain(|c| now.contains(&c.marked()));
            if !mentions.read().iter().any(|c| c.kind == chip.kind && c.value == chip.value) {
                mentions.write().push(chip);
            }
        };
        match sg.act {
            SuggAct::Chip(chip) => put(chip),
            SuggAct::Larrier(cmd) => run_slash(cmd, sid),
            SuggAct::RunAs(name, kind) => {
                // One agent to run as: picking another replaces it.
                let old: Vec<MentionChip> = mentions.read().iter().filter(|c| c.kind == super::mentions::MentionKind::RunAs).cloned().collect();
                for c in old {
                    let t = text.read().replace(&format!("{} ", c.marked()), "").replace(&c.marked(), "");
                    text.set(t); bump(put_gen);
                }
                put(MentionChip::run_as(&name, &kind));
                set_run_as(Some(name), sid);
            }
        }
        document::eval(&format!("setTimeout(() => {{ const el = document.getElementById('{input_id}'); if (el) {{ el.focus(); el.selectionStart = el.selectionEnd = el.value.length; }} }}, 10)"));
    };
    // Choose agent (+ model). On an existing session this ports it. Cursor
    // stores the model's default variant; an effort the new model doesn't
    // offer goes back to the default.
    let choose = move |k: AgentKind, m: Option<String>, sid: Option<String>| {
        let mut app = app;
        let (m, keep_effort) = match m {
            Some(id) => {
                let (id, _) = crate::agent_models::compose(&app, &k, &id, None, false);
                let efforts = crate::agent_models::choices(&app, &k).into_iter().find(|c| c.id == id).map(|c| c.efforts).unwrap_or_default();
                (Some(id), efforts)
            }
            None => (None, Vec::new()),
        };
        let fit = |e: Option<String>| e.filter(|e| keep_effort.contains(e));
        match &sid {
            Some(id) => {
                let current = app.peek_session(id, |s| s.agent.clone());
                if current.as_ref() != Some(&k) {
                    app.port(id, k);
                }
                let e = app.peek_session(id, |s| s.effort.clone()).flatten();
                app.set_session_model(id, m);
                app.set_session_effort(id, fit(e));
            }
            None => {
                let mut d = app.draft.write();
                if d.agent != k {
                    d.mode = None;
                    d.permission = None;
                    d.account_id = None;
                }
                d.agent = k;
                d.model = m;
                d.effort = fit(d.effort.clone());
            }
        }
    };
    // A context size (Cursor): the model runs with its parameters spelled
    // out, keeping its effort and speed.
    let set_context = move |ctx: String, sid: Option<String>| {
        let mut app = app;
        let (agent, model, effort) = match &sid {
            Some(id) => match app.peek_session(id, |s| (s.agent.clone(), s.model.clone(), s.effort.clone())) {
                Some(x) => x,
                None => return,
            },
            None => {
                let d = app.draft.read();
                (d.agent.clone(), d.model.clone(), d.effort.clone())
            }
        };
        let now = crate::agent_models::resolve(&app, &agent, model.as_deref(), effort.as_deref());
        let Some(family) = now.family.clone().or(now.model.clone()) else { return };
        let (m, _) = crate::agent_models::compose_with(&app, &agent, &family, now.effort.as_deref(), now.fast, Some(&ctx));
        match &sid {
            Some(id) => {
                app.set_session_model(id, Some(m));
                app.set_session_effort(id, None);
            }
            None => {
                let mut d = app.draft.write();
                d.model = Some(m);
                d.effort = None;
            }
        }
    };
    // Effort (and Cursor's speed) for the model in use: Cursor switches to
    // that variant (keeping a context picked), other agents keep the effort
    // beside the model.
    let set_effort = move |e: Option<String>, fast: bool, sid: Option<String>| {
        let mut app = app;
        let (agent, model, effort) = match &sid {
            Some(id) => match app.peek_session(id, |s| (s.agent.clone(), s.model.clone(), s.effort.clone())) {
                Some(x) => x,
                None => return,
            },
            None => {
                let d = app.draft.read();
                (d.agent.clone(), d.model.clone(), d.effort.clone())
            }
        };
        let now = crate::agent_models::resolve(&app, &agent, model.as_deref(), effort.as_deref());
        let Some(family) = now.family.clone().or(now.model.clone()) else { return };
        let (m, e) = crate::agent_models::compose_with(&app, &agent, &family, e.as_deref(), fast, now.context.as_deref());
        // Picking the agent's own default model and effort keeps following it.
        let model_changed = Some(m.as_str()) != now.model.as_deref() || model.is_some();
        match &sid {
            Some(id) => {
                if model_changed {
                    app.set_session_model(id, Some(m));
                }
                app.set_session_effort(id, e);
            }
            None => {
                let mut d = app.draft.write();
                if model_changed {
                    d.model = Some(m);
                }
                d.effort = e;
            }
        }
    };
    let do_send = move |sid: Option<String>, how: Option<BusySend>| {
        let mut app = app;
        let mut q_edit = q_edit;
        let mut text = text;
        let mut mentions = mentions;
        let mut at = attachments;
        let typed = text.read().clone();
        let a = at.read().clone();
        let chips = mentions.read().clone();
        let cwd = match &sid {
            Some(id) => app.peek_session(id, |s| s.cwd()).flatten(),
            None => app.draft.read().workspace.clone(),
        };
        // Objects become what the agent reads: a leading /skill, @paths, and
        // context blocks for chats and the diff.
        let t = super::mentions::expand(&app, &chips, &typed, cwd.as_deref());
        // Only the agent to run as: nothing to send yet.
        if t.trim().is_empty() && a.is_empty() {
            return;
        }
        // Claude Code's own sign-in commands aren't for the model: they run
        // in a terminal, as this chat's account.
        let run_agent = match &sid {
            Some(id) => app.peek_session(id, |s| (s.agent.clone(), s.account_id.clone())),
            None => {
                let d = app.draft.read();
                Some((d.agent.clone(), d.account_id.clone()))
            }
        };
        if let Some((AgentKind::ClaudeCode, acc_id)) = run_agent.filter(|_| a.is_empty()) {
            if let cmd @ ("/login" | "/logout") = t.trim() {
                let acc = {
                    let cfg = app.config.peek();
                    acc_id.as_deref().and_then(|i| cfg.account(i)).or_else(|| cfg.default_account(&AgentKind::ClaudeCode)).cloned()
                };
                text.set(String::new()); bump(put_gen);
                mentions.set(Vec::new());
                crate::keep::set_input(&crate::keep::key(sid.as_deref()), crate::keep::Input::default());
                match (cmd, acc) {
                    ("/login", Some(acc)) => app.sign_in(&acc),
                    (_, acc) => {
                        let line = crate::state::as_account(&AgentKind::ClaudeCode, acc.as_ref(), &format!("claude {cmd}"));
                        let home = dirs::home_dir().map(|h| h.to_string_lossy().to_string()).unwrap_or_else(|| ".".into());
                        match crate::state::open_terminal(&home, &line) {
                            Ok(()) => app.toast(format!("{cmd} runs in the terminal, for {}", acc.map(|a| format!("“{}”", a.label)).unwrap_or_else(|| "your current login".into()))),
                            Err(e) => app.toast_err(format!("Could not open a terminal: {e}")),
                        }
                    }
                }
                return;
            }
        }
        text.set(String::new()); bump(put_gen);
        at.set(Vec::new());
        mentions.set(Vec::new());
        let editing = q_edit.read().clone();
        // Sent: nothing is kept for this box any more. Done now, before the
        // view moves on to the new chat (which would list what the New Agent
        // box still kept as a draft); editing a queued message keeps the
        // message set aside, which goes back in the box below.
        if editing.is_none() {
            crate::keep::set_input(&crate::keep::key(sid.as_deref()), crate::keep::Input::default());
        }
        if let Some((qid, draft, draft_atts, draft_chips)) = editing {
            q_edit.set(None);
            if let Some(id) = &sid {
                app.queue_update(id, qid, t, a);
            }
            text.set(draft); bump(put_gen);
            at.set(draft_atts);
            mentions.set(draft_chips);
            return;
        }
        let agents = multi.read().clone();
        if sid.is_none() && agents.len() > 1 {
            // Multitask: the same prompt on several agents, side by side.
            let keep = app.draft.read().clone();
            for k in agents {
                {
                    let mut d = app.draft.write();
                    d.agent = k;
                    d.model = None;
                    d.mode = keep.mode.clone();
                    d.permission = keep.permission.clone();
                }
                app.send(None, t.clone(), a.clone());
            }
            *app.draft.write() = keep;
            app.toast("Started the prompt on every selected agent");
            return;
        }
        // A prompt of this chat opened for editing and left as it was closes:
        // the chat moved on. One with changes stays open.
        let unchanged_edit = {
            let ed = app.editing.peek();
            ed.msg.is_some() && !ed.dirty() && sid.is_some() && ed.session == sid
        };
        if unchanged_edit {
            app.set_edit(None);
        }
        match (&sid, how) {
            (Some(id), Some(h)) if app.replying(id) => app.send_while_busy(id, t, a, h),
            _ => app.send(sid, t, a),
        }
    };
    // Put the draft back and stop editing a queued message.
    let cancel_edit = move || {
        let mut q_edit = q_edit;
        let editing = q_edit.read().clone();
        if let Some((_, draft, draft_atts, draft_chips)) = editing {
            let (mut text, mut at, mut mentions) = (text, attachments, mentions);
            q_edit.set(None);
            text.set(draft); bump(put_gen);
            at.set(draft_atts);
            mentions.set(draft_chips);
        }
    };
    // Load a queued message into the box to edit; what was typed waits.
    let start_edit = move |q: crate::state::Queued| {
        let (mut text, mut at, mut q_edit, mut mentions) = (text, attachments, q_edit, mentions);
        let kept = q_edit.read().clone().map(|(_, d, da, dc)| (d, da, dc)).unwrap_or_else(|| (text.read().clone(), at.read().clone(), mentions.read().clone()));
        q_edit.set(Some((q.id, kept.0, kept.1, kept.2)));
        // Its objects back as objects, not the text the agent reads.
        let (t, chips) = super::mentions::to_edit(&q.text, &app.sessions.peek());
        text.set(t); bump(put_gen);
        mentions.set(chips);
        at.set(q.attachments.clone());
        document::eval(&format!("setTimeout(() => {{ const el = document.getElementById('{input_id}'); if (el) {{ el.focus(); el.selectionStart = el.selectionEnd = el.value.length; }} }}, 30)"));
    };
    let sid_send = session_id.clone();
    let sid_key = session_id.clone();
    let sid_input = session_id.clone();
    let first_group = groups.first().cloned();
    let suggs_k = suggs.clone();
    let sid_stop = session_id.clone();

    let workspaces = app.config.read().workspaces.clone();
    let ws_now = app.draft.read().workspace.clone();
    let ws_label = ws_now
        .as_ref()
        .map(|p| std::path::Path::new(p).file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or(p.clone()))
        .unwrap_or_else(|| "Home".into());
    let is_repo = branch.read().clone().flatten().is_some();
    // The environment chip: this PC, or the worktree a chat runs in.
    let env_text = if !worktree || !is_repo {
        "This PC".to_string()
    } else if let Some(p) = &existing {
        std::path::Path::new(p).file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_else(|| p.clone())
    } else if session_id.is_some() {
        "Worktree".to_string()
    } else {
        "New worktree".to_string()
    };
    let env_folder = match &session_id {
        Some(_) => sess.read().as_ref().and_then(|s| s.cwd.clone()),
        None => ws_now.clone(),
    };
    // Projects matching the search in the project menu.
    let pj_q = pj_query.read().trim().to_lowercase();
    let pj_shown: Vec<larrier_core::Workspace> = workspaces
        .iter()
        .filter(|w| pj_q.is_empty() || w.name.to_lowercase().contains(&pj_q) || w.path.to_lowercase().contains(&pj_q))
        .cloned()
        .collect();

    // The agent + model picker menu.
    let picker_menu = rsx! {
        ModelPicker {
            open: model_menu,
            up: !home,
            agent: agent.clone(),
            model: model.clone(),
            effort: effort.clone(),
            account: match &session_id {
                Some(_) => sess.read().as_ref().and_then(|s| s.account.clone()),
                None => app.draft.read().account_id.clone(),
            },
            onpick: {
                let sid = session_id.clone();
                move |(k, m): (AgentKind, Option<String>)| {
                    model_menu.set(false);
                    choose(k, m, sid.clone());
                }
            },
            oneffort: {
                let sid = session_id.clone();
                move |(e, fast): (Option<String>, bool)| set_effort(e, fast, sid.clone())
            },
            oncontext: {
                let sid = session_id.clone();
                move |ctx: String| set_context(ctx, sid.clone())
            },
            onaccount: {
                let sid = session_id.clone();
                move |acc: String| match &sid {
                    Some(id) => app.switch_account(id, acc),
                    None => app.draft.write().account_id = Some(acc),
                }
            },
        }
    };

    rsx! {
        div { class: "composer-wrap",
            ondragover: move |e| { e.prevent_default(); if !dragging() { dragging.set(true); } },
            ondragleave: move |_| dragging.set(false),
            ondrop: move |e| {
                e.prevent_default();
                dragging.set(false);
                let mut at = attachments;
                for f in e.files() {
                    let p = f.path().to_string_lossy().to_string();
                    if !p.is_empty() {
                        at.write().push(crate::state::attachment_for_path(&p));
                    }
                }
            },
            if home {
                div { class: "row", style: "gap: 2px; margin: 0 0 8px 4px;",
                    div { class: "menu-anchor",
                        button { class: "env-chip", onclick: move |_| { pj_query.set(String::new()); ws_menu.set(true); },
                            if let Some(w) = ws_now.as_deref().and_then(|p| app.config.read().project_for(p).cloned()) {
                                super::projects::ProjectMark { icon: w.icon.clone(), color: w.color.clone(), size: 14 }
                            } else if ws_now.is_some() {
                                Icon { i: I::Folder, size: 14, stroke: 1.5 }
                            } else {
                                {
                                    let h = app.config.read().home_project();
                                    rsx! { super::projects::ProjectMark { icon: h.icon.clone(), color: h.color.clone(), size: 14, home: true } }
                                }
                            }
                            span { "{ws_label}" }
                            Icon { i: I::ChevronDown, size: 12 }
                        }
                        Dropdown { open: ws_menu, up: false, class: "pj-menu".to_string(),
                            div { class: "menu-search",
                                Icon { i: I::Search, size: 13 }
                                input {
                                    placeholder: "Search projects",
                                    value: "{pj_query}",
                                    autofocus: true,
                                    spellcheck: "false",
                                    oninput: move |e| pj_query.set(e.value()),
                                    onkeydown: {
                                        let first = pj_shown.first().map(|w| w.path.clone());
                                        move |e: KeyboardEvent| {
                                            if e.key() == Key::Enter {
                                                e.prevent_default();
                                                if let Some(p) = &first {
                                                    ws_menu.set(false);
                                                    app.pick_project(Some(p.clone()));
                                                }
                                            }
                                        }
                                    },
                                }
                            }
                            for w in pj_shown.clone() {
                                MenuItem {
                                    key: "{w.id}",
                                    label: w.name.clone(),
                                    title: Some(short_path(&w.path)),
                                    lead: rsx! { super::projects::ProjectMark { icon: w.icon.clone(), color: w.color.clone(), size: 14 } },
                                    checked: ws_now.as_deref() == Some(w.path.as_str()),
                                    onclick: {
                                        let p = w.path.clone();
                                        move |_| {
                                            ws_menu.set(false);
                                            app.pick_project(Some(p.clone()));
                                        }
                                    },
                                }
                            }
                            if pj_shown.is_empty() && !pj_q.is_empty() {
                                div { class: "menu-empty", "No projects match" }
                            }
                            if !pj_shown.is_empty() || !pj_q.is_empty() {
                                div { class: "menu-sep" }
                            }
                            MenuItem { label: app.config.read().home_project().name, checked: ws_now.is_none(), title: Some("Chats without a project folder".to_string()),
                                lead: { let h = app.config.read().home_project(); rsx! { super::projects::ProjectMark { icon: h.icon.clone(), color: h.color.clone(), size: 14, home: true } } },
                                onclick: move |_| { ws_menu.set(false); app.pick_project(None); } }
                            MenuItem { label: "New project".to_string(), icon: Some(I::Plus), title: Some("Pick a folder to work in".to_string()),
                                onclick: move |_| { ws_menu.set(false); app.edit_project(None); } }
                            MenuItem { label: "Manage projects".to_string(), icon: Some(I::Settings),
                                onclick: move |_| { ws_menu.set(false); app.open_settings(SettingsTab::Workspaces); } }
                        }
                    }
                    EnvPicker { open: env_menu, worktree, is_repo, session: false, label: env_text.clone(), folder: env_folder.clone(), existing: existing.clone() }
                    div { class: "grow" }
                    if is_repo {
                        BranchPicker { folder: ws_now.clone(), current: branch.read().clone().flatten(), new_tree: worktree && existing.is_none(), existing: existing.clone() }
                    }
                }
            }
            if !suggs.is_empty() {
                div { class: "menu mention-menu in flip-up",
                    {
                        let mut last_group = "";
                        rsx! {
                            for (i, sg) in suggs.iter().enumerate() {
                                {
                                    let show_head = sg.group != last_group;
                                    last_group = sg.group;
                                    let sg2 = sg.clone();
                                    let sid = session_id.clone();
                                    rsx! {
                                        if show_head { div { key: "h-{i}", class: "menu-label", "{sg.group}" } }
                                        button { key: "{i}", class: if i == hl_i { "menu-item hl" } else { "menu-item" },
                                            onmouseenter: move |_| hl.set(i),
                                            onmousedown: move |e| { e.prevent_default(); apply(sg2.clone(), sid.clone()); },
                                            span { class: "ic",
                                                if let Some(svg) = sg.logo { Logo { svg, size: 14 } } else { Icon { i: sg.icon, size: 14 } }
                                            }
                                            span { class: "ellipsis", "{sg.label}" }
                                            if !sg.sub.is_empty() { span { class: "sub", "{sg.sub}" } }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            // One row of pills above the box (running agents and terminals,
            // the queue, the changes), the card one of them opened over it.
            if let Some(sid) = session_id.clone() {
                div { class: "row above-composer",
                    style { {super::tasks::CSS} }
                    super::chat::DockPills { id: sid, card: dock }
                    if !queued.is_empty() {
                        button { class: if dock() == Some(super::tasks::DockCard::Queue) { "pill-btn tk-chip queue-pill on" } else { "pill-btn tk-chip queue-pill" },
                            title: "Messages waiting for the agent",
                            onclick: move |_| dock.set(if dock() == Some(super::tasks::DockCard::Queue) { None } else { Some(super::tasks::DockCard::Queue) }),
                            Icon { i: I::List, size: 13 }
                            span { "Queued " span { class: "n", "{queued.len()}" } }
                        }
                    }
                    if !queued.is_empty() && dock() == Some(super::tasks::DockCard::Queue) {
                    div { class: "tk-card queue-card",
                        div { class: "tk-card-head",
                            span { class: "tk-ic", Icon { i: I::List, size: 13 } }
                            span { class: "ttl", "{queued.len()} Queued" }
                            span { class: "grow" }
                            button { class: "icon-btn", title: "Collapse", onclick: move |_| dock.set(None), Icon { i: I::Minus, size: 13 } }
                        }
                        div { class: "tk-card-list queue-list",
                        for (qi, q) in queued.clone().into_iter().enumerate() {
                            {
                                let sid = session_id.clone().unwrap_or_default();
                                let editing = q_edit.read().as_ref().map(|e| e.0) == Some(q.id);
                                let over = q_over() == Some(qi) && q_drag().is_some_and(|d| d != qi);
                                let thumb = q.attachments.iter().find(|a| matches!(a, Part::Image { .. })).and_then(crate::state::image_src);
                                let files = q.attachments.iter().filter(|a| matches!(a, Part::File { .. })).count();
                                let (s1, s2, s3, s4) = (sid.clone(), sid.clone(), sid.clone(), sid.clone());
                                let (q1, q2) = (q.clone(), q.clone());
                                let mut cls = String::from("queue-row");
                                if editing { cls.push_str(" editing"); }
                                if over { cls.push_str(" over"); }
                                if q_drag() == Some(qi) { cls.push_str(" dragging"); }
                                rsx! {
                                    div { key: "{q.id}", class: "{cls}", draggable: "true",
                                        ondragstart: move |e| { e.stop_propagation(); q_drag.set(Some(qi)); },
                                        ondragover: move |e| { e.prevent_default(); e.stop_propagation(); if q_over() != Some(qi) { q_over.set(Some(qi)); } },
                                        ondrop: move |e| {
                                            e.prevent_default();
                                            e.stop_propagation();
                                            if let Some(from) = q_drag() { app.queue_move(&s1, from, qi); }
                                            q_drag.set(None);
                                            q_over.set(None);
                                        },
                                        ondragend: move |_| { q_drag.set(None); q_over.set(None); },
                                        span { class: "grip", Icon { i: I::Grip, size: 12 } }
                                        if let Some(src) = thumb { img { class: "q-thumb", src: "{src}" } }
                                        else if files > 0 { span { class: "q-file", Icon { i: I::Paperclip, size: 12 } } }
                                        span { class: "t", dir: "auto", title: "{super::mentions::split_mentions(&q.text).0}",
                                            super::mentions::PromptText { text: q.text.clone() }
                                        }
                                        div { class: "q-acts",
                                            if running {
                                                button { class: "q-steer", title: "Send it into the running turn",
                                                    onclick: move |_| app.queue_send(&s2, q1.id, BusySend::Steer),
                                                    "Steer" Icon { i: I::CornerDownLeft, size: 12 }
                                                }
                                            }
                                            button { class: "icon-btn sm", title: "Edit", onclick: move |_| start_edit(q2.clone()), Icon { i: I::Pencil, size: 13 } }
                                            button { class: "icon-btn sm", title: "Remove from queue",
                                                onclick: move |_| { if editing { cancel_edit(); } app.queue_remove(&s3, q.id); },
                                                Icon { i: I::Trash, size: 13 }
                                            }
                                            // Waiting with the agent stopped (or after a restart): sent as is.
                                            button { class: "icon-btn sm", title: if running { "Stop the agent and send this now" } else { "Send this now" },
                                                onclick: move |_| app.queue_send(&s4, q.id, BusySend::Interrupt),
                                                Icon { i: I::Send, size: 13 }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    }
                    }
                }
            }
            // Asked just above the box, where it is answered: never in the chat.
            if let (Some((req, more)), Some(sid)) = (request, session_id.clone()) {
                super::request::RequestCard { key: "{req.id}", session_id: sid, request: req, more }
            }
            if dragging() {
                div { class: "drop-zone", "Drop files or images to attach" }
            }
            div { class: format!("composer{}{}", if tall { " tall" } else { "" }, if home { " home-c" } else { "" }),
                if !atts.is_empty() {
                    div { class: "thumb-row",
                        for (i, a) in atts.iter().enumerate() {
                            match a {
                                Part::Image { .. } => rsx! {
                                    div { key: "{i}", class: "thumb", title: "Image",
                                        if let Some(src) = crate::state::image_src(a) {
                                            img { src: "{src}", onclick: { let s2 = src.clone(); move |_| app.ui.write().lightbox = Some(s2.clone()) } }
                                        } else {
                                            Icon { i: I::Image, size: 20 }
                                        }
                                        button { class: "x", onclick: move |_| { let mut at = attachments; at.write().remove(i); }, Icon { i: I::X, size: 11 } }
                                    }
                                },
                                Part::File { path, text, .. } => rsx! {
                                    span { key: "{i}", class: "file-pill", title: "{path}",
                                        Icon { i: if text.is_some() { I::Lines } else { I::Paperclip }, size: 12 }
                                        span { class: "ellipsis", if text.is_some() { "{path}" } else { "{short_path(path)}" } }
                                        button { class: "x", onclick: move |_| { let mut at = attachments; at.write().remove(i); }, Icon { i: I::X, size: 11 } }
                                    }
                                },
                                _ => rsx! {},
                            }
                        }
                    }
                }
                // One grid for both layouts so the text box is never re-created
                // (and never loses focus) when the composer grows.
                div { class: "composer-row cgrid",
                    div { class: "c-plus",
                        PlusMenu { open: plus_menu, attachments, text, put_gen, input_id }
                        ConfigMenu { session_id: session_id.clone(), agent: agent.clone(), groups: groups.clone(),
                            onpick: { let sid = session_id.clone(); move |(g, v): (usize, Option<String>)| set_pick(g, v, sid.clone()) } }
                        if q_edit.read().is_some() {
                            span { class: "edit-badge", title: "Editing a queued message",
                                Icon { i: I::Pencil, size: 12 }
                                button { class: "x", title: "Cancel editing (Esc)", onclick: move |_| cancel_edit(), Icon { i: I::X, size: 11 } }
                            }
                        }
                    }
                    // The text box over a colored copy of its text: objects
                    // (@file, /command, @chat...) show as pills in place.
                    div { class: "tok-field c-text-wrap",
                    super::mentions::TokenBackdrop { text: text(), chips: chips_now.clone() }
                    textarea {
                        id: "{input_id}",
                        class: "c-text tok-input",
                        dir: "auto",
                        // Spelling is Larrier's own (underlined in the copy behind).
                        spellcheck: "false",
                        rows: if home { "3" } else { "1" },
                        placeholder: "{placeholder}",
                        // Before value: the box reads it when the value arrives.
                        "data-gen": "{put_gen}",
                        value: "{text}",
                        oninput: move |e| {
                            hl.set(0);
                            let v = e.value();
                            if v.is_empty() {
                                overflow.set(false);
                            }
                            // Deleting the agent to run as runs the chat as the agent itself.
                            let gone = mentions.read().iter().any(|c| c.kind == super::mentions::MentionKind::RunAs && !v.contains(&c.marked()));
                            if gone {
                                mentions.write().retain(|c| c.kind != super::mentions::MentionKind::RunAs);
                                set_run_as(None, sid_input.clone());
                            }
                            text.set(v);
                            // Measure against the one-line width so the layout
                            // doesn't flip back and forth.
                            let js = format!(
                                r#"const el = document.getElementById('{input_id}'); if (!el) return false;
                                const box = el.closest('.composer');
                                if (box && !box.classList.contains('tall')) el.dataset.cw = el.clientWidth;
                                const w = parseFloat(el.dataset.cw || el.clientWidth);
                                const c = window.__lmc || (window.__lmc = document.createElement('canvas').getContext('2d'));
                                const cs = getComputedStyle(el);
                                c.font = cs.fontStyle + ' ' + cs.fontWeight + ' ' + cs.fontSize + ' ' + cs.fontFamily;
                                return el.value.includes('\n') || c.measureText(el.value).width > w - 14;"#
                            );
                            let seq = measure_seq() + 1;
                            measure_seq.set(seq);
                            spawn(async move {
                                if let Ok(b) = document::eval(&js).join::<bool>().await {
                                    if *measure_seq.peek() == seq && overflow() != b {
                                        overflow.set(b);
                                    }
                                }
                            });
                        },
                        onkeydown: move |e| {
                            if !suggs_k.is_empty() {
                                match e.key() {
                                    Key::ArrowDown => { e.prevent_default(); hl.set((hl_i + 1) % suggs_k.len()); return; }
                                    Key::ArrowUp => { e.prevent_default(); hl.set((hl_i + suggs_k.len() - 1) % suggs_k.len()); return; }
                                    Key::Escape => { e.prevent_default(); dismissed.set(Some(text.read().clone())); return; }
                                    Key::Enter | Key::Tab if !e.modifiers().shift() => {
                                        e.prevent_default();
                                        apply(suggs_k[hl_i].clone(), sid_key.clone());
                                        return;
                                    }
                                    _ => {}
                                }
                            }
                            if let Some(g) = first_group.as_ref().filter(|g| g.options.len() > 1 && e.key() == Key::Tab && e.modifiers().shift()) {
                                // Shift+Tab cycles the agent's modes (Auto → Manual → …).
                                e.prevent_default();
                                set_mode(pick_value(g, (g.chosen + 1) % g.options.len()), sid_key.clone());
                                return;
                            }
                            if e.key() == Key::Escape && q_edit.read().is_some() {
                                e.prevent_default();
                                cancel_edit();
                                return;
                            }
                            if e.key() == Key::Enter {
                                let m = e.modifiers();
                                let cmd = m.ctrl() || m.meta();
                                // With Ctrl+Enter to send, Ctrl+Shift+Enter is the other way.
                                let (primary, other) = if ctrl_enter { (cmd && !m.shift(), cmd && m.shift()) } else { (!m.shift() && !cmd, cmd && !m.shift()) };
                                if primary || other {
                                    e.prevent_default();
                                    let how = running.then(|| if other { busy.alternate() } else { busy });
                                    do_send(sid_key.clone(), how);
                                }
                            }
                        },
                    }
                    }
                    div { class: "menu-anchor c-model",
                        button { class: if model_menu() { "picker open" } else { "picker" }, title: "{agent.display_name()}",
                            onclick: move |_| { model_query.set(String::new()); model_menu.set(true); },
                            if let Some(svg) = model_logo_svg { Logo { svg, size: 14 } }
                            span { class: "lb", "{model_text}" }
                            if !effort_text.is_empty() { span { class: "lb sub eff", "{effort_text}" } }
                            span { class: "chev", Icon { i: I::ChevronDown, size: 12 } }
                        }
                        {picker_menu}
                    }
                    if q_edit.read().is_some() {
                        button { class: "send-btn ready c-send", title: "Save the queued message (Enter) · Esc cancels",
                            onclick: move |_| do_send(sid_send.clone(), None),
                            Icon { i: I::Check, size: 15, stroke: 2.4 }
                        }
                    } else if running && can_send {
                        div { class: "send-split c-send",
                            button { class: "send-btn ready", title: "{busy.label()} (Enter)",
                                onclick: { let sid = sid_send.clone(); move |_| do_send(sid.clone(), Some(busy)) },
                                Icon { i: I::ArrowUp, size: 15, stroke: 2.2 }
                            }
                            div { class: "menu-anchor",
                                button { class: if send_menu() { "send-more open" } else { "send-more" }, title: "Other ways to send",
                                    onclick: move |_| send_menu.set(true),
                                    Icon { i: I::ChevronDown, size: 12 }
                                }
                                Dropdown { open: send_menu, up: true, right: true,
                                    div { class: "menu-label", "The agent is working" }
                                    for b in BusySend::ALL {
                                        {
                                            let sid = sid_send.clone();
                                            let kb = if b == busy { "Enter" } else if b == busy.alternate() { if ctrl_enter { "Ctrl+Shift+Enter" } else { "Ctrl+Enter" } } else { "" };
                                            rsx! {
                                                button { key: "{b:?}", class: "menu-item send-way",
                                                    onclick: move |e| { e.stop_propagation(); send_menu.set(false); do_send(sid.clone(), Some(b)); },
                                                    span { class: "muted", Icon { i: match b { BusySend::Steer => I::CornerDownLeft, BusySend::Queue => I::List, BusySend::Interrupt => I::Stop }, size: 13 } }
                                                    span { class: "sw-t",
                                                        span { "{b.label()}" }
                                                        span { class: "sub", "{b.description()}" }
                                                    }
                                                    if !kb.is_empty() { span { class: "kb", {crate::platform::keys(kb)} } }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    } else if elsewhere {
                        // Working in the agent's own app: shown as working,
                        // but stopped (and replied to) there.
                        button { class: "send-btn stop c-send", disabled: true, title: "Running in {agent.display_name()} — stop it there",
                            Icon { i: I::Stop, size: 10 }
                        }
                    } else if running && !delegating {
                        button { class: "send-btn stop c-send", title: "Stop (Ctrl+.)",
                            onclick: move |_| if let Some(id) = &sid_stop { app.stop_reply(id) },
                            Icon { i: I::Stop, size: 10 }
                        }
                    } else {
                        button { class: if can_send { "send-btn ready c-send" } else { "send-btn c-send" }, disabled: !can_send, title: "Send (Enter)",
                            onclick: move |_| do_send(sid_send.clone(), None),
                            Icon { i: I::ArrowUp, size: 15, stroke: 2.2 }
                        }
                    }
                }
            }
            if home {
                div { class: "row", style: "gap: 6px; margin: 10px 0 0 2px; flex-wrap: wrap;",
                    button {
                        class: if in_plan { "pill-btn active-blue" } else { "pill-btn" },
                        disabled: plan_id.is_none(),
                        title: if plan_id.is_none() { "This agent has no plan mode" } else { "" },
                        onclick: move |_| set_mode(if in_plan { None } else { plan_id.map(String::from) }, None),
                        "Plan New Idea"
                        span { class: "faint", style: "font-size: var(--ui-xs);", "⇧Tab" }
                    }
                    div { class: "menu-anchor",
                        button {
                            class: if multi_now.len() > 1 { "pill-btn active-blue" } else { "pill-btn" },
                            onclick: move |_| multi_menu.set(true),
                            "Multitask"
                            if multi_now.len() > 1 { span { "· {multi_now.len()} agents" } }
                        }
                        Dropdown { open: multi_menu, up: false,
                            div { class: "menu-label", "Run the prompt on several agents at once" }
                            for spec in larrier_core::agent::runnable().filter(|s| app.config.read().agent(&s.kind).enabled && (s.binaries.is_empty() || detected.iter().any(|d| d.kind == s.kind && d.installed()))) {
                                {
                                    let k = spec.kind.clone();
                                    let on = multi_now.contains(&k);
                                    let logo = agent_logo(&k);
                                    rsx! {
                                        button { key: "{k.id()}", class: "menu-item",
                                            onclick: move |e| {
                                                e.stop_propagation();
                                                let mut v = multi.write();
                                                if let Some(i) = v.iter().position(|x| *x == k) { v.remove(i); } else { v.push(k.clone()); }
                                            },
                                            span { class: "lg", if let Some(svg) = logo { Logo { svg, size: 14 } } }
                                            span { "{spec.name}" }
                                            if on { span { class: "check", Icon { i: I::Check, size: 13 } } }
                                        }
                                    }
                                }
                            }
                            div { class: "menu-sep" }
                            MenuItem { label: "Clear".to_string(), onclick: move |_| { multi.set(Vec::new()); multi_menu.set(false); } }
                        }
                    }
                }
            } else {
                div { class: "below-composer",
                    if let Some(b) = branch.read().clone().flatten() {
                        span { class: "env-chip", style: "cursor: default;", title: "Current branch", Icon { i: I::GitBranch, size: 13, stroke: 1.5 } "{b}" }
                    }
                    EnvPicker { open: env_menu, worktree, is_repo, session: true, label: env_text.clone(), folder: env_folder.clone(), existing: existing.clone() }
                    div { class: "grow" }
                    super::meters::PlanRing {
                        agent: agent.clone(),
                        account_id: sess.read().as_ref().and_then(|s| s.account.clone()),
                        usage: usage.clone(),
                        cost: sess.read().as_ref().and_then(|s| s.cost),
                    }
                    if let Some(id) = session_id.clone() {
                        super::meters::ContextRing {
                            session_id: id,
                            agent: agent.clone(),
                            context: sess.read().as_ref().and_then(|s| s.context),
                            messages: sess.read().as_ref().map(|s| s.messages).unwrap_or(0),
                        }
                    }
                }
            }
            if home {
                div { style: "position: fixed; bottom: 22px; left: var(--sidebar-w, 300px); right: 0; text-align: center; color: var(--fg-3); font-size: var(--ui-sm); pointer-events: none;",
                    "{TIPS[tip_idx]}"
                }
            }
        }
    }
}

/// Agents that can be picked: enabled and installed (API agents need a
/// provider), plus the one in use.
pub(super) fn pickable_agents(app: &App, current: &AgentKind) -> Vec<(AgentKind, String, String)> {
    let cfg = app.config.read();
    let detected = app.detected.read();
    let mut out: Vec<(AgentKind, String, String)> = Vec::new();
    for spec in larrier_core::agent::runnable() {
        let det = detected.iter().find(|d| d.kind == spec.kind);
        let api = spec.binaries.is_empty();
        let installed = api || det.map(|d| d.installed()).unwrap_or(false);
        let usable = if spec.protocol == Protocol::HttpChat { !cfg.accounts_for(&spec.kind).is_empty() } else { installed };
        if !(cfg.agent(&spec.kind).enabled && usable) && spec.kind != *current {
            continue;
        }
        let sub = if api { "API".to_string() } else { det.and_then(|d| d.version.clone()).map(|v| format!("v{v}")).unwrap_or_default() };
        out.push((spec.kind.clone(), spec.name.to_string(), sub));
    }
    for c in cfg.custom_agents.iter() {
        out.push((AgentKind::Custom(c.id.clone()), c.name.clone(), "custom".into()));
    }
    out
}

/// A row of the model picker.
#[derive(Clone, PartialEq)]
struct PickRow {
    /// What picking stores (`None`: the default); for Cursor, a family.
    id: Option<String>,
    label: String,
    note: String,
    meta: String,
    tip: String,
    /// Model id for its maker's logo.
    logo_id: Option<String>,
}

/// The picker's rows for an agent: its default, then the models shown in
/// Larrier's picker, or every model it offers when `all` (search).
fn pick_rows(app: &App, kind: &AgentKind, all: bool) -> Vec<PickRow> {
    use crate::agent_models as am;
    let mut rows = Vec::new();
    if let Some(list) = listed_models(app, kind) {
        rows.push(PickRow { id: None, label: "Auto".into(), note: "Default".into(), meta: String::new(), tip: "The default model".into(), logo_id: None });
        for m in list.into_iter().filter(|m| m != "default" && m != "auto") {
            let (label, note, meta, tip) = model_row_text(&Some(m.clone()));
            rows.push(PickRow { id: Some(m.clone()), label, note, meta, tip, logo_id: Some(m) });
        }
        return rows;
    }
    // The default row says what it is: Larrier's default, or the agent's own.
    let r = am::resolve(app, kind, None, None);
    let mut def_note: Vec<String> = r.model.as_deref().map(|m| am::short_name(app, kind, r.family.as_deref().unwrap_or(m))).into_iter().collect();
    def_note.extend(r.effort.as_deref().map(larrier_agents::models::effort_label));
    let whose = if r.model_from_agent { format!("{}'s own default (from its settings)", kind.display_name()) } else { "Your default for this agent in Larrier".to_string() };
    rows.push(PickRow { id: None, label: "Default".into(), note: def_note.join(" · "), meta: String::new(), tip: whose, logo_id: r.model.clone() });
    let choices = if all { am::choices(app, kind) } else { am::picker_choices(app, kind) };
    for c in choices {
        let (_, reg_note, meta, reg_tip) = model_row_text(&Some(c.id.clone()));
        let mut note: Vec<String> = Vec::new();
        if let Some(e) = &c.endpoint {
            note.push(format!("via {e}"));
        } else if c.added || c.custom {
            note.push("Custom".into());
        }
        if let Some(n) = c.note.clone().filter(|n| n.len() < 40) {
            note.push(n);
        } else if !reg_note.is_empty() && reg_note != "Default" && !c.id.contains('/') {
            note.push(reg_note);
        }
        let meta = if meta.is_empty() { c.context.map(larrier_core::models::tokens_short).unwrap_or_default() } else { meta };
        let tip = if c.efforts.is_empty() { reg_tip } else { format!("{reg_tip}
Effort: {}", c.efforts.iter().map(|e| larrier_agents::models::effort_label(e)).collect::<Vec<_>>().join(", ")) };
        rows.push(PickRow { id: Some(c.id.clone()), label: c.name.trim_start_matches("Claude ").to_string(), note: note.join(" · "), meta, tip, logo_id: Some(c.id) });
    }
    rows
}

/// Agent + model picker: a search box, a rail of icons (the favorites, then
/// each harness) and the chosen tab's models beside it; typing searches every
/// model each harness offers. A model's star keeps it in the favorites and
/// its Edit button opens its properties (context, effort, Fast). Fixed size
/// so it never jumps while you browse.
#[component]
fn ModelPicker(
    open: Signal<bool>,
    up: bool,
    agent: AgentKind,
    model: Option<String>,
    effort: Option<String>,
    /// The account the chat runs as (None: the agent's default).
    account: Option<String>,
    onpick: EventHandler<(AgentKind, Option<String>)>,
    oneffort: EventHandler<(Option<String>, bool)>,
    oncontext: EventHandler<String>,
    onaccount: EventHandler<String>,
) -> Element {
    let app = use_app();
    let mut query = use_signal(String::new);
    // The rail's tab: a harness, or the favorites (`None`).
    let mut browse = use_signal(|| Some(agent.clone()));
    // The model whose properties are open (agent, id, where its Edit was).
    let mut edit = use_signal(|| None::<(AgentKind, String, f64, f64)>);
    let is_open = open();
    let a0 = agent.clone();
    use_effect(use_reactive!(|is_open| {
        if is_open {
            browse.set(Some(a0.clone()));
            query.set(String::new());
        }
        edit.set(None);
    }));
    // Beside its Edit button: to the right when there's room, else to the
    // left, never over the button.
    let edit_at = edit().map(|(_, _, x, y)| (x, y));
    use_effect(use_reactive!(|edit_at| {
        if edit_at.is_some() {
            document::eval(
                "requestAnimationFrame(() => { const m = document.getElementById('mp-edit'); const b = document.querySelector('.mp-row.editing .mp-edit-btn:not(.mp-reset)'); if (!m || !b) return;
                  const r = b.getBoundingClientRect(), vw = innerWidth, vh = innerHeight, pad = 8;
                  m.style.maxHeight = (vh - pad * 2) + 'px';
                  const w = m.offsetWidth, h = Math.min(m.scrollHeight, vh - pad * 2);
                  let left = r.right + 8; if (left + w > vw - pad) left = r.left - w - 8;
                  let top = r.top - 10; if (top + h > vh - pad) top = vh - pad - h;
                  m.style.left = Math.max(pad, left) + 'px'; m.style.top = Math.max(pad, top) + 'px'; });",
            );
        }
    }));
    let agents = pickable_agents(&app, &agent);
    let q = query.read().trim().to_lowercase();
    let tab = browse();
    let cur = tab.clone().unwrap_or_else(|| agent.clone());
    let cur_meta = agents.iter().find(|(k, _, _)| *k == cur).cloned();
    // The family in use (Cursor stores a variant of it).
    let now = crate::agent_models::resolve(&app, &agent, model.as_deref(), effort.as_deref());
    let in_use = |row: &Option<String>| -> bool {
        match (row, &model) {
            (None, None) => true,
            (Some(r), Some(m)) => r == m || now.family.as_deref() == Some(r.as_str()),
            _ => false,
        }
    };
    let results: Vec<(AgentKind, String, PickRow)> = if q.is_empty() {
        Vec::new()
    } else {
        let mut v = Vec::new();
        for (k, name, _) in agents.iter() {
            let agent_hit = name.to_lowercase().contains(&q);
            for row in pick_rows(&app, k, true) {
                let raw = row.id.clone().unwrap_or_default().to_lowercase();
                if agent_hit || row.label.to_lowercase().contains(&q) || raw.contains(&q) {
                    v.push((k.clone(), name.clone(), row));
                }
            }
        }
        v.truncate(200);
        v
    };
    let typed = query.read().trim().to_string();
    let cur_name = cur_meta.as_ref().map(|m| m.1.clone()).unwrap_or_else(|| cur.display_name().to_string());
    // The model in use and its effort and speed (its Edit menu checks them).
    let family = now.family.clone().or(now.model.clone());
    let (cur_effort, cur_fast, cur_context) = (now.effort.clone(), now.fast, now.context.clone());
    let cur_choices = crate::agent_models::choices(&app, &cur);
    let editable = |id: &Option<String>| -> bool {
        id.as_ref().and_then(|id| cur_choices.iter().find(|c| &c.id == id)).is_some_and(|c| !c.efforts.is_empty() || c.fast || c.context.is_some() || c.params.is_some())
    };
    // A row's star: in the favorites or not (the default row can't be).
    let star = move |k: AgentKind, id: Option<String>| {
        if let Some(m) = id {
            app.update_config(move |c| c.settings.toggle_favorite(&k, &m));
        }
    };
    let favorites = app.config.read().settings.favorite_models.clone();
    let starred = |k: &AgentKind, id: &Option<String>| id.as_ref().is_some_and(|m| favorites.iter().any(|f| &f.agent == k && &f.model == m));
    // Favorites, under the harness that runs each.
    let fav_groups: Vec<(AgentKind, String, Vec<PickRow>)> = {
        let mut out: Vec<(AgentKind, String, Vec<PickRow>)> = Vec::new();
        for (k, name, _) in agents.iter() {
            let mine: Vec<&larrier_core::FavoriteModel> = favorites.iter().filter(|f| &f.agent == k).collect();
            if mine.is_empty() {
                continue;
            }
            let all = pick_rows(&app, k, true);
            let rows: Vec<PickRow> = mine
                .iter()
                .map(|f| {
                    all.iter().find(|r| r.id.as_deref() == Some(f.model.as_str())).cloned().unwrap_or_else(|| PickRow {
                        id: Some(f.model.clone()),
                        label: f.model.clone(),
                        note: String::new(),
                        meta: String::new(),
                        tip: f.model.clone(),
                        logo_id: Some(f.model.clone()),
                    })
                })
                .collect();
            out.push((k.clone(), name.clone(), rows));
        }
        out
    };
    let fav = tab.is_none();
    let sub_now = cur_meta.as_ref().map(|m| m.2.clone()).unwrap_or_default();
    rsx! {
        Dropdown { open, up, right: true, class: "model-picker",
            div { class: "mp-search",
                Icon { i: I::Search, size: 12 }
                input {
                    placeholder: "Search models",
                    value: "{query}",
                    autofocus: true,
                    spellcheck: "false",
                    oninput: move |e| query.set(e.value()),
                    onkeydown: {
                        let cur = cur.clone();
                        let first = results.first().cloned();
                        move |e: KeyboardEvent| {
                            if e.key() == Key::Enter {
                                e.prevent_default();
                                match &first {
                                    Some((k, _, row)) => onpick.call((k.clone(), row.id.clone())),
                                    None => {
                                        let t = query.read().trim().to_string();
                                        if !t.is_empty() {
                                            onpick.call((cur.clone(), Some(t)));
                                        }
                                    }
                                }
                            }
                        }
                    },
                }
            }
            div { class: "mp-body",
                // The rail: favorites, then each harness by its icon.
                div { class: "mp-rail",
                    button { class: if fav && q.is_empty() { "mp-agent mp-tab on" } else { "mp-agent mp-tab" }, title: "Favorites",
                        onclick: move |_| { browse.set(None); query.set(String::new()); },
                        Icon { i: I::Star, size: 14, stroke: 1.5 }
                    }
                    div { class: "mp-rail-sep" }
                    for (k, name, sub) in agents.clone() {
                        {
                            let on = !fav && k == cur && q.is_empty();
                            let using = k == agent;
                            let k1 = k.clone();
                            let logo = agent_logo(&k);
                            let tip = if using { format!("{name} · in use") } else if sub.is_empty() { name.clone() } else { format!("{name} · {sub}") };
                            rsx! {
                                button { key: "{k.id()}", class: if on { "mp-agent mp-tab on" } else { "mp-agent mp-tab" }, title: "{tip}",
                                    onclick: move |_| { browse.set(Some(k1.clone())); query.set(String::new()); },
                                    if let Some(svg) = logo { Logo { svg, size: 14 } } else { Icon { i: I::Bot, size: 14 } }
                                    if using { span { class: "mp-using" } }
                                }
                            }
                        }
                    }
                    div { class: "grow" }
                    button { class: "mp-agent mp-tab", title: "Manage {cur_name} models",
                        onclick: {
                            let k = cur.clone();
                            move |_| {
                                let mut o = open;
                                o.set(false);
                                app.open_models(Some(k.clone()));
                            }
                        },
                        Icon { i: I::Settings, size: 14, stroke: 1.5 }
                    }
                }
                div { class: "mp-list",
                    if !q.is_empty() {
                        // Search: every model each harness offers, by harness.
                        for (gk, gname) in agents.iter().filter(|(k, _, _)| results.iter().any(|(rk, _, _)| rk == k)).map(|(k, n, _)| (k.clone(), n.clone())) {
                            div { key: "h-{gk.id()}", class: "mp-group", "{gname}" }
                            for (k, _, row) in results.iter().filter(|(rk, _, _)| *rk == gk).cloned() {
                                PickRowItem { key: "{k.id()}-{row.id.clone().unwrap_or_default()}",
                                    agent: k.clone(), row: row.clone(),
                                    checked: k == agent && in_use(&row.id), starred: starred(&k, &row.id),
                                    can_edit: false, editing: false, reset: false,
                                    onpick: { let (k, id) = (k.clone(), row.id.clone()); move |_| onpick.call((k.clone(), id.clone())) },
                                    onstar: { let (k, id) = (k.clone(), row.id.clone()); move |_| star(k.clone(), id.clone()) },
                                    onedit: move |_| {},
                                    onreset: move |_| {},
                                }
                            }
                        }
                        if results.is_empty() {
                            div { class: "mp-empty", "No models match." }
                        }
                        {
                            let k = cur.clone();
                            let t = typed.clone();
                            rsx! {
                                button { class: "menu-item mp-row mp-use",
                                    onclick: move |_| onpick.call((k.clone(), Some(t.clone()))),
                                    span { class: "lg", Icon { i: I::CornerDownLeft, size: 13 } }
                                    span { class: "mp-name", span { class: "mp-label", "Use “{typed}”" } span { class: "mp-note", "with {cur_name}" } }
                                }
                            }
                        }
                    } else if fav {
                        if fav_groups.is_empty() {
                            div { class: "mp-empty",
                                Icon { i: I::Star, size: 16, stroke: 1.5 }
                                span { "Star a model to keep it here." }
                            }
                        }
                        for (gk, gname, rows) in fav_groups.clone() {
                            div { key: "f-{gk.id()}", class: "mp-group", "{gname}" }
                            for row in rows {
                                {
                                    let choices = crate::agent_models::choices(&app, &gk);
                                    let can_edit = row.id.as_ref().and_then(|id| choices.iter().find(|c| &c.id == id)).is_some_and(|c| !c.efforts.is_empty() || c.fast || c.context.is_some() || c.params.is_some());
                                    let editing = edit().is_some_and(|(ek, eid, _, _)| ek == gk && Some(eid) == row.id);
                                    let (k1, k2, k3) = (gk.clone(), gk.clone(), gk.clone());
                                    let (m1, m2, m3) = (row.id.clone(), row.id.clone(), row.id.clone().unwrap_or_default());
                                    rsx! {
                                        PickRowItem { key: "{gk.id()}-{m3}",
                                            agent: gk.clone(), row: row.clone(),
                                            checked: gk == agent && in_use(&row.id), starred: true,
                                            can_edit, editing, reset: false,
                                            onpick: move |_| onpick.call((k1.clone(), m1.clone())),
                                            onstar: move |_| star(k2.clone(), m2.clone()),
                                            onedit: move |(x, y): (f64, f64)| edit.set(if editing { None } else { Some((k3.clone(), m3.clone(), x, y)) }),
                                            onreset: move |_| {},
                                        }
                                    }
                                }
                            }
                        }
                    } else {
                        div { class: "mp-group",
                            "{cur_name}"
                            if !sub_now.is_empty() { span { class: "sub", "{sub_now}" } }
                        }
                        // The accounts it can run as (Settings → Harnesses). Larrier
                        // Chat's model already names its account.
                        {
                            let cfg = app.config.read();
                            let accounts: Vec<larrier_core::Account> = if cur == agent && cur != AgentKind::Chat { cfg.accounts_for(&cur).into_iter().cloned().collect() } else { Vec::new() };
                            let using = account.clone().or_else(|| cfg.default_account(&cur).map(|a| a.id.clone()));
                            rsx! {
                                if accounts.len() > 1 {
                                    div { class: "mp-accounts",
                                        span { class: "lbl", "Runs as" }
                                        for a in accounts {
                                            {
                                                let on = using.as_deref() == Some(a.id.as_str());
                                                let (ok, email) = crate::state::signed_in_as(&a);
                                                let tip = match (ok, email) { (true, Some(e)) => format!("Signed in as {e}"), (true, None) => "Signed in".to_string(), (false, _) => "Not signed in".to_string() };
                                                let id = a.id.clone();
                                                rsx! {
                                                    button { key: "{a.id}", class: if on { "mp-acct on" } else if ok { "mp-acct" } else { "mp-acct off" }, title: "{tip}",
                                                        onclick: move |e| { e.stop_propagation(); onaccount.call(id.clone()); },
                                                        Icon { i: match a.auth { larrier_core::AccountAuth::Api(_) | larrier_core::AccountAuth::ApiKey { .. } => I::Key, larrier_core::AccountAuth::ConfigDir { .. } => I::Lock, _ => I::User }, size: 12 }
                                                        span { "{a.label}" }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        for row in pick_rows(&app, &cur, false) {
                            {
                                let checked = cur == agent && in_use(&row.id);
                                let can_edit = editable(&row.id);
                                let editing = edit().is_some_and(|(ek, eid, _, _)| ek == cur && Some(eid) == row.id);
                                // The model in use, set off its defaults: Reset
                                // takes it back to them.
                                let reset = checked && can_edit && (effort.is_some() || cur_fast);
                                let (k1, k2, k3, k4) = (cur.clone(), cur.clone(), cur.clone(), cur.clone());
                                let (m1, m2, m3, m4) = (row.id.clone(), row.id.clone(), row.id.clone().unwrap_or_default(), row.id.clone());
                                rsx! {
                                    PickRowItem { key: "{row.id.clone().unwrap_or_else(|| \"default\".into())}",
                                        agent: cur.clone(), row: row.clone(),
                                        checked, starred: starred(&cur, &row.id),
                                        can_edit, editing, reset,
                                        onpick: move |_| onpick.call((k1.clone(), m1.clone())),
                                        onstar: move |_| star(k2.clone(), m2.clone()),
                                        onedit: move |(x, y): (f64, f64)| edit.set(if editing { None } else { Some((k3.clone(), m3.clone(), x, y)) }),
                                        onreset: move |_| { edit.set(None); onpick.call((k4.clone(), m4.clone())); oneffort.call((None, false)); },
                                    }
                                }
                            }
                        }
                    }
                }
            }
            // A model's properties: its context, effort and options.
            if let Some((ek, eid, _, _)) = edit() {
                {
                    let choice = crate::agent_models::choices(&app, &ek).into_iter().find(|c| c.id == eid);
                    let using = ek == agent && (model.as_deref() == Some(eid.as_str()) || family.as_deref() == Some(eid.as_str()));
                    let (sel_effort, sel_fast) = if using { (cur_effort.clone(), cur_fast) } else { (choice.as_ref().and_then(|c| c.default_effort.clone()), false) };
                    let efforts = choice.as_ref().map(|c| c.efforts.clone()).unwrap_or_default();
                    let has_fast = choice.as_ref().is_some_and(|c| c.fast);
                    let context = choice.as_ref().and_then(|c| c.context);
                    // Context sizes to choose from (Cursor), the one in use checked.
                    let params = choice.as_ref().and_then(|c| c.params.clone()).filter(|p| p.contexts.len() > 1);
                    let sel_context = params.as_ref().and_then(|p| if using { cur_context.clone().or(p.default_context.clone()) } else { p.default_context.clone() });
                    let (ek2, eid2) = (ek.clone(), eid.clone());
                    let pick_context = move |c: String| {
                        let mut ed = edit;
                        ed.set(None);
                        if !using {
                            onpick.call((ek2.clone(), Some(eid2.clone())));
                        }
                        oncontext.call(c);
                    };
                    // The model in use keeps what else was picked for it (its context).
                    let pick = move |e: Option<String>, fast: bool| {
                        let mut ed = edit;
                        ed.set(None);
                        if !using {
                            onpick.call((ek.clone(), Some(eid.clone())));
                        }
                        oneffort.call((e, fast));
                    };
                    let pick2 = pick.clone();
                    rsx! {
                        div { class: "mp-edit-backdrop", onclick: move |e| { e.stop_propagation(); edit.set(None); } }
                        div { id: "mp-edit", class: "menu mp-edit", role: "menu", onclick: move |e| e.stop_propagation(),
                            if let Some(p) = params.clone() {
                                div { class: "menu-label", "Context" }
                                for (v, label) in p.contexts.clone() {
                                    {
                                        let on = sel_context.as_deref() == Some(v.as_str());
                                        let pc = pick_context.clone();
                                        rsx! {
                                            button { key: "{v}", class: if on { "menu-item checked" } else { "menu-item" },
                                                onclick: move |_| pc(v.clone()),
                                                span { "{label}" }
                                                span { class: "check", style: if on { "visibility: visible;" } else { "visibility: hidden;" }, Icon { i: I::Check, size: 13 } }
                                            }
                                        }
                                    }
                                }
                            } else if let Some(t) = context {
                                div { class: "menu-label", "Context" }
                                div { class: "menu-item checked static", title: "The model's context window",
                                    span { "{larrier_core::models::tokens_short(t)}" }
                                    span { class: "check", Icon { i: I::Check, size: 13 } }
                                }
                            }
                            if !efforts.is_empty() {
                                if context.is_some() || params.is_some() { div { class: "menu-sep" } }
                                div { class: "menu-label", "Effort" }
                                for e in efforts.clone() {
                                    {
                                        let on = sel_effort.as_deref() == Some(e.as_str());
                                        let e2 = e.clone();
                                        let pick = pick.clone();
                                        rsx! {
                                            button { key: "{e}", class: if on { "menu-item checked" } else { "menu-item" },
                                                onclick: move |_| pick(Some(e2.clone()), sel_fast),
                                                span { {larrier_agents::models::effort_label(&e)} }
                                                span { class: "check", style: if on { "visibility: visible;" } else { "visibility: hidden;" }, Icon { i: I::Check, size: 13 } }
                                            }
                                        }
                                    }
                                }
                            }
                            if has_fast {
                                if context.is_some() || params.is_some() || !efforts.is_empty() { div { class: "menu-sep" } }
                                div { class: "menu-label", "Options" }
                                {
                                    let se = sel_effort.clone();
                                    rsx! {
                                        button { class: "menu-item", title: "Faster responses, at a higher price",
                                            onclick: move |_| pick2(se.clone(), !sel_fast),
                                            span { "Fast" }
                                            span { class: if sel_fast { "toggle on sm" } else { "toggle sm" } }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// One model in the picker: its logo and name, and on hover its star and
/// Edit (context, effort, Fast).
#[component]
fn PickRowItem(
    agent: AgentKind,
    row: PickRow,
    checked: bool,
    starred: bool,
    can_edit: bool,
    editing: bool,
    reset: bool,
    onpick: EventHandler<()>,
    onstar: EventHandler<()>,
    onedit: EventHandler<(f64, f64)>,
    onreset: EventHandler<()>,
) -> Element {
    let mlogo = row.logo_id.as_deref().and_then(model_logo).or_else(|| agent_logo(&agent));
    let mut cls = String::from("menu-item mp-row");
    if checked {
        cls.push_str(" checked");
    }
    if editing {
        cls.push_str(" editing");
    }
    if starred {
        cls.push_str(" starred");
    }
    rsx! {
        button { class: "{cls}", title: "{row.tip}", onclick: move |_| onpick.call(()),
            span { class: "lg", if let Some(svg) = mlogo { Logo { svg, size: 13 } } else { Icon { i: I::Sparkle, size: 12, stroke: 1.5 } } }
            span { class: "mp-name",
                span { class: "mp-label", "{row.label}" }
                if !row.note.is_empty() { span { class: "mp-note", "{row.note}" } }
            }
            if !row.meta.is_empty() { span { class: "mp-meta", "{row.meta}" } }
            if reset {
                span { class: "mp-edit-btn mp-reset", role: "button", title: "Back to its default effort and options",
                    onclick: move |e: MouseEvent| { e.stop_propagation(); onreset.call(()); },
                    Icon { i: I::Undo, size: 12 }
                }
            }
            if can_edit {
                span { class: "mp-edit-btn", role: "button", title: "Context, effort and options",
                    onclick: move |e: MouseEvent| { e.stop_propagation(); let p = e.client_coordinates(); onedit.call((p.x, p.y)); },
                    "Edit"
                }
            }
            if row.id.is_some() {
                span { class: if starred { "mp-star on" } else { "mp-star" }, role: "button", title: if starred { "Remove from favorites" } else { "Add to favorites" },
                    onclick: move |e: MouseEvent| { e.stop_propagation(); onstar.call(()); },
                    Icon { i: I::Star, size: 12, stroke: 1.5 }
                }
            }
            span { class: "check", style: if checked { "visibility: visible;" } else { "visibility: hidden;" }, Icon { i: I::Check, size: 13 } }
        }
    }
}

/// Larrier changed the composer's text itself (see `put_gen`).
fn bump(mut g: Signal<u64>) {
    g += 1;
}

/// The "+" menu: attach files and images, mention a file.
#[component]
fn PlusMenu(open: Signal<bool>, attachments: Signal<Vec<Part>>, text: Signal<String>, put_gen: Signal<u64>, input_id: &'static str) -> Element {
    let app = use_app();
    let mut open = open;
    let mut text = text;
    rsx! {
        div { class: "menu-anchor",
            button { class: if open() { "plus-btn open" } else { "plus-btn" }, title: "Add files or mention", onclick: move |_| open.set(true),
                Icon { i: I::Plus, size: 16, stroke: 1.6 }
            }
            Dropdown { open, up: true,
                MenuItem { label: "Attach files or images".to_string(), icon: Some(I::Paperclip), onclick: move |_| { open.set(false); app.attach_files(attachments); } }
                MenuItem { label: "Mention a file".to_string(), sub: Some("@".to_string()), icon: Some(I::AtSign), onclick: move |_| {
                    open.set(false);
                    let t = text.read().clone();
                    text.set(if t.is_empty() || t.ends_with(' ') { format!("{t}@") } else { format!("{t} @") }); bump(put_gen);
                    document::eval(&format!("setTimeout(() => document.getElementById('{input_id}')?.focus(), 10)"));
                } }
            }
        }
    }
}

/// The chat's settings (the sliders beside +): its mode and permissions,
/// each a submenu, the agent's MCP servers and skills, and for a chat that
/// has started, Pin, Fork, its own CLI and Export as.
#[component]
fn ConfigMenu(session_id: Option<String>, agent: AgentKind, groups: Vec<ModeGroupView>, onpick: EventHandler<(usize, Option<String>)>) -> Element {
    let mut app = use_app();
    let mut open = use_signal(|| false);
    let pinned = session_id.as_ref().is_some_and(|id| app.sessions.read().iter().any(|s| &s.id == id && s.pinned));
    let tip = groups.iter().map(|g| g.options.get(g.chosen).map(|o| o.label).unwrap_or("")).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(" · ");
    let mut customize = move |kind: larrier_agents::harness::ItemKind, k: AgentKind| {
        app.ui.write().customize_focus = Some((Some(kind), Some(k)));
        app.navigate(crate::state::View::Customize);
    };
    rsx! {
        div { class: "menu-anchor",
            button { class: if open() { "plus-btn cfg-btn open" } else { "plus-btn cfg-btn" }, title: if tip.is_empty() { "Chat settings".to_string() } else { format!("Chat settings · {tip}") },
                onclick: move |_| open.set(true),
                Icon { i: I::Settings2, size: 16, stroke: 1.6 }
            }
            Dropdown { open, up: true, class: "cfg-menu".to_string(),
                for (gi, g) in groups.iter().cloned().enumerate() {
                    {
                        let chosen = g.options.get(g.chosen).cloned();
                        let icon = if gi == 0 { chosen.as_ref().map(|o| mode_icon(o.id)).unwrap_or(I::Infinity) } else { I::Lock };
                        rsx! {
                            super::SubMenu { key: "g{gi}", label: g.label.to_string(), icon, value: chosen.map(|o| o.label.to_string()),
                                for (oi, o) in g.options.iter().cloned().enumerate() {
                                    {
                                        let v = pick_value(&g, oi);
                                        let on = oi == g.chosen;
                                        let tip = if oi == g.default && !o.description.is_empty() { format!("{} (default)", o.description) } else { o.description.to_string() };
                                        rsx! {
                                            button { key: "{o.id}", class: if on { "menu-item checked" } else { "menu-item" }, title: "{tip}",
                                                onclick: move |_| { open.set(false); onpick.call((gi, v.clone())); },
                                                span { class: "muted", Icon { i: if gi == 0 { mode_icon(o.id) } else { permission_icon(o.id) }, size: 14 } }
                                                span { class: "ellipsis", "{o.label}" }
                                                if on { span { class: "check", Icon { i: I::Check, size: 13 } } }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                if !groups.is_empty() {
                    div { class: "menu-sep" }
                }
                MenuItem { label: "MCP servers".to_string(), icon: Some(I::Server), title: Some(format!("{}'s MCP servers, in Customize", agent.display_name())),
                    onclick: { let k = agent.clone(); move |_| { open.set(false); customize(larrier_agents::harness::ItemKind::Mcp, k.clone()); } } }
                MenuItem { label: "Skills".to_string(), icon: Some(I::Wand), title: Some(format!("{}'s skills, in Customize", agent.display_name())),
                    onclick: { let k = agent.clone(); move |_| { open.set(false); customize(larrier_agents::harness::ItemKind::Skill, k.clone()); } } }
                if let Some(id) = session_id.clone() {
                    div { class: "menu-sep" }
                    {
                        let (a, b, c, d) = (id.clone(), id.clone(), id.clone(), id.clone());
                        rsx! {
                            MenuItem { label: if pinned { "Unpin".to_string() } else { "Pin".to_string() }, icon: Some(if pinned { I::PinOff } else { I::Pin }), onclick: move |_| { open.set(false); app.toggle_pin(&a); } }
                            MenuItem { label: "Fork".to_string(), icon: Some(I::Fork), onclick: move |_| { open.set(false); app.fork(&b, None); } }
                            MenuItem { label: "Open in the agent's CLI".to_string(), icon: Some(I::Terminal), onclick: move |_| { open.set(false); app.open_in_terminal(&c); } }
                            super::SubMenu { label: "Export as", icon: I::Upload,
                                for f in larrier_formats::ExportFormat::ALL.iter().copied() {
                                    MenuItem { key: "{f.label()}", label: f.label().to_string(),
                                        onclick: { let d = d.clone(); move |_| { open.set(false); app.export(&d, f); } } }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// An icon for a permissions option (Codex's sandbox and approvals).
fn permission_icon(id: &str) -> I {
    let id = id.to_ascii_lowercase();
    if id.contains("read") {
        I::Eye
    } else if id.contains("full") || id.contains("bypass") || id.contains("yolo") {
        I::Zap
    } else if id.contains("review") {
        I::Check
    } else {
        I::Lock
    }
}

/// An icon for an agent's mode, from what it lets the agent do.
fn mode_icon(id: &str) -> I {
    let id = id.to_ascii_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| id.contains(w));
    if has(&["plan", "spec", "architect"]) {
        I::List
    } else if has(&["read", "ask"]) {
        I::Eye
    } else if has(&["bypass", "skip", "full", "yolo", "everything", "rush", "allow-all"]) {
        I::Zap
    } else if has(&["edit"]) {
        I::Pencil
    } else if has(&["code", "build", "agent"]) {
        I::Code
    } else if has(&["auto", "smart"]) {
        I::Infinity
    } else if has(&["low", "medium", "high"]) {
        I::Gauge
    } else {
        // Default, normal, manual: it asks first.
        I::Lock
    }
}

/// A project's other worktrees (the Existing worktree submenu): a search,
/// then those a chat runs in, by the chat's name, and the rest, by their
/// folder; each with its branch (or "detached") and commit.
#[component]
fn WorktreeList(trees: Option<Vec<larrier_agents::git::Worktree>>, query: Signal<String>, existing: Option<String>, onpick: EventHandler<String>) -> Element {
    let app = use_app();
    let mut query = query;
    let q = query.read().trim().to_lowercase();
    let sessions = app.sessions.read();
    // The chat running in a worktree: it says so (or made it: its branch
    // is `larrier/agent-<the chat's id's end>`).
    let user_of = |t: &larrier_agents::git::Worktree| {
        let same = |a: &str, b: &str| a.replace('\\', "/").trim_end_matches('/').eq_ignore_ascii_case(b.replace('\\', "/").trim_end_matches('/'));
        sessions
            .iter()
            .find(|s| s.worktree_path.as_deref().is_some_and(|p| same(p, &t.path)))
            .or_else(|| t.branch.as_deref().and_then(|b| b.strip_prefix("larrier/agent-")).and_then(|end| sessions.iter().find(|s| s.id.ends_with(end))))
            .map(|s| s.title.clone())
    };
    let rows: Vec<(larrier_agents::git::Worktree, Option<String>)> = trees.clone().unwrap_or_default().into_iter().map(|t| { let u = user_of(&t); (t, u) }).collect();
    let folder = |t: &larrier_agents::git::Worktree| std::path::Path::new(&t.path).file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_else(|| t.path.clone());
    let shown: Vec<(larrier_agents::git::Worktree, Option<String>)> = rows
        .into_iter()
        .filter(|(t, u)| q.is_empty() || [u.clone().unwrap_or_default(), folder(t), t.name()].iter().any(|s| s.to_lowercase().contains(&q)))
        .collect();
    let in_use: Vec<_> = shown.iter().filter(|(_, u)| u.is_some()).cloned().collect();
    let free: Vec<_> = shown.iter().filter(|(_, u)| u.is_none()).cloned().collect();
    let row = move |t: larrier_agents::git::Worktree, user: Option<String>| {
        let title = user.unwrap_or_else(|| folder(&t));
        let at = format!("{} @ {}", t.branch.clone().unwrap_or_else(|| "detached".into()), t.head.clone().unwrap_or_default());
        let on = existing.as_deref() == Some(t.path.as_str());
        let p = t.path.clone();
        rsx! {
            button { key: "{t.path}", class: if on { "menu-item wt-row checked menu-fresh" } else { "menu-item wt-row menu-fresh" }, title: "{t.path}",
                onclick: move |_| onpick.call(p.clone()),
                span { class: "muted", Icon { i: I::GitBranch, size: 14 } }
                span { class: "wt-text",
                    span { class: "ellipsis wt-title", "{title}" }
                    span { class: "ellipsis wt-sub", "{at}" }
                }
                if on { span { class: "check", Icon { i: I::Check, size: 13 } } }
            }
        }
    };
    rsx! {
        div { class: "menu-search",
            Icon { i: I::Search, size: 13 }
            input { placeholder: "Search worktrees", value: "{query}", spellcheck: "false", oninput: move |e| query.set(e.value()) }
        }
        match trees {
            None => rsx! { super::MenuSkeleton { rows: 3, two_line: true } },
            Some(all) if all.is_empty() => rsx! { div { class: "menu-empty", "No other worktrees" } },
            Some(_) => rsx! {
                if shown.is_empty() {
                    div { class: "menu-empty", "No worktrees match" }
                }
                if !in_use.is_empty() {
                    div { class: "menu-label", "In use" }
                    for (t, u) in in_use { {row(t, u)} }
                }
                if !free.is_empty() {
                    div { class: "menu-label", "Not in use" }
                    for (t, u) in free { {row(t, u)} }
                }
            },
        }
    }
}

/// The branch a new chat starts on (far right above the composer): the
/// project's branches, its default marked; picked for a new worktree it's
/// the branch the worktree starts from, for the checkout itself it's
/// switched to. A chat running in a worktree that's already there keeps
/// that worktree's branch.
#[component]
fn BranchPicker(folder: Option<String>, current: Option<String>, new_tree: bool, existing: Option<String>) -> Element {
    let mut app = use_app();
    let mut open = use_signal(|| false);
    // `None` while they're read.
    let mut list = use_signal(|| None::<(Vec<String>, String)>);
    let mut q = use_signal(String::new);
    let base = app.draft.read().base_branch.clone();
    let shown_branch = if new_tree { base.clone().or(current.clone()) } else { current.clone() };
    let label = shown_branch.clone().unwrap_or_else(|| "Branch".into());
    let fixed = existing.is_some();
    let f2 = folder.clone();
    let query = q.read().trim().to_lowercase();
    let rows: Vec<String> = list.read().as_ref().map(|(b, _)| b.iter().filter(|n| query.is_empty() || n.to_lowercase().contains(&query)).cloned().collect()).unwrap_or_default();
    let default = list.read().as_ref().map(|(_, d)| d.clone());
    let many = list.read().as_ref().is_some_and(|(b, _)| b.len() > 8);
    rsx! {
        div { class: "menu-anchor branch-pick",
            button { class: "env-chip", disabled: fixed,
                title: if fixed { "The worktree's branch".to_string() } else if new_tree { "The branch the new worktree starts from".to_string() } else { "The branch checked out in the project".to_string() },
                onclick: move |_| {
                    let Some(f) = f2.clone() else { return };
                    list.set(None);
                    q.set(String::new());
                    open.set(true);
                    spawn(async move {
                        let got = tokio::task::spawn_blocking(move || {
                            let p = std::path::Path::new(&f);
                            (larrier_agents::git::branches(p), larrier_agents::git::default_branch(p))
                        })
                        .await
                        .ok();
                        // The default branch first, then the most recent.
                        let (mut names, default) = got.unwrap_or_default();
                        if let Some(i) = names.iter().position(|b| *b == default) {
                            let d = names.remove(i);
                            names.insert(0, d);
                        }
                        list.set(Some((names, default)));
                    });
                },
                Icon { i: I::GitBranch, size: 14, stroke: 1.5 }
                span { class: "ellipsis", "{label}" }
                if !fixed { Icon { i: I::ChevronDown, size: 12 } }
            }
            Dropdown { open, up: false, right: true, class: "branch-menu".to_string(),
                if many {
                    div { class: "menu-search",
                        Icon { i: I::Search, size: 13 }
                        input { placeholder: "Search branches", value: "{q}", autofocus: true, spellcheck: "false", oninput: move |e| q.set(e.value()) }
                    }
                }
                {
                    let pick = move |b: String| {
                        open.set(false);
                        if new_tree {
                            app.draft.write().base_branch = Some(b);
                        } else if let Some(f) = folder.clone() {
                            spawn(async move {
                                let b2 = b.clone();
                                let r = tokio::task::spawn_blocking(move || larrier_agents::git::switch(std::path::Path::new(&f), &b2)).await;
                                match r {
                                    Ok(Ok(())) => app.toast(format!("Switched to {b}")),
                                    Ok(Err(e)) => app.toast_err(format!("Couldn't switch to {b}: {e}")),
                                    Err(_) => {}
                                }
                            });
                        }
                    };
                    let sb = shown_branch.clone();
                    let item = move |b: String, fresh: bool| {
                        let on = sb.as_deref() == Some(b.as_str());
                        let is_default = default.as_deref() == Some(b.as_str());
                        let b2 = b.clone();
                        let mut pick = pick.clone();
                        rsx! {
                            button { key: "{b}", class: format!("menu-item{}{}", if on { " checked" } else { "" }, if fresh { " menu-fresh" } else { "" }), title: "{b}",
                                onclick: move |_| pick(b2.clone()),
                                span { class: "muted", Icon { i: I::GitBranch, size: 14 } }
                                span { class: "ellipsis", "{b}" }
                                if is_default { span { class: "sub", "Default" } }
                                if on { span { class: "check", Icon { i: I::Check, size: 13 } } }
                            }
                        }
                    };
                    match list.read().clone() {
                        // The branch in use shows at once; the rest load.
                        None => rsx! {
                            if let Some(b) = shown_branch.clone() { {item(b, false)} }
                            super::MenuSkeleton { rows: 2 }
                        },
                        Some(_) => rsx! {
                            for b in rows.clone() { {item(b, true)} }
                            if rows.is_empty() { div { class: "menu-empty", "No branches match" } }
                        },
                    }
                }
            }
        }
    }
}

/// This computer's name, as the system shows it ("MAIN").
fn host_name() -> Option<String> {
    std::env::var("COMPUTERNAME").or_else(|_| std::env::var("HOSTNAME")).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// Where a chat runs ("This PC ▾"): the machine (this PC; a cloud of your
/// own is coming), then, in a git project, how: in a new worktree, in one
/// that's already there, or in the checkout itself. One line a row. A chat
/// that has started shows where it runs; it can't move.
#[component]
fn EnvPicker(
    open: Signal<bool>,
    worktree: bool,
    is_repo: bool,
    session: bool,
    label: String,
    #[props(default)] folder: Option<String>,
    #[props(default)] existing: Option<String>,
) -> Element {
    let mut app = use_app();
    let mut open = open;
    // The project's worktrees: read in the background when the menu opens
    // (`None` while that runs), and at first when the chat runs in one, to
    // name it by its branch.
    let mut trees = use_signal(|| match (&existing, &folder) {
        (Some(_), Some(f)) => Some(larrier_agents::git::worktrees(std::path::Path::new(f))),
        _ => Some(Vec::new()),
    });
    let mut tree_q = use_signal(String::new);
    let host = host_name();
    let f2 = folder.clone();
    let in_tree = worktree && is_repo;
    let existing_name = existing.as_ref().map(|p| {
        trees
            .read()
            .iter()
            .flatten()
            .find(|t| &t.path == p)
            .map(|t| t.name())
            .unwrap_or_else(|| std::path::Path::new(p).file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default())
    });
    let set = move |wt: bool, path: Option<String>| {
        let mut d = app.draft.write();
        d.worktree = wt;
        d.existing_worktree = path;
    };
    rsx! {
        div { class: "menu-anchor",
            button { class: "env-chip",
                onclick: move |_| {
                    if !session && is_repo {
                        if let Some(f) = f2.clone() {
                            trees.set(None);
                            tree_q.set(String::new());
                            spawn(async move {
                                let list = tokio::task::spawn_blocking(move || larrier_agents::git::worktrees(std::path::Path::new(&f))).await.unwrap_or_default();
                                trees.set(Some(list));
                            });
                        }
                    }
                    open.set(true);
                },
                Icon { i: if in_tree { I::Layers } else { I::Laptop }, size: 14, stroke: 1.5 }
                span { {existing_name.clone().filter(|_| in_tree).unwrap_or(label)} }
                Icon { i: I::ChevronDown, size: 12 }
            }
            Dropdown { open, up: session, class: "env-menu".to_string(),
                div { class: "menu-label", "Run on" }
                div { class: "menu-item env-soon", title: "Coming soon",
                    span { class: "muted", Icon { i: I::Cloud, size: 14 } }
                    span { "Cloud" }
                    span { class: "env-faint ellipsis", "Run your own Cloud" }
                    span { class: "soon", "Coming soon" }
                }
                button { class: "menu-item checked", title: folder.clone().unwrap_or_default(), onclick: move |_| open.set(false),
                    span { class: "muted", Icon { i: I::Laptop, size: 14 } }
                    if let Some(h) = &host {
                        span { "{h}" }
                        span { class: "env-faint", "This PC" }
                    } else {
                        span { "This PC" }
                    }
                    if let Some(f) = &folder {
                        span { class: "env-faint env-path ellipsis", "{short_path(f)}" }
                    }
                    span { class: "check", Icon { i: I::Check, size: 13 } }
                }
                if is_repo {
                    div { class: "menu-sep" }
                    MenuItem { label: "New worktree".to_string(), icon: Some(I::Layers), checked: in_tree && existing.is_none(), disabled: session,
                        title: Some("A git checkout of its own, on a new branch".to_string()),
                        onclick: { let mut set = set; move |_| { open.set(false); set(true, None); } } }
                    super::SubMenu { label: "Existing worktree", icon: I::GitBranch, value: existing_name.clone(), checked: in_tree && existing.is_some(),
                        disabled: session,
                        WorktreeList { trees: trees.read().clone(), query: tree_q, existing: existing.clone(),
                            onpick: move |p: String| { open.set(false); let mut set = set; set(true, Some(p)); } }
                    }
                    MenuItem { label: "Current checkout".to_string(), icon: Some(I::Folder), checked: !in_tree, disabled: session,
                        title: Some("The project folder itself".to_string()),
                        onclick: { let mut set = set; move |_| { open.set(false); set(false, None); } } }
                }
                if session {
                    div { class: "env-note", Icon { i: I::Info, size: 12 } "A chat keeps the place it started in" }
                }
            }
        }
    }
}

#[derive(Clone, PartialEq)]
enum SuggAct {
    /// Turn the token being typed into this object.
    Chip(MentionChip),
    /// Run a Larrier command.
    Larrier(&'static str),
    /// Run the new chat as this agent of the agent's own.
    RunAs(String, AgentKind),
}

#[derive(Clone, PartialEq)]
struct Sugg {
    group: &'static str,
    icon: I,
    logo: Option<&'static str>,
    label: String,
    sub: String,
    act: SuggAct,
}

static HARNESS: OnceLock<Mutex<Option<Vec<HarnessItem>>>> = OnceLock::new();

/// Agent skills and commands (discovered once in the background).
fn harness_items(app: &App) -> Vec<HarnessItem> {
    let cell = HARNESS.get_or_init(|| Mutex::new(None));
    if let Some(v) = cell.lock().ok().and_then(|g| g.clone()) {
        return v;
    }
    static STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if !STARTED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        let ws: Vec<std::path::PathBuf> = app.config.read().workspaces.iter().map(|w| std::path::PathBuf::from(&w.path)).collect();
        std::thread::spawn(move || {
            let items = larrier_agents::harness::discover(&ws);
            if let Some(c) = HARNESS.get() {
                if let Ok(mut g) = c.lock() {
                    *g = Some(items);
                }
            }
        });
    }
    Vec::new()
}

fn suggestions(app: &App, text: &str, files: &[String], home: bool, agent: &AgentKind, current: Option<&str>, fresh: bool) -> Vec<Sugg> {
    let mut out = Vec::new();
    if let Some(q) = mention_query(text) {
        let ql = q.to_lowercase();
        // Files
        let mut hits: Vec<&String> = files.iter().filter(|p| p.to_lowercase().contains(&ql)).take(300).collect();
        hits.sort_by_key(|p| (!p.rsplit('/').next().unwrap_or(p).to_lowercase().starts_with(&ql), p.len()));
        for p in hits.into_iter().take(6) {
            let (dir, name) = match p.rsplit_once('/') { Some((d, n)) => (d.to_string(), n.to_string()), None => (String::new(), p.clone()) };
            out.push(Sugg { group: "Files", icon: I::File, logo: None, label: name, sub: dir, act: SuggAct::Chip(MentionChip::path(p, false)) });
        }
        // Folders
        let mut dirs: Vec<String> = files
            .iter()
            .filter_map(|p| p.rsplit_once('/').map(|(d, _)| d.to_string()))
            .filter(|d| d.to_lowercase().contains(&ql))
            .collect();
        dirs.sort();
        dirs.dedup();
        dirs.sort_by_key(|d| d.len());
        for d in dirs.into_iter().take(3) {
            out.push(Sugg { group: "Folders", icon: I::Folder, logo: None, label: format!("{d}/"), sub: String::new(), act: SuggAct::Chip(MentionChip::path(&format!("{d}/"), true)) });
        }
        // Other agents' conversations
        let sessions = app.sessions.read();
        for s in sessions.iter().filter(|s| Some(s.id.as_str()) != current && !s.archived && (ql.is_empty() || s.title.to_lowercase().contains(&ql))).take(4) {
            out.push(Sugg {
                group: "Agents",
                icon: I::Message,
                logo: crate::logos::agent_logo(&s.agent),
                label: s.title.clone(),
                sub: format!("{} · {}", s.agent.display_name(), crate::state::rel_time(s.updated_at)),
                act: SuggAct::Chip(MentionChip::agent(s)),
            });
        }
        // The agent's own agents: a chat that hasn't started can run as one
        // (listed first), and any chat can hand work to one as a subagent.
        if *agent == AgentKind::ClaudeCode {
            let logo = crate::logos::agent_logo(agent);
            let mut subs: Vec<(String, String)> = app.agent_models(agent).map(|m| m.subagents.iter().map(|s| (s.name.clone(), s.description.clone().unwrap_or_default())).collect()).unwrap_or_default();
            for it in harness_items(app).into_iter().filter(|it| it.kind == ItemKind::Subagent && it.agents.contains(agent)) {
                if !subs.iter().any(|(n, _)| *n == it.name) {
                    subs.push((it.name.clone(), it.description.clone().unwrap_or_default()));
                }
            }
            let subs: Vec<(String, String)> = subs.into_iter().filter(|(n, _)| ql.is_empty() || n.to_lowercase().contains(&ql) || "agent".starts_with(&ql)).take(6).collect();
            if fresh {
                // Claude Code's own built-ins aren't agents to run as.
                let own: Vec<Sugg> = subs
                    .iter()
                    .filter(|(n, _)| !matches!(n.as_str(), "general-purpose" | "Explore" | "Plan" | "statusline-setup" | "claude-code-guide" | "claude"))
                    .map(|(name, desc)| Sugg { group: "Run as", icon: I::Orbit, logo, label: name.clone(), sub: desc.chars().take(80).collect(), act: SuggAct::RunAs(name.clone(), agent.clone()) })
                    .collect();
                out.splice(0..0, own);
            }
            for (name, desc) in subs {
                let sub: String = desc.chars().take(80).collect();
                out.push(Sugg { group: "Subagents", icon: I::Orbit, logo, label: format!("@agent-{name}"), sub: sub.clone(), act: SuggAct::Chip(MentionChip::subagent(&name, &sub)) });
            }
        }
        if "git".starts_with(&ql) || ql.starts_with("git") || "diff".starts_with(&ql) {
            out.push(Sugg { group: "Context", icon: I::Diff, logo: None, label: "Git diff".into(), sub: "uncommitted changes".into(), act: SuggAct::Chip(MentionChip::git_diff()) });
        }
        return out;
    }
    if let Some(q) = slash_query(text) {
        let ql = q.to_lowercase();
        for (c, d) in SLASH.iter().copied() {
            if c[1..].starts_with(&ql) && (home || !matches!(c, "/worktree" | "/local")) {
                out.push(Sugg { group: "Larrier", icon: I::Command, logo: None, label: c.to_string(), sub: d.to_string(), act: SuggAct::Larrier(c) });
            }
        }
        let logo = crate::logos::agent_logo(agent);
        let mut seen = std::collections::BTreeSet::new();
        for c in app.agent_models(agent).map(|m| m.commands).unwrap_or_default() {
            if !c.name.to_lowercase().contains(&ql) || !seen.insert(c.name.clone()) {
                continue;
            }
            let mcp = c.name.starts_with("mcp__");
            let desc: String = c.hint.clone().map(|h| format!("{h} · ")).unwrap_or_default() + &c.description.clone().unwrap_or_default();
            let desc: String = desc.chars().take(80).collect();
            out.push(Sugg {
                group: if mcp { "MCP prompts" } else { "Commands" },
                icon: if mcp { I::Plug } else { I::Slash },
                logo: if mcp { None } else { logo },
                label: format!("/{}", c.name),
                sub: desc.clone(),
                act: SuggAct::Chip(MentionChip::skill(&c.name, false, &desc)),
            });
            if out.len() > 60 {
                break;
            }
        }
        for it in harness_items(app).into_iter().filter(|it| matches!(it.kind, ItemKind::Command | ItemKind::Skill) && it.agents.contains(agent)) {
            let name = it.name.trim_start_matches('/').to_string();
            if !name.to_lowercase().contains(&ql) || !seen.insert(name.clone()) {
                continue;
            }
            let group = if it.kind == ItemKind::Skill { "Skills" } else { "Commands" };
            let desc: String = it.description.clone().unwrap_or_default().chars().take(80).collect();
            out.push(Sugg {
                group,
                icon: if it.kind == ItemKind::Skill { I::Sparkle } else { I::Slash },
                logo: if it.kind == ItemKind::Command { logo } else { None },
                label: format!("/{name}"),
                sub: desc.clone(),
                act: SuggAct::Chip(MentionChip::skill(&name, it.kind == ItemKind::Skill, &desc)),
            });
            if out.len() > 40 {
                break;
            }
        }
        out.sort_by_key(|s| match s.group { "Larrier" => 0, "Commands" => 1, "Skills" => 2, _ => 3 });
    }
    out
}

const SLASH: &[(&str, &str)] = &[
    ("/plan", "Switch to Plan mode"),
    ("/agent", "Switch to Agent mode"),
    ("/ask", "Switch to Ask mode"),
    ("/fork", "Fork this agent"),
    ("/export", "Export as Larrier session"),
    ("/terminal", "Open in the agent's CLI"),
    ("/worktree", "Run in an isolated worktree"),
    ("/local", "Run in the folder itself"),
    ("/new", "Start a new agent"),
    ("/mcp", "This agent's MCP servers"),
    ("/usage", "Plan and token usage"),
    ("/settings", "Open settings"),
];

/// Text after a trailing `@` token, if the caret is inside one.
fn mention_query(text: &str) -> Option<String> {
    if text.ends_with(char::is_whitespace) {
        return None;
    }
    let token = text.rsplit(|c: char| c.is_whitespace() || c == super::mentions::CLOSE).next()?;
    let q = token.strip_prefix('@')?;
    (!q.contains('@')).then(|| q.to_string())
}

/// `/command` typed first (after any objects).
fn slash_query(text: &str) -> Option<String> {
    let token = text.rsplit(|c: char| c.is_whitespace() || c == super::mentions::CLOSE).next()?;
    let before = &text[..text.len() - token.len()];
    let t = token.strip_prefix('/')?;
    strip_tokens(before).trim().is_empty().then(|| t.to_string())
}

/// The text without its objects' tokens.
fn strip_tokens(text: &str) -> String {
    let mut out = String::new();
    let mut inside = false;
    for c in text.chars() {
        match c {
            super::mentions::OPEN => inside = true,
            super::mentions::CLOSE => inside = false,
            _ if !inside => out.push(c),
            _ => {}
        }
    }
    out
}

const SKIP_DIRS: &[&str] = &[".git", "node_modules", "target", ".next", "dist", "build", "__pycache__", ".venv", "venv", ".idea", ".turbo", ".cache", "vendor"];

/// Relative file paths under `root` (forward slashes), breadth-limited.
fn list_files(root: &str, max: usize) -> Vec<String> {
    let base = std::path::Path::new(root);
    let mut out = Vec::new();
    let mut stack = vec![base.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let p = e.path();
            if p.is_dir() {
                if !SKIP_DIRS.contains(&name.as_str()) && !name.starts_with('.') {
                    stack.push(p);
                }
            } else if let Ok(rel) = p.strip_prefix(base) {
                out.push(rel.to_string_lossy().replace(std::path::MAIN_SEPARATOR, "/"));
                if out.len() >= max {
                    return out;
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn mentions() {
        assert_eq!(super::mention_query("look at @src/ma").as_deref(), Some("src/ma"));
        assert_eq!(super::mention_query("look at @src/main.rs "), None);
        assert_eq!(super::slash_query("/pl").as_deref(), Some("pl"));
        assert_eq!(super::slash_query("/plan this"), None);
    }
}
