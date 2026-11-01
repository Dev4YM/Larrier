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

`scripts/release.ps1` does it end to end: it bumps the version, writes the
release's section of [CHANGELOG.md](../CHANGELOG.md), commits and tags,
builds the program, checks that it reports the new version, and with
`-Publish` pushes and publishes the GitHub release with the program and its
SHA-256.

```powershell
./scripts/release.ps1 -Notes                  # what the next release would say
./scripts/release.ps1 -Bump patch -Publish    # 0.2.0 → 0.2.1
./scripts/release.ps1 -Bump minor -Publish    # 0.2.1 → 0.3.0
./scripts/release.ps1 -Bump minor -Pre beta   # 0.3.0-beta.1, a preview
```

Pushing a `v*` tag also runs the Release workflow
(`.github/workflows/release.yml`), which builds and publishes on GitHub's
Windows runners and skips a release the script already published.

## What a release contains

- `Larrier-<version>-windows-x64.exe`: the app, which installs itself.
- `Larrier-<version>-windows-x64.exe.sha256`: installed copies check the
  download against it before updating.
- The version's changelog section as release notes. Versions with a suffix
  (`-beta.1`) are published as pre-releases and only reach the Preview
  channel.
