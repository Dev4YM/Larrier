# Installing and updating

> Lary was called **Larrier**. A Larrier that updates becomes Lary: on
> Windows it installs itself as Lary (Program files, shortcuts and the entry
> in Settings → Apps), and on every system its data folder (chats,
> settings, worktrees, pictures) moves to Lary's, with saved keys copied
> over. `LARRIER_HOME` still works as `LARY_HOME`.

Lary runs on **Windows 10 and 11**, **macOS 11 or newer** (Apple silicon
and Intel) and **Linux** (x86_64, with WebKitGTK 4.1). Every release has a
program for each, from the
[latest release](https://github.com/Dev4YM/Lary/releases/latest):

| System | Download |
|---|---|
| Windows 10 / 11 (x64) | `Lary-<version>-windows-x64.exe` |
| macOS, Apple silicon | `Lary-<version>-macos-arm64.zip` |
| macOS, Intel | `Lary-<version>-macos-x64.zip` |
| Linux (x64) | `Lary-<version>-linux-x64.tar.gz` |

Each comes with a `.sha256` file to check the download against.

## Lary isn't signed

Lary's programs aren't code-signed. The Windows program carries no
code-signing certificate, and the Mac app is signed ad hoc, without an
Apple Developer ID, and isn't notarized by Apple. So Windows and macOS warn
the first time you open a download; the steps for each are below. Updates
Lary installs itself don't ask again: Lary downloads them, not a
browser, so they carry no download mark.

To make sure a download is the published one, compare its SHA-256 with the
first word of its `.sha256` file:

```powershell
(Get-FileHash .\Lary-<version>-windows-x64.exe -Algorithm SHA256).Hash
```

```bash
shasum -a 256 Lary-<version>-macos-arm64.zip      # macOS
```

```bash
sha256sum Lary-<version>-linux-x64.tar.gz         # Linux
```

The checksums are published with the release, so they catch a broken or
swapped download, not a release replaced on GitHub as a whole. To trust no
prebuilt program at all, build Lary yourself ([building.md](building.md)).

## Windows

Run `Lary-<version>-windows-x64.exe`. Because it isn't signed, Microsoft
Defender SmartScreen may stop it with **Windows protected your PC**: click
**More info**, then **Run anyway**. On Windows 11 with Smart App Control
turned on, unsigned programs are blocked without that choice; Lary can't
start there unless Smart App Control is off (Windows Security → App &
browser control).

The program installs itself:

- **Just for me:** no administrator rights. The app goes to
  `%LOCALAPPDATA%\Programs\Lary`, everything else stays in your profile.
- **For everyone on this computer:** needs administrator rights. The app
  goes to `Program Files\Lary` and shared files to
  `ProgramData\Lary`; each person's sessions and settings stay in their
  own profile.
- **Run without installing:** starts that copy as it is (portable).

You get a Start menu shortcut (and a Desktop one if you like), and Lary
appears in Settings → Apps, where it uninstalls, optionally with your data.
WebView2, which Lary draws with, is part of Windows 10 and 11.

## macOS

1. Unzip `Lary-<version>-macos-<arch>.zip` and drag **Lary.app** to
   Applications (or anywhere you like).
2. The first time, macOS refuses to open it, because it isn't notarized.
   Allow it once; after that it opens normally:
   - **macOS 15 (Sequoia) and newer:** open Lary.app and click **Done**
     when macOS refuses. Then go to **System Settings → Privacy & Security**,
     scroll to **Security**, click **Open Anyway** next to Lary and
     confirm with your password or Touch ID.
   - **macOS 11 to 14:** right-click Lary.app → **Open**, then **Open**
     again.

   If macOS says Lary "is damaged", or there's no **Open Anyway** button,
   remove the download mark in Terminal and open it again:

   ```bash
   xattr -dr com.apple.quarantine /Applications/Lary.app
   ```

Lary's menus are in the menu bar, and shortcuts use ⌘ where Windows and
Linux use Ctrl. To uninstall, move Lary.app to the Trash; your sessions
and settings stay in `~/Library/Application Support/Lary` until you
delete that folder.

## Linux

Lary needs WebKitGTK 4.1 and GTK 3:

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
`~/.local/bin` and Lary in your applications menu (no root needed):

```bash
tar -xzf Lary-<version>-linux-x64.tar.gz
```

```bash
./lary/install.sh
```

Or run `./lary/lary` straight from the folder. `install.sh --remove`
uninstalls it; your sessions and settings stay in `~/.local/share/lary`.

The window has your desktop's own title bar, with Lary's menus under it.
On Wayland and X11 alike.

## Updates

Lary looks for a new release on its own (Settings → About):

- It downloads the new program for your system, checks it against its
  published SHA-256 and puts it in place on restart: the `.exe` on Windows,
  the whole Lary.app on macOS, the program on Linux. No building, no
  source code.
- **Stable** gets releases; **Preview** also gets pre-releases such as
  `0.3.0-beta.1`.
- Settings → About chooses whether Lary only tells you, downloads, or
  also installs when it closes.
- After an update Lary shows what's new; every version's notes are built
  in (About → Release notes).

On macOS and Linux, Lary updates itself where it is, so it needs to be
able to write there (Applications and `~/.local/bin` both work). A copy
installed by root or a package manager is updated the same way you
installed it.

Updates come from the public GitHub releases: no GitHub account or token
is needed or used.

## The browser engine

On Windows the Browser tab runs on Chromium (the Chromium Embedded
Framework). The engine isn't part of the program: the first time you use
the Browser tab, Lary downloads it from Lary's GitHub releases
(`Lary-engine-<version>-windows-x64.zip`, about 186 MB), checks it
against its published SHA-256 and unpacks it to
`%LOCALAPPDATA%\Lary\engine\<version>` (about 410 MB). Each Windows user
has their own copy. An update that moves to a newer engine downloads it
the same way and removes the old one; uninstalling removes it.

## Your data

| System | Location |
|---|---|
| Windows | `%APPDATA%\Lary` |
| macOS | `~/Library/Application Support/Lary` |
| Linux | `~/.local/share/lary` |

Set `LARY_HOME` to use another folder. What's in it is described in
[privacy.md](privacy.md#what-lary-stores).

## Agents

Lary finds the agent programs you already have (on `PATH` and in their
usual install folders, on every system). In **Settings → Harnesses** you can
point an agent at a specific program, turn agents off, and install or
update them with their makers' installer for your system; see
[agents.md](agents.md#installing-and-updating-agents).

## Command-line flags

| Flag | Effect |
|---|---|
| `--version` | Print the version, commit and build date, then exit. |
| `--portable` | Windows: run this copy without installing it. |
| `--uninstall` | Windows: start the uninstaller. |
