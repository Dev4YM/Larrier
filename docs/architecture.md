# Architecture

Larrier is a Cargo workspace: three library crates, a command line and the
desktop app.

```
crates/
  core/      larrier-core     LSF types, agent catalog, config, accounts, keychain secrets, the store
  formats/   larrier-formats  native session discovery and importers, native exporters, sync, live watching
  agents/    larrier-agents   agent drivers, the turn engine, detection, models, versions, git, the browser tools
  cli/       larrier          the command line
app/         larrier-app      the desktop app (Dioxus 0.7: WebView2 on Windows, WKWebView on macOS, WebKitGTK on Linux)
docs/                         documentation; LSF.md is the session format spec
scripts/                      development, release and demo tooling
```

## larrier-core

- `lsf.rs`: the [Larrier Session Format](LSF.md): sessions, messages,
  parts, tool calls and diffs, JSON Lines and document forms.
- `agent.rs`: the agent catalog: names, programs, install commands, the
  protocol each speaks, its ACP command, modes and environment variables.
- `config.rs`: everything in `config.json`.
- `store.rs`: the data folder (sessions, index, import fingerprints).
- `secrets.rs`: API keys in the OS credential store.
- `models.rs`: the model registry (names, context windows, prices).

## larrier-formats

One module per agent (`claude.rs`, `codex.rs`, `cursor.rs`, `opencode.rs`,
`gemini.rs`, `pi.rs`, `hermes.rs`, `droid.rs`) that discovers its native
sessions and parses them into LSF, plus `chats.rs` for ChatGPT and
claude.ai exports. Agents that can resume external sessions (Claude Code,
Codex, Gemini CLI, Pi) also have exporters. `sync.rs` imports what changed;
`live.rs` watches the folders.

## larrier-agents

- `drivers/`: one driver per protocol. Each spawns the agent (or calls the
  API), feeds it the prompt and translates its stream into `AgentEvent`s:
  text, reasoning, tool starts and updates, diffs, plans, requests,
  subagent events, usage.
  - `claude.rs`: Claude Code's stdio control protocol; also Amp and Qwen
    Code.
  - `codex_app.rs`: `codex app-server` JSON-RPC (`codex.rs` keeps `exec`).
  - `acp.rs`: the Agent Client Protocol, used by OpenCode, Hermes, Goose,
    Copilot, Kimi and any custom agent.
  - `cursor.rs`, `gemini.rs`, `opencode.rs`, `pi.rs`, `hermes.rs`,
    `droid.rs`, `plain.rs`, `chat.rs` (HTTP providers).
- `engine.rs`: applies a turn's events to the LSF session; shared by the
  app and the CLI, so both record identical transcripts.
- `request.rs`: what a turn needs: program, arguments, environment,
  account, resume handle, context handoff.
- `detect.rs`, `models.rs`, `updates.rs`: finding agents, their models and
  their published versions.
- `harness.rs`: each agent's commands, skills, subagents, MCP servers and
  other configuration, for Customize and the `/` and `@` menus.
- `git.rs`: changes, commits, pushes, worktrees and pull requests.
- `browser/`: the browser views agents can drive, served to them as an MCP
  server (`Larrier mcp-browser`).

## The desktop app

- `main.rs`: startup, the headless installer and updater entry points, the
  window.
- `state.rs`: `App`, a `Copy` handle of Dioxus signals: config, sessions,
  per-session reactive state, running turns, the view and tabs. Every
  session has its own signals, so a streaming agent re-renders only its own
  turn. Turns run here: the app starts the driver, feeds its events to the
  engine, and handles stops, steering and queued messages.
- `ui/`: components: `sidebar`, `chat`, `transcript`, `composer`, `right`
  (the side panel), `terminal`, `files`, `settings`, `customize`,
  `automations`, `usage`, `profile`, overlays and menus.
- `keep.rs`: window state saved to `kept.json` (debounced, flushed on close)
  and restored on start.
- `browser_host.rs`, `browser_control.rs`: the Browser tab's tabs and
  the agents' control of them; `browser_cef.rs`: on Windows, the pages
  themselves, run on Chromium (CEF) inside Larrier's window, with the
  engine's runtime fetched on first use.
- `update.rs`, `install.rs`, `version.rs`: self-install (Windows) and
  updates from GitHub releases on every system (the `.exe`, the whole
  Larrier.app, or the Linux program, swapped on restart).
- `platform.rs`: what differs by system in the window and in how shortcuts
  read: Windows draws its own frame and buttons, macOS keeps the traffic
  lights and a system menu bar (`ui::menubar::system_menu`), Linux uses
  the system's frame; `Ctrl+K` reads `⌘K` on macOS.
- `agent_models.rs`, `agent_versions.rs`, `quota.rs`, `notify.rs`,
  `automations.rs`, `live.rs`: background work feeding the UI.

### Rendering long chats

Only the newest turns are mounted; older ones load as you scroll up, and
long turns reveal their steps on demand. Saving runs on a background
thread and large sessions load off the UI thread, so a 22 MB session opens
without a stall and frames stay under 10 ms while an agent streams.
