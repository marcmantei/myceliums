"""Fetch pinned repositories, build their myceliums indexes, validate tasks."""

from __future__ import annotations

import datetime as dt
import json
import re
import subprocess
import sys
import time
from pathlib import Path

from . import config


def _git(*args: str, cwd: Path | None = None) -> str:
    res = subprocess.run(["git", *args], cwd=cwd, capture_output=True, text=True)
    if res.returncode != 0:
        raise SystemExit(f"git {' '.join(args)} failed: {res.stderr.strip()}")
    return res.stdout.strip()


def fetch(repos: dict[str, dict]) -> None:
    """Shallow-fetch each pinned commit into ``<cache>/repos/<name>``."""
    root = config.cache_dir() / "repos"
    root.mkdir(parents=True, exist_ok=True)
    for name, r in repos.items():
        dst = root / name
        if not (dst / ".git").exists():
            _git("init", "-q", str(dst))
        head = subprocess.run(["git", "-C", str(dst), "rev-parse", "HEAD"],
                              capture_output=True, text=True).stdout.strip()
        if head != r["commit"]:
            _git("-C", str(dst), "fetch", "-q", "--depth", "1", r["url"], r["commit"])
            _git("-C", str(dst), "-c", "advice.detachedHead=false", "checkout", "-q", r["commit"])
        status = _git("-C", str(dst), "status", "--porcelain")
        if status:
            raise SystemExit(f"{dst} has local changes; delete it and fetch again")
        print(f"{name:<8} {r['commit'][:12]} ({r.get('ref', '')}) -> {dst}")
        if r.get("gold_commit"):
            fetch_gold(r)


def fetch_gold(r: dict) -> Path:
    """Keep the files of a repository's ``gold_commit`` (#183: the replayed
    commit, whose parent is the pin) in ``config.gold_dir``, never in the
    checkout: the working copies hold the parent only, so the agent cannot
    find the commit in the history. ``gold.json`` records the commit's
    name-status against the pin (renames as delete + add); ``files/`` holds
    every added or modified file."""
    out = config.gold_dir(r["name"])
    meta = out / "gold.json"
    if meta.exists() and config.load_json(meta).get("gold_commit") == r["gold_commit"]:
        return out
    tmp = out.with_name(out.name + ".git")
    for d in (out, tmp):
        if d.exists():
            config.remove_tree(d, config.cache_dir() / "gold")
    _git("init", "-q", "--bare", str(tmp))
    _git("-C", str(tmp), "fetch", "-q", "--depth", "2", r["url"], r["gold_commit"])
    parent = _git("-C", str(tmp), "rev-parse", f"{r['gold_commit']}^")
    if parent != r["commit"]:
        raise SystemExit(f"{r['name']}: parent of {r['gold_commit'][:12]} is {parent[:12]}, not the pin {r['commit'][:12]}")
    files = {}
    for line in _git("-C", str(tmp), "diff", "--name-status", "--no-renames", r["commit"], r["gold_commit"]).splitlines():
        status, path = line.split("\t", 1)
        files[path] = status[0]
        if status[0] != "D":
            blob = subprocess.run(["git", "-C", str(tmp), "show", f"{r['gold_commit']}:{path}"], capture_output=True)
            if blob.returncode != 0:
                raise SystemExit(f"{r['name']}: cannot read {path} at {r['gold_commit'][:12]}")
            (out / "files" / path).parent.mkdir(parents=True, exist_ok=True)
            (out / "files" / path).write_bytes(blob.stdout)
    config.remove_tree(tmp, config.cache_dir() / "gold")
    meta.write_text(json.dumps({"gold_commit": r["gold_commit"], "parent": r["commit"], "files": files},
                               indent=1, sort_keys=True) + "\n")
    print(f"{r['name']:<8} gold {r['gold_commit'][:12]}: {len(files)} files -> {out}")
    return out


# myceliums' default `[analysis] exclude`; a project config replaces it, so it is repeated.
DEFAULT_EXCLUDE = ["node_modules/**", ".git/**", "target/**", "dist/**", "build/**",
                   "__pycache__/**", ".venv/**", "venv/**"]


