# Sessions

Every conversation in Lary is one file in the
[Lary Session Format](LSF.md): text, reasoning, tool calls with their
output, diffs, plans, images and token usage, whichever agent produced them.

## Normal and Companion

**Settings → Sessions → Mode** decides which sessions Lary holds:

- **Normal** (the default): Lary's own sessions, the ones started in Lary
  and the ones you import. A harness's own sessions stay where they are
  until you ask for one.
- **Companion**: every harness's sessions as well, imported and kept in
  sync (below). Chats can then change from outside Lary, some steps may
  not come across exactly, and Lary keeps reading the harnesses' folders.

Switching hides or shows the harnesses' sessions; nothing is deleted. A
session imported by hand, or continued in Lary, is Lary's own in both
modes.

## A harness's session, by hand

- **File → Import Session…** (Ctrl+Shift+I in Normal mode) lists the
  harnesses' sessions on this computer, newest first, with a search. The
  one you pick becomes a Lary session and opens. **Choose a file…** takes
  an LSF file or a ChatGPT or claude.ai data export instead.
- **@ → Sessions** in the composer finds one the same way and sends its
  conversation as context with your message, without importing it
  (**Settings → Sessions → Harness sessions in @** turns this off).

## Importing everything (Companion)

Lary reads the native session stores of:

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

In Companion mode, **File → Import Sessions from All Agents**
(Ctrl+Shift+I) imports everything new at once, and Lary does so when it
starts.

Imports are incremental: re-importing updates a session in place instead of
duplicating it, and importers that improve re-import what they read before.
Importing only reads the agents' folders.

## Live sync

In Companion mode, with **Settings → Sessions → Live sync** on, Lary watches those folders and
updates sessions as the agents write them. A chat running in another app
(a terminal, the agent's own app) shows as running in Lary, and Lary
won't run it at the same time.

## Continuing with another agent

Any chat can continue with any agent:

1. **Native resume** (Claude Code, Codex, Gemini CLI, Pi): Lary writes a
   real session file in the target agent's store and the agent resumes it
   with its full history.
2. **Context handoff** (every other agent): the messages that agent hasn't
   seen are sent once, as a compact transcript, ahead of your next prompt;
   later turns continue in the agent's own session.
3. **API chat** replays the whole history to the model provider.

Each agent's native session keeps growing on its own; the LSF file stays the
merged record of everything every agent did.

## Forks and edits

Editing an earlier prompt sends it as a fork: Lary rewinds the agent's
own session to that point where the agent supports it (Claude Code, Codex,
Gemini CLI, Pi) and continues from there. Forks are linked to the chat they
came from.

## Export

**File → Export Agent…** (or a chat's ⋯ menu, or `lary export`) writes a session as LSF (JSON Lines or
one JSON document), Markdown, or a native session for Claude Code, Codex,
Gemini CLI or Pi.
