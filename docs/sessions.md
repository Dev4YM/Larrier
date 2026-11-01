# Sessions

Every conversation in Larrier is one file in the
[Larrier Session Format](LSF.md): text, reasoning, tool calls with their
output, diffs, plans, images and token usage, whichever agent produced them.

## Importing

Larrier reads the native session stores of:

| Agent | Store |
|---|---|
| Claude Code | `~/.claude/projects/**.jsonl` (and every account's folder) |
| Codex | `~/.codex/sessions/**/rollout-*.jsonl` |
| Cursor | the CLI's transcripts and the editor's `state.vscdb` |
| OpenCode | `~/.local/share/opencode/opencode.db` |
| Gemini CLI | `~/.gemini/tmp/**/chats` |
| Pi | `~/.pi/agent/sessions` |
| Hermes Agent | `~/.hermes` |
| Droid | `~/.factory/sessions` |

It also imports ChatGPT (`conversations.json`) and claude.ai data exports,
and LSF files (**File → Import File…**). **File → Import Sessions from All
Agents** (Ctrl+Shift+I) imports everything new at once.

Imports are incremental: re-importing updates a session in place instead of
duplicating it, and importers that improve re-import what they read before.
Importing only reads the agents' folders.

## Live sync

With **Settings → Sessions → Live sync** on, Larrier watches those folders and
updates sessions as the agents write them. A chat running in another app
(a terminal, the agent's own app) shows as running in Larrier, and Larrier
won't run it at the same time.

## Continuing with another agent

Any chat can continue with any agent:

1. **Native resume** (Claude Code, Codex, Gemini CLI, Pi): Larrier writes a
   real session file in the target agent's store and the agent resumes it
   with its full history.
2. **Context handoff** (every other agent): the messages that agent hasn't
   seen are sent once, as a compact transcript, ahead of your next prompt;
   later turns continue in the agent's own session.
3. **API chat** replays the whole history to the model provider.

Each agent's native session keeps growing on its own; the LSF file stays the
merged record of everything every agent did.

## Forks and edits

Editing an earlier prompt sends it as a fork: Larrier rewinds the agent's
own session to that point where the agent supports it (Claude Code, Codex,
Gemini CLI, Pi) and continues from there. Forks are linked to the chat they
came from.

## Export

**File → Export Agent…** (or a chat's ⋯ menu, or `larrier export`) writes a session as LSF (JSON Lines or
one JSON document), Markdown, or a native session for Claude Code, Codex,
Gemini CLI or Pi.
