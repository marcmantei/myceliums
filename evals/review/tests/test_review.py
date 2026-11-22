"""Tests for the label review (#164): export determinism, queue sampling,
decision persistence and resume, apply. Standard library only; no checkouts
needed (graph-gold fixtures are in the repository).

    python3 -m unittest discover -s evals/review/tests -v
"""

from __future__ import annotations

import json
import shutil
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))

import apply  # noqa: E402
import common  # noqa: E402
import export  # noqa: E402

REPO = common.REPO_ROOT


def fake_items(dataset: str, n: int) -> list[dict]:
    return [{"dataset": dataset, "id": f"{dataset}:item-{i:03d}", "kind": "k", "claim": f"claim {i}",
             "label": "l", "context": {}, "evidence": []} for i in range(n)]


def confirmed(items: list[dict]) -> dict[str, dict]:
    return {it["id"]: {"id": it["id"], "verdict": "confirmed", "reason": "", "evidence": []} for it in items}


def doubtful(items: list[dict]) -> dict[str, dict]:
    return {it["id"]: {"id": it["id"], "verdict": "doubtful", "reason": "", "evidence": []} for it in items}


class Export(unittest.TestCase):
    def test_deterministic_and_unique(self):
        co = export.Checkouts(fixed={})  # no checkouts: real-repo snippets are marked missing
        a = export.render(export.build_items(co))
        b = export.render(export.build_items(export.Checkouts(fixed={})))
        self.assertEqual(a, b)
        items = [json.loads(line) for line in a.splitlines()]
        self.assertEqual(len({it["id"] for it in items}), len(items))
        self.assertTrue(all(it["id"].startswith(it["dataset"] + ":") for it in items))

    def test_one_item_per_label_plus_completeness(self):
        items = export.build_items(snippets=False)
        q = json.loads((export.REAL_REPOS / "queries.json").read_text())
        n_labels = sum(len(x["relevant"]) for x in q["queries"])
        mine = [it for it in items if it["dataset"] == "retrieval-queries"]
        self.assertEqual(len(mine), n_labels + len(q["queries"]))
        self.assertEqual(sum(it["kind"] == "retrieval-complete" for it in mine), len(q["queries"]))
        calls = sum(len(json.loads((export.GRAPH_GOLD / f"{lang}.json").read_text())["calls"])
                    for lang in export.GRAPH_LANGUAGES)
        self.assertEqual(sum(it["kind"] == "graph-call" for it in items), calls)
        self.assertEqual({it["dataset"] for it in items}, set(common.DATASETS))

    def test_claims_do_not_depend_on_snippets(self):
        plain = {it["id"]: common.claim_digest(it) for it in export.build_items(snippets=False)}
        full = {it["id"]: common.claim_digest(it) for it in export.build_items(export.Checkouts(fixed={}))}
        self.assertEqual(plain, full)

    def test_graph_gold_snippets_resolve_from_fixtures(self):
        items = [it for it in export.build_items(export.Checkouts(fixed={})) if it["kind"] == "graph-symbol"]
        for it in items:
            ev = it["evidence"][0]
            self.assertIn("lines", ev, it["id"])
            name = it["id"].split("::")[-1].split("(")[0].split(".")[-1]
            self.assertIn(name, ev["lines"][ev["focus"][0] - ev["start"]], it["id"])

    def test_source_pointer_and_line(self):
        items = export.build_items(snippets=False)
        it = next(i for i in items if i["kind"] == "graph-call")
        src = (REPO / it["source"]["file"]).read_text().splitlines()
        frm = it["id"].split(":call:", 1)[1].split("->")[0]
        self.assertIn(frm, src[it["source"]["line"] - 1])

    def test_locate(self):
        py = ["class A:", "    def f(self):", "        return 1", "", "def f():", "    pass"]
        self.assertEqual(export.locate(py, "x.py", "A.f")[0], 2)
        self.assertEqual(export.locate(py, "x.py", "f")[0], 5)
        go = ["package x", "func f() {}", "func (c *Ctx) Next() {", "}", "type node struct {", "}"]
        self.assertEqual(export.locate(go, "x.go", "Ctx.Next")[0], 3)
        self.assertEqual(export.locate(go, "x.go", "node")[0], 5)
        ts = ["export const getCookie = (c) => {", "}", "class Hono {", "  fetch = (req) => {", "  }", "}"]
        self.assertEqual(export.locate(ts, "x.ts", "getCookie")[0], 1)
        self.assertEqual(export.locate(ts, "x.ts", "Hono.fetch")[0], 4)
        self.assertEqual(export.block_end(go, 3, False), (4, False))
        self.assertEqual(export.block_end(py, 2, True), (3, False))


