#!/usr/bin/env python3
"""Builds a sample Larrier home for screenshots, demos and UI work: four
made-up projects (real git repositories, one with uncommitted changes), a
dozen chats across agents and states, and `demo_agent.py` standing in for
Claude Code so chats can run live without an account.

Nothing here reads or touches your own Larrier data or agent folders. Run
Larrier against the result with the environment this script prints:

    python scripts/demo/make_demo.py            # → %TEMP%/larrier-demo
    python scripts/demo/make_demo.py --out D:/demo --force

Every name, address and number in it is invented.
"""

import argparse
import json
import os
import shutil
import subprocess
import sys
import tempfile
import uuid
from datetime import datetime, timedelta, timezone

HERE = os.path.dirname(os.path.abspath(__file__))
NOW = datetime.now(timezone.utc)
PERSON = ("Sam Rivera", "sam@northwind.example")


def ago(minutes):
    return NOW - timedelta(minutes=minutes)


def ts(t):
    return t.strftime("%Y-%m-%dT%H:%M:%S.") + f"{t.microsecond // 1000:03d}Z"


def sample(name):
    with open(os.path.join(HERE, "sample", name), encoding="utf-8") as f:
        return f.read()


def write(path, text):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8", newline="\n") as f:
        f.write(text)


# ------------------------------------------------------------------ projects

def git(repo, *args, when=None):
    env = dict(os.environ, GIT_AUTHOR_NAME=PERSON[0], GIT_AUTHOR_EMAIL=PERSON[1], GIT_COMMITTER_NAME=PERSON[0], GIT_COMMITTER_EMAIL=PERSON[1])
    if when:
        env["GIT_AUTHOR_DATE"] = env["GIT_COMMITTER_DATE"] = ts(when)
    subprocess.run(["git", "-c", "core.autocrlf=false", "-c", "commit.gpgsign=false", *args], cwd=repo, env=env, check=True, capture_output=True)


def repo(root, name, commits, branch=None, changes=None):
    """A repository built from `commits` ([(message, days ago, {path: text})]),
    on `branch`, with `changes` left in the working tree."""
    path = os.path.join(root, name)
    os.makedirs(path)
    git(path, "init", "-q", "-b", "main")
    for message, days, files in commits:
        for rel, text in files.items():
            write(os.path.join(path, rel), text)
        git(path, "add", "-A")
        git(path, "commit", "-q", "-m", message, when=ago(days * 1440))
    if branch:
        git(path, "checkout", "-q", "-b", branch)
    for rel, text in (changes or {}).items():
        write(os.path.join(path, rel), text)
    return path


NORTHWIND_CARGO = """[package]
name = "northwind-api"
version = "2.3.1"
edition = "2021"

[dependencies]
axum = "0.8"
bytes = "1"
serde = { version = "1", features = ["derive"] }
sqlx = { version = "0.8", features = ["runtime-tokio", "postgres", "uuid", "chrono"] }
tokio = { version = "1", features = ["full"] }
tracing = "0.1"
"""

NORTHWIND_MAIN = """mod admin;
mod ledger;
mod payments;
mod webhooks;

use axum::{routing::post, Router};

#[derive(Clone)]
pub struct AppState {
    pub db: sqlx::PgPool,
    pub config: std::sync::Arc<Config>,
}

pub struct Config {
    pub stripe_webhook_secret: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    let db = sqlx::PgPool::connect(&std::env::var("DATABASE_URL")?).await?;
    sqlx::migrate!().run(&db).await?;
    let config = std::sync::Arc::new(Config { stripe_webhook_secret: std::env::var("STRIPE_WEBHOOK_SECRET")? });
    let app = Router::new()
        .route("/webhooks/stripe", post(webhooks::stripe::handle))
        .with_state(AppState { db, config });
    let listener = tokio::net::TcpListener::bind("0.0.0.0:8080").await?;
    axum::serve(listener, app).await?;
    Ok(())
}
"""

ORBIT_PAGE = """import { Suspense } from 'react'
import { Charts } from '@/components/charts'
import { RangePicker } from '@/components/charts/range-picker'

export default function Dashboard({ searchParams }: { searchParams: { range?: string } }) {
  return (
    <main className="grid gap-6 p-8">
      <header className="flex items-center justify-between">
        <h1 className="text-2xl font-semibold">Dashboard</h1>
        <RangePicker value={searchParams.range ?? '30d'} />
      </header>
      <Suspense fallback={<div className="h-80 animate-pulse rounded-xl bg-muted" />}>
        <Charts />
      </Suspense>
    </main>
  )
}
"""

