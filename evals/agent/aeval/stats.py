"""Paired bootstrap over tasks.

Tasks are the unit of analysis: repetitions of the same task are averaged
first, so a task run three times does not count as three independent
observations. The bootstrap then resamples tasks with replacement and reads a
percentile interval off the resampled means of each condition and of their
paired difference.
"""

from __future__ import annotations

import math
import random
from dataclasses import dataclass


def mean(xs: list[float]) -> float:
    return sum(xs) / len(xs) if xs else float("nan")


def median(xs: list[float]) -> float:
    s = sorted(xs)
    n = len(s)
    if n == 0:
        return float("nan")
    mid = n // 2
    return s[mid] if n % 2 else (s[mid - 1] + s[mid]) / 2


def percentile(sorted_xs: list[float], q: float) -> float:
    """Linear-interpolated percentile of an already sorted list, q in [0, 1]."""
    if not sorted_xs:
        return float("nan")
    pos = q * (len(sorted_xs) - 1)
    lo = math.floor(pos)
    hi = math.ceil(pos)
    if lo == hi:
        return sorted_xs[lo]
    return sorted_xs[lo] + (sorted_xs[hi] - sorted_xs[lo]) * (pos - lo)


@dataclass
class PairedCI:
    n_tasks: int
    a_mean: float
    a_lo: float
    a_hi: float
    b_mean: float
    b_lo: float
    b_hi: float
    diff_mean: float
    diff_lo: float
    diff_hi: float
    ratio_mean: float = float("nan")  # mean(a) / mean(b)
    ratio_lo: float = float("nan")
    ratio_hi: float = float("nan")

    def as_dict(self) -> dict:
        return dict(self.__dict__)


def paired_bootstrap(
    pairs: list[tuple[float, float]],
    n_resamples: int = 10_000,
    seed: int = 0,
    alpha: float = 0.05,
) -> PairedCI:
    """Percentile CIs for mean(a), mean(b) and mean(a - b) over paired tasks.

    ``pairs`` holds one ``(a, b)`` tuple per task, each value already averaged
    over that task's repetitions. The same resampled task indices are used for
    both conditions, which is what makes the difference interval paired.
    """
    k = len(pairs)
    if k == 0:
        nan = float("nan")
        return PairedCI(0, nan, nan, nan, nan, nan, nan, nan, nan, nan)
    rng = random.Random(seed)
    a_means, b_means, d_means, ratios = [], [], [], []
    for _ in range(n_resamples):
        idx = [rng.randrange(k) for _ in range(k)]
        a = sum(pairs[i][0] for i in idx) / k
        b = sum(pairs[i][1] for i in idx) / k
        a_means.append(a)
        b_means.append(b)
        d_means.append(a - b)
        if b:
            ratios.append(a / b)
    a_means.sort()
    b_means.sort()
    d_means.sort()
    ratios.sort()
    b_total = sum(p[1] for p in pairs)
    lo_q, hi_q = alpha / 2, 1 - alpha / 2
    return PairedCI(
        n_tasks=k,
        a_mean=mean([p[0] for p in pairs]),
        a_lo=percentile(a_means, lo_q),
        a_hi=percentile(a_means, hi_q),
        b_mean=mean([p[1] for p in pairs]),
        b_lo=percentile(b_means, lo_q),
        b_hi=percentile(b_means, hi_q),
        diff_mean=mean([p[0] - p[1] for p in pairs]),
        diff_lo=percentile(d_means, lo_q),
        diff_hi=percentile(d_means, hi_q),
        ratio_mean=sum(p[0] for p in pairs) / b_total if b_total else float("nan"),
        ratio_lo=percentile(ratios, lo_q),
        ratio_hi=percentile(ratios, hi_q),
    )
