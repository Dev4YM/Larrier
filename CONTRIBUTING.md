# Contributing to Larrier

Thanks for helping. This guide covers setting up, the conventions the code
follows, and how changes get in.

## Before you start

- **Bugs:** open an issue with what you did, what you expected, what
  happened, your Larrier version (Settings → About, or `--version`) and the
  agent and its version.
- **Features:** open an issue first for anything bigger than a small fix,
  so we can agree on the behavior before you build it.
- **Security issues:** don't open an issue; see [SECURITY.md](SECURITY.md).

## Setting up

```bash
git clone https://github.com/Dev4YM/Larrier
cd Larrier
```

| | Windows | macOS / Linux |
|---|---|---|
| Check what's missing | `./scripts/setup.ps1` | `./scripts/setup.sh` |
| Run with a throwaway data folder | `./scripts/dev.ps1 -Sandbox` | `./scripts/dev.sh --sandbox` |

The sandbox keeps development away from your own sessions. `python scripts/demo/make_demo.py` builds a full
demo home with sample projects, chats and a stand-in agent that runs live
without an account. See [docs/building.md](docs/building.md).

## Making a change

1. Branch from `main`.
2. Keep each commit to one concern; a feature and an unrelated fix are two
   commits.
3. Run `./scripts/check.ps1` or `./scripts/check.sh` (build, tests,
   Clippy) before you push. CI runs the same on Windows, macOS and Linux.
4. For UI changes, check the change in the running app: light and dark
   themes, a narrow window, with **Reduce Motion** on and off. Include a
   screenshot in the pull request.
5. Keep it working on all three systems: put platform code behind
   `cfg(windows)` / `cfg(target_os = "macos")` / `cfg(target_os = "linux")`
   with a path for the others, start programs without assuming a shell or
   `.exe`, and write shortcuts as `Ctrl+…` (`platform::keys` shows ⌘ on
   macOS).
6. Open a pull request describing what changed and why, and how you tested
   it (and on which system).

## Conventions

### Code

- Match the code around you: its naming, its idioms, its comment density.
  Comments explain why, or what isn't obvious; they don't narrate the code.
- Module docs (`//!`) say what the module is for. Public items get a short
  doc comment when their purpose isn't obvious from the name.
- Errors shown to people are plain sentences that say what happened and
  what to do: "Couldn't start Claude Code: …", not error codes.
- Agent folders (`~/.claude`, `~/.codex`, …) are read-only, except for
  writing a session an agent will resume. Never read or log credentials.
- No new dependency without a reason; prefer what the workspace already
  uses.

### UI

- Everything animates unless **Reduce Motion** is on; nothing jumps or
  reflows when state changes.
- Larrier draws its own tooltips, menus and context menus; don't use the
  system's.
- When an agent can't do something, say so in the UI rather than failing
  silently.

### Commits

[Conventional Commits](https://www.conventionalcommits.org): a subject in
the imperative, lowercase after the colon, at most 72 characters, then a
body explaining what changed and why.

```
fix(composer): keep the context size when the effort changes

Changing the effort re-picked the model, which reset Cursor's context
size to its default. Picking the model in use now leaves its options
alone.
```

Types: `feat`, `fix`, `perf`, `style`, `refactor`, `docs`, `test`,
`build`, `ci`, `chore`. `feat`, `fix`, `perf` and `style` subjects become
the release notes, so write them for the people using Larrier.

Scopes name the area: `chat`, `composer`, `transcript`, `sidebar`,
`settings`, `agents`, `claude`, `codex`, `cursor`, `formats`, `sync`,
`browser`, `terminal`, `updates`, `cli`, …

## Tests

- New importers and drivers come with tests over a sanitized fixture: no
  real names, emails, tokens or paths.
- Tests that need a real agent or the network are `#[ignore]`d.
- `cargo test --workspace` must pass.

## License

By contributing, you agree that your contributions are licensed under the
[MIT License](LICENSE).
