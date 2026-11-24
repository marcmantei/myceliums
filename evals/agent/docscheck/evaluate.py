"""Tune on dev, report dev (#210); score held-out once with the frozen configurations.

    python3 evaluate.py dev --hist HIST --work DIR        # tunes, writes data/dev.json
    python3 evaluate.py lock                              # after dev and the README: writes data/protocol.lock.json, to be committed
    python3 evaluate.py held-out --hist HIST --work DIR --i-have-the-review

Configurations: the best mean recall@5 on dev (ties by MRR, then grid order). Contenders
(which method stands for text, myc and text+myc): the best **cross-validated** recall@5.
Budget: text 120 configurations (grep 48, BM25 48, fusion 24), myceliums 120 (mentions 48,
keyword 24, hybrid 24, fusion 24); text with myceliums 48 (24 with the mentions, 24 with the
best myceliums method).

Held-out is scored once. ``lock`` records a hash of the protocol (README), the commit lists,
the dev results and this code. ``held-out`` needs that lock committed and unchanged, writes
``data/HELD_OUT_SCORED.json`` before it reads anything of held-out, and refuses when that marker
exists in the working tree or in any commit. Neither path is an argument.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import random
import subprocess
import sys
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))

from aeval.stats import median, percentile  # noqa: E402
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
        # The same with MEMBER_OF edges as they are (#212), to show what the workaround is worth.
        self.graph_edges = (graph.merge(self.graph_raw["old"]["member_of"], self.graph_raw["new"]["member_of"])
                            if self.graph_raw and "member_of" in self.graph_raw["old"] else None)
        self.search = json.loads(s.read_text()) if s.exists() else None
        self.memo: dict[tuple, dict[str, float]] = {}


def _search(mode: str):
    def run(c: Commit, cfg: dict) -> dict[str, float]:
        lists = [hits for q in cfg["queries"] for hits in c.search["answers"][f"{mode}:{q}"]]
        out = search.aggregate(lists, cfg["agg"])
        return {p: s for p, s in out.items() if p in c.corpus.units}
    return run


BASE = {
    "grep": (lambda c, cfg: text.grep(c.corpus, c.features, cfg), text.GREP_GRID),
    "bm25": (lambda c, cfg: text.bm25(c.corpus, c.features, cfg), text.BM25_GRID),
    "mentions": (lambda c, cfg: graph.mentions(c.corpus, c.graph, cfg), graph.GRID),
    "mentions-member_of": (lambda c, cfg: graph.mentions(c.corpus, c.graph_edges, cfg), graph.GRID),
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


TEXT = ("bm25", "grep", "text-fusion")
MYC = ("mentions", "keyword", "hybrid", "myc-fusion")
BOTH = ("text+mentions", "text+myc-best")
CONTENDERS = {"text": TEXT, "myc": MYC, "text+myc": BOTH}


def clusters(commits) -> list[list[int]]:
    """Positions of commits that share an issue number (transitively) as one cluster, in order of first position."""
    parent = list(range(len(commits)))

    def find(i: int) -> int:
        while parent[i] != i:
            i = parent[i]
        return i

    by_issue: dict[str, int] = {}
    for i, c in enumerate(commits):
        for issue in c.meta.get("issues", ()):
            if issue in by_issue:
                a, b = sorted((find(i), find(by_issue[issue])))
                parent[b] = a
            else:
                by_issue[issue] = i
    out: dict[int, list[int]] = {}
    for i in range(len(commits)):
        out.setdefault(find(i), []).append(i)
    return list(out.values())


def folds(commits) -> list[list[int]]:
    """FOLDS lists of positions; a cluster is never split."""
    out: list[list[int]] = [[] for _ in range(FOLDS)]
    for k, members in enumerate(clusters(commits)):
        out[k % FOLDS] += members
    return out


def cv_rows(commits: list[Commit], tuner) -> dict[str, list[dict[str, float]]]:
    """Per method, each commit scored with what ``tuner`` chose on the other folds."""
    rows: dict[str, list] = {}
    for fold in folds(commits):
        held = set(fold)
        for m, t in tuner([c for i, c in enumerate(commits) if i not in held]).items():
            r = rows.setdefault(m, [None] * len(commits))
            for i in fold:
                r[i] = row(commits[i], t["method"], t["cfg"])
    return rows


def pick(means: dict[str, dict[str, float]], names) -> str:
    """The method with the best (cross-validated) recall@5, ties by MRR, then order."""
    return max(names, key=lambda m: (means[m]["recall@5"], means[m]["mrr"], -list(names).index(m)))


def tune(commits: list[Commit], structure: dict[str, str]) -> dict[str, dict]:
    """Every method's tuned configuration, given which methods stand for search, text and myc (as far as chosen)."""
    out = {m: best(commits, m, grid) for m, (_, grid) in BASE.items()}
    out["text-fusion"] = best(commits, "text-fusion", fusion_grid(out["grep"], out["bm25"]))
    if "search" in structure:
        out["myc-fusion"] = best(commits, "myc-fusion", fusion_grid(out["mentions"], out[structure["search"]]))
    if "text" in structure and "myc" in structure:
        text_m, myc_m = out[structure["text"]], out[structure["myc"]]
        out["text+mentions"] = best(commits, "text+mentions", fusion_grid(text_m, out["mentions"]))
        out["text+myc-best"] = best(commits, "text+myc-best", fusion_grid(text_m, myc_m))
    for name in CONTENDERS:
        if name in structure:
            out[name] = out[structure[name]]
    return out