def index(repos: dict[str, dict], myc: str, models_dir: str, only: set[str] | None = None,
          record_only: bool = False, default_config: set[str] | None = None,
          skip_embeddings: bool = False, base: Path | None = None, record_difference: bool = False) -> None:
    """Build one myceliums index per repository with ``myc analyze``.

    ``myc analyze`` stores its data under ``$HOME/.myceliums``; ``HOME`` is set
    to ``<cache>/index/<repo>/home`` so the user's own data directory is never
    touched. The index is built for the fixed working path the sessions use.

    With ``record_only`` an index that already exists (built by the same
    command) is only described in ``meta.json``; its ``analyze.log`` must be
    present next to it.

    Repositories in ``default_config`` are indexed with myceliums' default
    configuration, without their ``index_exclude_extra`` (#171: the Django
    index with ``tests/``); ``meta.json`` records what was excluded.

    ``skip_embeddings`` (#182) builds the graph without vectors, for pipeline
    checks against the mock API only; ``meta.json`` says so and ``run``
    refuses such an index unless the run uses the mock API.
    """
    cache = config.cache_dir()
    order = list(repos.items())
    if base is not None:
        if skip_embeddings or record_only:
            raise SystemExit("--base needs an embedded build")
        # Newest parent first: each index is derived from its neighbour's.
        order.sort(key=lambda kv: kv[1].get("parent_date", ""), reverse=True)
    for name, r in order:
        if only and name not in only:
            continue
        derived = None
        home = cache / "index" / name / "home"
        work = config.work_root() / config.work_name(r)
        log_path = cache / "index" / name / "analyze.log"
        took = None
        extra = [] if name in (default_config or set()) else r.get("index_exclude_extra", [])
        if not record_only:
            if base is not None:
                check_base(base, config.work_root() / config.work_name(r))
                if home.resolve() == base.resolve():
                    raise SystemExit(f"{name}: --base is this repository's own index")
            if home.exists():
                config.remove_tree(home, cache / "index")
            if base is not None:
                config.copy_tree(base, home)
            else:
                home.mkdir(parents=True)
            if work.exists():
                config.remove_tree(work, config.work_root())
            config.copy_tree(cache / "repos" / name, work)
            if extra:
                # Index-time only: the session checkouts are fresh copies without it.
                patterns = DEFAULT_EXCLUDE + extra
                (work / ".myceliums.toml").write_text(
                    "[analysis]\nexclude = [" + ", ".join(json.dumps(x) for x in patterns) + "]\n")
            env = {"HOME": str(home), "MYCELIUMS_DATA_DIR": str(home / ".myceliums"),
                   "FASTEMBED_CACHE_DIR": models_dir, "PATH": "/usr/bin:/bin:/usr/sbin:/sbin"}
            started = time.time()
            flag = "--skip-embeddings" if skip_embeddings else "--strict-embeddings"
            res = subprocess.run([*TIME, myc, "analyze", str(work), flag] + (["--force"] if base is not None else []),
                                 env=env, capture_output=True, text=True)
            took = round(time.time() - started, 1)
            log_path.write_text(res.stdout + res.stderr)
            if res.returncode != 0:
                raise SystemExit(f"myc analyze failed for {name}; see {log_path}")
        log = log_path.read_text()
        if base is not None:
            derived = verify_derived(log, name, work, myc, env, cache, record_difference)
            derived["base"] = str(base)
            base = home
        if base is not None:
            strip_history(home / ".myceliums")
        registry = config.load_json(home / ".myceliums" / "repos.json")
        (info,) = registry["repos"].values()
        if info.get("analyzed_commit") != r["commit"]:
            raise SystemExit(f"{name}: indexed commit {info.get('analyzed_commit')} != {r['commit']}")
        meta = {
            "repo": name,
            "commit": r["commit"],
            "work_path": str(work),
            "repo_id": info["id"],
            "symbols": info["symbol_count"],
            "files": info["file_count"],
            "relationships": _grab(log, r"Relationships:\s+([\d,]+)"),
            "embedded": _grab(log, r"Embeddings:\s+([\d,]+)"),
            "communities": _grab(log, r"Communities:\s+([\d,]+)"),
            "processes": _grab(log, r"Processes:\s+([\d,]+)"),
            "index_exclude_extra": extra,
            **({"index_config": "default"} if name in (default_config or set()) else {}),
            **({"embeddings": "skipped"} if skip_embeddings else {}),
            **({"derived": derived} if derived else {}),
            "myc_version": config.tool_version([myc, "--version"]),
            "myc_sha256": config.sha256_file(Path(myc)),
            "indexed_at": info.get("analyzed_at"),
            **measured(log, home),
            "index_seconds": took if took is not None else _grab_ms(log),
        }
        (cache / "index" / name / "meta.json").write_text(json.dumps(meta, indent=2) + "\n")
        print(json.dumps(meta))


