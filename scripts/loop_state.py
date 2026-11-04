#!/usr/bin/env python3
"""Single-coordinator local task ledger; this does not run models or verify evidence truth."""
import argparse
from contextlib import contextmanager
from datetime import datetime, timezone
import fcntl
import json
import os
from pathlib import Path
import tempfile
import uuid

ROOT = Path(__file__).resolve().parents[1]


def now():
    return datetime.now(timezone.utc).isoformat()


@contextmanager
def locked_state(directory, definitions):
    directory.mkdir(parents=True, exist_ok=True, mode=0o700)
    with (directory / "loop.lock").open("a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        path = directory / "loop-state.json"
        state = json.loads(path.read_text()) if path.exists() else {
            "schema_version": 1, "paused": False, "tasks": {}, "events": []}
        if state.get("schema_version") != 1:
            raise ValueError("unsupported state schema")
        for task in definitions:
            state["tasks"].setdefault(task["id"], {"status": "ready", "attempts": []})
        yield state
        with tempfile.NamedTemporaryFile(mode="w", dir=directory, delete=False) as output:
            temporary = Path(output.name)
            json.dump(state, output, ensure_ascii=False, indent=2)
            output.write("\n")
            output.flush()
            os.fsync(output.fileno())
        os.replace(temporary, path)


def apply(state, definitions, action, task_id=None, attempt_id=None, evidence=None, reason=None):
    """Validate lifecycle and dependencies before recording a transition."""
    tasks = {item["id"]: item for item in definitions}
    if action in ("pause", "resume"):
        if not reason:
            raise ValueError("pause/resume requires the user's instruction reference")
        state["paused"] = action == "pause"
        state["events"].append({"at": now(), "action": action, "reason": reason})
        return
    if action == "status":
        return
    if state["paused"]:
        raise ValueError("loop paused; only an explicit resume may continue")
    if task_id not in tasks:
        raise ValueError("unknown task")
    task = state["tasks"][task_id]
    if action == "start":
        if task["status"] != "ready":
            raise ValueError("task must be ready; inspect interrupted work before reopening")
        if any(item["status"] == "running" for item in state["tasks"].values()):
            raise ValueError("another task is running")
        if any(state["tasks"][dependency]["status"] != "done" for dependency in tasks[task_id]["depends_on"]):
            raise ValueError("dependencies are not done")
        attempt = {"id": str(uuid.uuid4()), "started_at": now(), "status": "running"}
        task["attempts"].append(attempt)
        task["status"] = "running"
        return attempt["id"]
    if action in ("finish", "retry", "block"):
        if task["status"] != "running" or not task["attempts"] or task["attempts"][-1]["id"] != attempt_id:
            raise ValueError("stale or non-running attempt")
        if action == "finish" and (not evidence or not Path(evidence).is_file()):
            raise ValueError("finish requires an existing evidence file")
        if action in ("retry", "block") and not reason:
            raise ValueError("failure/block requires diagnosis and an unblock condition")
        status = {"finish": "done", "retry": "ready", "block": "blocked"}[action]
        task["status"] = status
        task["attempts"][-1].update(status=status, ended_at=now(), evidence=evidence, reason=reason)
    elif action == "reopen":
        if task["status"] not in ("blocked", "running", "done") or not reason:
            raise ValueError("reopen requires a blocked/interrupted/done task and new evidence or diagnosis")
        if task["status"] == "done":
            descendants = {task_id}
            changed = True
            while changed:
                changed = False
                for candidate in definitions:
                    if candidate["id"] not in descendants and set(candidate["depends_on"]) & descendants:
                        descendants.add(candidate["id"])
                        changed = True
            if any(state["tasks"][item]["status"] in ("done", "running") for item in descendants - {task_id}):
                raise ValueError("reopen downstream accepted tasks first")
        if task["status"] == "running":
            task["attempts"][-1].update(status="interrupted", ended_at=now(), reason=reason)
        task["status"] = "ready"
    else:
        raise ValueError("unknown action")
    state["events"].append({"at": now(), "task": task_id, "attempt": attempt_id,
                            "action": action, "reason": reason, "evidence": evidence})


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=["status", "start", "finish", "retry", "block", "reopen", "pause", "resume"])
    parser.add_argument("task", nargs="?")
    parser.add_argument("--attempt")
    parser.add_argument("--evidence")
    parser.add_argument("--reason")
    parser.add_argument("--state-dir", type=Path, default=ROOT / ".voiceport")
    args = parser.parse_args()
    definitions = json.loads((ROOT / "automation/tasks.json").read_text())["tasks"]
    try:
        with locked_state(args.state_dir, definitions) as state:
            attempt = apply(state, definitions, args.action, args.task, args.attempt, args.evidence, args.reason)
            if attempt:
                print(attempt)
            else:
                print(json.dumps({"paused": state["paused"], "tasks": {
                    key: {"status": value["status"], "attempt": value["attempts"][-1]["id"] if value["attempts"] else None}
                    for key, value in state["tasks"].items()}}, ensure_ascii=False, indent=2))
    except (ValueError, OSError, KeyError) as error:
        parser.exit(1, f"loop-state: {error}\n")


if __name__ == "__main__":
    main()
