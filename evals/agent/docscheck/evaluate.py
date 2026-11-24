"""Tune on dev, report dev (#210); score held-out once with the frozen configurations.

    python3 evaluate.py dev --hist HIST --work DIR --out data/dev.json       # tunes, writes the frozen configurations
    python3 evaluate.py held-out --hist HIST --work DIR --out data/held_out.json --i-have-the-review

Selection on dev: mean recall@5, ties by MRR, then grid order. Budget: text 120 configurations
(grep 48, BM25 48, fusion 24), myceliums 120 (mentions 48, keyword 24, hybrid 24, fusion 24);
text with myceliums 48 (24 with the mentions, 24 with the best myceliums method).
"""

from __future__ import annotations

import argparse
import json
import random
import sys
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))

from aeval.stats import percentile  # noqa: E402
from docscheck import change, fuse, graph, measure, search, text  # noqa: E402
from docscheck.corpus import Corpus  # noqa: E402

TOP = 100
FOLDS = 5
MARGIN = 0.10
BOOT = 10_000
SEED = 210
FLOOR = 0.50
COMPARISONS = (("text+myc", "text"), ("myc", "text"))


class Commit:
    def __init__(self, c: dict, hist: Path, work: Path) -> None:
        self.sha, self.gold, self.meta = c["sha"], c["gold"], c
        t = time.time()
        self.features = change.features(change.read_files(hist, c["parent"], c["sha"]))
        self.corpus = Corpus(hist, c["parent"])
        self.load_s = time.time() - t
        g, s = (work / "cache" / k / f"{c['sha']}.json" for k in ("graph", "search"))
        self.graph_raw = json.loads(g.read_text()) if g.exists() else None
        self.graph = graph.merge(self.graph_raw["old"], self.graph_raw["new"]) if self.graph_raw else None
        self.search = json.loads(s.read_text()) if s.exists() else None
        self.memo: dict[tuple, dict[str, float]] = {}


def _search(mode: str):
    def run(c: Commit, cfg: dict) -> dict[str, float]:
        out = search.aggregate(c.search["answers"][f"{mode}:{cfg['query']}"], cfg["agg"])
        return {p: s for p, s in out.items() if p in c.corpus.units}
    return run


BASE = {
    "grep": (lambda c, cfg: text.grep(c.corpus, c.features, cfg), text.GREP_GRID),
    "bm25": (lambda c, cfg: text.bm25(c.corpus, c.features, cfg), text.BM25_GRID),
    "mentions": (lambda c, cfg: graph.mentions(c.corpus, c.graph, cfg), graph.GRID),
    "keyword": (_search("keyword"), search.GRID),
    "hybrid": (_search("hybrid"), search.GRID),
}


def scores(c: Commit, method: str, cfg: dict) -> dict[str, float]:
    """Scores of a method (base or fusion, by its configuration) on a commit, its best TOP units."""
    k = (method, json.dumps(cfg, sort_keys=True))
    if k not in c.memo:
        if method in BASE:
            s = BASE[method][0](c, cfg)
        else:
            s = fuse.fuse(*(scores(c, p["method"], p["cfg"]) for p in cfg["parts"]), cfg["fuse"])
        c.memo[k] = {p: s[p] for p in fuse.ranked(s)[:TOP]}
    return c.memo[k]


def row(c: Commit, method: str, cfg: dict) -> dict[str, float]:
    return measure.score(fuse.ranked(scores(c, method, cfg)), c.gold)


def best(commits: list[Commit], method: str, grid: list[dict]) -> dict:
    """The configuration with the best mean recall@5 (ties: MRR, then grid order)."""
    top = None
    for cfg in grid:
        m = measure.mean([row(c, method, cfg) for c in commits])
        if top is None or (m["recall@5"], m["mrr"]) > (top[1]["recall@5"], top[1]["mrr"]):
            top = (cfg, m)
    return {"method": method, "cfg": top[0], "dev": top[1]}


def better(a: dict, b: dict) -> dict:
    """The better of two tuned methods, the first one at a tie."""
    return b if (b["dev"]["recall@5"], b["dev"]["mrr"]) > (a["dev"]["recall@5"], a["dev"]["mrr"]) else a


def fusion_grid(a: dict, b: dict) -> list[dict]:
    parts = [{"method": x["method"], "cfg": x["cfg"]} for x in (a, b)]
    return [{"parts": parts, "fuse": f} for f in fuse.GRID]


def tune(commits: list[Commit]) -> dict[str, dict]:
    """Every method's tuned configuration. ``text``, ``myc`` and ``text+myc`` are the registered contenders."""
    out = {m: best(commits, m, grid) for m, (_, grid) in BASE.items()}
    out["text-fusion"] = best(commits, "text-fusion", fusion_grid(out["grep"], out["bm25"]))
    out["myc-fusion"] = best(commits, "myc-fusion", fusion_grid(out["mentions"], better(out["keyword"], out["hybrid"])))
    out["text"] = better(better(out["bm25"], out["grep"]), out["text-fusion"])
    out["myc"] = better(better(better(out["mentions"], out["keyword"]), out["hybrid"]), out["myc-fusion"])
    out["text+mentions"] = best(commits, "text+mentions", fusion_grid(out["text"], out["mentions"]))
    out["text+myc-best"] = best(commits, "text+myc-best", fusion_grid(out["text"], out["myc"]))
    out["text+myc"] = better(out["text+mentions"], out["text+myc-best"])
    return out