ORBIT_CHART = """'use client'

import { useEffect, useState } from 'react'
import { Area, AreaChart, Tooltip, XAxis } from 'recharts'

export function RevenueChart() {
  const [data, setData] = useState<{ day: string; cents: number }[]>([])
  useEffect(() => {
    fetch('/api/metrics/revenue').then((r) => r.json()).then(setData)
  }, [])
  return (
    <AreaChart width={720} height={280} data={data}>
      <XAxis dataKey="day" />
      <Tooltip />
      <Area dataKey="cents" type="monotone" />
    </AreaChart>
  )
}
"""

LEDGER_IMPORT = """package cmd

import (
\t"encoding/csv"
\t"fmt"
\t"os"
)

// importCSV reads transactions from a bank export. Columns are fixed for now:
// date, description, amount.
func importCSV(path string) ([]Txn, error) {
\tf, err := os.Open(path)
\tif err != nil {
\t\treturn nil, err
\t}
\tdefer f.Close()
\trows, err := csv.NewReader(f).ReadAll()
\tif err != nil {
\t\treturn nil, fmt.Errorf("reading %s: %w", path, err)
\t}
\treturn parseRows(rows[1:])
}
"""

ATLAS_PG = """module "postgres" {
  source            = "./modules/postgres"
  name              = "northwind-staging"
  engine_version    = "15.8"
  instance_class    = "db.r6g.large"
  allocated_storage = 200
  multi_az          = true
  backup_window     = "03:00-04:00"
}
"""


def projects(root):
    northwind = repo(root, "northwind-api", [
        ("Initial API with Stripe webhooks", 41, {
            "Cargo.toml": NORTHWIND_CARGO,
            "README.md": "# northwind-api\n\nPayments and payouts for Northwind merchants.\n",
            "src/main.rs": NORTHWIND_MAIN,
            "src/webhooks/mod.rs": "pub mod signature;\npub mod stripe;\n",
            "src/webhooks/signature.rs": "pub use stripe_verify::{verify, Event};\n",
            "src/webhooks/stripe.rs": sample("stripe_before.rs"),
            "src/ledger/mod.rs": "mod apply;\n\npub use apply::{apply_payment, apply_refund};\n",
            "src/ledger/apply.rs": sample("apply_before.rs"),
            "src/payments.rs": "pub struct Payment {\n    pub account_id: uuid::Uuid,\n    pub amount_cents: i64,\n    pub currency: String,\n}\n\npub type Refund = Payment;\n",
            "src/admin/mod.rs": "pub mod replay;\n",
            "src/admin/replay.rs": "// Replays a payment from the admin console.\n",
            "tests/webhooks.rs": sample("tests_before.rs"),
        }),
        ("Add payouts table", 9, {"migrations/0041_payouts.sql": sample("payouts.sql")}),
    ], branch="fix/webhook-idempotency", changes={
        "Cargo.toml": NORTHWIND_CARGO.replace('bytes = "1"\n', 'bytes = "1"\nmetrics = "0.24"\n'),
        "src/webhooks/stripe.rs": sample("stripe_after.rs"),
        "src/ledger/apply.rs": sample("apply_after.rs"),
        "migrations/0042_processed_events.sql": sample("processed_events.sql"),
        "tests/webhooks.rs": sample("tests_after.rs"),
    })
    orbit = repo(root, "orbit-web", [
        ("Dashboard with revenue, signups and churn charts", 20, {
            "package.json": json.dumps({"name": "orbit-web", "private": True, "scripts": {"dev": "next dev", "build": "next build"},
                                        "dependencies": {"next": "15.4.2", "react": "19.1.0", "recharts": "2.15.1"}}, indent=2) + "\n",
            "app/dashboard/page.tsx": ORBIT_PAGE,
            "components/charts/revenue-chart.tsx": ORBIT_CHART,
            "components/charts/signups-chart.tsx": ORBIT_CHART.replace("Revenue", "Signups").replace("revenue", "signups"),
            "components/charts/churn-chart.tsx": ORBIT_CHART.replace("Revenue", "Churn").replace("revenue", "churn"),
            "components/charts/range-picker.tsx": "'use client'\n\nexport function RangePicker({ value }: { value: string }) {\n  return <select defaultValue={value} />\n}\n",
        }),
    ], branch="perf/server-charts")
    ledger = repo(root, "ledger-cli", [
        ("ledger: balances and monthly reports", 64, {
            "go.mod": "module github.com/northwind/ledger-cli\n\ngo 1.24\n",
            "main.go": "package main\n\nimport \"github.com/northwind/ledger-cli/cmd\"\n\nfunc main() { cmd.Execute() }\n",
            "cmd/import.go": LEDGER_IMPORT,
        }),
    ])
    atlas = repo(root, "atlas-infra", [
        ("Staging environment", 30, {
            "envs/staging/main.tf": ATLAS_PG,
            "modules/postgres/main.tf": "variable \"engine_version\" {}\nvariable \"name\" {}\n",
        }),
    ])
    return {"northwind": northwind, "orbit": orbit, "ledger": ledger, "atlas": atlas}


