"""Build what the methods need per commit (#210), once, into a cache: graph neighbourhoods and search answers.

    python3 prepare.py --hist HIST --work DIR --myc MYC --models DIR --split dev [--only graph|search] [--limit N]

One index at a time, at one shared path each (``DIR/work/spring-framework`` for the graph,
``DIR/work/docs-only`` for the search); the graph index is deleted after each commit. A
finished commit is skipped, so a killed run continues. Held-out needs ``--i-have-the-review``.
"""

from __future__ import annotations

import argparse
import json
import shutil
import subprocess
import sys
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))

from docscheck import change, gitdata, graph, search  # noqa: E402
from docscheck.corpus import Corpus  # noqa: E402

MIN_FREE_GB = 22


def env(home: Path, models: Path | None = None) -> dict:
    """Nothing of the caller's environment; HOME and data below the work directory."""
    out = {"HOME": str(home), "MYCELIUMS_DATA_DIR": str(home / ".myceliums"), "PATH": "/usr/bin:/bin", "RUST_LOG": "error"}
    if models:
        out["FASTEMBED_CACHE_DIR"] = str(models)
    return out


def checkout(hist: Path, rev: str, tree: Path, index: Path) -> None:
    """Make ``tree`` the files of ``rev``; only what differs is written."""
    tree.mkdir(parents=True, exist_ok=True)
    res = subprocess.run(["git", "--no-lazy-fetch", "-C", str(hist), f"--work-tree={tree}", "read-tree", "--reset", "-u", rev],
                         env={"PATH": "/usr/bin:/bin", "HOME": str(index.parent), "GIT_INDEX_FILE": str(index)}, capture_output=True, text=True)
    if res.returncode != 0:
        raise SystemExit(f"read-tree {rev}: {res.stderr[:500]}")


def write(root: Path, path: str, data: bytes | None) -> None:
    p = root / path
    if data is None:
        p.unlink(missing_ok=True)
    else:
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_bytes(data)


def analyze(myc: Path, tree: Path, e: dict, log: Path, *flags: str) -> float:
    t = time.time()
    with log.open("a") as out:
        res = subprocess.run([str(myc), "analyze", str(tree), "--force", "--no-git-check", "--skip-communities", *flags],
                             env=e, stdout=out, stderr=out)
    if res.returncode != 0:
        raise SystemExit(f"analyze failed, see {log}")
    return time.time() - t


def remove(path: Path, root: Path) -> None:
    """Delete a directory below ``root``; anything else is refused."""
    path, root = path.resolve(), root.resolve()
    if root not in path.parents:
        raise SystemExit(f"refusing to delete {path}")
    if path.exists():
        shutil.rmtree(path)


def graph_side(args, c: dict, files: list[dict], side: str, tree: Path) -> tuple[dict, float]:
    home, log = args.work / "graph-home", args.work / "graph.log"
    remove(home, args.work)
    home.mkdir()
    took = analyze(args.myc, tree, env(home), log, "--skip-embeddings")
    out = args.work / "export.json"
    t = time.time()
    res = subprocess.run([str(args.myc), "export", str(tree), "--format", "json", "-o", str(out)], env=env(home),
                         capture_output=True, text=True)
    if res.returncode != 0:
        raise SystemExit(f"export failed: {res.stderr[:300]}")
    nodes, edges = graph.load(out)
    out.unlink()
    g = graph.neighbourhood(nodes, edges, files, side)
    remove(home, args.work)
    g["index_s"], g["query_s"] = round(took, 1), round(time.time() - t, 1)
    return g, took


