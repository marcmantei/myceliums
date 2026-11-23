"""Token attribution report of a committed run (#178): ``TOKENS.md``.

``collect`` attributes the tokens of every session of a committed run from
its transcripts (:mod:`aeval.tokens`) and writes ``results/<label>/tokens.jsonl``,
one compact row per session, so the report can be rebuilt without the
git-ignored transcripts. Runs made after #178 already carry the same fields
in ``results.jsonl``; ``collect`` then needs no transcripts.

``render`` turns ``tokens.jsonl`` (and ``results.jsonl`` for success) into
``TOKENS.md`` and ``tokens_summary.json``, deterministically:

* tokens per session by category, per condition (and budget);
* cost per session by category and by cache class, with the CLI's estimate;
* tokens and cost per solved task (sum over sessions / successes) with a 95 %
  paired bootstrap over tasks;
* the reconciliation: the residual between the per-call usage and the
  session totals, and the priced cost against the CLI's.

A re-run of one condition (``meta.rerun_of``, #171) is compared with the
base run's other condition, read from the base run's ``tokens.jsonl``.
"""

from __future__ import annotations

import json
from collections import defaultdict
from pathlib import Path

from . import config
from .report import _fmt_ratio
from .searchaudit import BLIND_READ_LIMIT
from .searchaudit import audit as search_audit
from .searchaudit import summarise as audit_summary
from .stats import per_success_bootstrap
from .tokens import (CATEGORIES, CLASSES, PRICES_AS_OF, PRICES_SOURCE, attribute, fit_rates, load_prices, price_of,
                     rates_of)

TOKEN_FIELDS = ("attribution_method", "models", "tokens_by_category", "tokens_by_class", "result_tokens_by_category",
                "estimated_tokens_by_category",
                "thinking_tokens", "attribution_residual", "cost_by_category", "cost_by_class",
                "priced_cost_usd")
CAT_LABEL = {
    "base": "Base context (system prompt, tool definitions, task prompt)",
    "myceliums": "myceliums results (MCP + `myc` CLI)",
    "hook": "Hook text (myceliums hooks)",
    "search": "Search results (Grep, Glob, grep/rg/find in Bash)",
    "read": "File reads (Read, cat/sed/head in Bash)",
    "edit": "Edits",
    "other": "Other tool results",
    "assistant": "Model's earlier replies, re-read",
    "unattributed": "Unattributed (residual)",
    "output": "Output (generated, incl. thinking)",
}
CLASS_LABEL = {"uncached": "Uncached input", "cache_write_5m": "Cache write (5 min)",
               "cache_write_1h": "Cache write (1 h)", "cache_read": "Cache read", "output": "Output"}


def _rows(path: Path) -> list[dict]:
    return [json.loads(x) for x in path.read_text().splitlines() if x.strip()] if path.exists() else []


def transcript_path(row: dict, meta: dict, runs_root: Path, label: str) -> Path:
    """Where a committed row's transcript lives under ``runs_root``."""
    run_id = row["run_id"]
    if "budget" in row and meta.get("runs"):
        run_label = meta["runs"][str(row["budget"])]["label"]
        run_id = run_id.split("/", 1)[1]
    else:
        run_label = label
    return runs_root / run_label / "sessions" / run_id / f"attempt{row['attempt']}.jsonl"


