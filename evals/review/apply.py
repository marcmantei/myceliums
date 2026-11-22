#!/usr/bin/env python3
"""Write the review decisions back into the datasets (#164).

    python3 evals/review/apply.py --dry-run   # show what would change
    python3 evals/review/apply.py             # write datasets and README protocol sections

For every item: J (correct) marks it reviewed; N (wrong) removes or corrects
the label as the comment says (the directive grammar is in README.md); U
(unsure) and undecided items keep their unit unreviewed. Confirmed items
outside the sample count as accepted when their dataset's sample passed the
acceptance rule.

A unit (a retrieval query, a graph-gold language file, the held-out set, an
agent task) gets ``reviewed_by`` = the reviewer when all its items were
decided and fixed, or ``<reviewer> (sampled acceptance, <rule>)`` when some
were accepted by the sample. A ``reviewed_by`` already set is kept. The
protocol and the numbers go into each dataset's README between
``<!-- label-review:start -->`` and ``<!-- label-review:end -->``. Baselines
measured on the changed files are listed, never re-recorded here.
Python 3.10+ standard library only.
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
import time
from dataclasses import dataclass, field
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import common  # noqa: E402
import export  # noqa: E402

START = "<!-- label-review:start -->"
END = "<!-- label-review:end -->"

def readme_of(dataset: str) -> Path:
    """The README that documents a dataset (resolved at call time, so tests can move the root)."""
    if dataset.startswith("retrieval-"):
        return export.REAL_REPOS / "README.md"
    if dataset.startswith("graph-"):
        return export.GRAPH_GOLD / "README.md"
    return export.AGENT / "README.md"


# ---------------------------------------------------------------- directives

_DIRECTIVE = re.compile(r"^\s*(replace|add|targets|optional)\b\s*:?\s*(.*)$", re.IGNORECASE)
_WHY_SEP = re.compile(r"\s*(?:—|–|--)\s*")


def parse_directives(comment: str | None) -> list[tuple[str, str, str]]:
    """``(verb, argument, why)`` for every directive line of a comment."""
    out = []
    for line in (comment or "").splitlines():
        m = _DIRECTIVE.match(line)
        if not m:
            continue
        parts = _WHY_SEP.split(m.group(2).strip(), maxsplit=1)
        arg = parts[0].strip()
        why = parts[1].strip() if len(parts) > 1 else ""
        out.append((m.group(1).lower(), arg, why))
    return out


def free_text(comment: str | None) -> str:
    """The comment without its directive lines."""
    return " ".join(ln.strip() for ln in (comment or "").splitlines()
                    if ln.strip() and not _DIRECTIVE.match(ln))


# --------------------------------------------------------------------- state

# Owner policies: how items Marc did not decide himself get their final decision.
POLICIES = {
    "owner-2026-11-22": (
        "Marc's policy of 22 November 2026: pre-screen-confirmed items outside the sample are accepted as "
        "correct on the strength of the sample he checked; every other item he did not decide, and every "
        "item he marked unsure, is decided by an independent second-check agent "
        "(`evals/review/agent-check/decisions-*.jsonl`). An item whose final decision is unsure is not "
        "marked reviewed."
    ),
}
# Datasets whose evaluator reports any non-null reviewed_by as a human review and refuses agent
# reviewers (real-repo-eval): there a unit is marked only when every item was decided by a person.
HUMAN_ONLY_DATASETS = ("retrieval-queries", "retrieval-test-seeking")
CLASS_ORDER = ("human", "sample", "prescreen", "second-check", "rule")


def load_policy(policy: str, directory: Path | None = None) -> dict:
    """The policy's addendum file (``policy/<name>.json``): its text and per-item overrides."""
    path = (directory or common.REVIEW_DIR / "policy") / f"{policy}.json"
    if not path.exists():
        return {"policy": policy, "addendum": "", "overrides": []}
    data = json.loads(path.read_text(encoding="utf-8"))
    for o in data.get("overrides", []):
        if o.get("action") not in ("remove", "restate"):
            raise SystemExit(f"{path}: override {o.get('id')}: action must be remove or restate")
    return data


def load_agent_decisions(directory: Path, by_id: dict[str, dict]) -> tuple[dict[str, dict], list[str]]:
    """Second-check agent decisions by item id, and warnings.

    Format, one JSON object per line in ``agent-check/decisions-*.jsonl``:
    ``{"id", "decision": correct|wrong|unsure, "reason", "evidence": [...], "suggested_fix"?,
    "reviewer": "agent:second-check"}``. A later line for the same id replaces an earlier one.
    """
    out: dict[str, dict] = {}
    warnings: list[str] = []
    if not directory.is_dir():
        return out, warnings
    for path in sorted(directory.glob("decisions-*.jsonl")):
        for n, rec in enumerate(common.read_jsonl(path), 1):
            rid = rec.get("id")
            if rid not in by_id:
                warnings.append(f"{path.name} record {n}: unknown item id {rid!r}")
            elif rec.get("decision") not in common.DECISIONS:
                warnings.append(f"{path.name} record {n} ({rid}): decision must be one of {common.DECISIONS}")
            elif not str(rec.get("reviewer", "")).lower().startswith("agent:"):
                warnings.append(f"{path.name} record {n} ({rid}): reviewer must be an agent: identity")
            else:
                if rid in out:
                    warnings.append(f"{path.name}: {rid} decided twice; the later record counts")
                out[rid] = rec
    return out, warnings


@dataclass
class Outcome:
    """What applying did, per dataset."""
    dataset: str
    reviewed: int = 0
    accepted: int = 0
    wrong_fixed: int = 0
    wrong_manual: int = 0
    unsure: int = 0
    open: int = 0
    units_reviewed: int = 0
    units_total: int = 0
    units_human_only_kept: int = 0
    classes: dict[str, int] = field(default_factory=dict)  # provenance class -> items decided
    manual: list[str] = field(default_factory=list)
    unsure_ids: list[str] = field(default_factory=list)
    open_ids: list[str] = field(default_factory=list)
    changed_files: dict[str, str] = field(default_factory=dict)  # path -> "labels" | "review"