class Sampling(unittest.TestCase):
    def setUp(self):
        self.items = fake_items("graph-held-out", 100) + fake_items("agent-tasks", 20)
        # graph-held-out fully pre-screened (80 confirmed, 20 doubtful); agent-tasks unscreened
        self.pre = confirmed(self.items[:80])
        self.pre.update(doubtful(self.items[80:100]))

    def test_seeded_and_recorded(self):
        with tempfile.TemporaryDirectory() as d:
            p1, p2 = Path(d, "a.json"), Path(d, "b.json")
            s1 = common.Sample.load_or_create(p1, self.items, self.pre, seed=7)
            s2 = common.Sample.load_or_create(p2, self.items, self.pre, seed=7)
            self.assertEqual(s1.ids("graph-held-out"), s2.ids("graph-held-out"))
            self.assertEqual(len(s1.ids("graph-held-out")), 12)  # ceil(0.15 * 80)
            other = common.Sample.load_or_create(Path(d, "c.json"), self.items, self.pre, seed=8)
            self.assertNotEqual(s1.ids("graph-held-out"), other.ids("graph-held-out"))
            stored = json.loads(p1.read_text())
            self.assertEqual(stored["seed"], 7)
            self.assertEqual(stored["rate"], 0.15)
            self.assertEqual(stored["datasets"]["graph-held-out"]["ids"], s1.ids("graph-held-out"))
            again = common.Sample.load_or_create(p1, self.items, self.pre, seed=99)  # file wins over flag
            self.assertEqual(again.seed, 7)

    def test_queue_is_flagged_plus_sample(self):
        with tempfile.TemporaryDirectory() as d:
            s = common.Sample.load_or_create(Path(d, "s.json"), self.items, self.pre)
            r = common.Review(self.items, self.pre, common.DecisionStore(Path(d, "d.jsonl")), s)
            flagged = [it["id"] for it in self.items[80:]]
            self.assertTrue(all(common.verdict_of(i, self.pre) == "doubtful" for i in flagged))
            self.assertEqual(set(r.queue), set(flagged) | set(s.ids("graph-held-out")))
            self.assertEqual(len(r.queue), 40 + 12)

    def test_widening_extends_the_same_permutation(self):
        with tempfile.TemporaryDirectory() as d:
            s = common.Sample.load_or_create(Path(d, "s.json"), self.items, self.pre)
            store = common.DecisionStore(Path(d, "d.jsonl"))
            r = common.Review(self.items, self.pre, store, s)
            first = list(s.ids("graph-held-out"))
            for rid in first:
                store.record(r.by_id[rid], "correct")
            # 0 errors in 12: Wilson upper 0.24 >= 0.10, so the rule fails and the sample widens
            self.assertEqual(r.dataset_summary("graph-held-out")["status"], "widen")
            self.assertEqual(r.widen_if_needed("graph-held-out"), 12)
            perm = common.permutation([it["id"] for it in self.items[:80]], s.seed, "graph-held-out")
            self.assertEqual(s.ids("graph-held-out"), perm[:24])
            self.assertEqual(s.ids("graph-held-out")[:12], first)
            self.assertEqual(r.dataset_summary("graph-held-out")["status"], "sample-open")

    def test_acceptance(self):
        items = fake_items("graph-gold", 300)
        pre = confirmed(items)
        with tempfile.TemporaryDirectory() as d:
            s = common.Sample.load_or_create(Path(d, "s.json"), items, pre)
            store = common.DecisionStore(Path(d, "d.jsonl"))
            r = common.Review(items, pre, store, s)
            ids = s.ids("graph-gold")
            self.assertEqual(len(ids), 45)
            for rid in ids:
                store.record(r.by_id[rid], "correct")
            row = r.dataset_summary("graph-gold")
            self.assertEqual(row["status"], "accepted")
            self.assertLess(row["wilson95"][1], 0.10)
            store.record(r.by_id[ids[0]], "unsure")  # unsure counts as an error
            self.assertEqual(r.dataset_summary("graph-gold")["sample_errors"], 1)

    def test_no_sample_until_prescreen_covers_the_dataset(self):
        items = fake_items("graph-gold", 100)
        partial = confirmed(items[:50])
        with tempfile.TemporaryDirectory() as d:
            path = Path(d, "s.json")
            s = common.Sample.load_or_create(path, items, partial)
            self.assertNotIn("graph-gold", json.loads(path.read_text())["datasets"])
            r = common.Review(items, partial, common.DecisionStore(Path(d, "d.jsonl")), s)
            self.assertEqual(r.dataset_summary("graph-gold")["status"], "prescreen-incomplete")
            self.assertEqual(r.dataset_summary("graph-gold")["sample_size"], 0)
            self.assertEqual(len(r.queue), 50)  # only the unscreened (doubtful) items
            full = dict(partial, **confirmed(items[50:]))
            s = common.Sample.load_or_create(path, items, full)
            self.assertEqual(len(s.ids("graph-gold")), 15)
            self.assertEqual(json.loads(path.read_text())["datasets"]["graph-gold"]["confirmed_when_drawn"], 100)

    def test_prescreen_that_changes_after_the_draw(self):
        items = fake_items("graph-gold", 100)
        pre = dict(confirmed(items[:80]), **doubtful(items[80:]))
        with tempfile.TemporaryDirectory() as d:
            path = Path(d, "s.json")
            store = common.DecisionStore(Path(d, "d.jsonl"))
            by_id = {it["id"]: it for it in items}
            decided = lambda rid: store.effective(by_id[rid]) is not None  # noqa: E731
            s = common.Sample.load_or_create(path, items, pre, decided=decided)
            first = s.ids("graph-gold")
            self.assertEqual(len(first), 12)
            # a re-run pre-screen confirms ten more items: no decisions yet, so the sample is redrawn
            grown = dict(pre, **confirmed(items[80:90]))
            s = common.Sample.load_or_create(path, items, grown, decided=decided)
            redrawn = s.ids("graph-gold")
            perm = common.permutation([it["id"] for it in items[:90]], 164, "graph-gold")
            self.assertEqual(redrawn, perm[:14])  # ceil(0.15 * 90), drawn from all 90
            self.assertEqual(json.loads(path.read_text())["datasets"]["graph-gold"]["confirmed_when_drawn"], 90)
            # once a sampled item is decided, a further change is refused
            store.record(by_id[redrawn[0]], "correct")
            more = dict(grown, **confirmed(items[90:]))
            with self.assertRaises(common.SampleConflict):
                common.Sample.load_or_create(path, items, more, decided=decided)
            # and a sample that no longer matches its population accepts nothing
            stale = common.Sample(json.loads(path.read_text()))
            r = common.Review(items, more, store, stale)
            self.assertEqual(r.dataset_summary("graph-gold")["status"], "sample-stale")
            self.assertFalse(r.sampled)

    def test_wilson(self):
        lo, hi = common.wilson(0, 35)
        self.assertAlmostEqual(lo, 0.0, places=12)
        self.assertAlmostEqual(hi, common.Z95 ** 2 / (35 + common.Z95 ** 2), places=12)
        lo, hi = common.wilson(5, 50)
        self.assertAlmostEqual(lo, 0.0435, places=3)
        self.assertAlmostEqual(hi, 0.2136, places=3)
        self.assertEqual(common.wilson(0, 0), (0.0, 1.0))


