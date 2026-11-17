# Security

## Reporting a vulnerability

Please don't report security issues in public issues. Use GitHub's
[private vulnerability reporting](https://github.com/Dev4YM/Lary/security/advisories/new)
instead, with:

- what the issue is and what an attacker could do with it;
- steps to reproduce, or a proof of concept;
- the Lary version (`Lary.exe --version`) and your OS.

You'll get an answer within a week. Once a fix is released, the advisory is
published with credit to you, unless you'd rather stay anonymous.

## Supported versions

Only the latest release gets security fixes. Installed copies update
themselves, so staying current is usually automatic.

## Scope

In scope:

- The Lary app and CLI in this repository.
- Its updater (downloads, SHA-256 checks, replacing the program).
- How it stores secrets (Credential Manager, the macOS Keychain, the Linux
  Secret Service) and what it writes to disk.
- The browser tools it exposes to agents (`Lary mcp-browser`).

Out of scope: the agents Lary runs (report those to their makers) and
anything an agent does with the permissions you grant it.

## How Lary handles sensitive data

- API keys and tokens live in the OS credential store, never in config files
  or logs.
- Agent folders are read-only for Lary, except for writing a session an
  agent will resume.
- Updates are only installed after their SHA-256 matches the published one.
- Releases aren't code-signed: no Windows code-signing certificate, no Apple
  Developer ID or notarization. A download can be checked against the
  `.sha256` published beside it. Those checksums come from the same
  release, so they detect a broken or swapped file, not a compromised
  release; building from source avoids trusting the prebuilt programs.

See [docs/privacy.md](docs/privacy.md) for everything Lary stores, reads
and connects to.