def collect(label: str, runs_root: Path | None = None, prices_file: str | None = None) -> Path:
    """Write ``results/<label>/tokens.jsonl`` and ``tokens_crosscheck.json``.

    With the transcripts: attribute every session, fit the measured growth on
    the estimates over all main-thread calls of the run (:func:`fit_rates`),
    and attribute again with the fitted rates (``*_fitted`` fields): the
    split's sensitivity to the chars/4 assumption. Without them, rows that
    already carry the fields (runs after #178) are copied and there is no
    cross-check."""
    res_dir = config.RESULTS_DIR / label
    meta = config.load_json(res_dir / "meta.json")
    prices = load_prices(prices_file)
    runs_root = runs_root or config.RUNS_DIR
    rows = _rows(res_dir / "results.jsonl")
    paths = [transcript_path(r, meta, runs_root, label) for r in rows]
    have = all(p.exists() for p in paths)
    out, samples, atts = [], [], []
    for row, path in zip(rows, paths):
        keep = {k: row.get(k) for k in ("run_id", "task_id", "condition", "rep", "budget", "success",
                                        "total_input_tokens", "output_tokens", "cost_usd") if k in row}
        if have:
            att = attribute(str(path), prices)
            samples.extend(att.samples)
            keep.update(att.as_row())
            prompt = path.parent / "prompt.txt"
            keep["search_audit"] = search_audit(str(path), prompt.read_text() if prompt.exists() else "")
        elif row.get("tokens_by_category") and not prices_file:
            keep.update({k: row.get(k) for k in TOKEN_FIELDS + ("search_audit",)})
        else:
            raise SystemExit(f"no transcript for {row['run_id']}: {path}")
        out.append(keep)
        atts.append(path)
    cross = {"available": have}
    if have:
        fit = fit_rates(samples)
        rates = rates_of(fit)
        for keep, path in zip(out, atts):
            a = attribute(str(path), prices, rates)
            keep["tokens_by_category_fitted"] = {k: round(v, 1) for k, v in a.by_category().items() if v}
            keep["cost_by_category_fitted"] = {k: round(v, 6) for k, v in a.cost.items() if v}
        cross.update(fit=fit, rates=rates)
    with open(res_dir / "tokens.jsonl", "w", encoding="utf-8") as fh:
        for r in sorted(out, key=lambda r: r["run_id"]):
            fh.write(json.dumps(r, sort_keys=True) + "\n")
    (res_dir / "tokens_crosscheck.json").write_text(json.dumps(cross, indent=2, sort_keys=True) + "\n")
    return res_dir / "tokens.jsonl"


# --------------------------------------------------------------------------
# Rendering
# --------------------------------------------------------------------------


def _tok(v: float) -> str:
    return "–" if v != v else f"{v:,.0f}"


def _usd(v: float) -> str:
    return "–" if v != v else f"{v:.4f}"


def _mean(xs: list[float]) -> float:
    return sum(xs) / len(xs) if xs else float("nan")


def _groups(rows: list[dict]) -> list[tuple[str, list[dict]]]:
    """``(name, rows)`` per budget, plus every budget together when there are several."""
    budgets = sorted({r.get("budget") for r in rows}, key=lambda b: (b is None, b or 0))
    if budgets == [None]:
        return [("all", rows)]
    out = [(str(b), [r for r in rows if r.get("budget") == b]) for b in budgets]
    return out + ([("all budgets", rows)] if len(budgets) > 1 else [])


def _per_solved(rows: list[dict], ids: list[str], base: str, arm: str, key, n_resamples: int) -> dict:
    acc = defaultdict(lambda: [0.0, 0.0, 0.0, 0.0])
    for r in rows:
        if r["task_id"] not in ids or r["condition"] not in (base, arm):
            continue
        a = acc[r["task_id"]]
        off = 0 if r["condition"] == arm else 2
        a[off] += key(r)
        a[off + 1] += 1.0 if r.get("success") else 0.0
    per_task = [tuple(acc[t]) for t in ids if t in acc]
    return per_success_bootstrap(per_task, n_resamples)


def analyse(rows: list[dict], ids: list[str], base: str, arm: str, n_resamples: int = 10_000) -> dict:
    out = {}
    for name, group in _groups(rows):
        sub = [r for r in group if r["task_id"] in ids]
        g = {}
        for cond in (base, arm):
            cr = [r for r in sub if r["condition"] == cond]
            if not cr:
                continue
            g[cond] = {
                "sessions": len(cr),
                "solved": sum(1 for r in cr if r.get("success")),
                "tokens": {c: _mean([(r["tokens_by_category"] or {}).get(c, 0.0) for r in cr]) for c in CATEGORIES},
                "tokens_fitted": ({c: _mean([r["tokens_by_category_fitted"].get(c, 0.0) for r in cr])
                                   for c in CATEGORIES}
                                  if all(r.get("tokens_by_category_fitted") for r in cr) else None),
                "result_tokens": {c: _mean([(r["result_tokens_by_category"] or {}).get(c, 0.0) for r in cr])
                                  for c in CATEGORIES},
                "cost": {c: _mean([(r["cost_by_category"] or {}).get(c, 0.0) for r in cr]) for c in CATEGORIES},
                "cost_class": {c: _mean([(r["cost_by_class"] or {}).get(c, 0.0) for r in cr])
                               for c in CLASSES + ("output",)},
                "priced_cost": _mean([r["priced_cost_usd"] or 0.0 for r in cr]),
                "cli_cost": _mean([r.get("cost_usd") or 0.0 for r in cr]),
            }
        g["per_solved_tokens"] = _per_solved(
            sub, ids, base, arm, lambda r: sum((r["tokens_by_category"] or {}).values()), n_resamples)
        g["per_solved_cost"] = _per_solved(sub, ids, base, arm, lambda r: r["priced_cost_usd"] or 0.0, n_resamples)
        out[name] = g
    return out


