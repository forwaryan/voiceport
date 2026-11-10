#!/usr/bin/env python3
"""Single-coordinator local task ledger; this does not run models or verify evidence truth."""
import argparse
from contextlib import contextmanager
from datetime import datetime, timezone
import fcntl
import hashlib
import json
import os
from pathlib import Path
import stat
import tempfile
import uuid

ROOT = Path(__file__).resolve().parents[1]
MAX_EVIDENCE_BYTES = 16 * 1024 * 1024


def load_state(directory, definitions):
    path = directory / "loop-state.json"
    try:
        state = json.loads(path.read_text())
    except FileNotFoundError:
        state = {"schema_version": 1, "paused": False, "tasks": {}, "events": []}
    if state.get("schema_version") != 1:
        raise ValueError("unsupported state schema")
    for task in definitions:
        state["tasks"].setdefault(task["id"], {"status": "ready", "attempts": []})
    return state


@contextmanager
def read_state(directory, definitions):
    """Read under an existing shared lock, without creating or saving anything."""
    try:
        lock = (directory / "loop.lock").open("rb")
    except FileNotFoundError:
        yield load_state(directory, definitions)
    else:
        with lock:
            fcntl.flock(lock, fcntl.LOCK_SH)
            yield load_state(directory, definitions)


@contextmanager
def open_evidence(path):
    """Walk canonical directories without following substituted symlinks."""
    parent = os.open(path.anchor, os.O_RDONLY | os.O_DIRECTORY)
    try:
        for component in path.parent.parts[1:]:
            child = os.open(component, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=parent)
            os.close(parent)
            parent = child
        descriptor = os.open(path.name, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=parent)
        try:
            yield descriptor
        finally:
            os.close(descriptor)
    finally:
        os.close(parent)


def evidence_snapshot(evidence, *, canonicalize_parents=True):
    """Hash stable file bytes; this does not establish review or CI truth."""
    if not evidence:
        raise ValueError("finish requires an evidence file")
    # Canonicalize parents (e.g. macOS /var), but never resolve the final link.
    supplied = Path(evidence).absolute()
    path = supplied.parent.resolve(strict=True) / supplied.name if canonicalize_parents else supplied
    with open_evidence(path) as descriptor:
        before = os.fstat(descriptor)
        if not stat.S_ISREG(before.st_mode) or not 0 < before.st_size <= MAX_EVIDENCE_BYTES:
            raise ValueError("evidence must be a nonempty regular file of at most 16 MiB")
        digest = hashlib.sha256()
        size = 0
        while True:
            chunk = os.read(descriptor, min(65536, MAX_EVIDENCE_BYTES + 1 - size))
            if not chunk:
                break
            size += len(chunk)
            if size > MAX_EVIDENCE_BYTES:
                raise ValueError("evidence exceeds 16 MiB")
            digest.update(chunk)
        def identity(info):
            return (info.st_dev, info.st_ino, info.st_mode, info.st_size,
                    info.st_mtime_ns, info.st_ctime_ns)
        if (size != before.st_size or identity(before) != identity(os.fstat(descriptor))
                or identity(before) != identity(path.lstat())):
            raise ValueError("evidence changed during read")
        return {"evidence": str(path), "evidence_sha256": digest.hexdigest(), "evidence_bytes": size}


def audit(state):
    """Check every completed attempt without sealing or changing historical records."""
    results = []
    for task_id, task in state["tasks"].items():
        for attempt in task["attempts"]:
            if attempt["status"] != "done":
                continue
            status = "unsealed_legacy"
            if "evidence_sha256" in attempt and "evidence_bytes" in attempt:
                try:
                    snapshot = evidence_snapshot(attempt.get("evidence"), canonicalize_parents=False)
                except FileNotFoundError:
                    status = "missing"
                except (ValueError, OSError):
                    status = "changed"
                else:
                    status = "intact" if all(snapshot[key] == attempt[key] for key in
                                             ("evidence", "evidence_sha256", "evidence_bytes")) else "changed"
            results.append({"task": task_id, "attempt": attempt["id"], "status": status})
    incomplete = [key for key, task in state["tasks"].items() if task["status"] != "done"]
    missing_attempts = [key for key, task in state["tasks"].items() if task["status"] == "done"
                        and (not task["attempts"] or task["attempts"][-1]["status"] != "done")]
    return {"ok": not incomplete and not missing_attempts and all(item["status"] == "intact" for item in results),
            "attempts": results, "incomplete_tasks": incomplete, "missing_done_attempts": missing_attempts}


def now():
    return datetime.now(timezone.utc).isoformat()


@contextmanager
def locked_state(directory, definitions):
    directory.mkdir(parents=True, exist_ok=True, mode=0o700)
    with (directory / "loop.lock").open("a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        path = directory / "loop-state.json"
        state = load_state(directory, definitions)
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
        snapshot = evidence_snapshot(evidence) if action == "finish" else {}
        if action in ("retry", "block") and not reason:
            raise ValueError("failure/block requires diagnosis and an unblock condition")
        status = {"finish": "done", "retry": "ready", "block": "blocked"}[action]
        task["status"] = status
        task["attempts"][-1].update(status=status, ended_at=now(), evidence=evidence, reason=reason)
        task["attempts"][-1].update(snapshot)
        evidence = snapshot.get("evidence", evidence)
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
    event = {"at": now(), "task": task_id, "attempt": attempt_id,
             "action": action, "reason": reason, "evidence": evidence}
    if action == "finish":
        event.update(snapshot)
    state["events"].append(event)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=["status", "audit", "start", "finish", "retry", "block", "reopen", "pause", "resume"])
    parser.add_argument("task", nargs="?")
    parser.add_argument("--attempt")
    parser.add_argument("--evidence")
    parser.add_argument("--reason")
    parser.add_argument("--state-dir", type=Path, default=ROOT / ".voiceport")
    args = parser.parse_args()
    definitions = json.loads((ROOT / "automation/tasks.json").read_text())["tasks"]
    try:
        context = read_state if args.action in ("status", "audit") else locked_state
        with context(args.state_dir, definitions) as state:
            if args.action == "audit":
                report = audit(state)
                print(json.dumps(report, ensure_ascii=False, indent=2))
                return 0 if report["ok"] else 1
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
    raise SystemExit(main())