class Decisions(unittest.TestCase):
    def test_persist_resume_and_torn_line(self):
        items = fake_items("agent-tasks", 5)
        with tempfile.TemporaryDirectory() as d:
            path = Path(d, "decisions.jsonl")
            store = common.DecisionStore(path)
            store.record(items[0], "correct")
            store.record(items[1], "wrong")
            store.record(items[1], comment="optional — thin wrapper")  # comment keeps the decision
            with path.open("ab") as fh:
                fh.write(b'{"id": "agent-tasks:item-002", "decis')  # a crash mid-write
            reloaded = common.DecisionStore(path)
            self.assertEqual(reloaded.effective(items[0])["decision"], "correct")
            self.assertEqual(reloaded.effective(items[1])["decision"], "wrong")
            self.assertEqual(reloaded.effective(items[1])["comment"], "optional — thin wrapper")
            self.assertIsNone(reloaded.get(items[2]["id"]))
            reloaded.record(items[2], "unsure")  # appended after the torn line, on its own line
            again = common.DecisionStore(path)
            self.assertEqual(again.effective(items[2])["decision"], "unsure")
            r = common.Review(items, {}, again, common.Sample({"seed": 1, "rate": 0.15, "datasets": {}}))
            self.assertEqual(r.first_open(), 3)  # resume at the first undecided item

    def test_changed_claim_makes_decision_stale(self):
        items = fake_items("agent-tasks", 1)
        with tempfile.TemporaryDirectory() as d:
            store = common.DecisionStore(Path(d, "d.jsonl"))
            store.record(items[0], "correct")
            changed = dict(items[0], claim="another claim")
            self.assertIsNone(store.effective(changed))
            self.assertEqual(store.stale({changed["id"]: changed}), [changed["id"]])
            self.assertIsNotNone(store.effective(dict(items[0], evidence=[{"file": "x"}])))

    def test_rejects_unknown_decision(self):
        with tempfile.TemporaryDirectory() as d:
            with self.assertRaises(ValueError):
                common.DecisionStore(Path(d, "d.jsonl")).record(fake_items("x", 1)[0], "maybe")