def choose(commits: list[Commit]) -> tuple[dict[str, str], dict[str, dict[str, float]]]:
    """``(structure, cross-validated means)``: the search and the three contenders by cross-validated recall@5."""
    structure: dict[str, str] = {}
    means: dict[str, dict[str, float]] = {}
    for step in (("search", ("keyword", "hybrid")), None, ("text+myc", BOTH)):
        means = {m: measure.mean(r) for m, r in cv_rows(commits, lambda cs: tune(cs, structure)).items()}
        if step:
            structure[step[0]] = pick(means, step[1])
        else:
            structure["text"], structure["myc"] = pick(means, TEXT), pick(means, MYC)
    return structure, means


def tune_and_choose(commits: list[Commit]) -> dict[str, dict]:
    """Choice of the contenders and tuning together (inside a fold: nothing of the fold is used for either)."""
    return tune(commits, choose(commits)[0])


def bootstrap(diffs: list[float], groups: list[list[int]] | None = None, seed: int = SEED, n: int = BOOT,
              alpha: float = 0.05) -> dict:
    """Mean difference over commits, percentile interval and two-sided p (twice the smaller tail share at 0, at
    least 1/n); ``groups`` (positions) are resampled as wholes."""
    rng = random.Random(seed)
    groups = groups or [[i] for i in range(len(diffs))]
    sums = [(sum(diffs[i] for i in g), len(g)) for g in groups]
    k = len(sums)
    means = []
    for _ in range(n):
        drawn = [sums[rng.randrange(k)] for _ in range(k)]
        means.append(sum(s for s, _ in drawn) / sum(m for _, m in drawn))
    means.sort()
    low = sum(m <= 0 for m in means) / n
    high = sum(m >= 0 for m in means) / n
    mean_ = sum(diffs) / len(diffs)
    return {"diff": mean_, "lo": percentile(means, alpha / 2), "hi": percentile(means, 1 - alpha / 2),
            "p": max(1 / n, min(1.0, 2 * min(low, high))), "clusters": k,
            "sd": math.sqrt(sum((d - mean_) ** 2 for d in diffs) / max(1, len(diffs) - 1))}


def holm(ps: dict[str, float]) -> dict[str, float]:
    """Holm-adjusted p values."""
    out, running = {}, 0.0
    for i, (k, p) in enumerate(sorted(ps.items(), key=lambda kv: kv[1])):
        running = max(running, min(1.0, (len(ps) - i) * p))
        out[k] = running
    return out


def compare(rows: dict[str, list[dict]], groups: list[list[int]] | None = None, pairs=COMPARISONS) -> dict:
    """The registered comparisons (Holm over them), and whether an advantage of MARGIN is excluded."""
    res = {f"{a} - {b}": bootstrap([x["recall@5"] - y["recall@5"] for x, y in zip(rows[a], rows[b])], groups)
           for a, b in pairs}
    adj = holm({k: v["p"] for k, v in res.items()})
    for k, v in res.items():
        v["p_holm"] = adj[k]
        v["margin_excluded"] = v["hi"] < MARGIN
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


def short(path: str) -> str:
    return path.replace("framework-docs/modules/ROOT/pages/", "")


def per_commit(c: Commit, t: dict, r: dict[str, float]) -> dict:
    """The first five units and the rank of each gold unit (none: not shown within TOP)."""
    top = fuse.ranked(scores(c, t["method"], t["cfg"]))
    return {"top": [short(p) for p in top[:5]], "recall@5": r["recall@5"],
            "gold_rank": {short(g): top.index(g) + 1 if g in top else None for g in c.gold}}


LOSS = 0.05  # of recall@5 that the threshold may cost on dev's docs commits


def shown(raw: dict[str, float], threshold: float) -> list[str]:
    """The pages a step with a threshold shows: of the best five, those with at least that score."""
    return [p for p in fuse.ranked(raw)[:5] if raw[p] >= threshold]


def recall_at(commits: list[Commit], cfg: dict, threshold: float) -> float:
    return sum(len(set(shown(scores(c, "bm25", cfg), threshold)) & set(c.gold)) / len(c.gold) for c in commits) / len(commits)


