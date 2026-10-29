# Larrier Session Format (LSF) 1.0

LSF is one portable file format for every coding-agent session and AI chat.
Larrier stores every conversation in it. That covers sessions it runs itself,
sessions imported from Claude Code, Codex, Cursor, OpenCode, Gemini CLI, Pi,
Hermes or Droid, and chats imported from ChatGPT and Claude.ai. Any of them
can be continued with any agent.

## Files

| Form | Extension | Use |
|---|---|---|
| JSON Lines | `.lsf.jsonl` | The storage form. Line 1 is the session header, and each following line is one message. Appending a turn means appending lines. |
| Document | `.lsf.json` | `{ "lsf": "1.0", "session": { …, "messages": [ … ] } }`. Used for sharing and export. |

Readers must accept both forms. `Session::from_any` in `larrier-core` detects
which one it was given.

## Header (line 1)

```json
{
  "lsf": "1.0",
  "kind": "session",
  "id": "0192f0c4-…",
  "title": "Add GitHub OAuth login",
  "created_at": "2026-10-29T09:12:44.120Z",
  "updated_at": "2026-10-29T09:40:02.981Z",
  "status": "review",
  "agent": "claude-code",
  "model": "claude-sonnet-4-5",
  "mode": "agent",
  "account_id": "…",
  "workspace": { "path": "D:\\Code\\web", "name": "web", "branch": "main", "remote": "git@github.com:acme/web.git" },
  "environment": { "type": "worktree", "path": "…\\worktrees\\web\\agent-3f9a1c", "branch": "larrier/agent-3f9a1c", "base": "main" },
  "origin": { "agent": "codex", "native_id": "019a…", "path": "~/.codex/sessions/…", "imported_at": "…", "agent_version": "0.154.0" },
  "native": [
    { "agent": "codex", "id": "019a…", "path": "…", "synced_messages": 12 },
    { "agent": "claude-code", "id": "7d1e…", "path": "~/.claude/projects/…/7d1e….jsonl", "synced_messages": 14 }
  ],
  "parent": { "session_id": "…", "message_id": "…" },
  "tags": [], "pinned": false, "archived": false, "unread": true,
  "usage": { "input_tokens": 48211, "output_tokens": 6120, "cache_read_tokens": 210000, "cost_usd": 0.42 }
}
```

* `agent` is the agent currently driving the session. `origin` records where
  the session came from.
* `native` holds one handle per agent that has this conversation in its own
  storage. `synced_messages` is how many LSF messages that native session
  already contains. When a session moves to another agent, the messages after
  that index are handed over (see [Porting](#porting)).
* `status` is one of `idle`, `running`, `needs_input`, `review` (finished with
  changes to review), `done` or `error`.
* Agent ids are `claude-code`, `codex`, `cursor`, `opencode`, `gemini`, `pi`,
  `hermes`, `goose`, `amp`, `copilot`, `qwen`, `kimi`, `droid`, `aider`,
  `chat` (Larrier's built-in API chat), `chatgpt`, `claude-ai`, and
  `custom:<id>`.

## Messages (lines 2…)

```json
{"kind":"message","id":"…","parent_id":"…","role":"assistant","created_at":"…","agent":"claude-code","model":"claude-sonnet-4-5",
 "parts":[ … ], "usage":{ … }, "sidechain":false, "native_id":"msg_01…"}
```

`role` is `user`, `assistant` or `system`. System messages carry notices,
such as "Ported from Claude Code to Codex", and are never sent to agents.

### Parts

| `type` | Fields |
|---|---|
| `text` | `text` (Markdown) |
| `reasoning` | `text`, `duration_ms?` |
| `tool` | `id`, `name` (the native tool name), `kind`, `title`, `input`, `status`, `output?`, `is_error`, `locations[]`, `diffs[]`, `duration_ms?` |
| `image` | `mime?`, `source`: `{kind:"base64",data}` \| `{kind:"path",path}` \| `{kind:"url",url}` |
| `file` | `path`, `mime?`, `text?` |
| `plan` | `entries[]`: `{content, status: pending\|in_progress\|completed\|cancelled, priority?}` |
| `error` | `message` |
| `notice` | `text` |

Tool `kind` normalizes every agent's tool names to one of `read`, `edit`,
`write`, `delete`, `move`, `search`, `shell`, `fetch`, `web_search`, `think`,
`task`, `todo`, `mcp` or `other`.

Tool `status` is `pending`, `running`, `completed`, `failed`, `approval` or
`cancelled`.

A diff is `{path, old_text?, new_text?, unified?, added, removed}`. Agents
that report replacements (Claude `Edit`, OpenCode `edit`, Gemini `replace`,
Pi `edit`) fill `old_text` and `new_text`. Patch-based agents (Codex
`apply_patch`, git) fill `unified`.

Tool results are stored on the tool call, not as separate messages. Every
renderer and exporter therefore sees the call and its outcome together.

## Porting

Continuing a session with a different agent:

1. **Native export.** This applies when the target can load external
   sessions: Claude Code, Codex, Gemini CLI and Pi. Larrier writes a real
   session file into that agent's own store and records a `native` link. The
   agent then resumes with its normal mechanism (`--resume`, `exec resume`,
   `--session`), and the full history is in its context.
2. **Context handoff.** This applies to every other agent. The messages that
   agent has not seen (everything after `synced_messages`) are rendered into
   a compact transcript, which is sent once inside a
   `<conversation_history>` block ahead of the next prompt. Later turns
   resume natively through the agent's own session id.
3. **API chat.** Larrier's built-in Chat agent replays the whole LSF history
   to the model provider.

Each agent's native session keeps growing on its own. LSF stays the single
merged record of everything every agent did.

## Compatibility rules

* Unknown header fields are kept in `extra`. Unknown part types and
  malformed lines are skipped with a warning; they are never a hard error.
* Writers emit fields in the order shown above and omit empty optional
  fields.
* A file is valid if its header parses and every line it keeps is a valid
  message.
