"""Shared pieces of the label review (#164): paths, item and pre-screen files,
the decision log, the review queue with its seeded sample, and the sample
statistics (Wilson interval, acceptance rule).

Python 3.10+ standard library only.
"""

from __future__ import annotations

import hashlib
import json
import math
import os
import random
import tempfile
import time
from pathlib import Path

REVIEW_DIR = Path(__file__).resolve().parent
REPO_ROOT = REVIEW_DIR.parent.parent
ITEMS_FILE = REVIEW_DIR / "items.jsonl"
PRESCREEN_DIR = REVIEW_DIR / "prescreen"
DECISIONS_FILE = REVIEW_DIR / "decisions.jsonl"
SAMPLE_FILE = REVIEW_DIR / "sample.json"

# Order in which datasets are exported, queued and reported.
DATASETS = (
    "retrieval-queries",
    "retrieval-test-seeking",
    "graph-gold",
    "graph-held-out",
    "agent-tasks",
    "agent-tasks-hard",
    "agent-tasks-hard-v1",
    "agent-tasks-outcome",
)

VERDICTS = ("confirmed", "doubtful", "wrong")
DECISIONS = ("correct", "wrong", "unsure")

SAMPLE_RATE = 0.15
DEFAULT_SEED = 164
ACCEPT_MAX_UPPER = 0.10
Z95 = 1.959963984540054


def acceptance_rule(seed: int, rate: float = SAMPLE_RATE, max_upper: float = ACCEPT_MAX_UPPER) -> str:
    """The rule in one line, as apply.py writes it into reviewed_by."""
    return (f"seed {seed}, {round(rate * 100)} % sample of pre-screen-confirmed items, "
            f"Wilson 95 % upper bound of the sample error rate < {round(max_upper * 100)} %")


# --------------------------------------------------------------------- files

def read_jsonl(path: Path) -> list[dict]:
    """Every well-formed JSON object line; a torn last line is skipped."""
    out = []
    if not path.exists():
        return out
    with path.open("r", encoding="utf-8") as fh:
        for raw in fh:
            raw = raw.strip()
            if not raw:
                continue
            try:
                value = json.loads(raw)
            except json.JSONDecodeError:
                continue
            if isinstance(value, dict):
                out.append(value)
    return out


def write_atomic(path: Path, text: str) -> None:
    """Write via a temporary file in the same directory and rename."""
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, tmp = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent)
    try:
        with os.fdopen(fd, "w", encoding="utf-8", newline="\n") as fh:
            fh.write(text)
            fh.flush()
            os.fsync(fh.fileno())
        os.replace(tmp, path)
    except BaseException:
        try:
            os.unlink(tmp)
        except FileNotFoundError:
            pass
        raise


def write_jsonl(path: Path, rows: list[dict]) -> None:
    write_atomic(path, "".join(json.dumps(r, ensure_ascii=False, sort_keys=True) + "\n" for r in rows))


def claim_digest(item: dict) -> str:
    """What a decision is about: dataset, id, kind, claim and label.

    Code snippets and resolved line numbers are not part of it, so a decision
    stays valid when only the evidence display changes, and goes stale when
    the claim itself changes.
    """
    core = {k: item.get(k) for k in ("dataset", "id", "kind", "claim", "label")}
    blob = json.dumps(core, ensure_ascii=False, sort_keys=True)
    return hashlib.sha256(blob.encode("utf-8")).hexdigest()[:16]


def load_items(path: Path = ITEMS_FILE) -> list[dict]:
    if not path.exists():
        raise SystemExit(f"{path} is missing: run `python3 evals/review/export.py` first")
    return read_jsonl(path)