# ------------------------------------------------------------------ chats

class Chat:
    def __init__(self, title, agent, model, status, minutes, workspace=None, branch=None, **header):
        self.id = str(uuid.uuid4())
        self.created = ago(minutes + 25)
        self.updated = ago(minutes)
        self.header = {
            "id": self.id, "title": title, "created_at": ts(self.created), "updated_at": ts(self.updated),
            "status": status, "agent": agent, "model": model,
        }
        self.header.update(header)
        if workspace:
            self.header["workspace"] = {"path": workspace, "name": os.path.basename(workspace), "branch": branch or "main"}
        self.header["origin"] = {"agent": agent}
        self.agent, self.model = agent, model
        self.messages = []
        self.clock = self.created
        self.usage = {"input_tokens": 0, "output_tokens": 0, "cache_read_tokens": 0, "cache_write_tokens": 0, "reasoning_tokens": 0}

    def _msg(self, role, parts, usage=None):
        self.clock += timedelta(seconds=40)
        m = {"id": str(uuid.uuid4()), "role": role, "created_at": ts(min(self.clock, self.updated)), "parts": parts}
        if self.messages:
            m["parent_id"] = self.messages[-1]["id"]
        if role == "assistant":
            m["agent"], m["model"] = self.agent, self.model
        if usage:
            m["usage"] = usage
            for k in ("input_tokens", "output_tokens", "cache_read_tokens"):
                self.usage[k] += usage.get(k, 0)
        self.messages.append(m)
        return self

    def ask(self, text):
        return self._msg("user", [{"type": "text", "text": text}])

    def reply(self, *parts, context=52_000):
        usage = {"input_tokens": 1_800, "output_tokens": 2_400, "cache_read_tokens": context - 1_800, "cache_write_tokens": 0,
                 "reasoning_tokens": 600, "context_tokens": context, "context_window": 200_000}
        return self._msg("assistant", list(parts), usage)

    def lines(self):
        head = dict(self.header, usage=self.usage)
        out = [json.dumps(dict({"lsf": "1.0", "kind": "session"}, **head))]
        out += [json.dumps(dict({"kind": "message"}, **m)) for m in self.messages]
        return "\n".join(out) + "\n"


def text(t):
    return {"type": "text", "text": t}


def think(t, ms=4_000):
    return {"type": "reasoning", "text": t, "duration_ms": ms}


_n = [0]


def tool(name, kind, title, input=None, output=None, status="completed", diffs=None, ms=900):
    _n[0] += 1
    part = {"type": "tool", "id": f"toolu_demo{_n[0]:04d}", "name": name, "kind": kind, "title": title,
            "input": input or {}, "status": status, "duration_ms": ms}
    if output is not None:
        part["output"] = output
    if diffs:
        part["diffs"] = diffs
    return part


def diff(path, old, new):
    import difflib
    a, b = (old or "").splitlines(), new.splitlines()
    added = removed = 0
    for tag, i1, i2, j1, j2 in difflib.SequenceMatcher(a=a, b=b).get_opcodes():
        removed += (i2 - i1) if tag in ("replace", "delete") else 0
        added += (j2 - j1) if tag in ("replace", "insert") else 0
    d = {"path": path, "new_text": new, "added": added, "removed": removed}
    if old is not None:
        d["old_text"] = old
    return d


def plan(*entries):
    return {"type": "plan", "entries": [{"content": c, "status": s} for c, s in entries]}


