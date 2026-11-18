# Agents

Lary calls everything that runs a chat a **harness**: each agent's own
program, and Lary Chat, which talks to model APIs itself. It runs the
harness and translates what it streams into one transcript. Your
agents keep their own logins, settings and session stores; Lary reads
those and, when you continue a chat with an agent, writes only what that
agent needs to resume.

## How each agent connects

| Agent | Program | Connection | Live approvals and questions |
|---|---|---|---|
| Claude Code | `claude` | stdio control protocol (stream-json in and out) | Yes |
| Codex | `codex` | `codex app-server` (JSON-RPC) | Yes |
| Cursor Agent | `cursor-agent` | ACP (`cursor-agent acp`) | Yes |
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
| Grok Build | `grok` | ACP (`grok agent stdio`) | Depends on Grok |
| Custom | any command | ACP or one of the stream protocols above | Depends |
| Lary Chat | — | HTTP: OpenAI, Anthropic, Gemini, OpenRouter, Ollama, OpenAI-compatible | — |

**Settings → Harnesses → ‹agent› → Connection** shows how Lary connects to
each agent, with the exact command and what it can do. Claude Code, Codex,
Cursor and Grok connect only their official way (below); for the others it
lists each way, marks the one its maker documents for an app like Lary
(their own headless mode or ACP, where they can ask), and lets you pick.
Where an agent can't ask while Lary runs it, Lary says what it refused and
lets you change the permission mode.

## The official way: Claude Code, Codex, Cursor and Grok

These four run the way their makers support for apps like Lary, and apart
from your own setup:

- **How they connect:** only their official way. Claude Code over the
  stream-JSON control protocol the Claude Agent SDK uses; Codex over its
  app server; Cursor Agent over ACP (`cursor-agent acp`); Grok Build over
  ACP (`grok agent stdio`). Settings from before, such as Claude Code on
  the ACP adapter, move to these.
- **Which program runs:** Lary's own copy by default, downloaded from the
  maker's release source into Lary's folder (Claude Code from
  downloads.claude.ai and Codex from OpenAI's GitHub releases, both checked
  against their published SHA-256; Cursor from downloads.cursor.com; Grok
  from x.ai/cli). On Windows Codex comes as its whole package, with the
  helpers it runs commands through (a copy without them is fetched again).
  It downloads the first time a chat needs it, or from
  **Settings → Harnesses → ‹agent› → Version**, where you can switch to the
  copy installed on your computer instead. Lary never installs, updates or
  configures your own copy.
- **Where they run:** every account in a folder of Lary's of its own
  (`CLAUDE_CONFIG_DIR`, `CODEX_HOME`, `CURSOR_CONFIG_DIR`, `GROK_HOME`), so
  your own settings, hooks, plugins and sign-in are never read or changed;
  only an account set to **This computer** uses your own sign-in.
- **How they sign in:**

  | Agent | Ways, the first being the default |
  |---|---|
  | Claude Code | **Setup token** (run `claude setup-token`, copied from the dialog or run in a terminal from it, and paste what it prints) · **Sign in** (Claude Code's own login with your subscription, in Lary's folder) · **API key** · This computer |
  | Codex | **ChatGPT** (sign-in through Codex's app server, as its own apps do: OpenAI's page opens in your browser) · **API key** · This computer |
  | Cursor Agent | **API key** · **Sign in** (Cursor's own login, in Lary's folder) · This computer |
  | Grok Build | **API key** (xAI) · **Sign in** (`grok login`, in Lary's folder) · This computer |

  Keys and tokens stay in your OS keychain and reach the agent only when
  its account runs.

## Modes and permissions

Each agent's real modes (plan, ask, auto-accept, …) and permission settings
are offered in the composer. Lary passes them to the agent the way the
agent takes them (flags, ACP modes, app-server policies); it never widens
what you picked.

## Accounts

Every credential is an account of one harness, managed on that harness's
page in **Settings → Harnesses**. The default account runs new chats; a
chat can switch in the model picker. An account is one of:

- **Current sign-in:** whatever the agent is signed into on this computer
  (its subscription or login).
- **Separate sign-in:** an isolated config folder of its own
  (`CLAUDE_CONFIG_DIR`, `CODEX_HOME`, …). Sign in once from Lary; it runs
  the agent's own login in a terminal.
- **Existing folder:** a folder an agent already signs in from.
- **API:** a provider's API (Anthropic, OpenAI, OpenRouter, Ollama, any
  OpenAI-compatible endpoint, a gateway) with its key in the OS keychain.
  Lary points Claude Code, Codex, OpenCode, Droid and Hermes at it for
  the runs that use it, without changing their own settings; other agents
  get the key in the variable they read (`OPENAI_API_KEY`, …). Lary
  Chat's accounts are all API accounts, each with the models it offers.

