#!/usr/bin/env python3
"""A stand-in for Claude Code that plays a scripted turn over Claude Code's
stream-json control protocol, so Larrier can be shown and worked on with
live agents and no API account.

`make_demo.py` sets it as Claude Code's program in a sample Larrier home.
The turn it plays depends on the prompt (see TURNS); it ends with work
still running and holds the turn open until Larrier stops it or closes
its input.
"""

import difflib
import json
import os
import queue
import sys
import threading
import time
import uuid

MODEL = "claude-opus-5-5"
SESSION = str(uuid.uuid4())
HERE = os.path.dirname(os.path.abspath(__file__))

out_lock = threading.Lock()
inbox: "queue.Queue[dict]" = queue.Queue()
stopped = threading.Event()


def emit(obj):
    with out_lock:
        sys.stdout.write(json.dumps(obj) + "\n")
        sys.stdout.flush()


def read_input():
    for line in sys.stdin:
        try:
            msg = json.loads(line)
        except ValueError:
            continue
        if msg.get("type") == "control_request":
            answer(msg)
        else:
            inbox.put(msg)
    stopped.set()


def answer(msg):
    req = msg.get("request", {})
    kind = req.get("subtype")
    response = {}
    if kind == "initialize":
        response = {
            "models": [
                {"value": "default", "resolvedModel": MODEL, "displayName": "Opus 5.5"},
                {"value": "sonnet", "resolvedModel": "claude-sonnet-5-5", "displayName": "Sonnet 5.5"},
            ],
            "commands": [{"name": "review", "description": "Review the current changes"}],
            "agents": [],
        }
    elif kind == "interrupt":
        stopped.set()
    emit({"type": "control_response", "response": {"subtype": "success", "request_id": msg.get("request_id"), "response": response}})


def pause(seconds):
    if stopped.wait(seconds):
        finish("interrupted")


def finish(how):
    emit({"type": "result", "subtype": "success" if how == "done" else "error_during_execution", "is_error": how != "done",
          "num_turns": 1, "result": "", "session_id": SESSION, "total_cost_usd": 0.0})
    sys.exit(0)