def tune_threshold(commits: list[Commit], cfg: dict) -> float:
    """The largest raw BM25 score that loses at most LOSS recall@5 (candidates: the scores of the gold units)."""
    floor = recall_at(commits, cfg, 0.0) - LOSS
    cands = sorted({scores(c, "bm25", cfg)[p] for c in commits for p in c.gold if p in scores(c, "bm25", cfg)}, reverse=True)
    return next((t for t in cands if recall_at(commits, cfg, t) >= floor - 1e-12), 0.0)


def wilson(k: int, n: int, z: float = 1.96) -> list[float]:
    if not n:
        return [0.0, 1.0]
    p, d = k / n, 1 + z * z / n
    mid, half = (p + z * z / (2 * n)) / d, z * math.sqrt(p * (1 - p) / n + z * z / (4 * n * n)) / d
    return [mid - half, mid + half]


def alarm_report(quiet: list[Commit], docs: list[Commit], cfg: dict, threshold: float) -> dict:
    """Alarm rate and pages shown on commits without docs changes (all, code, comment-only), and recall@5 on the docs commits."""
    out: dict = {"threshold": threshold, "recall@5": recall_at(docs, cfg, threshold), "recall@5_without": recall_at(docs, cfg, 0.0)}
    docs_shown = [shown(scores(c, "bm25", cfg), threshold) for c in docs]
    out["docs_commits_silent"] = sum(not s for s in docs_shown)
    out["docs_pages_shown_per_commit"] = sum(map(len, docs_shown)) / len(docs_shown)
    for name, part in (("all", quiet), ("code", [c for c in quiet if not c.meta["comment_only"]]),
                       ("comment_only", [c for c in quiet if c.meta["comment_only"]])):
        pages = [len(shown(scores(c, "bm25", cfg), threshold)) for c in part]
        k = sum(n > 0 for n in pages)
        out[name] = {"commits": len(part), "alarms": k, "alarm_rate": k / len(part) if part else None,
                     "interval": wilson(k, len(part)), "pages_per_100": 100 * sum(pages) / len(part) if part else None}
    return out


DATA = HERE / "data"
MARKER, LOCK, RESULT = (DATA / n for n in ("HELD_OUT_SCORED.json", "protocol.lock.json", "held_out.json"))


def protocol_key() -> dict:
    """Hashes of what held-out is scored with: the protocol text, the lists, dev's results, the code."""
    readme = (HERE / "README.md").read_text()
    parts = {"README.md#protocol": readme[readme.index("## Protocol"):readme.index("## Result on dev")].encode()}
    for p in [DATA / n for n in ("commits.json", "alarms.json", "dev.json")] + sorted(HERE.glob("*.py")):
        parts[str(p.relative_to(HERE))] = p.read_bytes()
    files = {k: hashlib.sha256(v).hexdigest() for k, v in sorted(parts.items())}
    return {"key": hashlib.sha256(json.dumps(files, sort_keys=True).encode()).hexdigest(), "files": files}


def _git(*args: str) -> str:
    return subprocess.run(["git", "-C", str(HERE), *args], capture_output=True, text=True).stdout


def refuse_second_scoring() -> None:
    if MARKER.exists() or RESULT.exists():
        raise SystemExit(f"{MARKER.name} or {RESULT.name} exists: held-out is scored once")
    if _git("log", "--all", "--format=%h", "-1", "--", str(MARKER), str(RESULT)).strip():
        raise SystemExit(f"a commit has {MARKER.name} or {RESULT.name}: held-out is scored once")


def take_the_marker() -> dict:
    """Check the lock and take the one scoring, before anything of held-out is read."""
    refuse_second_scoring()
    key = protocol_key()
    committed = _git("show", f"HEAD:./data/{LOCK.name}")
    if not committed or json.loads(committed)["key"] != key["key"]:
        raise SystemExit("the committed protocol lock is missing or not the one of these files (evaluate.py lock, commit, review)")
    with MARKER.open("x") as f:
        f.write(json.dumps({"key": key["key"], "commit": _git("rev-parse", "HEAD").strip()}, indent=1) + "\n")
    return key


def costs(commits: list[Commit], tuned: dict[str, dict]) -> dict[str, float]:
    """Medians per commit in seconds: reading the diff and the docs, then each text method; the two graph indexes
    with their export and neighbourhood; the search index derived at the same path and the tuned searches."""
    def med(xs) -> float:
        return round(median(list(xs)), 2)
    out = {"text_read": med(c.load_s for c in commits)}
    for m in ("grep", "bm25"):
        ts = []
        for c in commits:
            t = time.time()
            BASE[m][0](c, tuned[m]["cfg"])
            ts.append(time.time() - t)
        out[f"text_{m}"] = med(ts)
    for part in ("index_s", "query_s"):
        out[f"graph_{part[:-2]}"] = med(c.graph_raw["old"][part] + c.graph_raw["new"][part] for c in commits)
    out["search_index"] = med(c.search["index_s"] for c in commits)
    for m in ("keyword", "hybrid"):
        qs = tuned[m]["cfg"]["queries"]
        out[f"search_{m}"] = med(sum(c.search["seconds"][f"{m}:{q}"] for q in qs) for c in commits)
    return out