class Plan:
    """Decisions joined with items, pre-screen and sample status (and, under a policy, agent checks)."""

    def __init__(self, items, prescreen, decisions: common.DecisionStore, sample: common.Sample,
                 reviewer: str, max_upper: float = common.ACCEPT_MAX_UPPER,
                 policy: str | None = None, agent_decisions: dict[str, dict] | None = None,
                 policy_dir: Path | None = None):
        self.review = common.Review(items, prescreen, decisions, sample, max_upper)
        self.decisions = decisions
        self.summary = {row["dataset"]: row for row in self.review.summary()["datasets"]}
        self.rule = common.acceptance_rule(sample.seed, sample.rate, max_upper)
        self.reviewer = reviewer
        self.handle = reviewer.split(":", 1)[1] if reviewer.startswith("human:") else reviewer
        self.items = items
        if policy is not None and policy not in POLICIES:
            raise SystemExit(f"unknown policy {policy!r}; known: {', '.join(POLICIES)}")
        self.policy = policy
        self.agent = agent_decisions or {}
        self.provenance: list[dict] = []
        self.current_baselines: list[str] = []
        self.addendum = load_policy(policy, policy_dir) if policy else {"addendum": "", "overrides": []}
        by_id = {it["id"] for it in items}
        unknown = [o["id"] for o in self.addendum["overrides"] if o["id"] not in by_id]
        if unknown:  # removed by an earlier apply, or a typo: either way nothing to do here
            print(f"note: policy {policy}: {len(unknown)} overrides name items not in the datasets "
                  f"(already applied?): {', '.join(unknown)}", file=sys.stderr)
        self.removals = {o["id"]: o for o in self.addendum["overrides"] if o["action"] == "remove"}
        self.restates = {o["id"]: o for o in self.addendum["overrides"] if o["action"] == "restate"}
        if policy:
            bad = [row["dataset"] for row in self.summary.values()
                   if row["status"] in ("prescreen-incomplete", "sample-stale")]
            if bad:
                raise SystemExit(f"policy {policy} needs a complete pre-screen and the sample it was "
                                 f"drawn from; not so for: {', '.join(bad)}")
            human = [rid for rid in self.review.sampled if self._human(rid)]
            self.sample_checked = len(human)
            self.sample_errors = sum(self._human(rid)["decision"] in ("wrong", "unsure") for rid in human)
            # per dataset: how many of its sampled confirmed items Marc decided, and his errors there
            self.sample_by_ds: dict[str, tuple[int, int]] = {}
            for ds in self.summary:
                mine = [rid for rid in human if self.review.by_id[rid]["dataset"] == ds]
                self.sample_by_ds[ds] = (len(mine), sum(self._human(rid)["decision"] in ("wrong", "unsure")
                                                        for rid in mine))

    def _human(self, rid: str) -> dict | None:
        return self.decisions.effective(self.review.by_id[rid])

    def resolve(self, it: dict) -> tuple[str, str | None, str]:
        """(status, provenance class, comment) of an item.

        status: reviewed | wrong | unsure | accepted | open. An item whose wording the
        policy restated gets the mixed class ``<class>+restated``.
        """
        st, cls, comment = self._resolve(it)
        if cls and it["id"] in self.restates:
            cls += "+restated"
        return st, cls, comment

    def _resolve(self, it: dict) -> tuple[str, str | None, str]:
        rec = self.decisions.effective(it)
        if not self.policy:
            if rec:
                st = {"correct": "reviewed", "wrong": "wrong", "unsure": "unsure"}[rec["decision"]]
                return st, "human", rec.get("comment") or ""
            ds = self.summary.get(it["dataset"], {})
            if (common.verdict_of(it["id"], self.review.prescreen) == "confirmed"
                    and ds.get("status") == "accepted"):
                return "accepted", "sample", ""
            return "open", None, ""
        rule = self.removals.get(it["id"])
        if rule:  # the policy's consistency rule overrides every single-item decision
            return "wrong", "rule", rule.get("reason", "")
        if rec and rec["decision"] in ("correct", "wrong"):
            return ("reviewed" if rec["decision"] == "correct" else "wrong"), "human", rec.get("comment") or ""
        agent = self.agent.get(it["id"])
        if agent:
            st = {"correct": "reviewed", "wrong": "wrong", "unsure": "unsure"}[agent["decision"]]
            comment = "\n".join(x for x in (agent.get("suggested_fix") or "", agent.get("reason") or "") if x)
            return st, "second-check", comment
        if rec:  # unsure, and no second check yet
            return "unsure", "human", rec.get("comment") or ""
        if (common.verdict_of(it["id"], self.review.prescreen) == "confirmed"
                and it["id"] not in self.review.sampled):
            return "accepted", "prescreen", ""
        return "open", None, ""

    def status(self, it: dict) -> str:
        return self.resolve(it)[0]

    def comment(self, it: dict) -> str:
        return self.resolve(it)[2]

    def label(self, cls: str, dataset: str | None = None) -> str:
        """The provenance string of one class (the prescreen class names its dataset's human sample)."""
        if cls.endswith("+restated"):
            return f"{self.label(cls[:-len('+restated')], dataset)}, wording restated by policy {self.policy}"
        if cls == "human":
            return self.reviewer
        if cls == "sample":
            return f"{self.reviewer} (sampled acceptance, {self.rule})"
        if cls == "prescreen":
            n, e = self.sample_by_ds.get(dataset or "", (0, 0))
            return (f"agent:prescreen, accepted by {self.handle}'s policy ({self.policy}); "
                    f"human sample in this dataset: {n} checked, {e} errors")
        if cls == "second-check":
            return f"agent:second-check, per {self.handle}'s policy ({self.policy})"
        if cls == "rule":
            return f"policy {self.policy} consistency rule"
        raise ValueError(cls)

    def unit_reviewed_by(self, classes: dict[str, int], dataset: str) -> str | None:
        """reviewed_by of a unit whose items were all decided; None: leave it null."""
        used = sorted((c for c in classes if classes[c]),
                      key=lambda c: (CLASS_ORDER.index(c.split("+")[0]), c))
        if not self.policy:
            if "sample" in used and dataset in HUMAN_ONLY_DATASETS:
                return None  # accepted by sample is not a human review of this query
            return self.label("sample") if "sample" in used else self.reviewer
        if used == ["human"]:
            return self.reviewer
        if not used:
            return None
        if len(used) == 1:
            return self.label(used[0], dataset)
        return f"mixed, policy {self.policy}: " + "; ".join(f"{classes[c]}× {self.label(c, dataset)}"
                                                           for c in used)


