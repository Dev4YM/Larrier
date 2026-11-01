# Installing and updating

Larrier runs on **Windows 10 and 11**, **macOS 11 or newer** (Apple silicon
and Intel) and **Linux** (x86_64, with WebKitGTK 4.1). Every release has a
program for each, from the
[latest release](https://github.com/Dev4YM/Larrier/releases/latest):

| System | Download |
|---|---|
| Windows 10 / 11 (x64) | `Larrier-<version>-windows-x64.exe` |
| macOS, Apple silicon | `Larrier-<version>-macos-arm64.zip` |
| macOS, Intel | `Larrier-<version>-macos-x64.zip` |
| Linux (x64) | `Larrier-<version>-linux-x64.tar.gz` |

Each comes with a `.sha256` file to check the download against.

## Larrier isn't signed

Larrier's programs aren't code-signed. The Windows program carries no
code-signing certificate, and the Mac app is signed ad hoc, without an
Apple Developer ID, and isn't notarized by Apple. So Windows and macOS warn
the first time you open a download; the steps for each are below. Updates
Larrier installs itself don't ask again: Larrier downloads them, not a
browser, so they carry no download mark.

To make sure a download is the published one, compare its SHA-256 with the
first word of its `.sha256` file:

```powershell
(Get-FileHash .\Larrier-<version>-windows-x64.exe -Algorithm SHA256).Hash
```

```bash
shasum -a 256 Larrier-<version>-macos-arm64.zip      # macOS
```

```bash
sha256sum Larrier-<version>-linux-x64.tar.gz         # Linux
```

The checksums are published with the release, so they catch a broken or
swapped download, not a release replaced on GitHub as a whole. To trust no
prebuilt program at all, build Larrier yourself ([building.md](building.md)).

## Windows

Run `Larrier-<version>-windows-x64.exe`. Because it isn't signed, Microsoft
Defender SmartScreen may stop it with **Windows protected your PC**: click
**More info**, then **Run anyway**. On Windows 11 with Smart App Control
turned on, unsigned programs are blocked without that choice; Larrier can't
start there unless Smart App Control is off (Windows Security → App &
browser control).

The program installs itself:

- **Just for me:** no administrator rights. The app goes to
  `%LOCALAPPDATA%\Programs\Larrier`, everything else stays in your profile.
- **For everyone on this computer:** needs administrator rights. The app
  goes to `Program Files\Larrier` and shared files to
  `ProgramData\Larrier`; each person's sessions and settings stay in their
  own profile.
- **Run without installing:** starts that copy as it is (portable).

You get a Start menu shortcut (and a Desktop one if you like), and Larrier
appears in Settings → Apps, where it uninstalls, optionally with your data.
WebView2, which Larrier draws with, is part of Windows 10 and 11.

## macOS

1. Unzip `Larrier-<version>-macos-<arch>.zip` and drag **Larrier.app** to
   Applications (or anywhere you like).
2. The first time, macOS refuses to open it, because it isn't notarized.
   Allow it once; after that it opens normally:
   - **macOS 15 (Sequoia) and newer:** open Larrier.app and click **Done**
     when macOS refuses. Then go to **System Settings → Privacy & Security**,
     scroll to **Security**, click **Open Anyway** next to Larrier and
     confirm with your password or Touch ID.
   - **macOS 11 to 14:** right-click Larrier.app → **Open**, then **Open**
     again.

   If macOS says Larrier "is damaged", or there's no **Open Anyway** button,
   remove the download mark in Terminal and open it again:

   ```bash
   xattr -dr com.apple.quarantine /Applications/Larrier.app
   ```

Larrier's menus are in the menu bar, and shortcuts use ⌘ where Windows and
Linux use Ctrl. To uninstall, move Larrier.app to the Trash; your sessions
and settings stay in `~/Library/Application Support/Larrier` until you
delete that folder.

## Linux

Larrier needs WebKitGTK 4.1 and GTK 3:

```bash
sudo apt install libwebkit2gtk-4.1-0 libgtk-3-0        # Debian 12+, Ubuntu 22.04+
```

```bash
sudo dnf install webkit2gtk4.1 gtk3                    # Fedora
```

```bash
sudo pacman -S webkit2gtk-4.1 gtk3                     # Arch
```

Then unpack the archive and run its installer, which puts the program in
`~/.local/bin` and Larrier in your applications menu (no root needed):

```bash
tar -xzf Larrier-<version>-linux-x64.tar.gz
```

```bash
./larrier/install.sh
```

Or run `./larrier/larrier` straight from the folder. `install.sh --remove`
uninstalls it; your sessions and settings stay in `~/.local/share/larrier`.

The window has your desktop's own title bar, with Larrier's menus under it.
On Wayland and X11 alike.

## Updates

Larrier looks for a new release on its own (Settings → About):

- It downloads the new program for your system, checks it against its
  published SHA-256 and puts it in place on restart: the `.exe` on Windows,
  the whole Larrier.app on macOS, the program on Linux. No building, no
  source code.
- **Stable** gets releases; **Preview** also gets pre-releases such as
  `0.3.0-beta.1`.
- Settings → About chooses whether Larrier only tells you, downloads, or
  also installs when it closes.
- After an update Larrier shows what's new; every version's notes are built
  in (About → Release notes).

On macOS and Linux, Larrier updates itself where it is, so it needs to be
able to write there (Applications and `~/.local/bin` both work). A copy
installed by root or a package manager is updated the same way you
installed it.

Updates come from the public GitHub releases and need no account. When
Larrier finds a GitHub token (`LARRIER_GITHUB_TOKEN`, `GITHUB_TOKEN`, or a
signed-in GitHub CLI) it sends it along, which raises GitHub's limit on
requests. A copy that updates from a private fork (`LARRIER_UPDATE_REPO`)
needs one.

## Your data

| System | Location |
|---|---|
| Windows | `%APPDATA%\Larrier` |
| macOS | `~/Library/Application Support/Larrier` |
| Linux | `~/.local/share/larrier` |

Set `LARRIER_HOME` to use another folder. What's in it is described in
[privacy.md](privacy.md#what-larrier-stores).

## Agents

Larrier finds the agent programs you already have (on `PATH` and in their
usual install folders, on every system). In **Settings → Agents** you can
point an agent at a specific program, turn agents off, and install or
update them with their makers' installer for your system; see
[agents.md](agents.md#installing-and-updating-agents).

## Command-line flags

| Flag | Effect |
|---|---|
| `--version` | Print the version, commit and build date, then exit. |
| `--portable` | Windows: run this copy without installing it. |
| `--uninstall` | Windows: start the uninstaller. |