def load(split: str, hist: Path, work: Path) -> list[Commit]:
    if split == "held-out" and not MARKER.exists():
        raise SystemExit("held-out is read only by the one scoring (take_the_marker)")
    doc = json.loads((DATA / "commits.json").read_text())
    return [Commit(c, hist, work) for c in doc["commits"] if c["split"] == split]


def load_alarms(split: str, hist: Path, work: Path) -> list[Commit]:
    if split == "held-out" and not MARKER.exists():
        raise SystemExit("held-out is read only by the one scoring (take_the_marker)")
    doc = json.loads((DATA / "alarms.json").read_text())
    return [Commit({**c, "gold": []}, hist, work) for c in doc["commits"] if c["split"] == split]


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
    p.add_argument("what", choices=("dev", "lock", "held-out"))
    p.add_argument("--hist", type=Path)
    p.add_argument("--work", type=Path)
    p.add_argument("--i-have-the-review", action="store_true")
    args = p.parse_args()
    if args.what == "lock":
        refuse_second_scoring()
        LOCK.write_text(json.dumps(protocol_key(), indent=1) + "\n")
        return 0
    if args.what == "held-out":
        if not args.i_have_the_review:
            raise SystemExit("held-out is scored once, after an independent review (README)")
        dev = json.loads((DATA / "dev.json").read_text())
        take_the_marker()
    commits = load(args.what, args.hist, args.work)
    if not all(c.graph and c.search for c in commits):
        raise SystemExit("not every commit is prepared (prepare.py)")
    quiet = load_alarms(args.what, args.hist, args.work)
    groups = clusters(commits)
    out: dict = {"split": args.what, "commits": len(commits), "clusters": len(groups)}
    if args.what == "dev":
        structure, _ = choose(commits)
        tuned = tune(commits, structure)
        threshold = tune_threshold(commits, tuned["bm25"]["cfg"])
        # Choice and tuning inside each fold; the registered comparisons, and against each text method.
        nested = cv_rows(commits, tune_and_choose)
        out["structure"] = structure
        out["cross_validated"] = {m: measure.mean(r) for m, r in nested.items()}
        out["comparisons_cross_validated"] = compare(nested, groups)
        out["against_each_text_cross_validated"] = compare(
            nested, groups, [(a, b) for a in ("text+myc", "myc") for b in ("bm25", "text-fusion", "grep")])
    else:
        structure, tuned, threshold = dev["structure"], dev["configurations"], dev["alarms"]["threshold"]
    rows = {m: [row(c, t["method"], t["cfg"]) for c in commits] for m, t in tuned.items()}
    rows["plain-grep"] = [measure.score(fuse.ranked(text.plain_grep(c.corpus, c.features)), c.gold) for c in commits]
    means = {m: measure.mean(r) for m, r in rows.items()}
    out.update({"structure": structure, "configurations": tuned, "means": means, "comparisons": compare(rows, groups),
                "alarms": alarm_report(quiet, commits, tuned["bm25"]["cfg"], threshold),
                "seconds": costs(commits, tuned),
                "per_commit": {c.sha: {m: per_commit(c, t, rows[m][i]) for m, t in tuned.items()}
                               for i, c in enumerate(commits)},
                "alarm_pages": {c.sha: [short(p) for p in shown(scores(c, "bm25", tuned["bm25"]["cfg"]), threshold)]
                                for c in quiet}})
    if args.what == "held-out":
        out["decision"] = decide(out["comparisons"], means)
    path = DATA / "dev.json" if args.what == "dev" else RESULT
    # Configurations are kept exactly: held-out uses them as written.
    path.write_text(json.dumps({**rounded({k: v for k, v in out.items() if k not in ("configurations", "alarms")}),
                                "configurations": tuned, "alarms": out["alarms"]}, indent=1) + "\n")
    print(structure)
    print(table(means, means))
    for k in ("cross_validated",):
        if k in out:
            print("\ncross-validated (choice and tuning)\n" + table(out[k], out[k]))
    for k in ("comparisons_cross_validated", "against_each_text_cross_validated", "comparisons"):
        if k in out:
            print(k, json.dumps(rounded(out[k])))
    print("alarms", json.dumps(rounded(out["alarms"])))
    print(out.get("decision", ""))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