def load_prescreen(directory: Path, items_by_id: dict[str, dict]) -> tuple[dict[str, dict], list[str]]:
    """Pre-screen records by item id, and warnings.

    Format, one JSON object per line in ``prescreen/<dataset>.jsonl``:
    ``{"id", "verdict": confirmed|doubtful|wrong, "reason", "evidence": ["file:line", ...],
    "suggested_fix"?}``. A later line for the same id replaces an earlier one.
    """
    out: dict[str, dict] = {}
    warnings: list[str] = []
    if not directory.is_dir():
        return out, warnings
    for path in sorted(directory.glob("*.jsonl")):
        for n, rec in enumerate(read_jsonl(path), 1):
            rid = rec.get("id")
            if rid not in items_by_id:
                warnings.append(f"{path.name} record {n}: unknown item id {rid!r}")
                continue
            if rec.get("verdict") not in VERDICTS:
                warnings.append(f"{path.name} record {n} ({rid}): verdict must be one of {VERDICTS}")
                continue
            if items_by_id[rid]["dataset"] != path.stem:
                warnings.append(f"{path.name} record {n} ({rid}): item belongs to "
                                f"{items_by_id[rid]['dataset']}, not {path.stem}")
            if rid in out:
                warnings.append(f"{path.name}: {rid} pre-screened twice; the later record counts")
            out[rid] = rec
    return out, warnings


def verdict_of(item_id: str, prescreen: dict[str, dict]) -> str:
    """An item without a pre-screen counts as doubtful."""
    rec = prescreen.get(item_id)
    return rec["verdict"] if rec else "doubtful"


# ----------------------------------------------------------------- decisions

