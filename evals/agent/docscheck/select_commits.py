"""Select the spring-framework commits of the docs check (#210).

    python3 -I select_commits.py HIST --out commits.json [--fetch]

HIST is a blobless clone with history. The rule looks at commits, paths, and
(rule 6) whether main code changes beyond comments; never at a method's output:

1. non-merge commits reachable from ``origin/main`` at HEAD_PIN, committer date
   on or after SINCE (the docs were split into one file per page on 2023-04-20);
2. subject not excluded by the rule of the commit replay (#183,
   ``commit_replay.EXCLUDE``: upgrades, polish, reverts, formatting, releases);
3. at most MAX_FILES files;
4. at least one file of main code (``spring-*/**/src/main/**``);
5. gold: 1 to MAX_GOLD docs units that exist at the parent and that the commit
   modifies or deletes. A unit is an AsciiDoc page or partial, or a sample file
   (``framework-docs/src/main/**``). A file git calls renamed (50 %) counts with
   its old path when its content changes, not when only the path does;
6. at least one main file changes in more than comments, imports and white space
   (``commit_replay.code_text``/``schema_text``): a docs check runs on code diffs,
   and a Javadoc-only commit is a docs edit itself;
7. of commits with the same subject (forward ports), the oldest;
8. split by committer date: before SPLIT dev, from then held-out;
9. held-out loses the 29 replayed commits of #183 (20 tasks, 9 reserves) and their
   neighbours: the same issue number, or within NEIGHBOUR_DAYS days of one and
   sharing a changed main file with it.

The committer date is compared here and not by ``git log --since``: that stops
walking at the first older commit, and on this history it drops 2,001 of 7,571 commits.
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import re
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))
sys.path.insert(0, str(HERE.parent / "groundtruth"))

from docscheck import gitdata  # noqa: E402
from commit_replay import EXCLUDE, code_text, schema_text  # noqa: E402

HEAD_PIN = "9a156f878af1b9fdc24397328794f7f79d4e1693"
SINCE = "2023-04-25"
SPLIT = "2024-06-01"
MAX_FILES = 40
MAX_GOLD = 10
NEIGHBOUR_DAYS = 14
MAIN = re.compile(r"^spring-[^/]+/(?:[^/]+/)*src/main/")
PAGE = re.compile(r"^framework-docs/modules/ROOT/(?:pages|partials)/.*\.adoc$")
SAMPLE = re.compile(r"^framework-docs/src/main/")
ISSUE = re.compile(r"(?:gh-|#|issues/)(\d{4,6})\b")


def is_main(path: str) -> bool:
    return bool(MAIN.match(path))


def is_unit(path: str) -> bool:
    return bool(PAGE.match(path) or SAMPLE.match(path))


def read_log(hist: Path) -> list[dict]:
    out = gitdata.git(hist, "log", HEAD_PIN, "--no-merges", "--no-renames", "--raw", "--no-abbrev", "-z",
                      "--format=%x01%H%x02%P%x02%cI%x02%s%x02%b%x03").decode(errors="replace")
    commits = []
    for rec in out.split("\x01")[1:]:
        head, _, raw = rec.partition("\x03")
        sha, parent, date, subject, body = head.split("\x02", 4)
        toks = [t for t in raw.strip("\0\n").split("\0") if t]
        files = []
        for meta, path in zip(toks[::2], toks[1::2]):
            _, _, old, new, st = meta.strip().lstrip(":").split()
            files.append({"status": st[0], "path": path, "old": old, "new": new})
        commits.append({"sha": sha, "parent": parent, "date": date, "subject": subject,
                        "issues": sorted(set(ISSUE.findall(subject + "\n" + body))), "files": files})
    return commits


def day(date: str) -> dt.date:
    """The committer's day in UTC."""
    return dt.datetime.fromisoformat(date).astimezone(dt.timezone.utc).date()


def by_paths(commits: list[dict]) -> tuple[list[dict], dict]:
    """Rules 1 to 4 and the path part of 5."""
    n = {"non_merge_in_range": 0, "main_and_docs_unit": 0, "subject": 0, "files": 0, "gold_paths": 0}
    keep = []
    for c in commits:
        if day(c["date"]) < dt.date.fromisoformat(SINCE):
            continue
        n["non_merge_in_range"] += 1
        touched = [f for f in c["files"] if f["status"] in "MD" and is_unit(f["path"])]
        if not touched or not any(is_main(f["path"]) for f in c["files"]):
            continue
        n["main_and_docs_unit"] += 1
        if EXCLUDE.match(c["subject"]):
            continue
        n["subject"] += 1
        if len(c["files"]) > MAX_FILES:
            continue
        n["files"] += 1
        if len(touched) > MAX_GOLD:
            continue
        n["gold_paths"] += 1
        keep.append(c)
    return keep, n