class Unit:
    """Bookkeeping for one unit: blockers and the provenance of its items."""

    def __init__(self, plan: Plan, out: Outcome, name: str, dataset: str | None = None):
        self.plan, self.out, self.name = plan, out, name
        self.dataset = dataset or out.dataset
        self.blocked = False
        self.classes: dict[str, int] = {}
        self.cls: str | None = None
        self.notes: list[str] = []

    def take(self, it: dict) -> str:
        st, cls, _ = self.plan.resolve(it)
        self.cls = cls
        if st == "reviewed":
            self.out.reviewed += 1
        elif st == "accepted":
            self.out.accepted += 1
        elif st == "unsure":
            self.out.unsure += 1
            self.out.unsure_ids.append(it["id"])
            self.blocked = True
        elif st == "open":
            self.out.open += 1
            self.out.open_ids.append(it["id"])
            self.blocked = True
        if st in ("reviewed", "accepted", "wrong") and cls:
            self.classes[cls] = self.classes.get(cls, 0) + 1
            self.out.classes[cls] = self.out.classes.get(cls, 0) + 1
        if self.plan.policy:
            rec = {"id": it["id"], "dataset": it["dataset"],
                   "final": {"reviewed": "correct", "accepted": "correct"}.get(st, st),
                   "class": cls, "by": self.plan.label(cls, it["dataset"]) if cls else None}
            if it["id"] in self.plan.restates:
                rec["edit"] = f"restated by policy {self.plan.policy}: {self.plan.restates[it['id']]['why']}"
            self.plan.provenance.append(rec)
        return st

    def manual(self, it: dict, why: str) -> None:
        self.out.wrong_manual += 1
        self.blocked = True
        text = free_text(self.plan.comment(it)) or "(no comment)"
        by = f" [{self.plan.label(self.cls, self.dataset)}]" if self.plan.policy and self.cls else ""
        self.out.manual.append(f"{it['id']}: {why}.{by} Comment: {text}")

    def fixed(self, note: str) -> None:
        self.out.wrong_fixed += 1
        if self.plan.policy and self.cls:
            note += f" [{self.plan.label(self.cls, self.dataset)}]"
        self.notes.append(note)

    def finish(self, holder: dict, key: str = "reviewed_by") -> bool:
        """Set reviewed_by when nothing blocks; True if it changed."""
        self.out.units_total += 1
        if self.blocked:
            return False
        value = self.plan.unit_reviewed_by(self.classes, self.dataset)
        if value is None:
            self.out.units_human_only_kept += 1
            return False
        self.out.units_reviewed += 1
        if holder.get(key) is None:
            holder[key] = value
            return True
        return False


def review_note(notes: list[str], policy: str | None = None) -> str:
    head = f" Label review (#164, policy {policy})" if policy else " Human review (#164)"
    return head + ": " + "; ".join(notes) + "."


def local_id(it: dict) -> str:
    return it["id"][len(it["dataset"]) + 1:]


# ------------------------------------------------------------------- writers

def dump_indent(data, indent: int) -> str:
    return json.dumps(data, indent=indent, ensure_ascii=False) + "\n"


def dump_lines(data: dict, group_by: str | None = None) -> str:
    """The graph-gold style: top-level keys one per line, one compact object per array line.

    ``group_by``: a blank line between array entries whose value of that key differs
    (the held-out set groups its pairs by repository).
    """
    parts = []
    for key, value in data.items():
        k = json.dumps(key, ensure_ascii=False)
        if isinstance(value, list) and value and all(isinstance(v, dict) for v in value):
            rows = ""
            for i, v in enumerate(value):
                if i:
                    rows += ",\n"
                    if group_by and v.get(group_by) != value[i - 1].get(group_by):
                        rows += "\n"
                rows += "    " + json.dumps(v, ensure_ascii=False)
            parts.append(f"  {k}: [\n{rows}\n  ]")
        elif isinstance(value, dict) and value and not any(isinstance(v, (dict, list)) for v in value.values()):
            rows = ",\n".join(f"    {json.dumps(a, ensure_ascii=False)}: {json.dumps(b, ensure_ascii=False)}"
                              for a, b in value.items())
            parts.append(f"  {k}: {{\n{rows}\n  }}")
        else:
            parts.append(f"  {k}: {json.dumps(value, ensure_ascii=False)}")
    return "{\n" + ",\n".join(parts) + "\n}\n"


# ------------------------------------------------------------------ datasets

def apply_retrieval(plan: Plan, ds: str, path: Path, items: list[dict]) -> tuple[str, Outcome]:
    out = Outcome(ds)
    original = path.read_text(encoding="utf-8")
    data = json.loads(original)
    by_query: dict[str, list[dict]] = {}
    for it in items:
        by_query.setdefault(local_id(it).split(":", 1)[0], []).append(it)
    labels_changed = False
    for q in data["queries"]:
        unit = Unit(plan, out, q["id"])
        relevant = [dict(r) for r in q["relevant"]]
        for it in by_query.get(q["id"], []):
            if unit.take(it) != "wrong":
                continue
            dirs = parse_directives(plan.comment(it))
            if it["kind"] == "retrieval-label":
                file, _, symbol = local_id(it).split(":label:", 1)[1].partition("::")
                at = next((i for i, r in enumerate(relevant) if r["file"] == file and r["symbol"] == symbol), None)
                if at is None:
                    unit.manual(it, "label no longer in the dataset")
                    continue
                rep = [d for d in dirs if d[0] == "replace" and "::" in d[1]]
                if rep:
                    nf, _, ns = rep[0][1].partition("::")
                    why = rep[0][2] or free_text(plan.comment(it)) or f"replaces {symbol} in human review (#164)"
                    relevant[at] = {"file": nf.strip(), "symbol": ns.strip(), "why": why}
                    unit.fixed(f"replaced `{symbol}` with `{ns.strip()}`")
                else:
                    relevant.pop(at)
                    unit.fixed(f"removed `{symbol}`" + (f" ({free_text(plan.comment(it))})"
                                                       if free_text(plan.comment(it)) else ""))
            else:  # retrieval-complete
                adds = [d for d in dirs if d[0] == "add" and "::" in d[1]]
                if not adds:
                    unit.manual(it, "a missing answer is not named with `add: file::symbol — why`")
                    continue
                for _, arg, why in adds:
                    nf, _, ns = arg.partition("::")
                    relevant.append({"file": nf.strip(), "symbol": ns.strip(),
                                     "why": why or "added in human review (#164)"})
                unit.fixed("added " + ", ".join(f"`{a[1].partition('::')[2].strip()}`" for a in adds))
        if not relevant:
            unit.blocked = True
            out.manual.append(f"{ds}:{q['id']}: every label was rejected; a query needs at least one — fix by hand")
            relevant = q["relevant"]
            unit.notes = []
        if relevant != q["relevant"]:
            q["relevant"] = relevant
            q["rationale"] += review_note(unit.notes, plan.policy)
            labels_changed = True
        unit.finish(q)
    text = dump_indent(data, 2)
    if text != original:
        out.changed_files[export.rel(path)] = "labels" if labels_changed else "review"
    return text, out


