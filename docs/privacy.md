# Privacy and data

Larrier runs on your computer. It has no account, no server of its own and
no telemetry.

## What Larrier stores

In its data folder (see [install.md](install.md#your-data)):

| Path | Contents |
|---|---|
| `config.json` | Settings, agents, accounts (never their secrets), providers, projects, automations, profile. |
| `sessions/<id>.lsf.jsonl` | One file per conversation. |
| `index.json` | Session summaries for fast listing. |
| `imports.json` | Fingerprints of native sessions already imported. |
| `kept.json` | Window state for restoring: open chats and tabs, side panels, browser addresses, the last 64 KB of each terminal's output, scroll positions, queued and half-typed messages. |
| `accounts/<id>/` | Config folders of separate logins you create in Larrier. |
| `worktrees/` | Git worktrees for agents, when you use them. |

API keys and other secrets go to the operating system's credential store,
never to these files: Credential Manager on Windows, the Keychain on macOS,
and the desktop's Secret Service on Linux (GNOME Keyring or KWallet, with
the kernel keyring in front of it).

## What Larrier reads

- **Agents' folders** (`~/.claude`, `~/.codex`, `~/.cursor`,
  `~/.local/share/opencode`, `~/.gemini`, `~/.pi`, `~/.hermes`,
  `~/.factory`): their sessions, models, commands, MCP servers and other
  configuration, to import and show them. Importing only reads.
- **Plan limits:** Codex's from its session logs; Claude Code's from
  Anthropic's usage endpoint with the token Claude Code keeps (sent only to
  Anthropic, never logged, shown or stored); Cursor's and Gemini CLI's from
  the account the CLI records.

## What Larrier writes outside its folder

- **Resuming a chat in an agent** that supports it (Claude Code, Codex,
  Gemini CLI, Pi) writes a session file into that agent's own session store.
- **Your projects:** whatever you ask the agents to change, and the git
  operations you start from the Changes panel.

## Network

Larrier itself connects to:

- **GitHub** (`api.github.com`, release downloads): checking for and
  downloading Larrier updates.
- **Where agents publish releases** (npm, PyPI, GitHub, the agents' install
  scripts): looking up agents' latest versions.
- **Model providers you configure** for API chat.
- **Anthropic's usage endpoint** for Claude Code plan limits, as above.
- **The pages you open** in the Browser tab (an address bar search goes to
  Google).

Everything else goes through the agents themselves, under their own
settings and privacy terms.
