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
  - "Needs Input", "In Progress" and "Ready for Review" sections, then pinned agents.
  - Every repository after that, with its agents nested under it, plus "No Repo" for chats without a folder.
  - The filter menu groups by status, repository or agent, sorts by activity, creation or name, and can show archived agents.
  - Your profile at the bottom, and back / forward navigation.
- **Tabs:** chats open in tabs next to the repository name, VS Code style.
  - A single click opens a preview tab (italic) that the next click replaces.
  - Double-click, Ctrl-click or middle-click keeps a tab. Sending a message keeps it too.
  - Ctrl+W closes a tab and Ctrl+Tab cycles through them. Right-click a tab to close others, keep it, rename or fork it.
- **Side by side and drag & drop:** drag any chat from the sidebar or a tab onto the chat area. Drop targets fade in:
  - **Side by side:** work in both chats at once, each with its own composer.
  - **Open in new tab** or **Open here** (replace the current tab).
  - **Add as context:** reference the dropped chat in your next message.
- **Agent view:**
  - IDE ↗ opens the folder in Cursor or VS Code.
  - Prompts sit in sticky cards. Click one to edit and resend it into a fork.
    - Only one prompt is edited at a time.
    - Esc or clicking elsewhere closes the editor; with unsaved changes it asks to keep editing or discard.
  - Finished turns collapse into "Worked for 16s", followed by 👍 👎 copy, fork and the time.
  - A turn navigator runs down the left, with a scroll-to-bottom button.
  - **Subagents and background tasks:** Claude Code agents, Codex `spawn_agent` children and background shells are tracked live.
    - A pill above the composer lists the running ones.
    - Each subagent opens its own transcript in a tab.
- **Composer:** a pill with "+", the agent + model picker, and send / stop.
  - The picker lists only enabled agents. Agents are on the left and models on the right, and typing searches every model.
  - Long or multi-line prompts give the text the full width, with the controls on a row below.
  - `@` mentions files, folders, other agents and the git diff. `/` runs Larrier commands and the agent's own skills and commands.
  - Paste or drop images and files. Prompts sent while the agent works are queued.
  - Below the composer:
    - the branch;
    - "This PC" or a worktree;
    - this chat's token usage (breakdown on hover);
    - the account's plan quota, such as "5h 70% · wk 14%".
  - Above it: "Changes +N −M" and "Commit & Push ▾".
- **Tools:** an "On ‹repo›" list of Changes · Browser · Terminal · Files, which opens a side panel with tabs, a "+" menu and launcher tiles.
- **New agent screen:** a centered composer with repository and environment pickers, "Plan New Idea" (Shift+Tab cycles modes) and **Multitask**, which runs one prompt on several agents in parallel.
- **Automations:** scheduled, interval and on-commit agent runs, with 13 templates. They run locally with any agent you have installed.
- **Customize:** every MCP server, skill, subagent, rule, command, hook and plugin across Claude Code, Codex, Cursor, OpenCode, Gemini CLI, Hermes and Droid. It reads them from each agent's real config, and each item is tagged with the agents that use it.
- **Settings:**
  - **Appearance:** a live preview, theme presets, accent and per-token colors for light and dark, fonts, sizes, density, sidebar side and **Reduce Motion**.
  - **Plan & Usage:**
    - plan limits per account, with reset times;
    - tokens, cost, sessions and cache hit rate compared with the previous period;
    - a daily chart, breakdowns by agent, model and workspace, an activity heatmap and your top sessions.
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
- **Fast with huge chats.**
  - Only the newest turns are mounted; older ones load as you scroll up, and long turns reveal their steps on demand.
  - Every session has its own reactive state, so a streaming agent re-renders only its own turn.
  - Saving runs on a background thread, and big sessions load off the UI thread.
  - A 22 MB Claude Code session opens without a stall, and frames stay under 10 ms while an agent streams.
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

## Install

Download `Larrier-<version>-windows-x64.exe` from the
[latest release](https://github.com/Dev4YM/Larrier/releases/latest) and run
it. It installs itself:

- **Just for me** — no administrator rights. The app goes to
  `%LOCALAPPDATA%\Programs\Larrier`, with everything else in your profile.
- **For everyone on this computer** — needs administrator rights. The app
  goes to `Program Files\Larrier` and shared files to
  `ProgramData\Larrier`; each person's sessions and settings stay in their
  own profile.

Either way you get a Start menu shortcut (and a Desktop one if you like),
and Larrier appears in Settings → Apps, where it uninstalls (optionally
with your data). **Run without installing** starts that copy as it is.

Installed copies check the latest release now and then (Settings → About):
they download its program, check it against its published SHA-256 and
swap it in on restart — no building, no source code. The repository is
private for now, so updates need a GitHub token (`LARRIER_GITHUB_TOKEN`,
`GITHUB_TOKEN`, or a signed-in GitHub CLI).

To publish a release, set the version in `Cargo.toml` and push a matching
tag (`git tag v0.2.0 && git push origin v0.2.0`): the Release workflow
builds the Windows program and attaches it with its checksum.

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
| Toggle side panel | Ctrl Shift J |
| Terminal / Changes / Files / Browser | Ctrl J / Ctrl E / Ctrl G / Ctrl Shift B |
| Close tab / next tab | Ctrl W / Ctrl Tab |
| Zoom in / out / reset | Ctrl = / Ctrl − / Ctrl 0 |
| Focus composer | Ctrl L |
| Stop agent | Ctrl . |
| Settings | Ctrl , |
| Import sessions | Ctrl Shift I |

On macOS, use Cmd in place of Ctrl.

## License

MIT
