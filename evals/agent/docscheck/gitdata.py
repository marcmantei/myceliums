"""Git access for the docs check (#210): a blobless clone, blobs fetched in batches.

A blobless clone fetches a missing blob on demand, one request per command.
:func:`prefetch` asks the promisor remote for many objects at once, with the
command git itself uses; nothing else here talks to the network.
"""

from __future__ import annotations

import subprocess
from pathlib import Path

BATCH = 4000


def git(hist: Path, *args: str, stdin: bytes | None = None, lazy: bool = False) -> bytes:
    """Run git in the clone. Objects that are not there are an error unless
    ``lazy``: every network access goes through :func:`prefetch`."""
    pre = [] if lazy else ["--no-lazy-fetch"]
    res = subprocess.run(["git", *pre, "-C", str(hist), *args], input=stdin, capture_output=True)
    if res.returncode != 0:
        raise RuntimeError(f"git {' '.join(args)[:200]}: {res.stderr.decode(errors='replace').strip()[:500]}")
    return res.stdout


def tree(hist: Path, rev: str, *paths: str) -> dict[str, str]:
    """``{path: blob id}`` of the files of ``rev`` (below ``paths``)."""
    out = {}
    for rec in git(hist, "ls-tree", "-r", "-z", rev, "--", *paths).split(b"\0"):
        if rec:
            meta, path = rec.split(b"\t", 1)
            mode, kind, oid = meta.split()
            if kind == b"blob" and mode != b"120000":
                out[path.decode()] = oid.decode()
    return out


def missing(hist: Path, oids) -> list[str]:
    """The objects of ``oids`` that the clone does not have."""
    req = "\n".join(sorted(set(oids))) + "\n"
    out = git(hist, "cat-file", "--batch-check=%(objectname)", stdin=req.encode())
    return [ln.split()[0].decode() for ln in out.splitlines() if ln.endswith(b" missing")]


def prefetch(hist: Path, oids) -> int:
    """Fetch the missing objects of ``oids`` from the promisor remote."""
    need = missing(hist, oids)
    for i in range(0, len(need), BATCH):
        git(hist, "-c", "fetch.negotiationAlgorithm=noop", "fetch", "origin", "--no-tags", "--no-write-fetch-head",
            "--recurse-submodules=no", "--filter=blob:none", "--stdin",
            stdin=("\n".join(need[i:i + BATCH]) + "\n").encode(), lazy=True)
    left = missing(hist, need)
    if left:
        raise RuntimeError(f"{len(left)} objects could not be fetched, e.g. {left[0]}")
    return len(need)


def blobs(hist: Path, oids) -> dict[str, bytes]:
    """Contents of present blobs."""
    want = sorted(set(oids))
    if not want:
        return {}
    out = git(hist, "cat-file", "--batch", stdin=("\n".join(want) + "\n").encode())
    res, pos = {}, 0
    while pos < len(out):
        eol = out.index(b"\n", pos)
        head = out[pos:eol].split()
        if head[1] == b"missing":
            raise RuntimeError(f"missing blob {head[0].decode()}")
        size = int(head[2])
        res[head[0].decode()] = out[eol + 1:eol + 1 + size]
        pos = eol + 1 + size + 1
    return res
