"""Rank fusion and its grid (#210)."""

from __future__ import annotations

import itertools

GRID = ([{"how": "rrf", "k": k, "w": w} for k, w in itertools.product((2, 10, 30, 60), (0.25, 0.5, 0.75))]
        + [{"how": how, "w": w} for how, w in itertools.product(("max", "sum"), (0.1, 0.25, 0.4, 0.5, 0.6, 0.75))])


def ranked(scores: dict[str, float]) -> list[str]:
    """Paths by score, ties by path (so a result never depends on iteration order)."""
    return [p for p, _ in sorted(scores.items(), key=lambda kv: (-kv[1], kv[0]))]


def fuse(a: dict[str, float], b: dict[str, float], cfg: dict) -> dict[str, float]:
    """Fuse two score maps; ``w`` is the second one's share."""
    out: dict[str, float] = {}
    for scores, w in ((a, 1 - cfg["w"]), (b, cfg["w"])):
        if not scores:
            continue
        if cfg["how"] == "rrf":
            for i, p in enumerate(ranked(scores)):
                out[p] = out.get(p, 0.0) + w / (cfg["k"] + i + 1)
        else:
            top = max(scores.values()) if cfg["how"] == "max" else sum(sorted(scores.values(), reverse=True)[:10])
            for p, s in scores.items():
                out[p] = out.get(p, 0.0) + w * s / top
    return out