def chats(p, demo_login, account):
    nw, orbit, ledger, atlas = p["northwind"], p["orbit"], p["ledger"], p["atlas"]
    j = os.path.join
    out = []

    def native(chat, count):
        sid = str(uuid.uuid4())
        path = j(demo_login, "projects", "demo", f"{sid}.jsonl")
        write(path, "")
        chat.header["native"] = [{"agent": "claude-code", "id": sid, "path": path, "synced_messages": count, "account_id": account}]
        chat.header["account_id"] = account

    # Runs live with the demo agent (see capture steps in docs/screenshots.md).
    c = Chat("Make Stripe webhooks idempotent", "claude-code", "claude-opus-5-5", "idle", 1, nw, "fix/webhook-idempotency", mode="default")
    c.ask("Where do Stripe webhooks enter the API, and what happens when Stripe retries one?")
    c.reply(
        tool("Read", "read", "src/main.rs", {"file_path": j(nw, "src/main.rs")}, NORTHWIND_MAIN),
        tool("Read", "read", "src/webhooks/stripe.rs", {"file_path": j(nw, "src/webhooks/stripe.rs")}, sample("stripe_before.rs")),
        tool("Grep", "search", "apply_payment", {"pattern": "apply_payment"}, "src/webhooks/stripe.rs\nsrc/ledger/apply.rs\nsrc/admin/replay.rs"),
        text("Webhooks enter at `POST /webhooks/stripe` (`src/main.rs`), which verifies the signature and calls "
             "`ledger::apply_payment` or `ledger::apply_refund` directly.\n\n"
             "**Retries aren't handled.** Stripe redelivers until it gets a 2xx, and nothing records which events were applied, so:\n\n"
             "- a delivery that times out after the ledger write is applied **again** on retry;\n"
             "- two deliveries of the same event can run **concurrently** and both credit the merchant.\n\n"
             "The fix is to claim each `event.id` in the same transaction as the ledger write."),
    )
    native(c, 2)
    out.append(c)

    c = Chat("Move dashboard charts to server components", "claude-code", "claude-opus-5-5", "idle", 3, orbit, "perf/server-charts")
    c.ask("Why does the dashboard take three seconds to show its charts on a cold load?")
    c.reply(text("Each chart is a client component that fetches its data in `useEffect`, after hydration, so a cold load waits on "
                 "the JavaScript bundle, then three API round trips in sequence. Rendering the charts on the server removes both waits."))
    native(c, 2)
    out.append(c)

    c = Chat("Add rate limiting to /v1/payouts", "codex", "gpt-5-codex", "review", 48, nw, "main", unread=True)
    c.ask("Rate-limit POST /v1/payouts to 10 requests a minute per merchant, with a Retry-After header.")
    limiter = "use std::time::Duration;\n\npub struct Limiter {\n    per_minute: u32,\n}\n\nimpl Limiter {\n    pub fn new(per_minute: u32) -> Self {\n        Self { per_minute }\n    }\n\n    pub fn retry_after(&self) -> Duration {\n        Duration::from_secs(60 / self.per_minute as u64)\n    }\n}\n"
    c.reply(think("A token bucket per merchant in Redis keeps limits across instances."),
            tool("apply_patch", "edit", "src/limits.rs", output="Success", diffs=[diff("src/limits.rs", None, limiter)]),
            tool("shell", "shell", "cargo test limits", {"command": "cargo test limits"}, "test result: ok. 6 passed; 0 failed"),
            text("Payouts are limited to 10 a minute per merchant with a Redis token bucket; a limited request gets `429` and a `Retry-After` header. Six tests cover the bucket and the header."))
    out.append(c)

    c = Chat("Investigate slow ledger reconciliation", "claude-code", "claude-sonnet-5-5", "done", 190, nw, "main")
    c.ask("Nightly reconciliation went from 4 to 19 minutes last week. Find out why.")
    c.reply(tool("Bash", "shell", "EXPLAIN ANALYZE the reconciliation query", {"command": "psql -f explain.sql"}, "Seq Scan on ledger_entries  (actual time=0.02..14213.9 rows=48211904)"),
            text("The reconciliation query lost its index when `0039_ledger_partitions.sql` recreated `ledger_entries`: it now scans 48M rows. Restoring `ledger_entries (account_id, created_at)` brings it back to about 3 minutes."))
    out.append(c)

    c = Chat("Fix hydration warning on /settings", "cursor", "composer-2", "review", 22, orbit, "main", unread=True)
    c.ask("/settings logs a hydration mismatch in dev. Fix it.")
    old = "export function LastSeen({ at }: { at: Date }) {\n  return <span>{at.toLocaleString()}</span>\n}\n"
    new = "'use client'\n\nimport { useEffect, useState } from 'react'\n\nexport function LastSeen({ at }: { at: Date }) {\n  const [text, setText] = useState(at.toISOString())\n  useEffect(() => setText(at.toLocaleString()), [at])\n  return <time dateTime={at.toISOString()}>{text}</time>\n}\n"
    c.reply(tool("edit_file", "edit", "components/last-seen.tsx", diffs=[diff("components/last-seen.tsx", old, new)]),
            text("`LastSeen` formatted the date with the server's locale, then again with the browser's. It now renders the ISO time on both and switches to the local format after hydration."))
    out.append(c)

    c = Chat("Add a keyboard shortcuts overlay", "gemini", "gemini-3-pro", "done", 1500, orbit, "main")
    c.ask("Add a ? overlay that lists the dashboard's keyboard shortcuts.")
    c.reply(text("Added `ShortcutsOverlay`, opened with `?` and closed with Esc, listing the shortcuts from the existing `useHotkeys` registry so it never drifts from them."))
    out.append(c)

    c = Chat("CSV import with column mapping", "opencode", "glm-4.6", "needs_input", 6, ledger, "main", unread=True)
    c.ask("Let `ledger import` take any bank's CSV: detect the columns, or let me map them with flags.")
    c.reply(tool("read", "read", "cmd/import.go", output=LEDGER_IMPORT),
            text("Banks disagree on the amount column: some use one signed column, others separate debit and credit columns. Which should `--map` accept?"),
            tool("question", "other", "Amount columns", {"question": "Signed amount, or debit and credit?"}, status="approval"))
    out.append(c)

    c = Chat("Speed up the balance report", "codex", "gpt-5-codex", "review", 130, ledger, "main")
    c.ask("`ledger report balance --year` takes 9 seconds on 3 years of data. Make it fast.")
    c.reply(tool("shell", "shell", "go test -bench Balance ./...", {"command": "go test -bench Balance ./..."}, "BenchmarkBalance-12    	     142	   8402113 ns/op\nBenchmarkBalance-12    	    1874	    611204 ns/op"),
            text("The report re-parsed every transaction once per month. It now aggregates in one pass and caches parsed amounts: 8.4 ms → 0.6 ms per report in the benchmark, 9 s → 0.7 s on the sample data."))
    out.append(c)

    c = Chat("Plan the Postgres 16 upgrade for staging", "claude-code", "claude-opus-5-5", "needs_input", 12, atlas, "main", pinned=True, mode="plan")
    c.ask("Plan upgrading staging Postgres from 15.8 to 16 with under 5 minutes of downtime.")
    c.reply(think("Blue/green deployment keeps the old instance until the switchover; logical replication needs every table to have a primary key."),
            plan(("Snapshot staging and confirm restore", "completed"), ("Create a blue/green deployment on 16.4", "in_progress"),
                 ("Check replication lag under the load test", "pending"), ("Switch over and keep the old instance for 48 h", "pending")),
            text("Ready to run the first step. `terraform apply` will create the green environment (no downtime yet)."),
            tool("Bash", "shell", "terraform apply -target=module.postgres", {"command": "terraform apply -target=module.postgres"}, status="approval"))
    out.append(c)

    c = Chat("Tag every resource with its cost center", "droid", None, "done", 300, atlas, "main")
    c.ask("Every AWS resource in staging needs a cost-center tag. Add it everywhere through default_tags.")
    c.reply(text("Added `default_tags { cost-center = var.cost_center }` to the AWS provider and removed 41 per-resource `tags` blocks that only set it."))
    out.append(c)

    c = Chat("Compare vector databases for search", "chat", "claude-sonnet-5-5", "done", 540)
    c.ask("We need semantic search over 30M support tickets. Compare pgvector, Qdrant and Turbopuffer.")
    c.reply(text("| | pgvector | Qdrant | Turbopuffer |\n|---|---|---|---|\n| Runs in | your Postgres | own cluster | managed |\n| Filtering | SQL | payload indexes | attributes |\n| 30M vectors | needs tuning | comfortable | comfortable |\n\nIf tickets already live in Postgres, start with **pgvector** and HNSW; move to Qdrant when filtered recall drops."))
    out.append(c)

    c = Chat("Release notes for v2.4", "hermes", None, "done", 1440)
    c.ask("Write release notes for v2.4 from these merged PRs.")
    c.reply(text("## v2.4\n\n- **Payouts:** schedule payouts weekly or monthly.\n- **Webhooks:** retries no longer apply an event twice.\n- **Dashboard:** charts load up to 3× faster."))
    out.append(c)

    c = Chat("Explain Rust's Pin in one page", "pi", "qwen3-coder", "done", 2900)
    c.ask("Explain Pin like I know Rust but never needed it.")
    c.reply(text("`Pin<P>` promises that the value behind `P` won't move again. Async state machines can hold references into themselves, so after the first poll they must stay put."))
    out.append(c)
    return out