def apply_graph_gold(plan: Plan, items: list[dict]) -> tuple[dict[Path, str], Outcome]:
    out = Outcome("graph-gold")
    texts: dict[Path, str] = {}
    for lang in export.GRAPH_LANGUAGES:
        path = export.GRAPH_GOLD / f"{lang}.json"
        original = path.read_text(encoding="utf-8")
        data = json.loads(original)
        unit = Unit(plan, out, lang)
        mine = [it for it in items if local_id(it).startswith(f"{lang}:")]
        remove_symbols, remove_calls, retarget = set(), set(), {}
        symbol_ids = {sym["id"] for sym in data["symbols"]}
        edges = {(c["from"], c["to"]) for c in data["calls"]}
        for it in mine:
            if unit.take(it) != "wrong":
                continue
            kind_key = local_id(it).split(":", 2)[2]
            if it["kind"] == "graph-symbol":
                sid = kind_key
                named = any(sid in (c["from"], c["to"], *c.get("also_accept", [])) for c in data["calls"]) or \
                    any(sid in (d["from"], d["to"]) for d in data.get("dynamic", [])) or \
                    any(sid == t["in"] for t in data.get("traps", []))
                if named:
                    unit.manual(it, "an edge, dynamic call or trap still names this symbol")
                else:
                    remove_symbols.add(sid)
                    unit.fixed(f"removed symbol {sid}")
            elif it["kind"] == "graph-call":
                frm, to = kind_key.split("->", 1)
                rep = [d for d in parse_directives(plan.comment(it)) if d[0] == "replace" and "::" in d[1]]
                if rep:
                    new_to = rep[0][1]
                    if new_to not in symbol_ids:
                        unit.manual(it, f"replace target {new_to} is not a gold symbol of {lang}.json")
                    elif (frm, new_to) in edges or (frm, new_to) in {(a, b) for (a, _), b in retarget.items()}:
                        unit.manual(it, f"the edge {frm} -> {new_to} is already in the gold set")
                    else:
                        retarget[(frm, to)] = new_to
                        unit.fixed(f"retargeted {frm} -> {new_to}")
                else:
                    remove_calls.add((frm, to))
                    unit.fixed(f"removed edge {frm} -> {to}")
            else:
                unit.manual(it, f"a wrong {it['kind'].split('-')[1]} entry is fixed by hand")
        labels_changed = bool(remove_symbols or remove_calls or retarget)
        data["symbols"] = [s for s in data["symbols"] if s["id"] not in remove_symbols]
        calls = []
        for c in data["calls"]:
            key = (c["from"], c["to"])
            if key in remove_calls:
                continue
            if key in retarget:
                c = {k: v for k, v in c.items() if k != "also_accept"}
                c["to"] = retarget[key]
            calls.append(c)
        data["calls"] = calls
        for t in data.get("traps", []):
            restate = plan.restates.get(f"graph-gold:{lang}:trap:{t['in']}:{t['name']}@{t['line']}")
            if restate and t["why"] != restate["why"]:
                t["why"] = restate["why"]  # traps are not scored: documentation only
        unit.finish(data)
        text = dump_lines(data)
        if text != original:
            texts[path] = text
            out.changed_files[export.rel(path)] = "labels" if labels_changed else "review"
    return texts, out


_TARGET = re.compile(r"^(.+?)::(.+):(\d+)$")


def apply_held_out(plan: Plan, items: list[dict]) -> tuple[str, Outcome]:
    out = Outcome("graph-held-out")
    path = export.GRAPH_GOLD / "held-out" / "real-repos.json"
    original = path.read_text(encoding="utf-8")
    data = json.loads(original)
    unit = Unit(plan, out, "held-out")
    by_id = {it["id"]: it for it in items}
    labels_changed = False
    for p in data["pairs"]:
        it = by_id.get(f"graph-held-out:{p['repo']}:{p['file']}:{p['caller']}@{p['call_line']}:{p['callee']}")
        if it is None or unit.take(it) != "wrong":
            continue
        dirs = [d for d in parse_directives(plan.comment(it)) if d[0] == "targets"]
        if not dirs:
            unit.manual(it, "the right targets are not given with `targets: file::name:line, …` or `targets: none`")
            continue
        arg = dirs[0][1]
        if arg.lower() == "none":
            targets = []
        else:
            targets = []
            for part in arg.split(","):
                m = _TARGET.match(part.strip())
                if not m:
                    targets = None
                    break
                targets.append({"file": m.group(1), "name": m.group(2), "line": int(m.group(3))})
            if targets is None:
                unit.manual(it, f"cannot read targets {arg!r}")
                continue
        p["targets"] = targets
        note = free_text(plan.comment(it))
        p["why"] = (note or p["why"]) + " (corrected in human review, #164)"
        labels_changed = True
        unit.fixed(f"{p['caller']}->{p['callee']}")
    unit.finish(data)
    text = dump_lines(data, group_by="repo")
    if text != original:
        out.changed_files[export.rel(path)] = "labels" if labels_changed else "review"
    return text, out


