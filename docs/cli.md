# Command line

`larrier` works with the same store as the desktop app (`LARRIER_HOME`
applies to both).

```bash
cargo run -p larrier-cli -- --help
```

| Command | What it does |
|---|---|
| `larrier agents` | List known agents and whether each is installed. |
| `larrier sessions [--agent ID] [--limit N]` | List sessions in Larrier's store, newest first. |
| `larrier discover [--agent ID]` | List native sessions found in the agents' own storage. |
| `larrier sync` | Import new or changed native sessions from every agent. |
| `larrier import FILE…` | Import files: LSF, Claude Code / Codex / Pi JSONL, Gemini JSON, ChatGPT and claude.ai exports. |
| `larrier export ID [--format F] [-o FILE]` | Export a session. Formats: `lsf-jsonl` (default), `lsf-json`, `markdown`, `claude-code`, `codex`, `gemini`, `pi`. |
| `larrier port ID --to AGENT` | Write a native, resumable session for another agent. |
| `larrier show ID` | Print a session as Markdown. |
| `larrier run [--agent ID] [--cwd DIR] [--model M] [--session ID] PROMPT…` | Run one turn with an agent and record it as a session. |

Session ids can be shortened to any unique prefix. Agent ids are the ones
listed in the [LSF spec](LSF.md#header-line-1) (`claude-code`, `codex`,
`cursor`, …).

## Examples

```bash
larrier sync
larrier sessions --agent codex --limit 10
larrier port 0192f0c4 --to claude-code
larrier run --agent claude-code --cwd ~/code/api "Explain the retry logic in src/webhooks"
```