def renamed(hist: Path, sha: str) -> dict[str, tuple[str, bool]]:
    """``{old path: (new path, content changed)}`` of the files git calls renamed."""
    out = gitdata.git(hist, "show", "--format=", "--name-status", "-M50%", "--diff-filter=R", "-z", sha).decode()
    toks = out.strip("\0\n").split("\0")
    return {toks[i + 1]: (toks[i + 2], toks[i] != "R100") for i in range(0, len(toks) - 2, 3)}


def changes_code(f: dict, text: dict[str, bytes]) -> bool:
    if f["status"] != "M":
        return True
    fn = code_text if f["path"].endswith((".java", ".kt")) else schema_text if f["path"].endswith(".xsd") else None
    if fn is None:
        return f["old"] != f["new"]
    old, new = (text[f[k]].decode(errors="replace") for k in ("old", "new"))
    return fn(old) != fn(new)


def neighbours(c: dict, replayed: list[dict]) -> str | None:
    """Why ``c`` is a replayed commit or its neighbour, or None."""
    mine = {f["path"] for f in c["files"] if is_main(f["path"])}
    for r in replayed:
        if r["sha"] == c["sha"]:
            return "replayed"
        if set(r["issues"]) & set(c["issues"]):
            return f"issue of {r['sha'][:10]}"
        if abs((day(r["date"]) - day(c["date"])).days) <= NEIGHBOUR_DAYS and \
                mine & {f["path"] for f in r["files"] if is_main(f["path"])}:
            return f"main file of {r['sha'][:10]}"
    return None


def replayed_shas() -> set[str]:
    out = set()
    for name in ("tasks-commit-replay.json", "tasks-commit-replay-reserve.json"):
        doc = json.loads((HERE.parent / "tasks" / name).read_text())
        out |= {r["gold_commit"] for r in doc["repos"]}
    return out


def select(hist: Path, fetch: bool) -> dict:
    commits = read_log(hist)
    replay = replayed_shas()
    replayed = [c for c in commits if c["sha"] in replay]
    keep, n = by_paths(commits)
    null = "0" * 40
    need = {o for c in keep for f in c["files"] for o in (f["old"], f["new"]) if o != null}
    if fetch:
        n["blobs_fetched"] = gitdata.prefetch(hist, need)
    text = gitdata.blobs(hist, need)
    out, seen = [], set()
    for key in ("code_change", "gold_content", "unique_subject"):
        n[key] = 0
    for c in sorted(keep, key=lambda c: (day(c["date"]), c["sha"])):
        if not any(changes_code(f, text) for f in c["files"] if is_main(f["path"])):
            continue
        n["code_change"] += 1
        moves = renamed(hist, c["sha"])
        gold = sorted(f["path"] for f in c["files"] if f["status"] in "MD" and is_unit(f["path"])
                      and moves.get(f["path"], ("", True))[1])
        if not gold:
            continue
        n["gold_content"] += 1
        if c["subject"] in seen:
            continue
        seen.add(c["subject"])
        n["unique_subject"] += 1
        split = "dev" if day(c["date"]) < dt.date.fromisoformat(SPLIT) else "held-out"
        why = neighbours(c, replayed) if split == "held-out" else None
        out.append({"sha": c["sha"], "parent": c["parent"], "date": day(c["date"]).isoformat(),
                    "subject": c["subject"], "files": len(c["files"]),
                    "split": "excluded-183" if why else split, **({"excluded": why} if why else {}), "gold": gold})
    n["split"] = {s: sum(c["split"] == s for c in out) for s in ("dev", "held-out", "excluded-183")}
    return {"head": HEAD_PIN, "since": SINCE, "split_at": SPLIT, "max_files": MAX_FILES, "max_gold": MAX_GOLD,
            "counts": n, "commits": out}


def main() -> int:
    p = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    p.add_argument("history", type=Path)
    p.add_argument("--out", type=Path, required=True)
    p.add_argument("--fetch", action="store_true", help="fetch missing blobs from the promisor remote")
    args = p.parse_args()
    data = select(args.history, args.fetch)
    data["counts"].pop("blobs_fetched", None)
    args.out.write_text(json.dumps(data, indent=1) + "\n")
    print(json.dumps(data["counts"], indent=1))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