def do_graph(args, c: dict, files: list[dict], path: Path) -> None:
    """Old side on the parent, new side on the parent with the non-docs diff applied."""
    tree, idx = args.work / "work" / "spring-framework", args.work / "work" / "index-graph"
    checkout(args.hist, c["parent"], tree, idx)
    old, _ = graph_side(args, c, files, "o", tree)
    new_tree = gitdata.tree(args.hist, c["sha"])
    added = []
    try:
        for f in files:
            if f["old"] and f["old"] != f["new"]:
                write(tree, f["old"], None)
        for f in files:
            if f["new"] in new_tree:
                if not (tree / f["new"]).exists():
                    added.append(f["new"])
                write(tree, f["new"], gitdata.blobs(args.hist, [new_tree[f["new"]]])[new_tree[f["new"]]])
        new, _ = graph_side(args, c, files, "n", tree)
    finally:
        for p in added:
            write(tree, p, None)
    path.write_text(json.dumps({"sha": c["sha"], "old": old, "new": new}))


def do_search(args, c: dict, corpus: Corpus, f: dict, path: Path) -> None:
    tree, idx = args.work / "work" / "docs-only", args.work / "work" / "index-docs"
    home, log = args.work / "docshome", args.work / "search.log"
    home.mkdir(exist_ok=True)
    checkout(args.hist, c["parent"] + ":framework-docs", tree, idx)
    e = env(home, args.models)
    index_s = analyze(args.myc, tree, e, log, "--strict-embeddings")
    out = {"sha": c["sha"], "index_s": round(index_s, 1), "answers": {}, "seconds": {}}
    for name, qs in search.queries(corpus, f).items():
        for mode in ("keyword", "hybrid"):
            t = time.time()
            lists = []
            for q in qs:
                res = subprocess.run([str(args.myc), "search", q, "-r", str(tree), "-l", str(search.LIMIT), "--tests", "include",
                                      *(["--hybrid"] if mode == "hybrid" else [])], env=e, capture_output=True, text=True)
                if res.returncode != 0:
                    raise SystemExit(f"search {q!r}: {res.stderr[:500]}")
                lists.append(search.parse(res.stdout))
            out["answers"][f"{mode}:{name}"] = lists
            out["seconds"][f"{mode}:{name}"] = round(time.time() - t, 2)
    path.write_text(json.dumps(out))


def main() -> int:
    p = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    p.add_argument("--hist", type=Path, required=True)
    p.add_argument("--work", type=Path, required=True)
    p.add_argument("--myc", type=Path, required=True)
    p.add_argument("--models", type=Path, required=True)
    p.add_argument("--split", choices=("dev", "held-out"), required=True)
    p.add_argument("--only", choices=("graph", "search"), action="append")
    p.add_argument("--limit", type=int)
    p.add_argument("--i-have-the-review", action="store_true", help="held-out is prepared only after the independent review")
    args = p.parse_args()
    if args.split == "held-out" and not args.i_have_the_review:
        raise SystemExit("held-out is scored once, after an independent review (README)")
    commits = [c for c in json.loads((HERE / "data" / "commits.json").read_text())["commits"] if c["split"] == args.split]
    commits = commits[:args.limit]
    fetched = gitdata.prefetch(args.hist, {o for c in commits for rev in (c["parent"], c["sha"])
                                           for o in gitdata.tree(args.hist, rev, links=True).values()})
    if fetched:
        # A fetch per batch leaves thousands of packs; with too many, git reports present objects as missing.
        gitdata.git(args.hist, "repack", "-a", "-d", "-q")
    for what in args.only or ("graph", "search"):
        cache = args.work / "cache" / what
        cache.mkdir(parents=True, exist_ok=True)
        for c in commits:
            path = cache / f"{c['sha']}.json"
            if path.exists():
                continue
            if shutil.disk_usage(args.work).free < MIN_FREE_GB * 1e9:
                raise SystemExit(f"less than {MIN_FREE_GB} GB free")
            t = time.time()
            files = change.read_files(args.hist, c["parent"], c["sha"])
            if what == "graph":
                do_graph(args, c, files, path)
            else:
                do_search(args, c, Corpus(args.hist, c["parent"]), change.features(files), path)
            print(f"{what} {c['sha'][:10]} {time.time() - t:.1f} s", flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
