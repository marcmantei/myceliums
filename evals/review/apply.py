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
    manual: list[str] = field(default_factory=list)
    changed_files: dict[str, str] = field(default_factory=dict)  # path -> "labels" | "review"


class Plan:
    """Decisions joined with items, pre-screen and sample status."""

    def __init__(self, items, prescreen, decisions: common.DecisionStore, sample: common.Sample,
                 reviewer: str, max_upper: float = common.ACCEPT_MAX_UPPER):
        self.review = common.Review(items, prescreen, decisions, sample, max_upper)
        self.decisions = decisions
        self.summary = {row["dataset"]: row for row in self.review.summary()["datasets"]}
        self.rule = common.acceptance_rule(sample.seed, sample.rate, max_upper)
        self.reviewer = reviewer
        self.items = items

    def status(self, it: dict) -> str:
        rec = self.decisions.effective(it)
        if rec:
            return {"correct": "reviewed", "wrong": "wrong", "unsure": "unsure"}[rec["decision"]]
        ds = self.summary.get(it["dataset"], {})
        if (common.verdict_of(it["id"], self.review.prescreen) == "confirmed"
                and ds.get("status") == "accepted"):
            return "accepted"
        return "open"

    def comment(self, it: dict) -> str:
        rec = self.decisions.effective(it)
        return (rec or {}).get("comment") or ""

    def reviewed_by(self, accepted_any: bool) -> str:
        return f"{self.reviewer} (sampled acceptance, {self.rule})" if accepted_any else self.reviewer


class Unit:
    """Bookkeeping for one unit: blockers and whether the sample accepted part of it."""

    def __init__(self, plan: Plan, out: Outcome, name: str):
        self.plan, self.out, self.name = plan, out, name
        self.blocked = False
        self.accepted_any = False
        self.notes: list[str] = []

    def take(self, it: dict) -> str:
        st = self.plan.status(it)
        if st == "reviewed":
            self.out.reviewed += 1
        elif st == "accepted":
            self.out.accepted += 1
            self.accepted_any = True
        elif st == "unsure":
            self.out.unsure += 1
            self.blocked = True
        elif st == "open":
            self.out.open += 1
            self.blocked = True
        return st

    def manual(self, it: dict, why: str) -> None:
        self.out.wrong_manual += 1
        self.blocked = True
        text = free_text(self.plan.comment(it)) or "(no comment)"
        self.out.manual.append(f"{it['id']}: {why}. Comment: {text}")

    def fixed(self, note: str) -> None:
        self.out.wrong_fixed += 1
        self.notes.append(note)

    def finish(self, holder: dict, key: str = "reviewed_by") -> bool:
        """Set reviewed_by when nothing blocks; True if it changed."""
        self.out.units_total += 1
        if self.blocked:
            return False
        self.out.units_reviewed += 1
        if holder.get(key) is None:
            holder[key] = self.plan.reviewed_by(self.accepted_any)
            return True
        return False


def review_note(notes: list[str]) -> str:
    return " Human review (#164): " + "; ".join(notes) + "."


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
            q["rationale"] += review_note(unit.notes)
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
            t["rationale"] += review_note(unit.notes)
            labels_changed = True
        unit.finish(t)
    text = dump_indent(data, 1)
    if text != original:
        out.changed_files[export.rel(path)] = "labels" if labels_changed else "review"
    return text, out


# ----------------------------------------------------------------- baselines

