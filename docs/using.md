# Using Larrier

![Larrier with a chat running and its changes in the side panel](assets/agent.png)

## The window

**Menu bar.** File / Edit / View / Help and the window buttons, drawn by
Larrier and themed with the rest of the window.

**Sidebar.**

- New Agent, Automations and Customize at the top.
- Pinned chats, then every project with its chats nested under it. Chats
  without a folder live in **Home**.
- The filter menu groups by status, project or agent, sorts by activity,
  creation or name, and can show archived chats. Search grows out of the
  magnifier icon.
- A status dot on every chat: running, needs input, ready for review,
  unread.
- Your profile at the bottom, with Settings beside it. A dot on the
  Settings button means an agent has an update.

**Tabs.** A chat's subagents open as tabs next to it.

- A single click opens a preview tab (italic) that the next click
  replaces. Double-click, Ctrl-click or middle-click keeps it.
- Ctrl+W closes a tab and Ctrl+Tab cycles through them. Right-click a tab to
  close others, keep it, rename or fork it.

**Side by side and drag and drop.** Drag a chat from the sidebar or a tab
onto the chat area:

- **Side by side:** work in both chats at once, each with its own composer.
- **Open in new tab** or **Open here**.
- **Add as context:** reference the dropped chat in your next message.

## A chat

- Prompts sit in sticky cards. Click one to edit it and send it again as a
  fork. Esc or clicking elsewhere closes the editor; with unsaved changes
  it asks whether to keep editing.
- Reading, searching and thinking steps fold into "Explored …"; commands,
  edits (with inline diffs), plans and to-dos get their own cards.
- Finished turns fold into "Worked for 16s", followed by 👍 👎, copy, fork
  and the time.
- A prompt timeline runs down the left edge: hover it to preview a prompt,
  click to jump there.
- **Subagents and background work** are tracked live: a pill above the
  composer lists what's running, and each subagent opens its own
  transcript in a tab.
- **Requests from the agent** (approve a command, answer a question, accept
  a plan) appear inline and wait for you.

## The composer

- "+" for files and images, the agent and model picker, and send / stop.
- The picker lists your enabled agents on the left and their models on the
  right; typing searches every model. A model's **Edit** shows its options
  (context size, reasoning effort, speed) and the account the chat runs as.
- `@` mentions files, folders, other chats, agents and the git diff. `/`
  runs Larrier's commands and the agent's own commands and skills.
- Paste or drop images and files. Ctrl+Shift+V pastes long text as text.
- Messages sent while the agent works steer the running turn or wait in a
  queue (Settings → General; Ctrl+Enter does the other). A queued message can be sent at once or taken back.
- Below it: the branch, where the chat runs (this PC or a worktree), the
  agent's mode, and two rings for plan usage and context; hover them for
  the breakdown.

## The side panel

"On ‹project›" lists **Changes · Browser · Terminal · Files**. Each chat
keeps its own panel.

- **Changes:** Local Changes (git) and Agent Edits, file by file, with
  discard, commit, commit & push and Create PR (through `gh`). A
  worktree's branch can be merged back from here.
- **Browser:** a real browser in the panel. Agents can drive it through
  Larrier's browser tools while you watch; annotate mode lets you point at
  elements and send them to the agent. Drag a page into a chat as context.
- **Terminal:** real terminals (ConPTY on Windows), plus read-only views of
  the agent's own background commands.
- **Files:** tabs, quick open, a file tree and a highlighted viewer.

## New Agent

![New Agent](assets/new-agent.png)

A centered composer with the project and environment pickers. **Plan New
Idea** (Shift+Tab cycles modes) starts in planning, and **Multitask** runs
one prompt on several agents in parallel. A message left here waits as a
draft.

## Automations

![Automations](assets/automations.png)

Scheduled, interval and on-commit agent runs, with templates to start from.
They run on your computer with the agents you have installed.

## Customize

Every MCP server, skill, subagent, rule, command, hook and plugin across
Claude Code, Codex, Cursor, OpenCode, Gemini CLI, Hermes and Droid, read from
each agent's own configuration and tagged with the agents that use it.

## Settings

- **Profile:** your name, photo and a year of your AI work.
- **General:** notifications, completion sound, tray icon, what sending
  during a turn does (steer or queue), window restore.
- **Appearance:** themes, accent and per-token colors for light and dark,
  fonts, text sizes, density, button corners, sidebar side and **Reduce
  Motion**.
- **Plan & Usage:** plan limits with reset times; tokens, cost, sessions
  and cache hits against the previous period; breakdowns by agent, model
  and project; an activity heatmap.
- **Agents:** turn agents on or off, their programs, versions and updates,
  accounts, running options and permissions.
- **Accounts**, **Models & API Keys**, **Source Control**, **Sessions**
  (import and live sync), **Workspaces**, **Shortcuts** and **About**
  (version, update channel, release notes).

## Larrier reopens as you left it

Whether Larrier was closed, updated or stopped, it comes back on the same
chat or page, with each chat's tabs, side panel (browser pages, terminals
with their recent output, open files, the commit message being typed),
scroll position, queued and half-typed messages, the sidebar's state, and
the searches and filters of every page.

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