def reset_review_state() -> None:
    """Clear reviewed_by in the copied datasets: the tests start from unreviewed labels."""
    for name in ("queries.json", "test-seeking.json"):
        path = export.REAL_REPOS / name
        data = json.loads(path.read_text())
        for q in data["queries"]:
            q["reviewed_by"] = None
        path.write_text(apply.dump_indent(data, 2))
    for path in [export.GRAPH_GOLD / f"{lang}.json" for lang in export.GRAPH_LANGUAGES]:
        data = json.loads(path.read_text())
        data["reviewed_by"] = None
        path.write_text(apply.dump_lines(data))
    path = export.GRAPH_GOLD / "held-out" / "real-repos.json"
    data = json.loads(path.read_text())
    data["reviewed_by"] = None
    path.write_text(apply.dump_lines(data, group_by="repo"))
    for _, name, _ in export.TASK_FILES:
        path = export.AGENT / "tasks" / name
        data = json.loads(path.read_text())
        for t in data["tasks"]:
            t.pop("reviewed_by", None)
        path.write_text(apply.dump_indent(data, 1))


class ApplyBase(unittest.TestCase):
    """Runs apply on a copy of the datasets in a temporary repository root."""

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        root = Path(self.tmp.name)
        for rel in ("crates/myceliums-benchmarks/real-repos", "crates/myceliums-benchmarks/graph-gold",
                    "evals/agent/tasks", "tests/fixtures/graph-accuracy"):
            shutil.copytree(REPO / rel, root / rel)
        for rel in ("evals/agent/repos.json", "evals/agent/README.md"):
            shutil.copy(REPO / rel, root / rel)
        for meta in (REPO / "evals/agent/results").glob("*/meta.json"):
            dest = root / meta.relative_to(REPO)
            dest.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy(meta, dest)
        self.root = root
        export.set_root(root)
        reset_review_state()
        self.items = export.build_items(snippets=False)
        self.by_id = {it["id"]: it for it in self.items}
        self.store = common.DecisionStore(root / "decisions.jsonl")

    def tearDown(self):
        export.set_root(REPO)
        self.tmp.cleanup()

    def decide(self, item_id, decision, comment=None):
        self.store.record(self.by_id[item_id], decision, comment)

    def run_apply(self, prescreen=None, max_upper=common.ACCEPT_MAX_UPPER, sample=None):
        prescreen = prescreen or {}
        sample = sample or common.Sample({"seed": 164, "rate": 0.15, "datasets": {}})
        sample.sync(self.items, prescreen)
        plan = apply.Plan(self.items, prescreen, self.store, sample, "human:marc", max_upper)
        return apply.run(plan, self.items, write=True, today="2026-11-22")