def reconciliation(rows: list[dict]) -> dict:
    res = defaultdict(int)
    nonzero = 0
    for r in rows:
        rr = r.get("attribution_residual") or {}
        for k, v in rr.items():
            res[k] += v
        nonzero += any(rr.values())
    diffs = [abs((r["priced_cost_usd"] or 0.0) - (r.get("cost_usd") or 0.0)) for r in rows]
    per_session = [sum(v for c, v in (r["tokens_by_category"] or {}).items() if c != "output") for r in rows]
    attributed = sum(per_session)
    gap = max((abs(a - (r.get("total_input_tokens") or 0)) for a, r in zip(per_session, rows)), default=0.0)
    return {"sessions": len(rows), "sessions_with_residual": nonzero, "residual": dict(sorted(res.items())),
            "max_abs_cost_diff_usd": max(diffs) if diffs else 0.0,
            "input_tokens_attributed": round(attributed, 1),
            "max_abs_session_gap_tokens": round(gap, 2),
            "input_tokens_session_totals": sum(r.get("total_input_tokens") or 0 for r in rows),
            "methods": sorted({r.get("attribution_method") for r in rows})}


def _ci(s: dict, name: str, fmt) -> str:
    return f"{fmt(s[name])} [{fmt(s[name + '_lo'])}, {fmt(s[name + '_hi'])}]"


def render(label: str, n_resamples: int = 10_000) -> tuple[str, dict]:
    res_dir = config.RESULTS_DIR / label
    meta = config.load_json(res_dir / "meta.json")
    rows = _rows(res_dir / "tokens.jsonl")
    own = list(rows)
    conds = sorted({r["condition"] for r in rows})
    base_label = meta.get("rerun_of")
    if base_label:  # #171: one condition re-run; its comparison arm comes from the base run
        base_rows = _rows(config.RESULTS_DIR / base_label / "tokens.jsonl")
        budgets = {r.get("budget") for r in rows}
        others = [r for r in base_rows if r["condition"] not in conds and r.get("budget") in budgets]
        rows = rows + others
        conds = sorted({r["condition"] for r in rows})
    base = "none" if "none" in conds else conds[0]
    arms = [c for c in conds if c != base] or [base]
    arm = arms[0]
    tasks = sorted({r["task_id"] for r in rows})
    excluded = meta.get("primary_exclude") or {}
    ids = [t for t in tasks if t not in excluded]
    analysis = analyse(rows, ids, base, arm, n_resamples)
    by_arm = {a: (analysis if a == arm else analyse(rows, ids, base, a, n_resamples)) for a in arms}
    rec = reconciliation(own)
    models = sorted({m for r in rows for m in (r.get("models") or [])})
    prices = {m: price_of(m, load_prices(None)) for m in models}
    cross_path = res_dir / "tokens_crosscheck.json"
    cross = config.load_json(cross_path) if cross_path.exists() else {"available": False}
    audit = audit_summary([r for r in own if r["task_id"] in ids])
    summary = {"label": label, "base": base, "arm": arm, "tasks": len(ids), "excluded": sorted(excluded),
               "reconciliation": rec, "analysis": analysis, "models": models, "prices": prices,
               "prices_source": PRICES_SOURCE, "prices_as_of": PRICES_AS_OF, "crosscheck": cross,
               "search_audit": audit, **({"compared_with": base_label} if base_label else {}),
               **({"arms": arms, "analysis_by_arm": by_arm} if len(arms) > 1 else {})}

    L = [f"# Token attribution: {label}", ""]
    L.append(f"Where the tokens of the sessions in [`REPORT.md`](REPORT.md) went (#178), attributed per tool from "
             f"the session transcripts; method in [`../../README.md`](../../README.md#token-attribution-and-cost-178). "
             f"Per-session rows: `tokens.jsonl`.")
    L.append("")
    if base_label:
        L.append(f"`{base}` rows come from the base run [`{base_label}`](../{base_label}/TOKENS.md), the same "
                 f"tasks at the same budgets; `{arm}` rows are this run's.")
        L.append("")
    if excluded:
        L.append(f"Tables cover the {len(ids)} tasks of the primary analysis (without "
                 + ", ".join(f"`{t}`" for t in sorted(excluded)) + ").")
        L.append("")
    L.append("## Model and prices")
    L.append("")
    for m in models:
        p = prices[m]
        L.append(f"- `{m}`: " + (", ".join(f"{CLASS_LABEL.get(k, k).lower() if k != 'input' else 'uncached input'} "
                                           f"${p[k]:g}" for k in ("input", "cache_write_5m", "cache_write_1h",
                                                                   "cache_read", "output"))
                                 + " per million tokens" if p else "no price in the table (cost not computed)"))
    L.append(f"- Prices: [{PRICES_SOURCE}]({PRICES_SOURCE}), as of {PRICES_AS_OF}.")
    L.append("")
    L.append("## Reconciliation")
    L.append("")
    L.append("The residual is zero by construction of what it compares: it checks that the per-call usage in the "
             "transcript adds up to the session totals (no hidden model calls), not how a call's growth is split "
             "between categories. The split is checked separately below.")
    L.append("")
    res_txt = ", ".join(f"{k} {v:+,}" for k, v in rec["residual"].items()) or "none"
    L.append(f"- {rec['sessions']} sessions of this run, attribution method {', '.join(rec['methods'])}.")
    L.append(f"- Input tokens attributed to categories: {rec['input_tokens_attributed']:,.0f}; session totals "
             f"(result events): {rec['input_tokens_session_totals']:,}; largest difference in one session "
             f"{rec['max_abs_session_gap_tokens']:g} tokens (`tokens.jsonl` rounds each category to 0.1 token).")
    L.append(f"- Residual (session totals − summed per-call usage): {res_txt}; sessions with a non-zero "
             f"residual: {rec['sessions_with_residual']}.")
    L.append(f"- Cost priced per class with the table of {PRICES_AS_OF} vs the CLI's `total_cost_usd`: largest "
             f"difference per session {rec['max_abs_cost_diff_usd']:.2g} USD.")
    L.append("")
    L.extend(_crosscheck_lines(cross))
    if len(arms) == 1:
        L.extend(_arm_lines(analysis, base, arm))
    else:  # #182: several arms (A/C/D), each against the base
        for a in arms:
            L.extend(_arm_lines(by_arm[a], base, a, f": `{a}` against `{base}`"))
    L.extend(_audit_lines(audit))
    L.append("*Tokens* are billed input tokens (every call re-reads the context, so a result counts once per later "
             "call) and output tokens; *result size* is a result's own size when it entered the context (its share "
             "of the growth between two calls).")
    L.append("")
    return "\n".join(L), summary


