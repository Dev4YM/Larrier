# Larrier

**One window for every coding agent and AI chat.** Larrier is a native desktop
app written in Rust for running, managing and moving conversations between
Claude Code, Codex, Cursor Agent, OpenCode, Gemini CLI, Pi, Hermes Agent,
Goose, Amp, Copilot CLI, Qwen Code, Kimi, Droid, Aider, any
[Agent Client Protocol](https://agentclientprotocol.com) agent, and plain
API chats (OpenAI, Anthropic, Gemini, OpenRouter, Ollama, and any
OpenAI-compatible endpoint).

Its interface follows Cursor's Agents Window:

* A sidebar that lists every agent: **In Progress**, **Ready for Review**,
  then grouped by workspace.
* A focused chat pane with sticky prompts, collapsed "Explored…" steps,
  command cards and inline diff cards.
* A review pane with git changes, commit, push and pull requests, plus a
  browser, a terminal and a file viewer.
* A command palette and keyboard-first navigation.

## The window

Larrier's window is laid out like Cursor 3's Agents Window:

- **Menu bar:** File / Edit / View / Help, drawn by Larrier and themed like the rest of the window.
- **Sidebar:**
  - New Agent, Automations and Customize at the top.
  - Pinned agents below them.
  - Every repository after that, with its agents nested under it, plus "No Repo" for chats without a folder.
  - Your profile at the bottom, and back / forward navigation.
- **Agent view:**
  - A title chip with the environment icon, and IDE ↗ to open the folder in Cursor or VS Code.
  - Prompts sit in sticky cards; click one to edit and resend it into a fork.
  - Finished turns collapse into "Worked for 16s", followed by 👍 👎 copy, fork and the time.
  - A turn navigator runs down the left, with a scroll-to-bottom button.
- **Composer:** a pill with "+", a combined agent + model picker showing brand logos, and send / stop.
  - Below it: the branch, "This PC" or a worktree, and token usage.
  - Above it: "Changes +N −M" and "Commit & Push ▾".
- **Tools:** an "On ‹repo›" list of Changes · Browser · Terminal · Files, which opens a side panel with tabs, a "+" menu and launcher tiles.
- **New agent screen:** a centered composer with repository and environment pickers, "Plan New Idea" (Shift+Tab cycles modes) and **Multitask**, which runs one prompt on several agents in parallel.
- **Automations:** scheduled, interval and on-commit agent runs, with 13 templates. They run locally with any agent you have installed.
- **Customize:** every MCP server, skill, subagent, rule, command, hook and plugin across Claude Code, Codex, Cursor, OpenCode, Gemini CLI, Hermes and Droid. It reads them from each agent's real config, and each item is tagged with the agents that use it.
- **Right-click:** Larrier's own context menu everywhere. It offers cut/copy/paste, "Quote in Follow-up", "Ask a New Agent", open or copy for links, copy for code blocks, and open or reveal for files.
- **Icons and logos:** the full [Lucide](https://lucide.dev) icon set (2,275 icons) and 81 brand logos for agents, model vendors and tools. Sources are listed in `app/assets/logos/LICENSES.md`.

## Highlights

- **Unified sessions.** Every conversation lives in the
  [Larrier Session Format](docs/LSF.md), one portable file per session with
  text, reasoning, tool calls with outputs, diffs, plans, images and usage.
- **Import everything.** Larrier reads the native session stores of Claude
  Code, Codex, Cursor (CLI and IDE), OpenCode, Gemini CLI, Pi, Hermes and
  Droid, plus ChatGPT and Claude.ai data exports. Imports are incremental:
  re-importing updates existing sessions instead of duplicating them.
- **Port any session to any agent.** Larrier writes real resumable native
  sessions for Claude Code, Codex, Gemini CLI and Pi, and hands the context
  to every other agent automatically.
- **Accounts per agent.** Each agent can have several identities: separate
  logins (isolated `CLAUDE_CONFIG_DIR`, `CODEX_HOME`, …), API keys stored in
  the OS keychain, or environment presets. You pick the account per session.
- **Parallel agents, isolated.** Optional per-agent git worktrees let many
  agents work on the same repo without collisions. Each worktree's branch can
  be merged back from the review pane.
- **Review and ship.** See a file-by-file diff of what the agent changed,
  then discard, commit, push or open a PR (`gh`) without leaving the window.
- **CLI included.** `larrier sessions | discover | sync | import | export |
  port | show | run`.

## Workspace

```
crates/
  core/     larrier-core     LSF types, agent catalog, config, accounts, keychain secrets, store
  formats/  larrier-formats  native session discovery, importers, native exporters, sync
  agents/   larrier-agents   runtime drivers (stream-json CLIs, ACP, HTTP chat), turn recorder, git
  cli/      larrier          command line
app/        larrier-desktop  Dioxus desktop UI (the agents window)
docs/LSF.md                  session format spec
```

## Build and run

Requires Rust 1.85 or newer. Windows also needs WebView2, which ships with
Windows 10 and 11.

```bash
cargo run -p larrier-app            # desktop app
```

```bash
cargo run -p larrier-cli -- --help  # CLI
```

```bash
cargo build --release               # optimized binaries in target/release
```

Data lives in `%APPDATA%\Larrier` on Windows, `~/Library/Application
Support/Larrier` on macOS, and `~/.local/share/larrier` on Linux. Set
`LARRIER_HOME` to override the location.

## Keyboard

| Action | Shortcut |
|---|---|
| New agent | Ctrl N |
| Command palette | Ctrl K / Ctrl Shift P |
| Toggle sidebar | Ctrl B |
| Toggle review pane | Ctrl J |
| Focus composer | Ctrl L |
| Stop agent | Ctrl . |
| Settings | Ctrl , |
| Import sessions | Ctrl Shift I |

On macOS, use Cmd in place of Ctrl.

## License

MIT