def check_base(base: Path, work: Path) -> None:
    """An index hands its vectors on only to an index of the same path (the
    store filters by repository id, which hashes the path); at another path
    ``myc`` would re-embed everything without saying so."""
    (info,) = config.load_json(base / ".myceliums" / "repos.json")["repos"].values()
    if Path(info["path"]) != Path(str(work.parent.resolve() / work.name)):
        raise SystemExit(f"base index is of {info['path']}, not {work}; its vectors would not be reused "
                         f"(set AEVAL_WORK_ROOT to the base's work root)")


GRAPH_COUNTS = {"symbols": r"Symbols:\s+([\d,]+)", "files": r"Files:\s+([\d,]+)",
                "relationships": r"Relationships:\s+([\d,]+)", "mentions": r"Mentions:\s+([\d,]+)"}


def graph_counts(log: str) -> dict:
    return {k: _grab(log, p) for k, p in GRAPH_COUNTS.items()}


# Peak memory of `myc analyze` (macOS; its report goes to stderr, so into analyze.log).
TIME = ["/usr/bin/time", "-l"] if Path("/usr/bin/time").exists() and sys.platform == "darwin" else []


def measured(log: str, home: Path) -> dict:
    """What the log and the disk say about one build: seconds per phase, peak
    memory, size (all files of the index's home)."""
    out: dict = {"disk_bytes": sum(f.stat().st_size for f in home.rglob("*") if f.is_file() and not f.is_symlink())}
    if m := re.search(r"Phases:\s+(.+)", log):
        out["phases_s"] = {k: float(v) for k, v in re.findall(r"(\w+) ([\d.]+) s", m.group(1))}
    if m := re.search(r"(\d+)\s+maximum resident set size", log):
        out["peak_rss_bytes"] = int(m.group(1))
    return out


def strip_history(data: Path) -> None:
    """Remove from an index's data directory what it keeps of *earlier*
    analyses: the graph snapshots of ``myc diff`` (ten, about 190 MB each for
    Spring). An index derived from a newer commit's would otherwise hold that
    commit's symbols, which a session on an older parent must not see."""
    for d in (data / "snapshots",):
        if d.exists():
            config.remove_tree(d, data)


def verify_derived(log: str, name: str, work: Path, myc: str, env: dict, cache: Path,
                   record_difference: bool = False) -> dict:
    """What a derived index reused, and its graph counts against a fresh
    graph-only build of the same working copy (stops on no reuse)."""
    reused = _grab(log, r"([\d,]+) kept from the previous index")
    if not reused:
        raise SystemExit(f"{name}: no vector of the base index was reused")
    tmp = cache / "tmp" / f"graph-{name}"
    if tmp.exists():
        config.remove_tree(tmp, cache / "tmp")
    tmp.mkdir(parents=True)
    try:
        res = subprocess.run([myc, "analyze", str(work), "--skip-embeddings"], capture_output=True, text=True,
                             env={**env, "HOME": str(tmp), "MYCELIUMS_DATA_DIR": str(tmp / ".myceliums")})
        if res.returncode != 0:
            raise SystemExit(f"{name}: fresh graph-only build failed: {res.stderr[-300:]}")
        fresh = graph_counts(res.stdout + res.stderr)
    finally:
        config.remove_tree(tmp, cache / "tmp")
    got = graph_counts(log)
    if got != fresh and not record_difference:
        raise SystemExit(f"{name}: the derived graph differs from a fresh one: {got} against {fresh}")
    embedded = _grab(log, r"Embeddings:\s+([\d,]+)") or 0
    return {"vectors_reused": reused, "vectors_embedded_new": embedded - reused, "graph_only_phases_s":
            measured(res.stdout + res.stderr, tmp).get("phases_s"), "counts": got, "fresh_graph_counts": fresh, "graph_equal": got == fresh}


