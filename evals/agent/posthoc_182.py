#!/usr/bin/env python3
"""Post-hoc analyses of the #182 run, added after an independent review (PR #199).

Nothing here was pre-registered. Reads the committed rows and the transcripts of
runs/182-spring and writes results/182-spring/POSTHOC.md and posthoc.json.

    python3 evals/agent/posthoc_182.py --scale-runs DIR   # DIR: the runs/ directory of #163
"""
from __future__ import annotations

import argparse
import json
import sys
from collections import Counter, defaultdict
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from aeval import config  # noqa: E402
from aeval.stats import mean, per_success_bootstrap  # noqa: E402
from aeval.stream import _result_text  # noqa: E402

LABEL, ARMS = "182-spring", ("none", "C", "D")
PAIRS = (("C", "none"), ("D", "none"), ("D", "C"))
REFUSED = "This session has no search and only a few shell commands"


def fit(xs, ys):
    mx, my = mean(xs), mean(ys)
    b = sum((x - mx) * (y - my) for x, y in zip(xs, ys)) / sum((x - mx) ** 2 for x in xs)
    return b, my - b * mx


def calls(path: Path) -> list[dict]:
    """Model calls of a transcript: context tokens, tool uses, which of them the D hook refused."""
    out, by_id, owner = [], {}, {}
    for line in path.read_text().splitlines():
        ev = json.loads(line)
        m = ev.get("message") or {}
        if ev.get("type") == "assistant":
            c = by_id.get(m.get("id"))
            if c is None:
                u = m.get("usage") or {}
                c = by_id[m["id"]] = {"ctx": sum(int(u.get(k) or 0) for k in (
                    "input_tokens", "cache_creation_input_tokens", "cache_read_input_tokens")), "uses": [], "refused": 0}
                out.append(c)
            for x in m.get("content") or []:
                if x.get("type") == "tool_use":
                    c["uses"].append(x["name"])
                    owner[x["id"]] = (c, x["name"])
        elif isinstance(m.get("content"), list):
            for x in m["content"]:
                if x.get("type") == "tool_result" and x.get("tool_use_id") in owner:
                    c, name = owner[x["tool_use_id"]]
                    t = _result_text(x.get("content"))
                    c["refused"] += REFUSED in t
                    c.setdefault("results", []).append((name, t))
    return out