class Turn:
    """Builds the messages of one turn."""

    def __init__(self):
        self.n = 0
        self.context = 48_000

    def _id(self, prefix):
        self.n += 1
        return f"{prefix}_{SESSION[:8]}{self.n:03d}"

    def _usage(self):
        self.context += 3_800
        return {"input_tokens": 6, "cache_read_input_tokens": self.context - 1_200, "cache_creation_input_tokens": 1_194, "output_tokens": 420}

    def assistant(self, content, parent=None, mid=None):
        emit({"type": "assistant", "parent_tool_use_id": parent, "session_id": SESSION, "uuid": str(uuid.uuid4()),
              "message": {"id": mid or self._id("msg"), "type": "message", "role": "assistant", "model": MODEL,
                          "content": content, "stop_reason": None, "usage": self._usage()}})

    def think(self, text):
        self.assistant([{"type": "thinking", "thinking": text, "signature": ""}])

    def say(self, text, words_per_tick=3, tick=0.05):
        """Text written word by word, as Claude streams it."""
        mid = self._id("msg")
        emit({"type": "stream_event", "session_id": SESSION, "parent_tool_use_id": None,
              "event": {"type": "message_start", "message": {"id": mid, "role": "assistant", "model": MODEL, "content": []}}})
        emit({"type": "stream_event", "session_id": SESSION, "parent_tool_use_id": None,
              "event": {"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}})
        words = text.split(" ")
        for i in range(0, len(words), words_per_tick):
            chunk = " ".join(words[i:i + words_per_tick]) + (" " if i + words_per_tick < len(words) else "")
            emit({"type": "stream_event", "session_id": SESSION, "parent_tool_use_id": None,
                  "event": {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": chunk}}})
            pause(tick)
        emit({"type": "stream_event", "session_id": SESSION, "parent_tool_use_id": None, "event": {"type": "content_block_stop", "index": 0}})
        self.assistant([{"type": "text", "text": text}], mid=mid)

    def tool(self, name, input, parent=None):
        tid = self._id("toolu")
        self.assistant([{"type": "tool_use", "id": tid, "name": name, "input": input}], parent=parent)
        return tid

    def result(self, tid, text, meta=None, parent=None):
        line = {"type": "user", "parent_tool_use_id": parent, "session_id": SESSION, "uuid": str(uuid.uuid4()),
                "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": tid, "content": text}]}}
        if meta is not None:
            line["tool_use_result"] = meta
        emit(line)

    def read(self, path, lines):
        tid = self.tool("Read", {"file_path": path})
        pause(0.35)
        self.result(tid, "\n".join(f"{i + 1:>6}\t{l}" for i, l in enumerate(lines)))

    def edit(self, path, before, after):
        old, new = changed_region(before, after)
        tid = self.tool("Edit", {"file_path": path, "old_string": old, "new_string": new})
        pause(0.5)
        self.result(tid, f"The file {path} has been updated.", {"filePath": path, "structuredPatch": hunks(before, after), "originalFile": before})

    def write(self, path, content):
        tid = self.tool("Write", {"file_path": path, "content": content})
        pause(0.4)
        self.result(tid, f"File created successfully at: {path}", {"type": "create", "filePath": path, "content": content})


def changed_region(before, after):
    a, b = before.splitlines(), after.splitlines()
    m = difflib.SequenceMatcher(a=a, b=b)
    ops = [op for op in m.get_opcodes() if op[0] != "equal"]
    i1, j1 = ops[0][1], ops[0][3]
    i2, j2 = ops[-1][2], ops[-1][4]
    return "\n".join(a[i1:i2]), "\n".join(b[j1:j2])


def hunks(before, after):
    a, b = before.splitlines(), after.splitlines()
    out = []
    for group in difflib.SequenceMatcher(a=a, b=b).get_grouped_opcodes(3):
        lines = []
        for tag, i1, i2, j1, j2 in group:
            if tag == "equal":
                lines += [" " + l for l in a[i1:i2]]
            if tag in ("replace", "delete"):
                lines += ["-" + l for l in a[i1:i2]]
            if tag in ("replace", "insert"):
                lines += ["+" + l for l in b[j1:j2]]
        first, last = group[0], group[-1]
        out.append({"oldStart": first[1] + 1, "oldLines": last[2] - first[1], "newStart": first[3] + 1, "newLines": last[4] - first[3], "lines": lines})
    return out


def sample(name):
    with open(os.path.join(HERE, "sample", name), encoding="utf-8") as f:
        return f.read()


def webhooks_turn(cwd):
    """Idempotent Stripe webhooks in northwind-api."""
    t = Turn()
    p = lambda rel: os.path.join(cwd, rel)
    emit({"type": "rate_limit_event", "rate_limit_info": {"status": "allowed", "rateLimitType": "five_hour", "utilization": 0.38, "resetsAt": int(time.time()) + 7_400}})
    emit({"type": "rate_limit_event", "rate_limit_info": {"status": "allowed", "rateLimitType": "seven_day", "utilization": 0.21, "resetsAt": int(time.time()) + 300_000}})
    t.think("Stripe retries a delivery until it gets a 2xx, and a retry can land while the first attempt is still inside apply_payment. "
            "Checking a flag before the write races. The event id has to be claimed in the same transaction as the ledger write, "
            "so the second delivery blocks on the row lock and then sees the committed row.")
    pause(0.6)
    t.say("I'll trace how a webhook reaches the ledger, then claim each Stripe event id in the same transaction as the ledger write.")
    t.read(p("src/webhooks/stripe.rs"), sample("stripe_before.rs").splitlines())
    t.read(p("src/ledger/apply.rs"), sample("apply_before.rs").splitlines())
    tid = t.tool("Grep", {"pattern": "apply_payment|apply_refund", "path": p("src"), "output_mode": "files_with_matches"})
    pause(0.3)
    t.result(tid, "Found 3 files\nsrc/webhooks/stripe.rs\nsrc/ledger/apply.rs\nsrc/admin/replay.rs")
    t.read(p("migrations/0041_payouts.sql"), sample("payouts.sql").splitlines())
    todos = [
        ("Trace webhook → ledger path", "completed"),
        ("Add processed_events table and migration", "completed"),
        ("Apply each event once, in one transaction", "in_progress"),
        ("Test duplicate and concurrent deliveries", "pending"),
        ("Check p99 latency stays under 40 ms", "pending"),
    ]
    tid = t.tool("TodoWrite", {"todos": [{"content": c, "status": s, "activeForm": c} for c, s in todos]})
    t.result(tid, "Todos have been modified successfully.")
    pause(0.4)
    t.write(p("migrations/0042_processed_events.sql"), sample("processed_events.sql"))
    t.edit(p("src/webhooks/stripe.rs"), sample("stripe_before.rs"), sample("stripe_after.rs"))
    t.edit(p("src/ledger/apply.rs"), sample("apply_before.rs"), sample("apply_after.rs"))
    t.say("The handler now claims the event id with `INSERT … ON CONFLICT DO NOTHING` inside the ledger transaction. A retry that "
          "races the first delivery waits on the row lock, then sees the committed row and returns 200 without touching the ledger. "
          "The ledger functions take the transaction instead of the pool, so both writes commit or roll back together.")
    tid = t.tool("Bash", {"command": "cargo watch -q -x 'check --tests'", "description": "Watch the crate for type errors", "run_in_background": True})
    t.result(tid, "Command running in background with ID: bk7f2a9")
    emit({"type": "system", "subtype": "background_tasks_changed", "session_id": SESSION,
          "tasks": [{"task_id": "bk7f2a9", "task_type": "local_bash", "description": "cargo watch -q -x 'check --tests'"}]})
    task = t.tool("Task", {"description": "Review retry races", "subagent_type": "Explore",
                           "prompt": "Find every path that writes to the ledger outside apply_once and check it can't double-apply a Stripe event."})
    pause(0.5)
    t.assistant([{"type": "text", "text": "Looking for ledger writes outside the webhook handler."}], parent=task)
    sub = t.tool("Grep", {"pattern": "ledger::apply_", "path": p("src")}, parent=task)
    t.result(sub, "src/webhooks/stripe.rs:61\nsrc/webhooks/stripe.rs:62\nsrc/admin/replay.rs:28", parent=task)
    sub = t.tool("Read", {"file_path": p("src/admin/replay.rs")}, parent=task)
    t.result(sub, "    28\t    ledger::apply_payment(&state.db, &payment).await?;", parent=task)
    pause(0.4)
    t.edit(p("tests/webhooks.rs"), sample("tests_before.rs"), sample("tests_after.rs"))
    t.tool("Bash", {"command": "cargo test -p northwind-api webhooks:: -- --test-threads=8", "description": "Run the webhook tests"})
    while True:
        pause(3600)


def charts_turn(cwd):
    """Server components in orbit-web: a long answer, written slowly."""
    t = Turn()
    p = lambda rel: os.path.join(cwd, rel)
    t.think("The charts fetch on the client with useEffect. Moving the data load into a server component removes the waterfall; "
            "only the tooltip and range picker need to stay client-side.")
    pause(0.5)
    t.read(p("app/dashboard/page.tsx"), ["export default function Dashboard() {", "  return <Charts />", "}"])
    t.read(p("components/charts/revenue-chart.tsx"), ["'use client'", "", "export function RevenueChart() {", "  const [data, setData] = useState([])"])
    tid = t.tool("Glob", {"pattern": "components/charts/*.tsx"})
    t.result(tid, "components/charts/revenue-chart.tsx\ncomponents/charts/signups-chart.tsx\ncomponents/charts/churn-chart.tsx\ncomponents/charts/range-picker.tsx")
    t.say("Three of the four charts only need data at render time, so they can become async server components that query "
          "directly; the range picker and the tooltips stay client components and receive plain props. That removes two "
          "round trips on first load and drops recharts' data hooks from the client bundle. I'll start with the revenue chart, "
          "since the other two share its query helper, then measure the bundle before and after with next build "
          "and compare the dashboard's largest contentful paint on a throttled connection.", words_per_tick=1, tick=0.45)
    while True:
        pause(3600)


def generic_turn(cwd):
    t = Turn()
    t.think("Reading the project before changing anything.")
    t.say("I'll look around the project first.", words_per_tick=1, tick=0.3)
    while True:
        pause(3600)


TURNS = [("idempot", webhooks_turn), ("server component", charts_turn)]


def main():
    if "--version" in sys.argv[1:]:
        print("2.1.316 (Claude Code)")
        return
    threading.Thread(target=read_input, daemon=True).start()
    while True:
        try:
            msg = inbox.get(timeout=0.5)
        except queue.Empty:
            if stopped.is_set():
                return
            continue
        if msg.get("type") == "user":
            break
    emit({"type": "system", "subtype": "init", "session_id": SESSION, "model": MODEL, "cwd": os.getcwd(), "tools": [], "permissionMode": "default"})
    emit({"type": "user", "isReplay": True, "uuid": msg.get("uuid"), "session_id": SESSION, "message": msg.get("message")})
    blocks = msg.get("message", {}).get("content", [])
    prompt = " ".join(b.get("text", "") for b in blocks if isinstance(b, dict)).lower()
    turn = next((f for key, f in TURNS if key in prompt), generic_turn)
    turn(os.getcwd())


if __name__ == "__main__":
    main()
