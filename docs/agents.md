# Agents

Larrier calls everything that runs a chat a **harness**: each agent's own
program, and Larrier Chat, which talks to model APIs itself. It runs the
harness and translates what it streams into one transcript. Your
agents keep their own logins, settings and session stores; Larrier reads
those and, when you continue a chat with an agent, writes only what that
agent needs to resume.

## How each agent connects

| Agent | Program | Connection | Live approvals and questions |
|---|---|---|---|
| Claude Code | `claude` | stdio control protocol (stream-json in and out) | Yes |
| Codex | `codex` | `codex app-server` (JSON-RPC) | Yes |
| Cursor Agent | `cursor-agent` | stream-json print mode, or ACP (`cursor-agent acp`) | With ACP |
| OpenCode | `opencode` | ACP (`opencode acp`) by default, or `opencode run` | Yes |
| Gemini CLI | `gemini` | stream-json, or ACP (`gemini --acp`) | With ACP |
| Pi | `pi` | JSON mode, or ACP | With ACP |
| Hermes Agent | `hermes` | ACP (`hermes acp`) by default | Yes |
| Droid | `droid` | `droid exec` stream-jsonrpc | Yes |
| Goose | `goose` | ACP (`goose acp`) | Yes |
| Amp | `amp` | stream-json | — |
| Copilot CLI | `copilot` | ACP (`copilot --acp`) | Yes |
| Qwen Code | `qwen` | stream-json, or ACP | With ACP |
| Kimi | `kimi` | ACP (`kimi acp`) | Yes |
| Aider | `aider` | plain text | — |
| Custom | any command | ACP or one of the stream protocols above | Depends |
| Larrier Chat | — | HTTP: OpenAI, Anthropic, Gemini, OpenRouter, Ollama, OpenAI-compatible | — |

Agents with an ACP mode can be switched to it in **Settings → Harnesses →
‹agent› → Running**. Where an agent can't ask while Larrier runs it, Larrier
says what it refused and lets you change the permission mode.

## Modes and permissions

Each agent's real modes (plan, ask, auto-accept, …) and permission settings
are offered in the composer. Larrier passes them to the agent the way the
agent takes them (flags, ACP modes, app-server policies); it never widens
what you picked.

## Accounts

Every credential is an account of one harness, managed on that harness's
page in **Settings → Harnesses**. The default account runs new chats; a
chat can switch in the model picker. An account is one of:

- **Current sign-in:** whatever the agent is signed into on this computer
  (its subscription or login).
- **Separate sign-in:** an isolated config folder of its own
  (`CLAUDE_CONFIG_DIR`, `CODEX_HOME`, …). Sign in once from Larrier; it runs
  the agent's own login in a terminal.
- **Existing folder:** a folder an agent already signs in from.
- **API:** a provider's API (Anthropic, OpenAI, OpenRouter, Ollama, any
  OpenAI-compatible endpoint, a gateway) with its key in the OS keychain.
  Larrier points Claude Code, Codex, OpenCode, Droid and Hermes at it for
  the runs that use it, without changing their own settings; other agents
  get the key in the variable they read (`OPENAI_API_KEY`, …). Larrier
  Chat's accounts are all API accounts, each with the models it offers.

Each account shows whether it is signed in or has its key saved, and how
many chats use it. **Check** asks an API for its models with the key, so a
wrong key or address shows up before a chat; nothing is sent until you
press it. A chat on an account whose key is missing says so instead of
starting.

**Plan & Usage** shows each account's plan limits and reset times where the
agent records them (Claude Code, Codex, Cursor, Gemini CLI).

## Models

Larrier reads each agent's own model list and defaults (Claude Code's model
menu, Codex's model list, Cursor's enabled models, OpenCode and Hermes
providers, …), so the picker shows exactly what the agent offers. A model's
**Edit** sets its options where the agent has them: reasoning effort,
context size (for example Cursor's 256K / 500K variants), speed. You can add
your own models, on the agent's own service or served by one of its API
accounts.

## Installing and updating agents

**Settings → Harnesses** shows each agent's installed version and the latest
one its makers have published, read only from where they publish it:

| Source | Agents |
|---|---|
| npm (`latest`) | Claude Code, Codex, OpenCode, Gemini CLI, Pi, Amp, Copilot CLI, Qwen Code |
| Official install script | Cursor Agent, Droid |
| GitHub releases | Hermes Agent, Goose |
| PyPI | Kimi, Aider |

Larrier looks the versions up shortly after it starts, every six hours and
on demand. An outdated agent shows **Update to vX**, a missing one
**Install**; the agents page in Settings counts updates and the Settings
button gets a dot.

Updates use the agent's own updater where it has one (`claude update`,
`codex update`, `cursor-agent update`, `opencode upgrade`, `hermes update`,
`droid update`); otherwise npm (when npm installed it) or the official
installer again. Installs use the official installer for your system:
PowerShell installers on Windows, the shell installers (`curl … | sh`) on
macOS and Linux. Larrier shows the exact command first and its output as it
runs.

Not covered: Goose has no official Windows installer, and Aider is updated
from its own site.

## Custom agents

Add any command under **Settings → Harnesses → Custom agents**: its
program and arguments, and its protocol (ACP or one of the stream
protocols).