def blocked_input(cs: list[dict]) -> tuple[int, int]:
    """Input tokens of the calls made only for refused commands, and of the re-reads of what they added."""
    alone = reread = 0
    for i, c in enumerate(cs):
        if c["uses"] and c["refused"] == len(c["uses"]):
            growth = max(0, cs[i + 1]["ctx"] - c["ctx"]) if i + 1 < len(cs) else 0
            alone += c["ctx"]
            reread += growth * (len(cs) - 1 - i)
    return alone, reread


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--scale-runs", type=Path, required=True)
    a = ap.parse_args()
    res = config.RESULTS_DIR / LABEL
    rows = [json.loads(x) for x in (res / "results.jsonl").read_text().splitlines()]
    cat = {t["id"]: t.get("category") or t["type"] for t in config.load_json(res / "tasks_snapshot.json")}
    sess = config.RUNS_DIR / LABEL / "sessions"
    for r in rows:
        cs = calls(sess / r["run_id"] / f"attempt{r['attempt']}.jsonl")
        r["api_calls"], (r["blocked"], r["reread"]) = len(cs), blocked_input(cs)
        r["refusing"] = [i for i, c in enumerate(cs) if c["refused"]]
        r["all_refused_calls"] = sum(1 for c in cs if c["uses"] and c["refused"] == len(c["uses"]))
        r["results"] = [x for c in cs for x in c.get("results", [])]
        r["prompt_chars"] = len((sess / r["run_id"] / "prompt.txt").read_text())
        r["tokens"] = r["total_input_tokens"] + r["output_tokens"]
    by = {c: [r for r in rows if r["condition"] == c] for c in ARMS}

    def ratio(x, y, f, keep=lambda r: True):
        g = defaultdict(lambda: [0.0, 0.0, 0.0, 0.0])
        for r in rows:
            if keep(r) and r["condition"] in (x, y):
                o = 0 if r["condition"] == x else 2
                g[r["task_id"]][o] += f(r)
                g[r["task_id"]][o + 1] += bool(r["success"])
        p = per_success_bootstrap([tuple(v) for v in g.values()])
        return f"{p['ratio']:.2f}× [{p['ratio_lo']:.2f}×, {p['ratio_hi']:.2f}×]"

    # AGENTS.md: first-call usage at equal prompt length, against #163's none arm (same CLI, no instruction file).
    same = [r for r in by["none"] if r["claude_version"] == "2.1.330"]
    b, i182 = fit([r["prompt_chars"] for r in same], [r["first_turn_input_tokens"] for r in same])
    run = a.scale_runs / "2026-11-22-scale-t40"
    cfg = config.load_json(run / "config.json")
    assert cfg["max_turns"] == 40 and not cfg.get("prompt_budget") and cfg["bash"] and cfg["effort"] == "medium"
    pts = []
    for x in (run / "results.jsonl").read_text().splitlines():
        r = json.loads(x)
        if r["final"] and r["condition"] == "none" and r["claude_version"] == "2.1.330":
            pts.append((len((run / "sessions" / r["run_id"] / "prompt.txt").read_text()), r["first_turn_input_tokens"]))
    b163, i163 = fit([p[0] for p in pts], [p[1] for p in pts])
    agents = i182 - i163
    ref = mean([r["prompt_chars"] for r in by["none"]])
    agents_at = (i182 + b * ref) - (i163 + b163 * ref)
    out = {"agents_tokens_intercept": agents, "agents_tokens_at_mean_prompt": agents_at, "n163": len(pts), "slope182": b, "slope163": b163}
    adj = lambda r: r["tokens"] + (agents_at * r["api_calls"] if r["condition"] == "C" else 0)  # noqa: E731
    lo = lambda r: r["cost_usd"] + (agents_at * r["api_calls"] * 0.1e-6 if r["condition"] == "C" else 0)  # noqa: E731
    hi = lambda r: r["cost_usd"] + (agents_at * (4e-6 + (r["api_calls"] - 1) * 0.1e-6) if r["condition"] == "C" else 0)  # noqa: E731
    tok, usd = (lambda r: r["tokens"]), (lambda r: r["cost_usd"])
    rep12 = lambda r: r["rep"] < 3  # noqa: E731
    nob = lambda r: r["tokens"] - r["blocked"] - r["reread"]  # noqa: E731
    noc = lambda r: r["tokens"] - r["blocked"]  # noqa: E731
    L = ["# Post-hoc analyses: 182-spring", "",
         "Generated by `python3 evals/agent/posthoc_182.py`, after an independent review of PR #199. **Nothing here was "
         "pre-registered.** Ratios are per solved task with a 95 % paired bootstrap over tasks unless stated otherwise.", "",
         "## Instruction-file confound (#200)", "",
         f"A and D loaded the checkout's `AGENTS.md` (4,262 characters); in C the `CLAUDE.md` that `myc setup-claude` wrote (before #200) "
         f"took its place. Size from first-call usage at the same prompt length ({ref:.0f} characters): A here against #163's "
         f"`none` sessions without a stated budget (n = {len(pts)}, same CLI 2.1.330, Django and Vue, no instruction file): "
         f"**{agents_at:,.0f} tokens** (intercepts {i182:,.0f} and {i163:,.0f}; slopes {b:.3f} and {b163:.3f} tokens per character). "
         "Other differences between the two checkouts' contexts are included in that number.", "",
         "| C ÷ A | Tokens per solved | Cost per solved |", "|---|---|---|",
         f"| measured | {ratio('C', 'none', tok)} | {ratio('C', 'none', usd)} |",
         f"| adjusted: AGENTS.md added to each of C's model calls, behaviour held fixed (point estimate) | "
         f"{ratio('C', 'none', adj).split(' ')[0]} | {ratio('C', 'none', lo).split(' ')[0]} (all cache reads) to "
         f"{ratio('C', 'none', hi).split(' ')[0]} (written on every first call) |", "",
         "Whether C would have behaved differently with the file is unknown. D ÷ C is affected the other way: "
         f"{ratio('D', 'C', adj).split(' ')[0]} in tokens instead of {ratio('D', 'C', tok).split(' ')[0]}. D ÷ A is not affected.", "",
         "C ÷ A per category (6 tasks each, exploratory; adjusted: point estimates, cost from all cache reads to written on "
         "every first call):", "",
         "| Category | Tokens, measured | Tokens, adjusted | Cost, measured | Cost, adjusted |", "|---|---|---:|---|---:|",
         *(f"| {k} | {ratio('C', 'none', tok, kc)} | {ratio('C', 'none', adj, kc).split(' ')[0]} | {ratio('C', 'none', usd, kc)} | "
           f"{ratio('C', 'none', lo, kc).split(' ')[0]} to {ratio('C', 'none', hi, kc).split(' ')[0]} |"
           for k in sorted(set(cat.values())) for kc in [lambda r, k=k: cat[r["task_id"]] == k]), "",
         "## Claude Code version", "",
         "| Version | " + " | ".join(ARMS) + " | Repetitions |", "|---|" + "---:|" * 4]
    for v in sorted({r["claude_version"] for r in rows}):
        vs = [r for r in rows if r["claude_version"] == v]
        n = Counter(r["condition"] for r in vs)
        L.append(f"| {v} | " + " | ".join(str(n[c]) for c in ARMS) + f" | {sorted({r['rep'] for r in vs})} |")
    L += ["", "| Pair | Tokens, all | Tokens, repetitions 1-2 | Cost, all | Cost, repetitions 1-2 |", "|---|---|---|---|---|"]
    L += [f"| {x} ÷ {y} | {ratio(x, y, tok)} | {ratio(x, y, tok, rep12)} | {ratio(x, y, usd)} | {ratio(x, y, usd, rep12)} |" for x, y in PAIRS]
    L += ["", "Repetition 3 also has sessions on 2.1.330; leaving it out removes every 2.1.331 session.", "",
          "## Model calls", "", "`num_turns` is the number of tool calls + 1 or close to it, not model calls.", "",
          "| | " + " | ".join(ARMS) + " |", "|---|" + "---:|" * 3,
          "| model calls per session | " + " | ".join(f"{mean([r['api_calls'] for r in by[c]]):.2f}" for c in ARMS) + " |",
          "| `num_turns` | " + " | ".join(f"{mean([r['num_turns'] for r in by[c]]):.2f}" for c in ARMS) + " |",
          "| tool calls + 1 | " + " | ".join(f"{mean([r['tool_calls'] + 1 for r in by[c]]):.2f}" for c in ARMS) + " |",
          "| sessions without any cache write | " + " | ".join(str(sum(r["cache_creation_input_tokens"] == 0 for r in by[c])) for c in ARMS) + " |"]
    bl, rr = sum(r["blocked"] for r in by["D"]), sum(r["reread"] for r in by["D"])
    tot = sum(r["total_input_tokens"] for r in by["D"])
    L += ["", "## Arm D: refused commands", "",
          f"{sum(r['all_refused_calls'] for r in by['D'])} model calls in {sum(r['all_refused_calls'] > 0 for r in by['D'])} of 90 sessions issued only "
          f"refused commands; those calls alone are {100 * bl / tot:.1f} % of D's input tokens, with the re-reads of what they added to the "
          f"context {100 * (bl + rr) / tot:.1f} %. D ÷ A in tokens: measured {ratio('D', 'none', tok).split(' ')[0]}, without the calls "
          f"{ratio('D', 'none', noc).split(' ')[0]}, without the calls and the re-reads {ratio('D', 'none', nob).split(' ')[0]} (point estimates, "
          f"behaviour held fixed). {sum(len(r['refusing']) for r in by['D'])} model calls had at least one refusal; "
          f"{sum(r['refusing'].count(0) for r in by['D'])} of them were the session's first call, the rest came later."]
    sc = [t for r in by["D"] for n, t in r["results"] if n.endswith("search_code")]
    amb = sum("is ambiguous" in t for r in by["D"] for n, t in r["results"] if n.startswith("mcp__"))
    L += ["", f"`search_code`: {len(sc)} calls, mean {mean([len(t) for t in sc]):,.0f} characters. Ambiguous-symbol errors: {amb}.", "",
          "## Arm C: when did it use myceliums?", "", "| Category | Sessions with a myceliums call |", "|---|---:|"]
    for k in sorted(set(cat.values())):
        v = [r for r in by["C"] if cat[r["task_id"]] == k]
        L.append(f"| {k} | {sum(1 for r in v if r['mcp_tool_calls'] or r['myc_cli_calls'])} of {len(v)} |")
    (res / "POSTHOC.md").write_text("\n".join(L) + "\n")
    out["d_refused_only_calls_input_share"], out["d_with_rereads_input_share"] = bl / tot, (bl + rr) / tot
    first = lambda x, y, f: ratio(x, y, f).split(" ")[0]  # noqa: E731
    out["pair_notes"] = [
        {"pair": ["C", "none"], "text":
         f"confounded (#200). Spring's `AGENTS.md` ({agents_at:,.0f} tokens) was in `none`'s context and not in C's. With it added to "
         f"each of C's model calls, behaviour held fixed (post hoc point estimates): tokens per solved task {first('C', 'none', adj)} "
         f"(measured {first('C', 'none', tok)}), cost {first('C', 'none', lo)} to {first('C', 'none', hi)} (measured "
         f"{first('C', 'none', usd)}). Ratios here are C ÷ none; tables that list `none` vs `C` show the inverse. Per category: "
         "[`POSTHOC.md`](POSTHOC.md)."},
        {"pair": ["D", "C"], "text":
         f"the same confound (#200), the other way round: tokens per solved task {first('D', 'C', adj)} (measured {first('D', 'C', tok)}), "
         f"cost {first('D', 'C', hi)} to {first('D', 'C', lo)} (measured {first('D', 'C', usd)})."}]
    (res / "posthoc.json").write_text(json.dumps(out, indent=1) + "\n")
    print("\n".join(L))
    return 0


if __name__ == "__main__":
    sys.exit(main())