def _arm_lines(analysis: dict, base: str, arm: str, suffix: str = "") -> list[str]:
    """The tables of one arm against the base, per budget group."""
    L: list[str] = []
    for b, g in analysis.items():
        heading = {"all": "All sessions", "all budgets": "All budgets together"}.get(b, f"{b} turns")
        L.append(f"## {heading}{suffix}")
        L.append("")
        if base not in g or arm not in g:
            continue
        gb, ga = g[base], g[arm]
        L.append(f"Sessions: `{base}` {gb['sessions']} ({gb['solved']} solved), `{arm}` {ga['sessions']} "
                 f"({ga['solved']} solved). Means per session.")
        L.append("")
        fitted = gb.get("tokens_fitted") is not None and ga.get("tokens_fitted") is not None
        L.append(f"| Category | `{base}` tokens | `{arm}` tokens | `{arm}` − `{base}` | `{base}` USD | `{arm}` USD "
                 f"| result size `{base}` | result size `{arm}` |"
                 + (f" fitted split `{base}` | fitted split `{arm}` |" if fitted else ""))
        L.append("|---|---:|---:|---:|---:|---:|---:|---:|" + ("---:|---:|" if fitted else ""))
        for c in CATEGORIES:
            vb, va = gb["tokens"][c], ga["tokens"][c]
            if not (vb or va):
                continue
            rb = gb["result_tokens"][c] if c not in ("base", "output", "unattributed") else float("nan")
            ra = ga["result_tokens"][c] if c not in ("base", "output", "unattributed") else float("nan")
            d = va - vb
            L.append(f"| {CAT_LABEL[c]} | {_tok(vb)} | {_tok(va)} | {'+' if d >= 0 else '−'}{abs(d):,.0f} | "
                     f"{_usd(gb['cost'][c])} | {_usd(ga['cost'][c])} | {_tok(rb)} | {_tok(ra)} |"
                     + (f" {_tok(gb['tokens_fitted'][c])} | {_tok(ga['tokens_fitted'][c])} |" if fitted else ""))
        tb, ta = sum(gb["tokens"].values()), sum(ga["tokens"].values())
        L.append(f"| **Total** | **{_tok(tb)}** | **{_tok(ta)}** | **{'+' if ta >= tb else '−'}{abs(ta - tb):,.0f}** "
                 f"| **{_usd(gb['priced_cost'])}** | **{_usd(ga['priced_cost'])}** | | |" + (" | |" if fitted else ""))
        L.append("")
        L.append(f"Cost by class (USD per session): " + "; ".join(
            f"{CLASS_LABEL[k]} {_usd(gb['cost_class'][k])} / {_usd(ga['cost_class'][k])}"
            for k in CLASSES + ("output",) if gb["cost_class"][k] or ga["cost_class"][k])
            + f" (`{base}` / `{arm}`); CLI estimate {_usd(gb['cli_cost'])} / {_usd(ga['cli_cost'])}.")
        L.append("")
        pt, pc = g["per_solved_tokens"], g["per_solved_cost"]
        L.append(f"Per solved task (sum over sessions ÷ solved, 95 % paired bootstrap over {pt['n_tasks']} tasks): "
                 f"tokens `{base}` {_ci(pt, 'b', _tok)}, `{arm}` {_ci(pt, 'a', _tok)}, ratio "
                 f"{_ci(pt, 'ratio', _fmt_ratio)}; cost `{base}` {_ci(pc, 'b', _usd)} USD, `{arm}` "
                 f"{_ci(pc, 'a', _usd)} USD, ratio {_ci(pc, 'ratio', _fmt_ratio)}"
                 + (f" ({pt['skipped']} resamples without a solved task skipped)" if pt["skipped"] else "") + ".")
        L.append("")
    return L


