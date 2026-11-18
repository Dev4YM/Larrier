# Command line

`lary` works with the same store as the desktop app (`LARY_HOME`
applies to both).

```bash
cargo run -p lary-cli -- --help
```

| Command | What it does |
|---|---|
| `lary agents` | List known agents and whether each is installed. |
| `lary sessions [--agent ID] [--limit N]` | List sessions in Lary's store, newest first. |
| `lary discover [--agent ID]` | List native sessions found in the agents' own storage. |
| `lary sync` | Import new or changed native sessions from every agent. |
| `lary import FILE…` | Import files: LSF, Claude Code / Codex / Pi JSONL, Gemini JSON, ChatGPT and claude.ai exports. |
| `lary export ID [--format F] [-o FILE]` | Export a session. Formats: `lsf-jsonl` (default), `lsf-json`, `markdown`, `claude-code`, `codex`, `gemini`, `pi`. |
| `lary port ID --to AGENT` | Write a native, resumable session for another agent. |
| `lary show ID` | Print a session as Markdown. |
| `lary run [--agent ID] [--cwd DIR] [--model M] [--account A] [--session ID] PROMPT…` | Run one turn with an agent (as an account, by id or name) and record it as a session; it ends by saying which model was asked for and which ran. |
| `lary models [--agent ID] [--account A]` | What an agent offers for an account and what its Default runs, read the way that account's chats run. |

Session ids can be shortened to any unique prefix. Agent ids are the ones
listed in the [LSF spec](LSF.md#header-line-1) (`claude-code`, `codex`,
`cursor`, …).

## Examples

```bash
lary sync
lary sessions --agent codex --limit 10
lary port 0192f0c4 --to claude-code
lary run --agent claude-code --cwd ~/code/api "Explain the retry logic in src/webhooks"
lary models --agent claude-code --account Premium
lary run --agent claude-code --account Premium --model opus "Reply with just: ok"
```