Each account shows whether it is signed in or has its key saved, and how
many chats use it. **Check** asks an API for its models with the key, so a
wrong key or address shows up before a chat; nothing is sent until you
press it. A chat on an account whose key is missing says so instead of
starting.

**Plan & Usage** shows each account's plan limits and reset times where the
agent records them (Claude Code, Codex, Cursor, Gemini CLI): Claude Code's
5-hour window, its week and each model's own week (Fable's). A Claude Code
account on a **setup token** is a subscription too, but its token can't ask
for the limits, so Lary shows the ones Claude Code reported in that
account's latest chat, with how long ago.

## Models

Lary reads each agent's own model list and defaults (Claude Code's model
menu, Codex's model list, Cursor's enabled models, OpenCode and Hermes
providers, …), so the picker shows exactly what the agent offers. It reads
them per account, the way that account's chats run (its folder, token or
key): what an agent offers and what its **Default** runs depend on the
account (a subscription's plan picks Claude Code's default), so the
Default row says what that account really gets. Every place that shows a
harness's models (the picker, the `/` menu's Model page, an automation, its
Settings page) reads the list again when it's a few minutes old or was
read by another Lary version, and Lary reads them all when it starts.

Claude Code's menu names its models by family (Opus, Sonnet, …), each
always running the newest of it. Lary's picker shows the real models
those run now, by version (Opus 5.5, Sonnet 5.5, …). In **Settings →
Harnesses → Claude Code → Models**, **All** also lists the family names
(tagged **Alias**, to always run the newest) and every older model Claude
Code still takes, each to turn on for the picker. **Ultracode** is not an
effort: it's a switch beside Fast on every Claude Code model but Haiku,
passed to Claude Code as its `ultracode` setting, at whatever effort the
chat runs.

Each reply records the model the agent reported running; when that isn't
the one picked (a plan without it, a limit reached, a fallback), the reply
says so. A model's
**Edit** sets its options where the agent has them: reasoning effort,
context size (for example Cursor's 256K / 500K variants), speed. You can add
your own models, on the agent's own service or served by one of its API
accounts.

## Installing and updating agents

**Settings → Harnesses** shows each agent's installed version and the latest
one its makers have published, read only from where they publish it:

| Source | Agents |
|---|---|
| npm (`latest`) | Claude Code, Codex, OpenCode, Gemini CLI, Pi, Amp, Copilot CLI, Qwen Code, Grok Build |
| Official install script | Cursor Agent, Droid |
| GitHub releases | Hermes Agent, Goose |
| PyPI | Kimi, Aider |

Lary looks the versions up shortly after it starts, every six hours and
on demand. An outdated agent shows **Update to vX**, a missing one
**Install**; the agents page in Settings counts updates and the Settings
button gets a dot.

Updates use the agent's own updater where it has one (`claude update`,
`codex update`, `cursor-agent update`, `opencode upgrade`, `hermes update`,
`droid update`, `grok update`); otherwise npm (when npm installed it) or the official
installer again. Installs use the official installer for your system:
PowerShell installers on Windows, the shell installers (`curl … | sh`) on
macOS and Linux. Lary shows the exact command first and its output as it
runs.

Not covered: Goose has no official Windows installer, and Aider is updated
from its own site.

## Custom agents

Add any command under **Settings → Harnesses → Custom agents**: its
program and arguments, and its protocol (ACP or one of the stream
protocols).