def apply_tasks(plan: Plan, ds: str, path: Path, items: list[dict]) -> tuple[str, Outcome]:
    out = Outcome(ds)
    original = path.read_text(encoding="utf-8")
    data = json.loads(original)
    by_task: dict[str, list[dict]] = {}
    for it in items:
        by_task.setdefault(local_id(it).split(":", 1)[0], []).append(it)
    labels_changed = False
    for t in data["tasks"]:
        unit = Unit(plan, out, t["id"])
        gold = [dict(g) for g in t["gold"]]
        optional = [dict(o) if isinstance(o, dict) else o for o in t.get("optional", [])]
        for it in by_task.get(t["id"], []):
            if unit.take(it) != "wrong":
                continue
            rest = local_id(it).split(":", 1)[1]
            note = free_text(plan.comment(it))
            if it["kind"] == "task-gold":
                name = rest.split("gold:", 1)[1]
                at = next((i for i, g in enumerate(gold) if g["name"] == name), None)
                if at is None:
                    unit.manual(it, "answer no longer in the gold set")
                    continue
                opt = [d for d in parse_directives(plan.comment(it)) if d[0] == "optional"]
                g = gold.pop(at)
                if opt:
                    entry = {"name": name, "why": opt[0][2] or opt[0][1] or note or "made optional in human review (#164)"}
                    if g.get("aliases"):
                        entry["aliases"] = g["aliases"]
                    optional.append(entry)
                    unit.fixed(f"made `{name}` optional")
                else:
                    unit.fixed(f"removed `{name}`" + (f" ({note})" if note else ""))
            elif it["kind"] == "task-optional":
                name = rest.split("optional:", 1)[1]
                before = len(optional)
                optional = [o for o in optional if (o["name"] if isinstance(o, dict) else o) != name]
                if len(optional) == before:
                    unit.manual(it, "optional answer no longer in the task")
                else:
                    unit.fixed(f"removed optional `{name}`")
            else:
                unit.manual(it, "a missing gold answer needs evidence lines: add it by hand")
        if not gold:
            unit.blocked = True
            out.manual.append(f"{ds}:{t['id']}: every gold answer was rejected — fix by hand")
            gold, optional, unit.notes = t["gold"], t.get("optional", []), []
        if gold != t["gold"] or optional != t.get("optional", []):
            t["gold"] = gold
            if optional or "optional" in t:
                t["optional"] = optional
            t["rationale"] += review_note(unit.notes, plan.policy)
            labels_changed = True
        unit.finish(t)
    text = dump_indent(data, 1)
    if text != original:
        out.changed_files[export.rel(path)] = "labels" if labels_changed else "review"
    return text, out


# ----------------------------------------------------------------- baselines

def graph_gold_digest(read) -> str:
    """The Rust ``GoldSet::digest`` (graph_eval/gold.rs), computed from ``read(path) -> bytes``:
    each language's name and gold JSON, then every fixture file, all length-prefixed."""
    import hashlib
    import os
    import struct

    h = hashlib.sha256()

    def feed(data: bytes) -> None:
        h.update(struct.pack("<Q", len(data)))
        h.update(data)

    for lang in export.GRAPH_LANGUAGES:
        feed(lang.encode())
        feed(read(export.GRAPH_GOLD / f"{lang}.json"))
    base = export.FIXTURES / "graph-accuracy"

    def walk(d: Path):
        for name in sorted(os.listdir(d), key=lambda n: n.encode()):
            p = d / name
            if p.is_dir() and not p.is_symlink():
                yield from walk(p)
            elif p.is_file():
                yield p

    for p in walk(base):
        feed(p.relative_to(base).as_posix().encode())
        feed(p.read_bytes())
    return h.hexdigest()


def stale_grades(res_dir: Path, task_text: str) -> int:
    """How many stored sessions of a run grade differently against ``task_text`` (agent_eval's grading)."""
    code = str(Path(__file__).resolve().parents[1] / "agent")  # the grading code of this checkout
    if code not in sys.path:
        sys.path.insert(0, code)
    from aeval.grading import grade, is_success  # noqa: E402

    meta = json.loads((res_dir / "meta.json").read_text(encoding="utf-8"))
    threshold = meta.get("success_threshold", 0.8)
    tasks = {t["id"]: t for t in json.loads(task_text)["tasks"]}
    n = 0
    for line in (res_dir / "results.jsonl").read_text(encoding="utf-8").splitlines():
        if not line.strip():
            continue
        row = json.loads(line)
        g = grade(tasks[row["task_id"]], row["answer"], threshold)
        now = dict(f1=round(g.f1, 4), precision=round(g.precision, 4), recall=round(g.recall, 4),
                   success=is_success(g.f1, threshold), tp=g.tp, fp=g.fp, neutral=g.neutral,
                   n_gold=g.n_gold, missed=g.missed, false_positives=g.false_positives)
        n += any(row.get(k) != v for k, v in now.items())
    return n


def baselines(changed: dict[str, str], writes: dict[Path, str] | None = None) -> tuple[list[str], list[str]]:
    """(to redo, checked and current): what was measured on the datasets, checked against the
    content this apply leaves behind (``writes`` over the files on disk)."""
    writes = writes or {}

    def read(path: Path) -> bytes:
        return writes[path].encode("utf-8") if path in writes else path.read_bytes()

    import hashlib

    todo: list[str] = []
    current: list[str] = []
    rr = "crates/myceliums-benchmarks/real-repos/"
    if rr + "queries.json" in changed or rr + "test-seeking.json" in changed:
        rec = json.loads((export.REAL_REPOS / "recorded-run.json").read_text(encoding="utf-8"))
        digest = hashlib.sha256(read(export.REAL_REPOS / "queries.json")).hexdigest()
        if rec.get("dataset_digest") == digest:
            current.append(f"{rr}recorded-run.json — recorded on the current queries.json")
        else:
            todo.append(f"{rr}recorded-run.json — measured on dataset {rec.get('dataset_version')} "
                        f"(digest {str(rec.get('dataset_digest'))[:12]}…), not on the current file; report-only. "
                        "Re-run `cargo run -p myceliums-benchmarks --bin real-repo-eval -- --record` when its "
                        "numbers should belong to the reviewed file")
    if changed.get(rr + "queries.json") == "labels":
        todo.append(f"{rr}exact-vs-ann-hono.json, {rr}embedding-variants.json and the numbers in "
                    "crates/myceliums-benchmarks/benchmarks/METHODOLOGY.md (real-repository sections) "
                    "were measured on the old labels — re-measure")
    if changed.get(rr + "test-seeking.json") == "labels":
        todo.append("METHODOLOGY.md, test-code demotion section (#146): the weight was chosen on the old "
                    "test-seeking labels — re-run the demotion sweep")
    gg = [p for p in changed if p.startswith("crates/myceliums-benchmarks/graph-gold/") and "held-out" not in p]
    if gg:
        base = json.loads((export.GRAPH_GOLD / "baseline.json").read_text(encoding="utf-8"))
        digest = graph_gold_digest(read)
        if base.get("dataset_digest") == digest:
            current.append(f"crates/myceliums-benchmarks/graph-gold/baseline.json — re-recorded on the reviewed "
                           f"labels (digest {digest[:12]}…)")
        else:
            todo.append("crates/myceliums-benchmarks/graph-gold/baseline.json — REQUIRED before merge: the gate "
                        "refuses a baseline recorded against other labels; re-run "
                        "`cargo run -p myceliums-benchmarks --bin graph-eval -- --update-baseline`")
        if any(changed[p] == "labels" for p in gg):
            todo.append("docs/guides/call-resolution-limitations.md — fixture numbers measured on the old labels")
    if changed.get("crates/myceliums-benchmarks/graph-gold/held-out/real-repos.json") == "labels":
        todo.append("docs/guides/call-resolution-limitations.md — held-out numbers (printed by graph-eval) "
                    "measured on the old labels")
    for path in sorted(changed):
        if not path.startswith("evals/agent/tasks/"):
            continue
        name = path.split("/", 2)[2]
        text = read(common.REPO_ROOT / path).decode("utf-8")
        sha = hashlib.sha256(text.encode("utf-8")).hexdigest()
        for meta in sorted((export.AGENT / "results").glob("*/meta.json")):
            m = json.loads(meta.read_text(encoding="utf-8"))
            tf = m.get("tasks_file") or ("tasks/tasks.json" if "tasks_sha256" in m else None)
            if tf != name or not (meta.parent / "results.jsonl").exists():
                continue
            run = meta.parent.relative_to(common.REPO_ROOT).as_posix()
            n = stale_grades(meta.parent, text)
            if n:
                todo.append(f"{run} — {n} sessions grade differently against the reviewed {name}: "
                            f"`agent_eval.py regrade` and regenerate REPORT.md")
            elif m.get("tasks_sha256") == sha:
                current.append(f"{run} — graded against the reviewed {name}")
            else:
                current.append(f"{run} — grades checked against the reviewed {name}: unchanged (its meta "
                               f"records the sha256 of the file before review metadata was added)")
    return todo, current