class DecisionStore:
    """Append-only decision log, latest record per id wins.

    Every record is the item's full state (decision and comment), written as
    one line with a single ``write`` on an ``O_APPEND`` descriptor and fsynced,
    so a crash leaves at most one torn last line, which loading skips.
    """

    def __init__(self, path: Path = DECISIONS_FILE):
        self.path = path
        self.state: dict[str, dict] = {}
        for rec in read_jsonl(path):
            if isinstance(rec.get("id"), str):
                self.state[rec["id"]] = rec

    def get(self, item_id: str) -> dict | None:
        return self.state.get(item_id)

    def record(self, item: dict, decision: str | None = None, comment: str | None = None,
               reviewer: str = "marc") -> dict:
        if decision is not None and decision not in DECISIONS:
            raise ValueError(f"decision must be one of {DECISIONS}")
        prev = self.state.get(item["id"]) or {}
        digest = claim_digest(item)
        if prev.get("claim_digest") != digest:
            prev = {}  # a decision on an older claim does not carry over
        rec = {
            "id": item["id"],
            "dataset": item["dataset"],
            "claim_digest": digest,
            "decision": decision if decision is not None else prev.get("decision"),
            "comment": comment if comment is not None else prev.get("comment", ""),
            "reviewer": reviewer,
            "at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        }
        line = (json.dumps(rec, ensure_ascii=False, sort_keys=True) + "\n").encode("utf-8")
        self.path.parent.mkdir(parents=True, exist_ok=True)
        fd = os.open(self.path, os.O_WRONLY | os.O_APPEND | os.O_CREAT, 0o644)
        try:
            size = os.fstat(fd).st_size
            if size:
                with self.path.open("rb") as fh:
                    fh.seek(size - 1)
                    if fh.read(1) != b"\n":
                        line = b"\n" + line  # close a torn line first
            os.write(fd, line)
            os.fsync(fd)
        finally:
            os.close(fd)
        self.state[item["id"]] = rec
        return rec

    def effective(self, item: dict) -> dict | None:
        """The decision for this item's current claim, or None (none or stale)."""
        rec = self.state.get(item["id"])
        if rec and rec.get("claim_digest") == claim_digest(item) and rec.get("decision") in DECISIONS:
            return rec
        return None

    def stale(self, items_by_id: dict[str, dict]) -> list[str]:
        """Ids whose recorded decision was made on a claim that has changed (or vanished)."""
        out = []
        for rid, rec in sorted(self.state.items()):
            item = items_by_id.get(rid)
            if item is None or rec.get("claim_digest") != claim_digest(item):
                out.append(rid)
        return out


# -------------------------------------------------------------------- sample

def permutation(ids: list[str], seed: int, dataset: str) -> list[str]:
    """A seeded shuffle of the sorted ids; the sample is always a prefix of it."""
    order = sorted(ids)
    random.Random(f"{seed}/{dataset}").shuffle(order)
    return order


def sample_step(n_confirmed: int, rate: float) -> int:
    return math.ceil(n_confirmed * rate) if n_confirmed else 0


class Sample:
    """The seeded sample of pre-screen-confirmed items, per dataset.

    Stored in ``sample.json``: the seed, the rate, and per dataset the number
    of confirmed items when it was drawn, the sampled ids and every round
    (the initial draw and each widening). Widening takes the next items of
    the same seeded permutation, so the sample is reproducible from the seed.
    """

    def __init__(self, data: dict):
        self.data = data

    @property
    def seed(self) -> int:
        return self.data["seed"]

    @property
    def rate(self) -> float:
        return self.data["rate"]

    @classmethod
    def load_or_create(cls, path: Path, items: list[dict], prescreen: dict[str, dict],
                       seed: int = DEFAULT_SEED, rate: float = SAMPLE_RATE) -> "Sample":
        if path.exists():
            sample = cls(json.loads(path.read_text(encoding="utf-8")))
        else:
            sample = cls({"seed": seed, "rate": rate, "datasets": {}})
        changed = sample.sync(items, prescreen)
        if changed or not path.exists():
            sample.save(path)
        return sample

    def confirmed_by_dataset(self, items: list[dict], prescreen: dict[str, dict]) -> dict[str, list[str]]:
        out: dict[str, list[str]] = {}
        for it in items:
            if verdict_of(it["id"], prescreen) == "confirmed":
                out.setdefault(it["dataset"], []).append(it["id"])
        return out

    def sync(self, items: list[dict], prescreen: dict[str, dict]) -> bool:
        """Draw the initial sample for datasets that have none yet. True if changed."""
        changed = False
        for ds, ids in self.confirmed_by_dataset(items, prescreen).items():
            if ds in self.data["datasets"]:
                continue
            k = sample_step(len(ids), self.rate)
            chosen = permutation(ids, self.seed, ds)[:k]
            self.data["datasets"][ds] = {
                "confirmed_when_drawn": len(ids),
                "ids": chosen,
                "rounds": [{"added": len(chosen), "at": _now()}],
            }
            changed = True
        return changed

    def ids(self, dataset: str, confirmed: set[str] | None = None) -> list[str]:
        ids = self.data["datasets"].get(dataset, {}).get("ids", [])
        return [i for i in ids if confirmed is None or i in confirmed]

    def widen(self, dataset: str, confirmed_ids: list[str]) -> int:
        """Add the next step of the permutation; returns how many were added."""
        entry = self.data["datasets"].get(dataset)
        if entry is None:
            return 0
        have = set(entry["ids"])
        step = sample_step(len(confirmed_ids), self.rate)
        nxt = [i for i in permutation(confirmed_ids, self.seed, dataset) if i not in have][:step]
        if nxt:
            entry["ids"].extend(nxt)
            entry["rounds"].append({"added": len(nxt), "at": _now()})
        return len(nxt)

    def save(self, path: Path) -> None:
        write_atomic(path, json.dumps(self.data, indent=2, sort_keys=True) + "\n")


def _now() -> str:
    return time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())


# ---------------------------------------------------------------- statistics

def wilson(errors: int, n: int, z: float = Z95) -> tuple[float, float]:
    """Wilson score interval for a binomial proportion; (0, 1) for n = 0."""
    if n <= 0:
        return 0.0, 1.0
    p = errors / n
    denom = 1 + z * z / n
    centre = (p + z * z / (2 * n)) / denom
    half = z * math.sqrt(p * (1 - p) / n + z * z / (4 * n * n)) / denom
    return max(0.0, centre - half), min(1.0, centre + half)


# --------------------------------------------------------------------- queue

