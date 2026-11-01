# Building and testing

Larrier builds on Windows, macOS and Linux from the same code. Each system
has the same set of scripts: PowerShell (`.ps1`) on Windows, Bash (`.sh`)
on macOS and Linux.

## Requirements

All systems: Rust 1.85 or newer (`rustup` recommended) and Git.

| System | Also needs |
|---|---|
| Windows 10 / 11 | The MSVC build tools ("Desktop development with C++"). WebView2 is part of Windows. |
| macOS 11+ | The Xcode command line tools: `xcode-select --install`. |
| Linux | A C toolchain, `pkg-config` and the WebKitGTK 4.1 / GTK 3 development packages (below). |

Linux packages:

```bash
sudo apt install build-essential pkg-config libwebkit2gtk-4.1-dev libgtk-3-dev libsoup-3.0-dev libayatana-appindicator3-dev librsvg2-dev libxdo-dev libssl-dev
```

```bash
sudo dnf install gcc pkgconf-pkg-config webkit2gtk4.1-devel gtk3-devel libsoup3-devel libappindicator-gtk3-devel librsvg2-devel libxdo-devel openssl-devel
```

```bash
sudo pacman -S base-devel webkit2gtk-4.1 gtk3 libsoup3 libappindicator-gtk3 librsvg xdotool openssl
```

Optional everywhere: Python 3.10+ with Pillow and NumPy for the demo and
screenshot tools, the GitHub CLI (`gh`) for releases, Docker for portable
Linux builds.

`scripts/setup.ps1` (Windows) or `scripts/setup.sh` (macOS, Linux) checks
all of this and says what's missing.

## Everyday commands

| | Windows | macOS / Linux |
|---|---|---|
| Run the app (debug) | `./scripts/dev.ps1` | `./scripts/dev.sh` |
| … with a throwaway data folder | `./scripts/dev.ps1 -Sandbox` | `./scripts/dev.sh --sandbox` |
| Build, test and lint | `./scripts/check.ps1` | `./scripts/check.sh` |
| Release build into `dist/` | `./scripts/build.ps1` | `./scripts/build.sh` |

Or with Cargo directly, on any system:

```bash
cargo run -p larrier-app
cargo test --workspace
cargo build --release -p larrier-app
```

## Packages

`build.ps1` and `build.sh` produce what a release carries, with its SHA-256:

| System | Output |
|---|---|
| Windows | `dist/Larrier-<version>-windows-x64.exe` |
| macOS | `dist/Larrier-<version>-macos-<arch>.zip` holding Larrier.app (signed ad hoc, not notarized) |
| Linux | `dist/Larrier-<version>-linux-<arch>.tar.gz`: the program, `larrier.desktop`, the icon and `install.sh` |

On an Apple silicon Mac, `./scripts/build.sh --target x86_64-apple-darwin`
builds the Intel app too (after `rustup target add x86_64-apple-darwin`).
On Linux, `./scripts/build.sh --docker` builds inside Ubuntu 22.04
(`scripts/linux/Dockerfile`), so the program runs on every distribution at
least that new; Windows and macOS can use the same image to check the Linux
build.

## Working on the app without your own data

`LARRIER_HOME` points Larrier at another data folder, so a development copy
never touches your real sessions and settings. `./scripts/dev.ps1 -Sandbox`
(or `./scripts/dev.sh --sandbox`) creates a fresh one for each run.

For a realistic window (projects, a dozen chats across agents, live runs)
build the demo home:

```bash
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
