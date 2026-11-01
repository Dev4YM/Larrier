# Releasing

## Versions

Larrier follows [Semantic Versioning](https://semver.org). The version lives
in `Cargo.toml` (`[workspace.package]`). Each build also records its commit
and date: `Larrier.exe --version` prints `Larrier 0.2.0 (1a2b3c4d5,
2026-11-02)`.

Commits follow [Conventional Commits](https://www.conventionalcommits.org);
the changelog is made from their subjects:

| Type | Changelog section |
|---|---|
| `feat` | New |
| `style` | Look and feel |
| `perf` | Faster |
| `fix` | Fixed |
| `!` after the type, or `BREAKING CHANGE` in the body | Breaking |

Other types (`refactor`, `docs`, `build`, `ci`, `test`, `chore`) don't
appear in the changelog.

## Cutting a release

`scripts/release.ps1` (on Windows) does it end to end: it bumps the
version, writes the release's section of [CHANGELOG.md](../CHANGELOG.md),
commits and tags, builds the Windows program, checks that it reports the
new version, and with `-Publish` pushes and publishes the GitHub release
with the program and its SHA-256.

```powershell
./scripts/release.ps1 -Notes                  # what the next release would say
./scripts/release.ps1 -Bump patch -Publish    # 0.2.0 → 0.2.1
./scripts/release.ps1 -Bump minor -Publish    # 0.2.1 → 0.3.0
./scripts/release.ps1 -Bump minor -Pre beta   # 0.3.0-beta.1, a preview
```

Pushing a `v*` tag runs the Release workflow
(`.github/workflows/release.yml`). It builds on GitHub's runners for every
system the release doesn't have a program for yet (Windows on
`windows-latest`, both Macs on `macos-15`, Linux on `ubuntu-22.04`) and
adds them to the release, creating it with the changelog notes if the
script didn't. A release cut without the script (a tag pushed by hand, or
the workflow run with a tag) gets all four programs from the workflow.

Each platform can also be built by hand with `scripts/build.ps1` or
`scripts/build.sh` (see [building.md](building.md#packages)).

## What a release contains

| Asset | What it is |
|---|---|
| `Larrier-<version>-windows-x64.exe` | The Windows app, which installs itself. |
| `Larrier-<version>-macos-arm64.zip` | Larrier.app for Apple silicon (signed ad hoc, not notarized). |
| `Larrier-<version>-macos-x64.zip` | Larrier.app for Intel Macs. |
| `Larrier-<version>-linux-x64.tar.gz` | The Linux program, its launcher entry, icon and `install.sh`. |

Each has a `.sha256` beside it; installed copies check the download against
it before updating, and pick the asset for their own system.

- The version's changelog section as release notes. Versions with a suffix
  (`-beta.1`) are published as pre-releases and only reach the Preview
  channel.