def _grab(text: str, pattern: str) -> int | None:
    m = re.search(pattern, text)
    return int(m.group(1).replace(",", "")) if m else None


def _grab_ms(text: str) -> float | None:
    m = re.search(r"Total time:\s+([\d.]+)ms", text)
    return round(float(m.group(1)) / 1000, 1) if m else None


def index_meta() -> dict[str, dict]:
    out = {}
    for meta in sorted((config.cache_dir() / "index").glob("*/meta.json")):
        d = config.load_json(meta)
        out[d["repo"]] = d
    return out


TASK_TYPES = {"callers", "callees", "impact", "where", "dead_code", "flow",
              # multi-hop types of the hard set (#125)
              "transitive_callers", "blast_radius", "ambiguous", "tests",
              # questions that are not about calls (#163)
              "config",
              # changes graded by the files they touch and a test command (#178)
              "edit",
              # types of the large Java set (#182): questions without a
              # symbol name, implementations reached through an interface, classes named in
              # strings and config files, documentation to update for a change
              "concept", "dispatch", "by_name", "docs"}


def check_tasks(tasks: list[dict], repos: dict[str, dict], check_evidence: bool = True) -> list[str]:
    """Schema problems, plus evidence lines that no longer say what they claim.

    Every gold item carries evidence ``{"file", "line", "contains"}`` taken from
    the pinned checkout. If a repository pin is bumped without relabelling, the
    evidence check is what fails.
    """
    problems = []
    ids = set()
    root = config.cache_dir() / "repos"
    for t in tasks:
        tid = t.get("id", "?")
        if tid in ids:
            problems.append(f"{tid}: duplicate id")
        ids.add(tid)
        if t.get("answer_kind") == "diff":
            problems.extend(_check_edit_task(t, repos, root if check_evidence else None))
            continue
        for key in ("id", "repo", "type", "answer_kind", "question", "gold", "rationale"):
            if not t.get(key):
                problems.append(f"{tid}: missing {key}")
        if t.get("repo") not in repos:
            problems.append(f"{tid}: unknown repo {t.get('repo')}")
            continue
        if t.get("type") not in TASK_TYPES:
            problems.append(f"{tid}: unknown type {t.get('type')}")
        if t.get("answer_kind") not in ("symbols", "names", "files"):
            problems.append(f"{tid}: answer_kind must be symbols, names or files")
        names = [g.get("name") for g in t.get("gold", [])]
        if len(names) != len(set(names)):
            problems.append(f"{tid}: duplicate gold names")
        for g in t.get("gold", []):
            if not g.get("evidence"):
                problems.append(f"{tid}/{g.get('name')}: no evidence")
            if not check_evidence:
                continue
            for ev in g.get("evidence", []):
                path = root / t["repo"] / ev["file"]
                if not path.exists():
                    problems.append(f"{tid}/{g['name']}: {ev['file']} missing in pinned checkout")
                    continue
                lines = path.read_text(encoding="utf-8", errors="replace").splitlines()
                line = ev.get("line")
                if line is not None:
                    if not (1 <= line <= len(lines)) or ev["contains"] not in lines[line - 1]:
                        problems.append(f"{tid}/{g['name']}: {ev['file']}:{line} does not contain "
                                        f"{ev['contains']!r}")
                elif not any(ev["contains"] in ln for ln in lines):
                    problems.append(f"{tid}/{g['name']}: {ev['file']} does not contain {ev['contains']!r}")
    return problems