def _crosscheck_lines(cross: dict) -> list[str]:
    L = ["## Cross-check of the split", ""]
    fit = cross.get("fit") if cross.get("available") else None
    if not fit or not fit.get("features"):
        return L + ["Not available: the transcripts were not at hand when `tokens.jsonl` was written.", ""]
    L.append(f"Each call's measured context growth fitted by least squares on the chars/4 estimates of what "
             f"arrived before it ({fit['n_calls']} main-agent calls of this run; R² = {fit['r2']}; a constant of "
             f"{fit['constant']:g} tokens per call). If the estimate undercounts every kind of content by the same "
             f"factor, the proportional split is fair between them:")
    L.append("")
    L.append("| Part | Calls with it | Measured tokens per estimated token | Tokens per character |")
    L.append("|---|---:|---:|---:|")
    for f, v in fit["features"].items():
        L.append(f"| {f} | {v['n_calls']} | {v['coef']:.3f} | {v.get('tokens_per_char', float('nan')):.3f} |"
                 .replace("| nan |", "| (CLI estimate) |"))
    if fit.get("left_out"):
        L.append("")
        L.append(f"Left out (in fewer than 20 calls, or the same in every call): {', '.join(fit['left_out'])}.")
    L.append("")
    L.append("The *fitted split* columns below re-split every growth with these rates instead of 1 (the "
             "attribution's sensitivity to the chars/4 assumption).")
    L.append("")
    return L


def _audit_lines(audit: dict) -> list[str]:
    if not audit:
        return []
    L = ["## Search audit", "",
         f"Per condition, over its sessions: Grep/Glob calls, Bash search commands (blocked by a hook / ran), "
         f"Bash calls running an interpreter, Read calls, blind reads (a file whose name appeared nowhere before), "
         f"and sessions flagged (any Grep/Glob, search that ran, interpreter that ran, or ≥ {BLIND_READ_LIMIT} "
         f"blind reads). In an arm with search this measures how much the agent searched; in D every flag is a "
         f"hole in the isolation.", "",
         "| Condition | Sessions | Grep/Glob | Bash search blocked / ran | Interpreter calls ran | Reads | "
         "Blind reads | Flagged |",
         "|---|---:|---:|---:|---:|---:|---:|---:|"]
    for c, a in sorted(audit.items()):
        L.append(f"| `{c}` | {a['sessions']} | {a['grep_glob_calls']} | {a['search_blocked']} / {a['search_leaks']} | "
                 f"{a['interpreted_ran']} | {a['reads']} | {a['blind_reads']} | {a['flagged']} |")
    L.append("")
    return L


def write(label: str, check: bool = False) -> bool:
    text, summary = render(label)
    res_dir = config.RESULTS_DIR / label
    stext = json.dumps(summary, indent=2, sort_keys=True) + "\n"
    if check:
        return ((res_dir / "TOKENS.md").exists() and (res_dir / "TOKENS.md").read_text() == text
                and (res_dir / "tokens_summary.json").exists()
                and (res_dir / "tokens_summary.json").read_text() == stext)
    (res_dir / "TOKENS.md").write_text(text)
    (res_dir / "tokens_summary.json").write_text(stext)
    return True