class Apply(ApplyBase):
    def test_nothing_decided_changes_nothing(self):
        before = {p: p.read_bytes() for p in self.root.rglob("*.json")}
        _, _, writes = self.run_apply()
        self.assertEqual(writes, {})
        self.assertEqual(before, {p: p.read_bytes() for p in self.root.rglob("*.json")})

    def test_retrieval_fixes_and_reviewed_by(self):
        q = json.loads((export.REAL_REPOS / "queries.json").read_text())["queries"]
        q0, q1 = q[0], next(x for x in q[1:] if len(x["relevant"]) >= 2)
        for it in self.items:
            if it["id"].startswith(f"retrieval-queries:{q0['id']}:"):
                self.decide(it["id"], "correct")
        r0, r1 = q1["relevant"][0], q1["relevant"][1]
        p = f"retrieval-queries:{q1['id']}"
        self.decide(f"{p}:label:{r0['file']}::{r0['symbol']}", "wrong", "only a caller")
        self.decide(f"{p}:label:{r1['file']}::{r1['symbol']}", "wrong", "replace: src/x.py::Real.thing — does it")
        self.decide(f"{p}:complete", "wrong", "add: src/y.py::missing — the other half")
        outcomes, todo, _ = self.run_apply()
        after = {x["id"]: x for x in json.loads((export.REAL_REPOS / "queries.json").read_text())["queries"]}
        self.assertEqual(after[q0["id"]]["reviewed_by"], "human:marc")
        labels = [(r["file"], r["symbol"]) for r in after[q1["id"]]["relevant"]]
        self.assertNotIn((r0["file"], r0["symbol"]), labels)
        self.assertIn(("src/x.py", "Real.thing"), labels)
        self.assertIn(("src/y.py", "missing"), labels)
        self.assertIn("Human review (#164)", after[q1["id"]]["rationale"])
        # every item decided only if q1 had exactly the two labels decided above
        expected = "human:marc" if len(q1["relevant"]) == 2 else None
        self.assertEqual(after[q1["id"]]["reviewed_by"], expected)
        self.assertTrue(any("recorded-run.json" in t for t in todo))
        readme = (export.REAL_REPOS / "README.md").read_text()
        self.assertIn(apply.START, readme)
        # re-applying replaces the section rather than adding a second one
        self.run_apply()
        self.assertEqual((export.REAL_REPOS / "README.md").read_text().count(apply.START), 1)

    def test_sampled_acceptance_never_marks_a_retrieval_query(self):
        rq = [it for it in self.items if it["dataset"] == "retrieval-queries"]
        pre = confirmed(rq)
        pre.update(confirmed([it for it in self.items if it["dataset"] != "retrieval-queries"]))
        sample = common.Sample({"seed": 164, "rate": 0.15, "datasets": {}})
        sample.sync(self.items, pre)
        for rid in sample.ids("retrieval-queries"):
            self.decide(rid, "correct")
        outcomes, _, _ = self.run_apply(prescreen=pre, max_upper=0.5, sample=sample)
        queries = json.loads((export.REAL_REPOS / "queries.json").read_text())["queries"]
        decided = set(sample.ids("retrieval-queries"))
        marked = [q for q in queries if q["reviewed_by"] is not None]
        for q in marked:  # only queries whose every item Marc decided
            self.assertEqual(q["reviewed_by"], "human:marc")
            mine = [it["id"] for it in rq if it["id"].startswith(f"retrieval-queries:{q['id']}:")]
            self.assertTrue(set(mine) <= decided, q["id"])
        o = next(o for o in outcomes if o.dataset == "retrieval-queries")
        self.assertEqual(o.units_human_only_kept, len(queries) - len(marked))
        self.assertGreater(o.units_human_only_kept, 100)

    def test_graph_gold_edge_removed_and_baseline_listed(self):
        call = next(it for it in self.items if it["kind"] == "graph-call" and it["id"].startswith("graph-gold:python:"))
        self.decide(call["id"], "wrong")
        _, todo, _ = self.run_apply()
        frm, to = call["id"].split(":call:", 1)[1].split("->")
        data = json.loads((export.GRAPH_GOLD / "python.json").read_text())
        self.assertFalse(any(c["from"] == frm and c["to"] == to for c in data["calls"]))
        self.assertIsNone(data["reviewed_by"])  # the other items are still open
        self.assertTrue(any("--update-baseline" in t for t in todo))

    def test_sampled_acceptance_on_held_out(self):
        held = [it for it in self.items if it["dataset"] == "graph-held-out"]
        pre = confirmed(held)
        sample = common.Sample({"seed": 164, "rate": 0.15, "datasets": {}})
        sample.sync(self.items, pre)
        for rid in sample.ids("graph-held-out"):
            self.decide(rid, "correct")
        wrong = next(it for it in held if it["id"] not in set(sample.ids("graph-held-out")))
        self.decide(wrong["id"], "wrong", "targets: none")
        self.run_apply(prescreen=pre, max_upper=0.5, sample=sample)
        data = json.loads((export.GRAPH_GOLD / "held-out" / "real-repos.json").read_text())
        self.assertTrue(data["reviewed_by"].startswith("human:marc (sampled acceptance, seed 164"))
        pair = next(p for p in data["pairs"] if wrong["id"].endswith(f"{p['caller']}@{p['call_line']}:{p['callee']}"))
        self.assertEqual(pair["targets"], [])

    def test_task_gold_made_optional_and_results_listed(self):
        gold = next(it for it in self.items if it["kind"] == "task-gold" and it["dataset"] == "agent-tasks-outcome")
        self.decide(gold["id"], "wrong", "optional — reached only through a decorator")
        _, todo, _ = self.run_apply()
        tasks = json.loads((export.AGENT / "tasks" / "tasks-outcome.json").read_text())["tasks"]
        tid, name = gold["id"].split(":", 1)[1].split(":gold:")
        task = next(t for t in tasks if t["id"] == tid)
        self.assertNotIn(name, [g["name"] for g in task["gold"]])
        self.assertIn({"name": name, "why": "reached only through a decorator"}, task["optional"])
        self.assertTrue(any("2026-11-21-outcome" in t and "re-grade" in t for t in todo))

    def test_graph_call_replace_is_validated(self):
        data = json.loads((export.GRAPH_GOLD / "python.json").read_text())
        calls = [it for it in self.items if it["kind"] == "graph-call" and it["id"].startswith("graph-gold:python:")]
        a, b, c = calls[0], calls[1], calls[2]
        frm_b = b["id"].split(":call:", 1)[1].split("->")[0]
        existing = next(x["to"] for x in data["calls"] if x["from"] == frm_b)  # an edge frm_b already has
        frm_c, to_c = c["id"].split(":call:", 1)[1].split("->")
        free = next(sym["id"] for sym in data["symbols"]
                    if (frm_c, sym["id"]) not in {(x["from"], x["to"]) for x in data["calls"]})
        self.decide(a["id"], "wrong", "replace: shop/nowhere.py::ghost")
        self.decide(b["id"], "wrong", f"replace: {existing}")
        self.decide(c["id"], "wrong", f"replace: {free}")
        outcomes, _, _ = self.run_apply()
        gg = next(o for o in outcomes if o.dataset == "graph-gold")
        self.assertEqual(gg.wrong_manual, 2)
        self.assertEqual(gg.wrong_fixed, 1)
        self.assertTrue(any("not a gold symbol" in m for m in gg.manual))
        self.assertTrue(any("already in the gold set" in m for m in gg.manual))
        after = json.loads((export.GRAPH_GOLD / "python.json").read_text())
        self.assertIn((frm_c, free), {(x["from"], x["to"]) for x in after["calls"]})
        self.assertNotIn((frm_c, to_c), {(x["from"], x["to"]) for x in after["calls"]})

    def test_manual_fix_blocks_review(self):
        trap = next(it for it in self.items if it["kind"] == "graph-trap")
        self.decide(trap["id"], "wrong", "this is a real call to shop.utils.get")
        outcomes, _, _ = self.run_apply()
        gg = next(o for o in outcomes if o.dataset == "graph-gold")
        self.assertEqual(gg.wrong_manual, 1)
        self.assertIn("real call", gg.manual[0])