def _check_edit_task(t: dict, repos: dict[str, dict], root: Path | None) -> list[str]:
    """Schema of an edit task (#178, aeval/editgrade.py); with ``root``, the
    gold files must match the pinned checkout (modified and deleted files
    exist, added files do not)."""
    tid = t.get("id", "?")
    problems = []
    for key in ("id", "repo", "type", "question", "gold_files", "rationale"):
        if not t.get(key):
            problems.append(f"{tid}: missing {key}")
    if t.get("type") != "edit":
        problems.append(f"{tid}: answer_kind diff needs type edit")
    if t.get("repo") not in repos:
        return problems + [f"{tid}: unknown repo {t.get('repo')}"]
    paths = []
    for g in t.get("gold_files") or []:
        path, status = (g, "M") if isinstance(g, str) else (g.get("path"), g.get("status", "M"))
        if not path or status not in ("M", "A", "D"):
            problems.append(f"{tid}: bad gold file {g!r}")
            continue
        paths.append(path)
        if root is not None and (root / t["repo"] / path).exists() != (status != "A"):
            problems.append(f"{tid}: gold file {path} (status {status}) does not match the pinned checkout")
    if len(paths) != len(set(paths)):
        problems.append(f"{tid}: duplicate gold files")
    for p in t.get("primary_files") or []:
        if p not in paths:
            problems.append(f"{tid}: primary file {p} is not a gold file")
    if paths and t.get("primary_files") is not None and set(t["primary_files"]) >= set(paths):
        problems.append(f"{tid}: primary_files cover every gold file; leave out the dependents")
    if "dependents" in t:
        problems.append(f"{tid}: `dependents` is replaced by `primary_files` (every other gold file is a dependent)")
    repo = repos[t["repo"]]
    overlay = t.get("test_overlay")
    if overlay is not None:
        from .editgrade import file_kind
        if not repo.get("gold_commit"):
            problems.append(f"{tid}: test_overlay needs a gold_commit on repo {t['repo']}")
        for p in overlay if isinstance(overlay, list) else [overlay]:
            if p not in paths:
                problems.append(f"{tid}: overlay file {p} is not a gold file")
            elif file_kind(p) != "tests" and p not in (t.get("build_overlay") or []):
                problems.append(f"{tid}: overlay file {p} is not a test file")
    if root is not None and repo.get("gold_commit"):
        meta = config.gold_dir(t["repo"]) / "gold.json"
        if not meta.exists() or config.load_json(meta).get("gold_commit") != repo["gold_commit"]:
            problems.append(f"{tid}: gold files of {repo['gold_commit'][:12]} not fetched; run `agent_eval.py fetch`")
        else:
            got = config.load_json(meta)["files"]
            want = {(g if isinstance(g, str) else g["path"]): ("M" if isinstance(g, str) else g.get("status", "M"))
                    for g in t.get("gold_files") or []}
            if got != want:
                problems.append(f"{tid}: gold_files differ from the commit's own changes: "
                                f"{sorted(set(got.items()) ^ set(want.items()))[:6]}")
    if "test_command" in t:
        if not isinstance(t["test_command"], str) or not t["test_command"].strip():
            problems.append(f"{tid}: test_command must be a non-empty string")
        pre = t.get("precheck") or {}
        if pre.get("command") != t.get("test_command") or pre.get("commit") != repo["commit"]:
            problems.append(f"{tid}: no precheck of this test command on the pinned commit; run "
                            f"`agent_eval.py precheck-edit`")
        elif not pre.get("fails_on_parent"):
            problems.append(f"{tid}: the test command does not fail on the pinned (parent) commit, so passing it "
                            f"proves nothing")
        elif repo.get("gold_commit") and (pre.get("gold_commit") != repo["gold_commit"] or not pre.get("passes_on_gold")):
            problems.append(f"{tid}: the test command was not shown to pass on the gold commit "
                            f"{repo['gold_commit'][:12]}; run `agent_eval.py precheck-edit`")
        elif (pre.get("test_overlay") or None) != (t.get("test_overlay") or None):
            problems.append(f"{tid}: the precheck ran with another test_overlay; run `agent_eval.py precheck-edit`")
    for group in t.get("equivalents") or []:
        if len(group) < 2 or not set(group) <= set(paths):
            problems.append(f"{tid}: equivalents must be groups of gold files: {group}")
        elif set(group) & set(t.get("primary_files") or []):
            problems.append(f"{tid}: a primary file in equivalents would make its group never forgotten; use `either`")
    for f in t.get("either") or []:
        if f not in paths or f in (t.get("primary_files") or []):
            problems.append(f"{tid}: either must name dependents: {f}")
    for a in t.get("allowed_commands") or []:
        if isinstance(a, str) and a.split():
            continue
        if isinstance(a, dict) and set(a) == {"prefix", "args"} and str(a["prefix"]).split():
            try:
                re.compile(str(a["args"]))
                continue
            except re.error:
                pass
        problems.append(f"{tid}: bad allowed command {a!r} (a command, or {{\"prefix\": ..., \"args\": REGEX}})")
    return problems