# ------------------------------------------------------------------- READMEs

def protocol_section(plan: Plan, outcomes: list[Outcome], todo: list[str], today: str,
                     current: list[str] | None = None) -> str:
    s = plan.review.sample
    lines = [
        START,
        "## Human review of the labels (#164)",
        "",
        f"Applied on {today} with `evals/review` (reviewer `{plan.reviewer}`). An independent AI pre-screen "
        "marked every item confirmed, doubtful or wrong (no pre-screen counts as doubtful). The reviewer "
        "decided every doubtful and wrong item and a seeded random sample of the confirmed ones per dataset "
        f"(seed {s.seed}, {round(s.rate * 100)} % per round, widened while the rule failed; `evals/review/sample.json` "
        f"lists the drawn ids; a sample is drawn only once the pre-screen covers the whole dataset). "
        f"Rule: {plan.rule}; an unsure decision counts as an error. The sample is widened after looking at "
        "its result, so the rule is a sequential check and the reported Wilson bound is not an exact "
        "95 % guarantee. Labels decided wrong "
        "were removed or corrected as the reviewer's comment says (`evals/review/decisions.jsonl`). "
        f"`reviewed_by` is `{plan.reviewer}` where every item was decided, "
        f"`{plan.reviewer} (sampled acceptance, …)` where confirmed items were accepted by the sample, and "
        "stays `null` where an item is open, unsure or needs a fix by hand.",
        "",
        "| dataset | items | reviewed | accepted by sample | wrong: fixed / by hand | unsure | open | "
        "units reviewed | sample errors / n | Wilson 95 % |",
        "|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|",
    ]
    for o in outcomes:
        row = plan.summary.get(o.dataset, {})
        w = row.get("wilson95")
        wil = f"{w[0] * 100:.1f}–{w[1] * 100:.1f} %" if w else "—"
        lines.append(f"| {o.dataset} | {row.get('items', 0)} | {o.reviewed} | {o.accepted} | "
                     f"{o.wrong_fixed} / {o.wrong_manual} | {o.unsure} | {o.open} | "
                     f"{o.units_reviewed} of {o.units_total} | {row.get('sample_errors', 0)} / "
                     f"{row.get('sample_reviewed', 0)} | {wil} |")
    if plan.policy:
        lines = policy_section(plan, outcomes, today)
    if todo:
        lines += ["", "Measured on the labels before this review (re-record before relying on them):", ""]
        lines += [f"- {t}" for t in todo]
    if current:
        lines += ["", "Checked against the reviewed labels and current:", ""]
        lines += [f"- {c}" for c in current]
    lines.append(END)
    return "\n".join(lines) + "\n"


def policy_section(plan: Plan, outcomes: list[Outcome], today: str) -> list[str]:
    """The protocol under an owner policy: who decided what, per provenance class."""
    s = plan.review.sample
    lines = [
        START,
        "## Review of the labels (#164)",
        "",
        f"Applied on {today} with `evals/review` under policy `{plan.policy}`. An independent AI pre-screen "
        "marked every item confirmed, doubtful or wrong. Marc reviewed items one by one in the review page: "
        f"the queue held every doubtful and wrong item and a seeded random sample of the confirmed ones "
        f"(seed {s.seed}, {round(s.rate * 100)} % per dataset; `evals/review/sample.json`). Over all datasets he "
        f"decided {plan.sample_checked} sampled confirmed items with {plan.sample_errors} errors "
        "(`evals/review/decisions.jsonl`); the table gives each dataset's own human sample, which can be 0, "
        "and the prescreen provenance string names it. " + POLICIES[plan.policy],
        "",
        "Provenance, per item in `evals/review/provenance.jsonl`:",
        "",
        f"- **human** — `{plan.label('human')}`: Marc decided the item himself;",
        f"- **prescreen** — `agent:prescreen, accepted by {plan.handle}'s policy ({plan.policy}); human sample "
        "in this dataset: N checked, E errors`: confirmed by the pre-screen, outside the sample, accepted by "
        "the policy — no person looked at this item;",
        f"- **second-check** — `{plan.label('second-check')}`: decided by the second-check agent "
        "(`evals/review/agent-check/`) — no person looked at this item;",
        f"- **rule** — `{plan.label('rule')}`: decided by the policy's consistency rule "
        f"(`evals/review/policy/{plan.policy}.json`), whatever any single check said;",
        f"- **restated** — `<class>, wording restated by policy {plan.policy}`: decided as its class says; "
        "the policy then restated its wording (a graph-gold trap now names the one target it must not have).",
        "",
        *([f"Policy addendum: {plan.addendum['addendum']}", ""] if plan.addendum.get("addendum") else []),
        "`reviewed_by` of a unit (query, language file, held-out set, task) names the classes of its items: "
        f"`{plan.label('human')}` only when Marc decided every item, otherwise the class, or "
        f"`mixed, policy {plan.policy}: …` with a count per class. It stays `null` where an item is unsure, "
        "open or needs a fix by hand. `real-repo-eval` reports retrieval queries as human-reviewed, "
        "agent-verified under owner policy, or unreviewed; only `human:` provenance counts as human.",
        "",
        "| dataset | items | human | prescreen | second-check | rule | restated | wrong: fixed / by hand | "
        "unsure | open | units marked | human sample in this dataset: checked / errors |",
        "|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|",
    ]
    for o in outcomes:
        row = plan.summary.get(o.dataset, {})
        c = o.classes
        n, e = plan.sample_by_ds.get(o.dataset, (0, 0))
        restated = sum(v for k, v in c.items() if k.endswith("+restated"))
        lines.append(f"| {o.dataset} | {row.get('items', 0)} | {c.get('human', 0)} | {c.get('prescreen', 0)} | "
                     f"{c.get('second-check', 0)} | {c.get('rule', 0)} | {restated} | "
                     f"{o.wrong_fixed} / {o.wrong_manual} | {o.unsure} | {o.open} | "
                     f"{o.units_reviewed} of {o.units_total} | {n} / {e} |")
    return lines