class OwnerPolicy(ApplyBase):
    """--policy owner-2026-11-22: truthful provenance per item and per unit."""

    POLICY = "owner-2026-11-22"

    def agent(self, item_id, decision, fix=None, reviewer="agent:second-check"):
        rec = {"id": item_id, "decision": decision, "reason": "checked the code", "evidence": [],
               "reviewer": reviewer}
        if fix:
            rec["suggested_fix"] = fix
        self.agent_recs[item_id] = rec

    def setUp(self):
        super().setUp()
        self.agent_recs: dict[str, dict] = {}
        self.pre = confirmed(self.items)
        q = json.loads((export.REAL_REPOS / "queries.json").read_text())["queries"]
        self.q_human, self.q_mixed = q[0]["id"], q[1]["id"]
        tasks = json.loads((export.AGENT / "tasks" / "tasks.json").read_text())["tasks"]
        self.t_unsure, self.t_fix = tasks[0]["id"], tasks[1]["id"]
        self.flagged = [it for it in self.items if any(
            it["id"].startswith(f"{ds}:{u}:") for ds, u in
            (("retrieval-queries", self.q_human), ("agent-tasks", self.t_unsure), ("agent-tasks", self.t_fix)))]
        self.pre.update(doubtful(self.flagged))
        self.sample = common.Sample({"seed": 164, "rate": 0.15, "datasets": {}})
        self.sample.sync(self.items, self.pre)

    def run_policy(self, overrides=None, addendum=""):
        pdir = self.root / "policy"
        pdir.mkdir(exist_ok=True)
        (pdir / f"{self.POLICY}.json").write_text(json.dumps(
            {"policy": self.POLICY, "addendum": addendum, "overrides": overrides or []}))
        plan = apply.Plan(self.items, self.pre, self.store, self.sample, "human:marc",
                          policy=self.POLICY, agent_decisions=self.agent_recs, policy_dir=pdir)
        out = apply.run(plan, self.items, write=True, today="2026-11-22",
                        provenance_path=self.root / "provenance.jsonl")
        return plan, out

    def test_provenance_classes(self):
        for it in self.flagged:
            if it["id"].startswith(f"retrieval-queries:{self.q_human}:"):
                self.decide(it["id"], "correct")  # Marc decided this query in full
        t_unsure = [it for it in self.flagged if it["id"].startswith(f"agent-tasks:{self.t_unsure}:")]
        for n, it in enumerate(t_unsure):
            self.agent(it["id"], "unsure" if n == 0 else "correct")
        t_fix = [it for it in self.flagged if it["id"].startswith(f"agent-tasks:{self.t_fix}:")]
        gold = next(it for it in t_fix if it["kind"] == "task-gold")
        for it in t_fix:
            self.agent(it["id"], "correct")
        self.agent(gold["id"], "wrong", fix="optional — a thin wrapper")
        held = self.sample.ids("graph-held-out")
        for n, rid in enumerate(held):
            if n % 2:
                self.decide(rid, "correct")
            else:
                self.agent(rid, "correct")
        self.decide(held[1], "correct")
        self.agent(held[1], "wrong")  # a human decision wins over an agent's
        q_mixed = [it for it in self.items if it["id"].startswith(f"retrieval-queries:{self.q_mixed}:")]
        for it in q_mixed:
            if it["id"] in set(self.sample.ids("retrieval-queries")):
                self.agent(it["id"], "correct")

        plan, (outcomes, _, _) = self.run_policy()
        queries = {q["id"]: q for q in json.loads((export.REAL_REPOS / "queries.json").read_text())["queries"]}
        self.assertEqual(queries[self.q_human]["reviewed_by"], "human:marc")
        mixed = queries[self.q_mixed]["reviewed_by"]  # not all human: agent provenance, never "human:"
        self.assertIn("agent:", mixed)
        self.assertFalse(mixed.startswith("human:"))
        tasks = {t["id"]: t for t in json.loads((export.AGENT / "tasks" / "tasks.json").read_text())["tasks"]}
        self.assertIsNone(tasks[self.t_unsure].get("reviewed_by"))
        self.assertEqual(tasks[self.t_fix]["reviewed_by"], "agent:second-check, per marc's policy")
        name = gold["id"].split(":gold:")[1]
        self.assertIn(name, [o["name"] for o in tasks[self.t_fix]["optional"]])
        self.assertIn("[agent:second-check, per marc's policy]", tasks[self.t_fix]["rationale"])
        ho = json.loads((export.GRAPH_GOLD / "held-out" / "real-repos.json").read_text())
        n_h, n_a = len(held) // 2, len(held) - len(held) // 2
        self.assertEqual(ho["reviewed_by"], (
            f"mixed, policy {self.POLICY}: human:marc ×{n_h}; agent:prescreen, accepted by marc "
            f"(sample: {plan.sample_checked} human-checked confirmed items, 0 errors) ×{52 - len(held)}; "
            f"agent:second-check, per marc's policy ×{n_a}"))
        unsure = [i for o in outcomes for i in o.unsure_ids]
        self.assertEqual(unsure, [t_unsure[0]["id"]])

        prov = {r["id"]: r for r in common.read_jsonl(self.root / "provenance.jsonl")}
        self.assertEqual(len(prov), len(self.items))
        decided_by_marc = {rid for rid, rec in self.store.state.items() if rec["decision"] in ("correct", "wrong")}
        self.assertEqual({rid for rid, r in prov.items() if r["class"] == "human"}, decided_by_marc)
        self.assertEqual(prov[held[1]]["final"], "correct")
        self.assertEqual(prov[t_unsure[0]["id"]]["final"], "unsure")
        self.assertEqual(prov[t_unsure[0]["id"]]["class"], "second-check")  # who said unsure
        for r in prov.values():
            if r["class"] == "prescreen":
                self.assertNotIn(r["id"], set(plan.review.sampled))
                self.assertTrue(r["by"].startswith("agent:prescreen, accepted by marc"))
        readme = (export.AGENT / "README.md").read_text()
        self.assertIn(f"policy `{self.POLICY}`", readme)
        self.assertIn("no person looked at this item", readme)

    def test_addendum_rule_and_restatement(self):
        opt = "agent-tasks:fastapi-callees-solve-dependencies:optional:solve_dependencies"
        trap = "graph-gold:go:trap:report/report.go::Report.Render:RenderRow@58"
        self.agent(opt, "correct")  # the rule overrides a single check
        _, (outcomes, todo, _) = self.run_policy(
            overrides=[{"id": opt, "action": "remove", "reason": "class instantiation"},
                       {"id": trap, "action": "restate", "why": "must not target OrderReport.RenderRow"}],
            addendum="consistency rule text")
        task = next(t for t in json.loads((export.AGENT / "tasks" / "tasks.json").read_text())["tasks"]
                    if t["id"] == "fastapi-callees-solve-dependencies")
        self.assertNotIn("solve_dependencies", [o["name"] for o in task["optional"]])
        self.assertIn(f"[policy {self.POLICY} consistency rule]", task["rationale"])
        go = json.loads((export.GRAPH_GOLD / "go.json").read_text())
        t = next(t for t in go["traps"] if t["in"] == "report/report.go::Report.Render" and t["line"] == 58)
        self.assertEqual(t["why"], "must not target OrderReport.RenderRow")
        prov = {r["id"]: r for r in common.read_jsonl(self.root / "provenance.jsonl")}
        self.assertEqual(prov[opt]["class"], "rule")
        self.assertEqual(prov[opt]["final"], "wrong")
        self.assertIn("restated", prov[trap]["edit"])
        self.assertFalse(any("call-resolution" in x for x in todo))  # traps are not scored
        self.assertIn("consistency rule text", (export.AGENT / "README.md").read_text())

    def test_unsure_by_marc_without_second_check_stays_unsure(self):
        it = self.flagged[0]
        self.decide(it["id"], "unsure")
        _, (outcomes, _, _) = self.run_policy()
        self.assertIn(it["id"], [i for o in outcomes for i in o.unsure_ids])

    def test_policy_needs_a_complete_prescreen(self):
        partial = {k: v for k, v in self.pre.items() if not k.startswith("graph-gold:")}
        sample = common.Sample({"seed": 164, "rate": 0.15, "datasets": {}})
        sample.sync(self.items, partial)
        with self.assertRaises(SystemExit):
            apply.Plan(self.items, partial, self.store, sample, "human:marc", policy=self.POLICY)
        with self.assertRaises(SystemExit):
            apply.Plan(self.items, self.pre, self.store, self.sample, "human:marc", policy="no-such-policy")

    def test_agent_file_validation(self):
        d = self.root / "agent-check"
        d.mkdir()
        ids = [it["id"] for it in self.items[:3]]
        lines = [{"id": ids[0], "decision": "correct", "reviewer": "agent:second-check"},
                 {"id": ids[1], "decision": "correct", "reviewer": "marc"},
                 {"id": ids[2], "decision": "maybe", "reviewer": "agent:second-check"},
                 {"id": "nope", "decision": "correct", "reviewer": "agent:second-check"}]
        (d / "decisions-1.jsonl").write_text("".join(json.dumps(x) + "\n" for x in lines))
        recs, warnings = apply.load_agent_decisions(d, self.by_id)
        self.assertEqual(list(recs), [ids[0]])
        self.assertEqual(len(warnings), 3)


