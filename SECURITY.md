# Security

## Reporting a vulnerability

Please don't report security issues in public issues. Use GitHub's
[private vulnerability reporting](https://github.com/Dev4YM/Larrier/security/advisories/new)
instead, with:

- what the issue is and what an attacker could do with it;
- steps to reproduce, or a proof of concept;
- the Larrier version (`Larrier.exe --version`) and your OS.

You'll get an answer within a week. Once a fix is released, the advisory is
published with credit to you, unless you'd rather stay anonymous.

## Supported versions

Only the latest release gets security fixes. Installed copies update
themselves, so staying current is usually automatic.

## Scope

In scope:

- The Larrier app and CLI in this repository.
- Its updater (downloads, SHA-256 checks, replacing the program).
- How it stores secrets (the OS credential store) and what it writes to
  disk.
- The browser tools it exposes to agents (`Larrier mcp-browser`).

Out of scope: the agents Larrier runs (report those to their makers) and
anything an agent does with the permissions you grant it.

## How Larrier handles sensitive data

- API keys and tokens live in the OS credential store, never in config files
  or logs.
- Agent folders are read-only for Larrier, except for writing a session an
  agent will resume.
- Updates are only installed after their SHA-256 matches the published one.

See [docs/privacy.md](docs/privacy.md) for everything Larrier stores, reads
and connects to.
