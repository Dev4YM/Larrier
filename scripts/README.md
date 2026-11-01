# scripts

Every task has a script for each system: PowerShell (`.ps1`, PowerShell 7
or Windows PowerShell 5) on Windows, Bash (`.sh`) on macOS and Linux. They
run from any folder. The demo tools need Python 3.10+.

| Windows | macOS / Linux | What it does |
|---|---|---|
| [`setup.ps1`](setup.ps1) | [`setup.sh`](setup.sh) | Checks Rust and the system's build tools (MSVC, Xcode tools, WebKitGTK), Python, the GitHub CLI, and says how to get what's missing. Changes nothing. |
| [`dev.ps1`](dev.ps1) | [`dev.sh`](dev.sh) | Runs the desktop app from source. Sandbox (`-Sandbox` / `--sandbox`) uses a fresh data folder, `-DataDir` / `--data-dir` a folder you pick; release builds with `-Release` / `--release`. |
| [`check.ps1`](check.ps1) | [`check.sh`](check.sh) | Builds, tests and runs Clippy on the workspace: what a pull request must pass. Quick mode runs the tests only; ignored mode adds the tests that need real agents or the network. |
| [`build.ps1`](build.ps1) | [`build.sh`](build.sh) | Builds the release package into `dist/` with its `.sha256`: the Windows `.exe`, Larrier.app zipped for macOS, or the Linux `.tar.gz`. `build.sh --docker` builds Linux in Ubuntu 22.04. |
| [`release.ps1`](release.ps1) | | Cuts a release: version bump, changelog section, commit and tag, the Windows build, and with `-Publish` push and GitHub release. The Release workflow then adds the macOS and Linux programs. See [docs/releasing.md](../docs/releasing.md). |

| File | What it is |
|---|---|
| [`linux/Dockerfile`](linux/Dockerfile) | Ubuntu 22.04 with Rust and WebKitGTK's development packages, for portable Linux builds. |
| [`linux/install.sh`](linux/install.sh) | Shipped in the Linux package: installs Larrier for the current user (`~/.local/bin`, applications menu, icon); `--remove` uninstalls. |
| [`linux/larrier.desktop`](linux/larrier.desktop) | The applications-menu entry. |
| [`demo/make_demo.py`](demo/make_demo.py) | Builds a sample Larrier home: four git projects, a dozen chats across agents, and a stand-in Claude Code. |
| [`demo/demo_agent.py`](demo/demo_agent.py) | The stand-in: plays scripted turns over Claude Code's protocol so chats run live without an account. |
| [`demo/compose_hero.py`](demo/compose_hero.py) | Sets a screenshot on the README backdrop (grainy ink and bone-white light). |