# ------------------------------------------------------------------ home

def home(out, p, agent_cmd, demo_login, account):
    root = os.path.join(out, "home")
    os.makedirs(os.path.join(root, "sessions"))
    for c in chats(p, demo_login, account):
        write(os.path.join(root, "sessions", f"{c.id}.lsf.jsonl"), c.lines())
    workspaces = [{"id": str(uuid.uuid4()), "name": os.path.basename(path), "path": path, "added_at": ts(ago(60 * 24 * 30))} for path in p.values()]
    config = {
        "version": 1,
        "settings": {
            "theme": "dark", "auto_import": False, "live_sync": False, "continue_interrupted": False,
            "notify_on_finish": False, "completion_sound": False, "tray_icon": False, "show_tips": False,
            "restore_window": False, "updates": "off", "auto_update": False, "default_agent": "claude-code", "chat_width": 760,
            "sidebar_opened": list(p.values()),
        },
        "agents": [{"kind": "claude-code", "enabled": True, "binary": agent_cmd, "import": False}]
                  + [{"kind": k, "enabled": True, "import": False} for k in ("codex", "cursor", "opencode", "gemini", "hermes", "droid", "pi")],
        "accounts": [{"id": account, "agent": "claude-code", "label": "Work", "auth": {"type": "config_dir", "path": demo_login},
                      "is_default": True, "email": "sam@northwind.example", "plan": "Max", "created_at": ts(ago(60 * 24 * 30))}],
        "workspaces": workspaces,
        "profile": {"name": PERSON[0], "role": "Staff engineer, Payments", "location": "Lisbon", "about": "Demo profile"},
    }
    write(os.path.join(root, "config.json"), json.dumps(config, indent=2) + "\n")
    return root


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--out", default=os.path.join(tempfile.gettempdir(), "larrier-demo"))
    ap.add_argument("--force", action="store_true", help="replace an existing demo folder")
    args = ap.parse_args()
    out = os.path.abspath(args.out)
    if os.path.exists(out):
        if not args.force:
            sys.exit(f"{out} exists; pass --force to replace it")
        # Git keeps its objects read-only, which Windows won't delete as is.
        shutil.rmtree(out, onerror=lambda f, path, _: (os.chmod(path, 0o700), f(path)))
    os.makedirs(out)

    p = projects(os.path.join(out, "projects"))
    demo_login = os.path.join(out, "logins", "claude-work")
    write(os.path.join(demo_login, ".claude.json"), json.dumps({"oauthAccount": {"emailAddress": PERSON[1]}}) + "\n")
    agent = os.path.join(HERE, "demo_agent.py")
    if os.name == "nt":
        agent_cmd = os.path.join(out, "agent", "claude.cmd")
        write(agent_cmd, f'@echo off\r\n"{sys.executable}" "{agent}" %*\r\n')
    else:
        agent_cmd = os.path.join(out, "agent", "claude")
        write(agent_cmd, f'#!/bin/sh\nexec "{sys.executable}" "{agent}" "$@"\n')
        os.chmod(agent_cmd, 0o755)
    root = home(out, p, agent_cmd, demo_login, "acc-demo-claude")

    print(f"Demo home: {root}")
    print("Run Larrier against it (PowerShell):")
    print(f"  $env:LARRIER_HOME = '{root}'; $env:CLAUDE_CONFIG_DIR = '{demo_login}'")
    print("  cargo run -p larrier-app")


if __name__ == "__main__":
    main()
