"""report.regrade: re-grading a committed run from its stored answers (#164)."""

import json
import os
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from aeval import config, report  # noqa: E402

TASK = {"id": "t1", "repo": "hono", "type": "dead_code", "answer_kind": "symbols", "question": "q",
        "rationale": "r", "gold": [{"name": "a", "evidence": [{"file": "f", "line": 1, "contains": "a"}]},
                                   {"name": "b", "evidence": [{"file": "f", "line": 2, "contains": "b"}]}],
        "optional": [{"name": "c", "why": "wrapper"}]}


def row(answer, f1, run_id):
    return {"run_id": run_id, "task_id": "t1", "answer": answer, "f1": f1, "precision": f1, "recall": f1,
            "success": f1 >= 0.8, "tp": 0, "fp": 0, "neutral": 0, "n_gold": 2, "n_answer": len(answer),
            "missed": [], "false_positives": []}


class Regrade(unittest.TestCase):
    def test_moves_gold_to_optional_and_records_the_note(self):
        with tempfile.TemporaryDirectory() as d:
            d = Path(d)
            run = d / "results" / "run1"
            run.mkdir(parents=True)
            tasks = d / "tasks.json"
            # the corrected gold: b is optional now, c no longer acceptable
            fixed = dict(TASK, gold=TASK["gold"][:1], optional=[{"name": "b", "why": "moved"}])
            tasks.write_text(json.dumps({"tasks": [fixed]}))
            rows = [row(["a"], 0.6667, "t1__x__r1"), row(["a", "b", "c"], 1.0, "t1__x__r2")]
            (run / "results.jsonl").write_text("".join(json.dumps(r, sort_keys=True) + "\n" for r in rows))
            (run / "tasks_snapshot.json").write_text(json.dumps([{"id": "t1", "n_gold": 2}]))
            (run / "meta.json").write_text(json.dumps({"success_threshold": 0.8, "notes": ["old"]}))
            with mock.patch.object(config, "RESULTS_DIR", d / "results"), \
                    mock.patch.dict(os.environ, {"AEVAL_TASKS": str(tasks)}):
                changed = report.regrade("run1", "gold amended")
            self.assertEqual(changed, 2)
            got = {r["run_id"]: r for r in map(json.loads, (run / "results.jsonl").read_text().splitlines())}
            self.assertEqual(got["t1__x__r1"]["f1"], 1.0)       # a alone is now complete
            self.assertEqual(got["t1__x__r2"]["fp"], 1)         # c is a false positive now
            self.assertEqual(got["t1__x__r2"]["neutral"], 1)    # b is neutral
            meta = json.loads((run / "meta.json").read_text())
            self.assertEqual(meta["notes"], ["old", "gold amended"])
            self.assertEqual(meta["tasks_sha256"], config.sha256_file(tasks))
            self.assertEqual(json.loads((run / "tasks_snapshot.json").read_text())[0]["n_gold"], 1)


if __name__ == "__main__":
    unittest.main()