class Review:
    """Items + pre-screen + decisions + sample: the queue and its summary."""

    def __init__(self, items: list[dict], prescreen: dict[str, dict], decisions: DecisionStore,
                 sample: Sample, max_upper: float = ACCEPT_MAX_UPPER):
        self.items = items
        self.by_id = {it["id"]: it for it in items}
        self.prescreen = prescreen
        self.decisions = decisions
        self.sample = sample
        self.max_upper = max_upper
        self.rebuild()

    def rebuild(self) -> None:
        self.confirmed = self.sample.confirmed_by_dataset(self.items, self.prescreen)
        self.sampled: set[str] = set()
        for ds, ids in self.confirmed.items():
            self.sampled.update(self.sample.ids(ds, set(ids)))
        self.queue = [it["id"] for it in self.items
                      if verdict_of(it["id"], self.prescreen) != "confirmed" or it["id"] in self.sampled]

    def decision(self, item_id: str) -> str | None:
        rec = self.decisions.effective(self.by_id[item_id])
        return rec["decision"] if rec else None

    def first_open(self) -> int:
        for n, rid in enumerate(self.queue):
            if self.decision(rid) is None:
                return n
        return 0

    def dataset_summary(self, ds: str) -> dict:
        items = [it for it in self.items if it["dataset"] == ds]
        verdicts = {v: 0 for v in VERDICTS}
        screened = 0
        for it in items:
            verdicts[verdict_of(it["id"], self.prescreen)] += 1
            screened += it["id"] in self.prescreen
        in_queue = [rid for rid in self.queue if self.by_id[rid]["dataset"] == ds]
        decided = {d: 0 for d in DECISIONS}
        for rid in in_queue:
            d = self.decision(rid)
            if d:
                decided[d] += 1
        sample_ids = [rid for rid in self.sample.ids(ds, set(self.confirmed.get(ds, [])))]
        s_decided = [self.decision(rid) for rid in sample_ids]
        s_n = sum(d is not None for d in s_decided)
        # Unsure counts as an error: the rule must not accept what the reviewer doubted.
        s_err = sum(d in ("wrong", "unsure") for d in s_decided)
        lo, hi = wilson(s_err, s_n)
        n_confirmed = len(self.confirmed.get(ds, []))
        unsampled = n_confirmed - len(sample_ids)
        if n_confirmed == 0:
            status = "no-confirmed"
        elif s_n < len(sample_ids):
            status = "sample-open"
        elif unsampled == 0:
            status = "all-confirmed-reviewed"
        elif hi < self.max_upper:
            status = "accepted"
        else:
            status = "widen"
        nonconf = [rid for rid in in_queue if rid not in self.sampled]
        nc_dec = [self.decision(rid) for rid in nonconf]
        return {
            "dataset": ds,
            "items": len(items),
            "prescreened": screened,
            "verdicts": verdicts,
            "queue": len(in_queue),
            "reviewed": sum(decided.values()),
            "decided": decided,
            "flagged_reviewed": sum(d is not None for d in nc_dec),
            "flagged_wrong": sum(d == "wrong" for d in nc_dec),
            "confirmed": n_confirmed,
            "sample_size": len(sample_ids),
            "sample_reviewed": s_n,
            "sample_errors": s_err,
            "sample_error_rate": (s_err / s_n) if s_n else None,
            "wilson95": [lo, hi] if s_n else None,
            "unsampled_confirmed": unsampled,
            "status": status,
        }

    def summary(self) -> dict:
        present = [ds for ds in DATASETS if any(it["dataset"] == ds for it in self.items)]
        extra = sorted({it["dataset"] for it in self.items} - set(DATASETS))
        rows = [self.dataset_summary(ds) for ds in present + extra]
        return {
            "seed": self.sample.seed,
            "rate": self.sample.rate,
            "max_upper": self.max_upper,
            "rule": acceptance_rule(self.sample.seed, self.sample.rate, self.max_upper),
            "datasets": rows,
            "queue": len(self.queue),
            "reviewed": sum(self.decision(rid) is not None for rid in self.queue),
        }

    def widen_if_needed(self, ds: str) -> int:
        """Widen a dataset's sample once its current sample is decided and fails the rule."""
        if self.dataset_summary(ds)["status"] != "widen":
            return 0
        added = self.sample.widen(ds, self.confirmed.get(ds, []))
        if added:
            self.rebuild()
        return added
