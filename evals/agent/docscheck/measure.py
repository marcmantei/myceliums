"""Metrics of a ranked list against the gold of a commit (#210)."""

from __future__ import annotations

KS = (1, 3, 5, 10)


def score(ranking: list[str], gold: list[str]) -> dict[str, float]:
    g = set(gold)
    out: dict[str, float] = {}
    for k in KS:
        hit = sum(p in g for p in ranking[:k])
        out[f"recall@{k}"] = hit / len(g)
        out[f"precision@{k}"] = hit / k
    first = next((i for i, p in enumerate(ranking) if p in g), None)
    out["mrr"] = 1 / (first + 1) if first is not None else 0.0
    out["shown@5"] = float(min(5, len(ranking)))
    out["success@5"] = float(any(p in g for p in ranking[:5]))
    return out


def mean(rows: list[dict[str, float]]) -> dict[str, float]:
    return {k: sum(r[k] for r in rows) / len(rows) for k in rows[0]} if rows else {}