def cross_validated(commits: list[Commit]) -> dict[str, list[dict[str, float]]]:
    """Per method, each commit scored with the configuration tuned on the other folds (by position)."""
    rows: dict[str, list] = {}
    for fold in range(FOLDS):
        tuned = tune([c for i, c in enumerate(commits) if i % FOLDS != fold])
        for m, t in tuned.items():
            rows.setdefault(m, [None] * len(commits))
            for i, c in enumerate(commits):
                if i % FOLDS == fold:
                    rows[m][i] = row(c, t["method"], t["cfg"])
    return rows


def bootstrap(diffs: list[float], seed: int = SEED, n: int = BOOT, alpha: float = 0.05) -> dict:
    """Mean difference, percentile interval and a two-sided p (twice the smaller tail share at 0, at least 1/n)."""
    rng = random.Random(seed)
    k = len(diffs)
    means = sorted(sum(diffs[rng.randrange(k)] for _ in range(k)) / k for _ in range(n))
    low = sum(m <= 0 for m in means) / n
    high = sum(m >= 0 for m in means) / n
    return {"diff": sum(diffs) / k, "lo": percentile(means, alpha / 2), "hi": percentile(means, 1 - alpha / 2),
            "p": max(1 / n, min(1.0, 2 * min(low, high)))}


def holm(ps: dict[str, float]) -> dict[str, float]:
    """Holm-adjusted p values."""
    out, running = {}, 0.0
    for i, (k, p) in enumerate(sorted(ps.items(), key=lambda kv: kv[1])):
        running = max(running, min(1.0, (len(ps) - i) * p))
        out[k] = running
    return out


def compare(rows: dict[str, list[dict]], metric: str = "recall@5") -> dict:
    res = {f"{a} - {b}": bootstrap([x[metric] - y[metric] for x, y in zip(rows[a], rows[b])]) for a, b in COMPARISONS}
    adj = holm({k: v["p"] for k, v in res.items()})
    for k, v in res.items():
        v["p_holm"] = adj[k]
    return res


def clearly_better(c: dict) -> bool:
    return c["diff"] >= MARGIN and c["lo"] > 0 and c["p_holm"] <= 0.05


def decide(cmp: dict, means: dict[str, dict[str, float]]) -> str:
    """The registered decision table (README)."""
    added, alone = cmp["text+myc - text"], cmp["myc - text"]
    best_m = "text+myc" if clearly_better(added) else "myc" if clearly_better(alone) else "text"
    if clearly_better(added) and clearly_better(alone) and means["myc"]["recall@5"] >= means["text+myc"]["recall@5"]:
        best_m = "myc"
    if means[best_m]["recall@5"] < FLOOR:
        return "none"
    return {"text": "grep", "myc": "myceliums", "text+myc": "text with myceliums"}[best_m]


def load(split: str, hist: Path, work: Path) -> list[Commit]:
    doc = json.loads((HERE / "data" / "commits.json").read_text())
    return [Commit(c, hist, work) for c in doc["commits"] if c["split"] == split]


def rounded(x):
    if isinstance(x, float):
        return round(x, 4)
    if isinstance(x, dict):
        return {k: rounded(v) for k, v in x.items()}
    return [rounded(v) for v in x] if isinstance(x, (list, tuple)) else x


def table(means: dict[str, dict[str, float]], order) -> str:
    cols = ("recall@1", "recall@3", "recall@5", "recall@10", "precision@5", "mrr", "shown@5", "success@5")
    lines = ["| Method | " + " | ".join(cols) + " |", "|---|" + "---:|" * len(cols)]
    for m in order:
        lines.append(f"| {m} | " + " | ".join(f"{means[m][c]:.3f}" if c != "shown@5" else f"{means[m][c]:.1f}" for c in cols) + " |")
    return "\n".join(lines)


def main() -> int:
    p = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    p.add_argument("split", choices=("dev", "held-out"))
    p.add_argument("--hist", type=Path, required=True)
    p.add_argument("--work", type=Path, required=True)
    p.add_argument("--out", type=Path, required=True)
    p.add_argument("--i-have-the-review", action="store_true")
    args = p.parse_args()
    if args.split == "held-out":
        if not args.i_have_the_review:
            raise SystemExit("held-out is scored once, after an independent review (README)")
        if args.out.exists():
            raise SystemExit(f"{args.out} exists: held-out is scored once")
    commits = load(args.split, args.hist, args.work)
    if not all(c.graph and c.search for c in commits):
        raise SystemExit("not every commit is prepared (prepare.py)")
    if args.split == "dev":
        tuned = tune(commits)
        cv = cross_validated(commits)
    else:
        tuned = json.loads((HERE / "data" / "dev.json").read_text())["configurations"]
        cv = None
    rows = {m: [row(c, t["method"], t["cfg"]) for c in commits] for m, t in tuned.items()}
    rows["plain-grep"] = [measure.score(fuse.ranked(text.plain_grep(c.corpus, c.features)), c.gold) for c in commits]
    means = {m: measure.mean(r) for m, r in rows.items()}
    comparisons = compare(cv or rows)
    out = {"split": args.split, "commits": len(commits), "configurations": tuned, "means": means,
           "comparisons": comparisons,
           "per_commit": {c.sha: {m: {"top": fuse.ranked(scores(c, t["method"], t["cfg"]))[:10],
                                      "recall@5": rows[m][i]["recall@5"]} for m, t in tuned.items()}
                          for i, c in enumerate(commits)}}
    if cv:
        out["cross_validated"] = {m: measure.mean(r) for m, r in cv.items()}
    else:
        out["decision"] = decide(comparisons, means)
    args.out.write_text(json.dumps(rounded(out), indent=1) + "\n")
    print(table(means, means))
    if cv:
        print("\ncross-validated\n" + table(out["cross_validated"], out["cross_validated"]))
    print(json.dumps(rounded(comparisons), indent=1), out.get("decision", ""))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