def with_section(text: str, section: str) -> str:
    if START in text and END in text:
        head, rest = text.split(START, 1)
        tail = rest.split(END, 1)[1].lstrip("\n")
        return head + section + ("\n" + tail if tail else "")
    return text.rstrip("\n") + "\n\n" + section


# ---------------------------------------------------------------------- main

def run(plan: Plan, items: list[dict], write: bool, today: str,
        provenance_path: Path | None = None) -> tuple[list[Outcome], list[str], dict[Path, str]]:
    by_ds: dict[str, list[dict]] = {}
    for it in items:
        by_ds.setdefault(it["dataset"], []).append(it)
    writes: dict[Path, str] = {}
    outcomes: list[Outcome] = []
    for ds, path in (("retrieval-queries", export.REAL_REPOS / "queries.json"),
                     ("retrieval-test-seeking", export.REAL_REPOS / "test-seeking.json")):
        text, o = apply_retrieval(plan, ds, path, by_ds.get(ds, []))
        writes[path] = text
        outcomes.append(o)
    texts, o = apply_graph_gold(plan, by_ds.get("graph-gold", []))
    writes.update(texts)
    outcomes.append(o)
    text, o = apply_held_out(plan, by_ds.get("graph-held-out", []))
    writes[export.GRAPH_GOLD / "held-out" / "real-repos.json"] = text
    outcomes.append(o)
    for ds, filename, _ in export.TASK_FILES:
        if ds in common.NOT_APPLIED:  # exported for review only, see common.NOT_APPLIED
            continue
        path = export.AGENT / "tasks" / filename
        text, o = apply_tasks(plan, ds, path, by_ds.get(ds, []))
        writes[path] = text
        outcomes.append(o)
    changed: dict[str, str] = {}
    for o in outcomes:
        changed.update(o.changed_files)
    todo, current = baselines(changed, writes)
    plan.current_baselines = current
    readmes: dict[Path, list[Outcome]] = {}
    for o in outcomes:
        readmes.setdefault(readme_of(o.dataset), []).append(o)
    for path, outs in readmes.items():
        if not any(o.reviewed or o.accepted or o.wrong_fixed or o.wrong_manual or o.unsure for o in outs):
            continue  # nothing reviewed yet: no protocol to record
        writes[path] = with_section(path.read_text(encoding="utf-8"),
                                    protocol_section(plan, outs, [t for t in todo if _relevant(t, outs)], today,
                                                     [c for c in current if _relevant(c, outs)]))
    writes = {p: t for p, t in writes.items() if p.read_text(encoding="utf-8") != t}
    if plan.policy:
        path = provenance_path or common.REVIEW_DIR / "provenance.jsonl"
        writes[path] = "".join(json.dumps(r, ensure_ascii=False, sort_keys=True) + "\n" for r in plan.provenance)
    if write:
        for path, text in writes.items():
            common.write_atomic(path, text)
    return outcomes, todo, writes


def _relevant(todo_line: str, outs: list[Outcome]) -> bool:
    ds = {o.dataset for o in outs}
    if ds & {"retrieval-queries", "retrieval-test-seeking"}:
        return "real-repos" in todo_line or "METHODOLOGY" in todo_line
    if ds & {"graph-gold", "graph-held-out"}:
        return "graph-gold" in todo_line or "call-resolution" in todo_line
    return "evals/agent" in todo_line


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description="Apply review decisions to the datasets (#164)")
    ap.add_argument("--dry-run", action="store_true", help="print the plan, write nothing")
    ap.add_argument("--reviewer", default="marc",
                    help="a bare handle is stored as human:<handle>, as the real-repo helper does")
    ap.add_argument("--prescreen", type=Path, default=common.PRESCREEN_DIR)
    ap.add_argument("--decisions", type=Path, default=common.DECISIONS_FILE)
    ap.add_argument("--sample", type=Path, default=common.SAMPLE_FILE)
    ap.add_argument("--max-upper", type=float, default=common.ACCEPT_MAX_UPPER)
    ap.add_argument("--policy", choices=sorted(POLICIES),
                    help="owner policy for items the reviewer did not decide (see README.md)")
    ap.add_argument("--agent-check", type=Path, default=common.REVIEW_DIR / "agent-check",
                    help="second-check agent decisions (with --policy)")
    ap.add_argument("--provenance", type=Path, default=common.REVIEW_DIR / "provenance.jsonl",
                    help="per-item provenance written under --policy")
    ap.add_argument("--reset-datasets-to", metavar="REV",
                    help="start from the dataset files as of git revision REV (the reviewed state), so a "
                         "review can be applied again after its first apply; READMEs are not reset")
    args = ap.parse_args(argv)
    reviewer = args.reviewer.strip()
    if not reviewer or reviewer.lower().startswith("agent:"):
        raise SystemExit("the reviewer must be a person")
    if ":" not in reviewer:
        reviewer = f"human:{reviewer}"

    restore: dict[Path, bytes] = {}
    if args.reset_datasets_to:
        for path in dataset_files():
            rel = path.relative_to(common.REPO_ROOT).as_posix()
            old = subprocess.run(["git", "-C", str(common.REPO_ROOT), "show", f"{args.reset_datasets_to}:{rel}"],
                                 capture_output=True)
            if old.returncode:
                raise SystemExit(f"cannot read {rel} at {args.reset_datasets_to}: {old.stderr.decode().strip()}")
            restore[path] = path.read_bytes()
            path.write_bytes(old.stdout)
    try:
        return _main(args, reviewer)
    finally:
        if args.dry_run:  # a dry run leaves the files as they were
            for path, data in restore.items():
                path.write_bytes(data)


