"""Fetch pinned repositories, build their myceliums indexes, validate tasks."""

from __future__ import annotations

import datetime as dt
import json
import re
import subprocess
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


# myceliums' default `[analysis] exclude`; a project config replaces it, so it is repeated.
DEFAULT_EXCLUDE = ["node_modules/**", ".git/**", "target/**", "dist/**", "build/**",
                   "__pycache__/**", ".venv/**", "venv/**"]


def index(repos: dict[str, dict], myc: str, models_dir: str, only: set[str] | None = None,
          record_only: bool = False) -> None:
    """Build one myceliums index per repository with ``myc analyze``.

    ``myc analyze`` stores its data under ``$HOME/.myceliums``; ``HOME`` is set
    to ``<cache>/index/<repo>/home`` so the user's own data directory is never
    touched. The index is built for the fixed working path the sessions use.

    With ``record_only`` an index that already exists (built by the same
    command) is only described in ``meta.json``; its ``analyze.log`` must be
    present next to it.
    """
    cache = config.cache_dir()
    for name, r in repos.items():
        if only and name not in only:
            continue
        home = cache / "index" / name / "home"
        work = config.work_root() / name
        log_path = cache / "index" / name / "analyze.log"
        took = None
        if not record_only:
            if home.exists():
                config.remove_tree(home, cache / "index")
            home.mkdir(parents=True)
            if work.exists():
                config.remove_tree(work, config.work_root())
            config.copy_tree(cache / "repos" / name, work)
            if r.get("index_exclude_extra"):
                # Index-time only: the session checkouts are fresh copies without it.
                patterns = DEFAULT_EXCLUDE + r["index_exclude_extra"]
                (work / ".myceliums.toml").write_text(
                    "[analysis]\nexclude = [" + ", ".join(json.dumps(x) for x in patterns) + "]\n")
            env = {"HOME": str(home), "FASTEMBED_CACHE_DIR": models_dir,
                   "PATH": "/usr/bin:/bin:/usr/sbin:/sbin"}
            started = time.time()
            res = subprocess.run([myc, "analyze", str(work), "--strict-embeddings"],
                                 env=env, capture_output=True, text=True)
            took = round(time.time() - started, 1)
            log_path.write_text(res.stdout + res.stderr)
            if res.returncode != 0:
                raise SystemExit(f"myc analyze failed for {name}; see {log_path}")
        log = log_path.read_text()
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
            "index_exclude_extra": r.get("index_exclude_extra", []),
            "myc_version": config.tool_version([myc, "--version"]),
            "myc_sha256": config.sha256_file(Path(myc)),
            "indexed_at": info.get("analyzed_at"),
            "index_seconds": took if took is not None else _grab_ms(log),
        }
        (cache / "index" / name / "meta.json").write_text(json.dumps(meta, indent=2) + "\n")
        print(json.dumps(meta))


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


TASK_TYPES = {"callers", "callees", "impact", "where", "dead_code", "flow"}


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
        for key in ("id", "repo", "type", "answer_kind", "question", "gold", "rationale"):
            if not t.get(key):
                problems.append(f"{tid}: missing {key}")
        if t.get("repo") not in repos:
            problems.append(f"{tid}: unknown repo {t.get('repo')}")
            continue
        if t.get("type") not in TASK_TYPES:
            problems.append(f"{tid}: unknown type {t.get('type')}")
        if t.get("answer_kind") not in ("symbols", "files"):
            problems.append(f"{tid}: answer_kind must be symbols or files")
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