class Writers(unittest.TestCase):
    def test_dataset_writers_round_trip(self):
        for lang in export.GRAPH_LANGUAGES:
            text = (export.GRAPH_GOLD / f"{lang}.json").read_text()
            self.assertEqual(apply.dump_lines(json.loads(text)), text, lang)
        held = (export.GRAPH_GOLD / "held-out" / "real-repos.json").read_text()
        self.assertEqual(apply.dump_lines(json.loads(held), group_by="repo"), held)
        for name in ("queries.json", "test-seeking.json"):
            text = (export.REAL_REPOS / name).read_text()
            self.assertEqual(apply.dump_indent(json.loads(text), 2), text, name)
        for _, name, _ in export.TASK_FILES:
            text = (export.AGENT / "tasks" / name).read_text()
            self.assertEqual(apply.dump_indent(json.loads(text), 1), text, name)

    def test_directives(self):
        self.assertEqual(apply.parse_directives("note\nreplace: a.py::B.c — because\nADD: x::y"),
                         [("replace", "a.py::B.c", "because"), ("add", "x::y", "")])
        self.assertEqual(apply.parse_directives("optional — wrapper"), [("optional", "", "wrapper")])
        self.assertEqual(apply.free_text("why not\ntargets: none"), "why not")


if __name__ == "__main__":
    unittest.main()