def dataset_files() -> list[Path]:
    """Every dataset file apply.py writes."""
    return ([export.REAL_REPOS / "queries.json", export.REAL_REPOS / "test-seeking.json"]
            + [export.GRAPH_GOLD / f"{lang}.json" for lang in export.GRAPH_LANGUAGES]
            + [export.GRAPH_GOLD / "held-out" / "real-repos.json"]
            + [export.AGENT / "tasks" / name for ds, name, _ in export.TASK_FILES if ds not in common.NOT_APPLIED])


def applied_items(items: list[dict]) -> list[dict]:
    """The items apply.py acts on: every dataset except ``common.NOT_APPLIED``."""
    return [it for it in items if it["dataset"] not in common.NOT_APPLIED]


def _main(args, reviewer: str) -> int:
    items = applied_items(export.build_items(snippets=False))
    by_id = {it["id"]: it for it in items}
    prescreen, warnings = common.load_prescreen(args.prescreen, by_id)
    for w in warnings[:10]:
        print(f"warning: {w}", file=sys.stderr)
    decisions = common.DecisionStore(args.decisions)
    if args.sample.exists():
        sample = common.Sample(json.loads(args.sample.read_text(encoding="utf-8")))
    else:
        sample = common.Sample({"seed": common.DEFAULT_SEED, "rate": common.SAMPLE_RATE, "datasets": {}})
    try:  # the same population check the review page makes; nothing is saved here
        sample.sync(items, prescreen, lambda rid: decisions.effective(by_id[rid]) is not None)
    except common.SampleConflict as err:
        raise SystemExit(f"error: {err}")
    stale = decisions.stale(by_id)
    if stale:
        print(f"note: {len(stale)} decisions refer to claims that changed or vanished; ignored", file=sys.stderr)
    agent = {}
    if args.policy:
        agent, agent_warnings = load_agent_decisions(args.agent_check, by_id)
        for w in agent_warnings[:20]:
            print(f"warning: {w}", file=sys.stderr)
        both = [rid for rid in agent if (decisions.effective(by_id[rid]) or {}).get("decision") in ("correct", "wrong")]
        if both:
            print(f"note: {len(both)} items have both a human and an agent decision; the human one counts",
                  file=sys.stderr)
    plan = Plan(items, prescreen, decisions, sample, reviewer, args.max_upper, args.policy, agent)
    outcomes, todo, writes = run(plan, items, write=not args.dry_run, today=time.strftime("%Y-%m-%d"),
                                 provenance_path=args.provenance)

    if args.policy:
        print(f"policy {args.policy}: Marc checked {plan.sample_checked} sampled confirmed items, "
              f"{plan.sample_errors} errors\n")
        print(f"{'dataset':<24}{'human':>7}{'prescreen':>10}{'2nd-check':>10}{'rule':>6}{'restated':>9}"
              f"{'fixed':>7}{'by hand':>8}{'unsure':>7}{'open':>6}{'sample n/err':>13}  units marked")
        for o in outcomes:
            c = o.classes
            restated = sum(v for k, v in c.items() if k.endswith("+restated"))
            n, e = plan.sample_by_ds.get(o.dataset, (0, 0))
            kept = f" ({o.units_human_only_kept} kept null: not all human)" if o.units_human_only_kept else ""
            print(f"{o.dataset:<24}{c.get('human', 0):>7}{c.get('prescreen', 0):>10}{c.get('second-check', 0):>10}"
                  f"{c.get('rule', 0):>6}{restated:>9}{o.wrong_fixed:>7}{o.wrong_manual:>8}{o.unsure:>7}{o.open:>6}"
                  f"{f'{n}/{e}':>13}  {o.units_reviewed} of {o.units_total}{kept}")
        total: dict[str, int] = {}
        for o in outcomes:
            for k, v in o.classes.items():
                total[k] = total.get(k, 0) + v
        print("total by provenance: " + ", ".join(f"{k} {total[k]}" for k in sorted(
            total, key=lambda k: (CLASS_ORDER.index(k.split("+")[0]), k))))
    else:
        print(f"{'dataset':<24}{'reviewed':>9}{'accepted':>9}{'fixed':>7}{'by hand':>8}{'unsure':>7}{'open':>7}"
              "  units reviewed")
        for o in outcomes:
            print(f"{o.dataset:<24}{o.reviewed:>9}{o.accepted:>9}{o.wrong_fixed:>7}{o.wrong_manual:>8}"
                  f"{o.unsure:>7}{o.open:>7}  {o.units_reviewed} of {o.units_total}")
    for label, ids in (("Final decision unsure (not marked reviewed)", [i for o in outcomes for i in o.unsure_ids]),
                       ("Undecided (not marked reviewed)", [i for o in outcomes for i in o.open_ids])):
        if ids and (args.policy or len(ids) <= 50):
            print(f"\n{label}: {len(ids)}")
            for i in ids:
                print(f"  - {i}")
    manual = [m for o in outcomes for m in o.manual]
    if manual:
        print("\nTo fix by hand:")
        for m in manual:
            print(f"  - {m}")
    verb = "would write" if args.dry_run else "wrote"
    print(f"\n{verb} {len(writes)} files:")
    for p in sorted(writes):
        print(f"  {export.rel(p)}")
    if plan.current_baselines:
        print("\nChecked and current:")
        for c in plan.current_baselines:
            print(f"  - {c}")
    if todo:
        print("\nBaselines to re-record (not done here):")
        for t in todo:
            print(f"  - {t}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
