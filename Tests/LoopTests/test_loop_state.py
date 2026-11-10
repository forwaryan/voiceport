import copy
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

SCRIPT = Path(__file__).resolve().parents[2] / "scripts/loop_state.py"
specification = importlib.util.spec_from_file_location("voiceport_loop_state", SCRIPT)
loop = importlib.util.module_from_spec(specification)
specification.loader.exec_module(loop)


class LoopStateTests(unittest.TestCase):
    definitions = [{"id": "A", "depends_on": []}, {"id": "B", "depends_on": ["A"]}]

    def initial(self):
        return {"paused": False, "tasks": {key: {"status": "ready", "attempts": []} for key in ("A", "B")}, "events": []}

    def test_dependencies_and_stale_results_cannot_advance_tasks(self):
        state = self.initial()
        with self.assertRaisesRegex(ValueError, "dependencies"):
            loop.apply(state, self.definitions, "start", "B")
        first = loop.apply(state, self.definitions, "start", "A")
        loop.apply(state, self.definitions, "retry", "A", first, reason="assertion failed; revise parser")
        second = loop.apply(state, self.definitions, "start", "A")
        with tempfile.NamedTemporaryFile() as evidence:
            evidence.write(b"parser regression: old code FAIL, revised code PASS\n")
            evidence.flush()
            with self.assertRaisesRegex(ValueError, "stale"):
                loop.apply(state, self.definitions, "finish", "A", first, evidence.name)
            loop.apply(state, self.definitions, "finish", "A", second, evidence.name)
        self.assertIsNotNone(loop.apply(state, self.definitions, "start", "B"))

    def test_pause_survives_save_and_restart(self):
        with tempfile.TemporaryDirectory() as directory:
            folder = Path(directory)
            with loop.locked_state(folder, self.definitions) as state:
                loop.apply(state, self.definitions, "pause", reason="user asked to pause")
            with loop.locked_state(folder, self.definitions) as state:
                with self.assertRaisesRegex(ValueError, "paused"):
                    loop.apply(state, self.definitions, "start", "A")
                loop.apply(state, self.definitions, "resume", reason="user explicitly resumed")
                loop.apply(state, self.definitions, "start", "A")

    def test_interrupted_attempt_requires_inspection_and_new_identity(self):
        state = self.initial()
        first = loop.apply(state, self.definitions, "start", "A")
        with self.assertRaises(ValueError):
            loop.apply(state, self.definitions, "start", "A")
        loop.apply(state, self.definitions, "reopen", "A", reason="inspected workspace; no validation completed")
        second = loop.apply(state, self.definitions, "start", "A")
        self.assertNotEqual(first, second)
        self.assertEqual(state["tasks"]["A"]["attempts"][0]["status"], "interrupted")

    def test_no_acceptance_without_evidence(self):
        state = self.initial()
        attempt = loop.apply(state, self.definitions, "start", "A")
        with self.assertRaisesRegex(ValueError, "evidence"):
            loop.apply(state, self.definitions, "finish", "A", attempt)

    def cli(self, root, action, directory):
        # Exercise the real CLI using fixture task definitions, never live state.
        bootstrap = (
            "import importlib.util, pathlib, sys; "
            "spec = importlib.util.spec_from_file_location('loop_state', sys.argv[1]); "
            "module = importlib.util.module_from_spec(spec); spec.loader.exec_module(module); "
            "module.ROOT = pathlib.Path(sys.argv[2]); "
            "sys.argv = [sys.argv[1], *sys.argv[3:]]; sys.exit(module.main())"
        )
        return subprocess.run([sys.executable, "-I", "-B", "-c", bootstrap, str(SCRIPT), str(root),
                               action, "--state-dir", str(directory)],
                              capture_output=True, text=True, timeout=10)

    def fixture_root(self, directory):
        root = Path(directory)
        (root / "automation").mkdir()
        (root / "automation/tasks.json").write_text(json.dumps({"tasks": self.definitions}))
        return root

    def test_finish_rejects_empty_symlink_and_nonregular_evidence(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            empty = root / "empty"
            empty.touch()
            target = root / "target"
            target.write_text("verification passed\n")
            link = root / "link"
            link.symlink_to(target)
            fifo = root / "fifo"
            os.mkfifo(fifo)
            for evidence in (empty, link, root, fifo):
                with self.subTest(evidence=evidence):
                    state = self.initial()
                    attempt = loop.apply(state, self.definitions, "start", "A")
                    before = copy.deepcopy(state)
                    with self.assertRaises((ValueError, OSError)):
                        loop.apply(state, self.definitions, "finish", "A", attempt, str(evidence))
                    self.assertEqual(state, before)

    def test_finish_records_snapshot_and_preserves_earlier_attempts(self):
        with tempfile.TemporaryDirectory() as directory:
            evidence = Path(directory) / "checks.txt"
            evidence.write_bytes(b"unit parser check PASS\n")
            state = self.initial()
            first = loop.apply(state, self.definitions, "start", "A")
            loop.apply(state, self.definitions, "retry", "A", first, reason="parser red; fix before retry")
            earlier = copy.deepcopy(state["tasks"]["A"]["attempts"][0])
            second = loop.apply(state, self.definitions, "start", "A")
            loop.apply(state, self.definitions, "finish", "A", second, str(evidence))
            self.assertEqual(state["tasks"]["A"]["attempts"][0], earlier)
            for record in (state["tasks"]["A"]["attempts"][-1], state["events"][-1]):
                self.assertEqual(Path(record["evidence"]), evidence.resolve())
                self.assertEqual(record["evidence_sha256"], hashlib.sha256(evidence.read_bytes()).hexdigest())
                self.assertEqual(record["evidence_bytes"], len(evidence.read_bytes()))

    def test_finish_rejects_oversized_or_changing_evidence(self):
        with tempfile.TemporaryDirectory() as directory:
            evidence = Path(directory) / "checks.txt"
            evidence.write_bytes(b"check PASS\n")
            state = self.initial()
            attempt = loop.apply(state, self.definitions, "start", "A")
            before = copy.deepcopy(state)
            with patch.object(loop, "MAX_EVIDENCE_BYTES", 4):
                with self.assertRaises(ValueError):
                    loop.apply(state, self.definitions, "finish", "A", attempt, str(evidence))
            original_read = os.read
            changed = False

            def read_and_change(descriptor, size):
                nonlocal changed
                chunk = original_read(descriptor, size)
                if not changed:
                    changed = True
                    evidence.write_bytes(b"check FAIL\nextra output\n")
                return chunk

            with patch.object(loop.os, "read", side_effect=read_and_change):
                with self.assertRaisesRegex(ValueError, "changed"):
                    loop.apply(state, self.definitions, "finish", "A", attempt, str(evidence))
            self.assertEqual(state, before)

    def test_audit_cli_detects_changed_missing_empty_and_symlink_evidence(self):
        with tempfile.TemporaryDirectory() as directory:
            root = self.fixture_root(directory)
            folder = root / "state"
            evidence = root / "checks.txt"
            original = b"unit checks PASS\n"
            evidence.write_bytes(original)
            with loop.locked_state(folder, self.definitions) as state:
                for task_id in ("A", "B"):
                    attempt = loop.apply(state, self.definitions, "start", task_id)
                    loop.apply(state, self.definitions, "finish", task_id, attempt, str(evidence))
            ledger = folder / "loop-state.json"
            saved = ledger.read_bytes()
            result = self.cli(root, "audit", folder)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual([item["status"] for item in json.loads(result.stdout)["attempts"]], ["intact", "intact"])
            for replacement, expected in ((b"unit checks FAIL\n", "changed"), (b"", "changed"),
                                           (None, "missing"), ("symlink", "changed")):
                if evidence.exists() or evidence.is_symlink():
                    evidence.unlink()
                if replacement == "symlink":
                    target = root / "substitute"
                    target.write_bytes(original)
                    evidence.symlink_to(target)
                elif replacement is not None:
                    evidence.write_bytes(replacement)
                result = self.cli(root, "audit", folder)
                self.assertNotEqual(result.returncode, 0, result.stderr)
                self.assertEqual([item["status"] for item in json.loads(result.stdout)["attempts"]], [expected, expected])
                self.assertEqual(ledger.read_bytes(), saved)

    def test_audit_rejects_substituted_parent_symlink(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            parent = root / "evidence"
            parent.mkdir()
            evidence = parent / "checks.txt"
            evidence.write_text("checks PASS\n")
            state = self.initial()
            attempt = loop.apply(state, self.definitions, "start", "A")
            loop.apply(state, self.definitions, "finish", "A", attempt, str(evidence))
            saved = copy.deepcopy(state)
            relocated = root / "relocated"
            parent.rename(relocated)
            parent.symlink_to(relocated, target_is_directory=True)
            self.assertEqual(loop.audit(state)["attempts"][0]["status"], "changed")
            self.assertEqual(state, saved)

    def test_audit_retains_historical_legacy_as_unknown(self):
        with tempfile.TemporaryDirectory() as directory:
            root = self.fixture_root(directory)
            folder = root / "state"
            evidence = root / "checks.txt"
            evidence.write_text("new attempt checks PASS\n")
            with loop.locked_state(folder, self.definitions) as state:
                legacy = {"id": "legacy", "status": "done", "evidence": str(evidence)}
                state["tasks"]["A"].update(status="done", attempts=[legacy])
                loop.apply(state, self.definitions, "reopen", "A", reason="new regression requires verification")
                for task_id in ("A", "B"):
                    attempt = loop.apply(state, self.definitions, "start", task_id)
                    loop.apply(state, self.definitions, "finish", task_id, attempt, str(evidence))
            ledger = folder / "loop-state.json"
            saved = ledger.read_bytes()
            result = self.cli(root, "audit", folder)
            self.assertNotEqual(result.returncode, 0, result.stderr)
            self.assertEqual([item["status"] for item in json.loads(result.stdout)["attempts"]],
                             ["unsealed_legacy", "intact", "intact"])
            self.assertEqual(ledger.read_bytes(), saved)
            self.assertEqual(loop.audit(state)["attempts"][0]["status"], "unsealed_legacy")
            self.assertEqual(state["tasks"]["A"]["attempts"][0], legacy)

    def test_audit_cannot_pass_done_task_without_a_completed_attempt(self):
        state = self.initial()
        state['tasks']['A'].update(status='done', attempts=[])
        state['tasks']['B'].update(status='done', attempts=[{'id': 'interrupted', 'status': 'interrupted'}])
        report = loop.audit(state)
        self.assertFalse(report['ok'])
        self.assertEqual(report['missing_done_attempts'], ['A', 'B'])

    def test_status_and_audit_cli_never_initialize_or_rewrite_state(self):
        with tempfile.TemporaryDirectory() as directory:
            root = self.fixture_root(directory)
            folder = root / "absent"
            for action in ("status", "audit"):
                result = self.cli(root, action, folder)
                self.assertEqual(result.returncode, 0 if action == "status" else 1, result.stderr)
                self.assertFalse(folder.exists())
            with loop.read_state(folder, self.definitions) as state:
                self.assertEqual(state["tasks"]["A"]["status"], "ready")
            self.assertFalse(folder.exists())
            folder.mkdir()
            for action in ("status", "audit"):
                result = self.cli(root, action, folder)
                self.assertEqual(result.returncode, 0 if action == "status" else 1, result.stderr)
                self.assertEqual(list(folder.iterdir()), [])
            ledger = folder / "loop-state.json"
            # Intentionally missing B: status may project it, but must not save it.
            ledger.write_text('{"schema_version":1,"paused":false,"tasks":{"A":{"status":"ready","attempts":[]}},"events":[]}')
            for with_lock in (False, True):
                if with_lock:
                    (folder / "loop.lock").write_text("existing lock\n")
                before = {path.name: (path.read_bytes(), path.stat().st_mtime_ns)
                          for path in folder.iterdir()}
                result = self.cli(root, "status", folder)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(json.loads(result.stdout)["tasks"]["B"], {"status": "ready", "attempt": None})
                result = self.cli(root, "audit", folder)
                self.assertNotEqual(result.returncode, 0, result.stderr)
                after = {path.name: (path.read_bytes(), path.stat().st_mtime_ns)
                         for path in folder.iterdir()}
                self.assertEqual(before, after)


if __name__ == "__main__":
    unittest.main()
