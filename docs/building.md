# Building and testing

## Requirements

- Rust 1.85 or newer (`rustup` recommended).
- Windows: WebView2 (included in Windows 10 and 11) and the MSVC build
  tools.
- Optional: PowerShell 7 for the scripts, Python 3.10+ with Pillow and
  NumPy for the demo and screenshot tools, the GitHub CLI (`gh`) for
  releases.

`scripts/setup.ps1` checks all of these and tells you what's missing.

## Everyday commands

```powershell
./scripts/dev.ps1                 # run the desktop app (debug)
./scripts/dev.ps1 -Sandbox        # … with a throwaway data folder
./scripts/check.ps1               # build, test and lint everything
./scripts/build.ps1               # optimized program into dist/, with its SHA-256
```

Or with Cargo directly:

```bash
cargo run -p larrier-app
cargo test --workspace
cargo build --release -p larrier-app
```

## Working on the app without your own data

`LARRIER_HOME` points Larrier at another data folder, so a development copy
never touches your real sessions and settings. `./scripts/dev.ps1 -Sandbox`
creates a fresh one for each run.

For a realistic window (projects, a dozen chats across agents, live runs)
build the demo home:

```powershell
python scripts/demo/make_demo.py
```

It makes four sample git repositories and a Larrier home whose Claude Code
is `scripts/demo/demo_agent.py`, a stand-in that plays scripted turns over
Claude Code's protocol, so chats run live with no account. The script
prints the environment to start Larrier with. Everything in it is invented.

## Screenshots

The README's hero image comes from the demo home:

1. `python scripts/demo/make_demo.py`, then start Larrier with the printed
   environment.
2. Send a prompt mentioning "idempotent" in **Make Stripe webhooks
   idempotent** and one mentioning "server components" in **Move dashboard
   charts to server components**; the demo agent streams a turn for each.
3. Capture the window at 1920 × 1200 (2× scale) with the Changes panel
   open.
4. `python scripts/demo/compose_hero.py shot.png docs/assets/hero.jpg`
   sets it on the backdrop.

## Tests

- `cargo test --workspace` runs the unit tests and the importer tests
  against sanitized fixtures (`crates/agents/tests/fixtures` and inline
  samples in the importers).
- Some tests talk to real installed agents or the network; they're marked
  `#[ignore]` and run with `cargo test -- --ignored`.

## Debugging the UI

Set `LARRIER_DEVTOOLS_PORT` (for example `9333`) to open the WebView's
DevTools protocol on that port; any Chrome DevTools client can attach to
inspect the DOM, run JavaScript or take screenshots.
