# scripts

PowerShell scripts run in PowerShell 7 (`pwsh`) or Windows PowerShell 5,
from any folder. The demo tools need Python 3.10+.

| Script | What it does |
|---|---|
| [`setup.ps1`](setup.ps1) | Checks Rust, WebView2, the build tools, Python and the GitHub CLI, and says how to get what's missing. Changes nothing. |
| [`dev.ps1`](dev.ps1) | Runs the desktop app from source. `-Sandbox` uses a fresh data folder, `-DataDir` a folder you pick, `-DevTools <port>` opens the WebView's DevTools protocol, `-Release` builds optimized. |
| [`check.ps1`](check.ps1) | Builds, tests and runs Clippy on the workspace: what a pull request must pass. `-Quick` runs the tests only; `-Ignored` adds the tests that need real agents or the network. |
| [`build.ps1`](build.ps1) | Builds `dist/Larrier-<version>-windows-x64.exe` and its `.sha256`, named as releases are. `-Cli` adds `dist/larrier.exe`. |
| [`release.ps1`](release.ps1) | Cuts a release: version bump, changelog section, commit and tag, build, and with `-Publish` push and GitHub release. See [docs/releasing.md](../docs/releasing.md). |
| [`demo/make_demo.py`](demo/make_demo.py) | Builds a sample Larrier home: four git projects, a dozen chats across agents, and a stand-in Claude Code. |
| [`demo/demo_agent.py`](demo/demo_agent.py) | The stand-in: plays scripted turns over Claude Code's protocol so chats run live without an account. |
| [`demo/compose_hero.py`](demo/compose_hero.py) | Sets a screenshot on the README backdrop (grainy ink and bone-white light). |