def baselines(changed: dict[str, str]) -> list[str]:
    """What was measured on a file that changed, and how to re-record it."""
    todo = []
    rr = "crates/myceliums-benchmarks/real-repos/"
    if rr + "queries.json" in changed or rr + "test-seeking.json" in changed:
        todo.append(f"{rr}recorded-run.json — the dataset digest changed: re-run "
                    "`cargo run -p myceliums-benchmarks --bin real-repo-eval -- --record` (report-only CI)")
    if changed.get(rr + "queries.json") == "labels":
        todo.append(f"{rr}exact-vs-ann-hono.json, {rr}embedding-variants.json and the numbers in "
                    "crates/myceliums-benchmarks/benchmarks/METHODOLOGY.md (real-repository sections) "
                    "were measured on the old labels — re-measure")
    if changed.get(rr + "test-seeking.json") == "labels":
        todo.append("METHODOLOGY.md, test-code demotion section (#146): the weight was chosen on the old "
                    "test-seeking labels — re-run the demotion sweep")
    gg = [p for p in changed if p.startswith("crates/myceliums-benchmarks/graph-gold/") and "held-out" not in p]
    if gg:
        todo.append("crates/myceliums-benchmarks/graph-gold/baseline.json — REQUIRED before merge: the gate "
                    "refuses a baseline recorded against other labels; re-run "
                    "`cargo run -p myceliums-benchmarks --bin graph-eval -- --update-baseline`")
        if any(changed[p] == "labels" for p in gg):
            todo.append("docs/guides/call-resolution-limitations.md — fixture numbers measured on the old labels")
    if changed.get("crates/myceliums-benchmarks/graph-gold/held-out/real-repos.json") == "labels":
        todo.append("docs/guides/call-resolution-limitations.md — held-out numbers (printed by graph-eval) "
                    "measured on the old labels")
    for path, how in sorted(changed.items()):
        if not path.startswith("evals/agent/tasks/"):
            continue
        name = path.split("/", 2)[2]
        for meta in sorted((export.AGENT / "results").glob("*/meta.json")):
            m = json.loads(meta.read_text(encoding="utf-8"))
            tf = m.get("tasks_file") or ("tasks/tasks.json" if "tasks_sha256" in m else None)
            if tf != name:
                continue
            run = meta.parent.relative_to(common.REPO_ROOT).as_posix()
            if how == "labels":
                todo.append(f"{run} — graded against the old gold of {name}: re-grade its sessions "
                            f"and regenerate REPORT.md")
            else:
                todo.append(f"{run} — records the old sha256 of {name} (review metadata only; grading unchanged)")
    return todo


# ------------------------------------------------------------------- READMEs

def protocol_section(plan: Plan, outcomes: list[Outcome], todo: list[str], today: str) -> str:
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
    if todo:
        lines += ["", "Measured on the labels before this review (re-record before relying on them):", ""]
        lines += [f"- {t}" for t in todo]
    lines.append(END)
    return "\n".join(lines) + "\n"


def with_section(text: str, section: str) -> str:
    if START in text and END in text:
        head, rest = text.split(START, 1)
        tail = rest.split(END, 1)[1].lstrip("\n")
        return head + section + ("\n" + tail if tail else "")
    return text.rstrip("\n") + "\n\n" + section


# ---------------------------------------------------------------------- main

def run(plan: Plan, items: list[dict], write: bool, today: str) -> tuple[list[Outcome], list[str], dict[Path, str]]:
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
        path = export.AGENT / "tasks" / filename
        text, o = apply_tasks(plan, ds, path, by_ds.get(ds, []))
        writes[path] = text
        outcomes.append(o)
    changed: dict[str, str] = {}
    for o in outcomes:
        changed.update(o.changed_files)
    todo = baselines(changed)
    readmes: dict[Path, list[Outcome]] = {}
    for o in outcomes:
        readmes.setdefault(readme_of(o.dataset), []).append(o)
    for path, outs in readmes.items():
        if not any(o.reviewed or o.accepted or o.wrong_fixed or o.wrong_manual or o.unsure for o in outs):
            continue  # nothing reviewed yet: no protocol to record
        writes[path] = with_section(path.read_text(encoding="utf-8"),
                                    protocol_section(plan, outs, [t for t in todo if _relevant(t, outs)], today))
    writes = {p: t for p, t in writes.items() if p.read_text(encoding="utf-8") != t}
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
    args = ap.parse_args(argv)
    reviewer = args.reviewer.strip()
    if not reviewer or reviewer.lower().startswith("agent:"):
        raise SystemExit("the reviewer must be a person")
    if ":" not in reviewer:
        reviewer = f"human:{reviewer}"

    items = export.build_items(snippets=False)
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
    plan = Plan(items, prescreen, decisions, sample, reviewer, args.max_upper)
    outcomes, todo, writes = run(plan, items, write=not args.dry_run, today=time.strftime("%Y-%m-%d"))

    print(f"{'dataset':<24}{'reviewed':>9}{'accepted':>9}{'fixed':>7}{'by hand':>8}{'unsure':>7}{'open':>7}  units reviewed")
    for o in outcomes:
        print(f"{o.dataset:<24}{o.reviewed:>9}{o.accepted:>9}{o.wrong_fixed:>7}{o.wrong_manual:>8}"
              f"{o.unsure:>7}{o.open:>7}  {o.units_reviewed} of {o.units_total}")
    manual = [m for o in outcomes for m in o.manual]
    if manual:
        print("\nTo fix by hand:")
        for m in manual:
            print(f"  - {m}")
    verb = "would write" if args.dry_run else "wrote"
    print(f"\n{verb} {len(writes)} files:")
    for p in sorted(writes):
        print(f"  {export.rel(p)}")
    if todo:
        print("\nBaselines to re-record (not done here):")
        for t in todo:
            print(f"  - {t}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
