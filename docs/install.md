# Installing and updating

## Install

Download `Larrier-<version>-windows-x64.exe` from the
[latest release](https://github.com/Dev4YM/Larrier/releases/latest) and run
it. The program installs itself:

- **Just for me:** no administrator rights. The app goes to
  `%LOCALAPPDATA%\Programs\Larrier`, everything else stays in your profile.
- **For everyone on this computer:** needs administrator rights. The app
  goes to `Program Files\Larrier` and shared files to
  `ProgramData\Larrier`; each person's sessions and settings stay in their
  own profile.
- **Run without installing:** starts that copy as it is (portable).

You get a Start menu shortcut (and a Desktop one if you like), and Larrier
appears in Settings → Apps, where it uninstalls, optionally with your data.

Releases are built for Windows (x64). Larrier is written to be
cross-platform, but macOS and Linux builds aren't released or tested yet.

## Updates

Installed copies look for a new release on their own (Settings → About):

- They download the new program, check it against its published SHA-256
  and swap it in on restart. No building, no source code.
- **Stable** gets releases; **Preview** also gets pre-releases such as
  `0.3.0-beta.1`.
- Settings → About chooses whether Larrier only tells you, downloads, or
  also installs when it closes.
- After an update Larrier shows what's new; every version's notes are built
  in (About → Release notes).

While the repository is private, updates need a GitHub token:
`LARRIER_GITHUB_TOKEN`, `GITHUB_TOKEN`, or a signed-in GitHub CLI.

## Your data

| Platform | Location |
|---|---|
| Windows | `%APPDATA%\Larrier` |
| macOS | `~/Library/Application Support/Larrier` |
| Linux | `~/.local/share/larrier` |

Set `LARRIER_HOME` to use another folder. What's in it is described in
[privacy.md](privacy.md#what-larrier-stores).

## Agents

Larrier finds the agent programs you already have (on `PATH` and in their
usual install folders). In **Settings → Agents** you can point an agent at a
specific program, turn agents off, and install or update them; see
[agents.md](agents.md#installing-and-updating-agents).

## Command-line flags

| Flag | Effect |
|---|---|
| `--version` | Print the version, commit and build date, then exit. |
| `--portable` | Run this copy without installing it. |
| `--uninstall` | Start the uninstaller. |
