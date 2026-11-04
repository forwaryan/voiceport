import importlib.util
from pathlib import Path
import tempfile
import unittest

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


if __name__ == "__main__":
    unittest.main()
