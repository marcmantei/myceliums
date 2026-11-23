"""Where a session's tokens go: per-tool token attribution and cost (#178).

Input tokens
------------
Every model call re-sends the whole conversation, so a session's input tokens
are the sum, over its calls, of each call's context size ``input_tokens +
cache_creation_input_tokens + cache_read_input_tokens`` (the stream-json
transcript carries this ``usage`` on every assistant event; events of one
call share a message id and are counted once). The context of call *k* is the
context of call *k − 1* plus what arrived in between: the previous reply of
the model and the tool results (and hook text) it led to. So

* the first call's context is the **base**: system prompt, tool definitions,
  the task prompt and the SessionStart hook's text;
* the growth ``Δk = context(k) − context(k − 1)`` is split over what arrived
  between the two calls, in proportion to each part's size estimated as
  characters / 4 (the model's earlier reply: its text, tool-call inputs and
  the CLI's estimate of its thinking tokens; each tool result; each hook's
  added context). The measured ``Δk`` fixes the total, the estimate only the
  split; a growth with nothing to explain it goes to ``other``.

Each part becomes a segment of the context that stays there for every later
call. A call's usage is then laid over the segments in order: the first
``cache_read`` tokens are cache reads, the next ``cache_creation`` tokens
cache writes (split into 5-minute and 1-hour writes as the call reports), the
rest uncached input. Each segment's share of each class goes to its category.
By construction the categories add up to the summed per-call usage; the
**residual** is the session's total usage (the result event's
``modelUsage``) minus that sum, per model and field: calls the transcript
does not show. It is reported, and kept as category ``unattributed``.

Without per-call usage (no ``usage`` on an assistant event) the context is
estimated from characters / 4 alone, scaled to the session's input total and
split over the cache classes in the proportions of the session totals;
``method`` says ``chars/4`` then and there is no residual to report.

Output tokens come from the result event (the per-event ``output_tokens`` are
streaming snapshots and undercount); ``thinking`` is the part the CLI reports
as thinking.

Categories
----------
``base``, ``myceliums`` (MCP results and ``myc`` CLI runs in Bash), ``hook``
(context the hooks added after the first call: in these runs the hooks of
``myc setup-claude``), ``search`` (``Grep``, ``Glob``, search commands in
Bash), ``read`` (``Read``, ``cat``/``sed``/``head``/... in Bash), ``edit``
(``Edit``, ``Write``, ``MultiEdit``, ``NotebookEdit``, ``sed -i``),
``other`` (any other tool, e.g. ``ToolSearch``, other Bash commands),
``assistant`` (the model's earlier replies, re-read as input),
``unattributed`` (the residual), and ``output``. Bash commands are sorted by
:mod:`aeval.bashcmd`.

Cost
----
Each class is priced separately with the published per-token prices in
:data:`PRICES` (USD per million tokens; ``--prices FILE`` replaces or adds
entries) for the model each call reports, next to the CLI's own estimate
(``total_cost_usd``).
"""

from __future__ import annotations

import json
from dataclasses import dataclass, field

from .bashcmd import classify as classify_bash

MCP_PREFIX = "mcp__myceliums__"
INPUT_CATEGORIES = ("base", "myceliums", "hook", "search", "read", "edit", "other", "assistant", "unattributed")
CATEGORIES = INPUT_CATEGORIES + ("output",)
CLASSES = ("uncached", "cache_write_5m", "cache_write_1h", "cache_read")
EDIT_TOOLS = frozenset({"Edit", "Write", "MultiEdit", "NotebookEdit"})
CHARS_PER_TOKEN = 4.0

# Published Claude API list prices, USD per million tokens. Cache writes are
# 1.25x (5-minute) and 2x (1-hour) the input price; cache reads differ by
# model. Checked against the CLI's own cost of all 240 #163/#171 sessions
# (equal to 5e-7 USD; tests/test_tokens.py, README).
PRICES_SOURCE = "https://platform.claude.com/docs/en/about-claude/pricing"
PRICES_AS_OF = "2026-11-03"
PRICES: dict[str, dict[str, float]] = {
    "claude-sonnet-5-5": {"input": 2.0, "cache_write_5m": 2.5, "cache_write_1h": 4.0, "cache_read": 0.10,
                          "output": 10.0},
    "claude-sonnet-5": {"input": 2.0, "cache_write_5m": 2.5, "cache_write_1h": 4.0, "cache_read": 0.20,
                        "output": 10.0},
    "claude-sonnet-4-6": {"input": 3.0, "cache_write_5m": 3.75, "cache_write_1h": 6.0, "cache_read": 0.30,
                          "output": 15.0},
    "claude-opus-5-5": {"input": 4.0, "cache_write_5m": 5.0, "cache_write_1h": 8.0, "cache_read": 0.20,
                        "output": 20.0},
    "claude-opus-5": {"input": 5.0, "cache_write_5m": 6.25, "cache_write_1h": 10.0, "cache_read": 0.50,
                      "output": 25.0},
    "claude-haiku-5-5": {"input": 0.10, "cache_write_5m": 0.125, "cache_write_1h": 0.20, "cache_read": 0.01,
                         "output": 0.50},
    "claude-haiku-4-5": {"input": 1.0, "cache_write_5m": 1.25, "cache_write_1h": 2.0, "cache_read": 0.10,
                         "output": 5.0},
    "claude-fable-5-2": {"input": 8.0, "cache_write_5m": 10.0, "cache_write_1h": 16.0, "cache_read": 0.20,
                         "output": 40.0},
}
CLASS_PRICE = {"uncached": "input", "cache_write_5m": "cache_write_5m", "cache_write_1h": "cache_write_1h",
               "cache_read": "cache_read"}


def load_prices(path: str | None) -> dict[str, dict[str, float]]:
    """:data:`PRICES`, updated from a JSON file ``{model: {input: …, …}}``."""
    table = {k: dict(v) for k, v in PRICES.items()}
    if path:
        with open(path, encoding="utf-8") as fh:
            for model, row in json.load(fh).items():
                table[model] = {**table.get(model, {}), **row}
    return table


def price_of(model: str | None, table: dict) -> dict | None:
    """Prices for ``model``: exact id, else the longest table id it starts with
    (``claude-sonnet-5-5-20261101`` -> ``claude-sonnet-5-5``)."""
    if not model:
        return None
    if model in table:
        return table[model]
    hits = [k for k in table if model.startswith(k)]
    return table[max(hits, key=len)] if hits else None


def category_of(name: str | None, tool_input: dict | None) -> str:
    """The category of a tool call's result."""
    name = name or ""
    if name.startswith(MCP_PREFIX):
        return "myceliums"
    if name in ("Grep", "Glob"):
        return "search"
    if name == "Read":
        return "read"
    if name in EDIT_TOOLS:
        return "edit"
    if name == "Bash":
        return classify_bash(str((tool_input or {}).get("command") or ""))
    return "other"


def _text_len(content) -> int:
    if isinstance(content, str):
        return len(content)
    if isinstance(content, list):
        n = 0
        for b in content:
            if isinstance(b, dict):
                n += len(str(b.get("text") or "")) if b.get("type", "text") == "text" else 0
        return n
    return 0


def _usage_ctx(u: dict) -> tuple[int, int, int, int, int]:
    """(context, cache_read, cache_write_5m, cache_write_1h, uncached) of a call."""
    inp = int(u.get("input_tokens") or 0)
    cc = int(u.get("cache_creation_input_tokens") or 0)
    cr = int(u.get("cache_read_input_tokens") or 0)
    detail = u.get("cache_creation") or {}
    w1h = int(detail.get("ephemeral_1h_input_tokens") or 0)
    w5m = int(detail.get("ephemeral_5m_input_tokens") or 0)
    if w1h + w5m != cc:  # no (or inconsistent) breakdown: count as 5-minute writes
        w1h, w5m = 0, cc
    return inp + cc + cr, cr, w5m, w1h, inp


@dataclass
class _Thread:
    """One conversation: the main agent, or a subagent (by its Agent call id)."""

    segments: list = field(default_factory=list)  # [category, tokens]
    pending: list = field(default_factory=list)  # [category, estimated tokens, feature]
    ctx: float | None = None
    calls: int = 0


@dataclass
class Attribution:
    method: str = "usage-delta"
    calls: int = 0
    shrinks: int = 0  # calls whose context was smaller than the previous one's
    tokens: dict = field(default_factory=lambda: {c: {k: 0.0 for k in CLASSES} for c in INPUT_CATEGORIES})
    result_tokens: dict = field(default_factory=lambda: {c: 0.0 for c in INPUT_CATEGORIES})  # Δ shares
    estimated_tokens: dict = field(default_factory=lambda: {c: 0.0 for c in INPUT_CATEGORIES})  # chars/4
    output_tokens: int = 0
    thinking_tokens: int = 0
    per_call: dict = field(default_factory=dict)  # model -> summed per-call usage
    totals: dict = field(default_factory=dict)  # model -> result-event usage
    residual: dict = field(default_factory=dict)  # field -> totals - per-call sums
    cost: dict = field(default_factory=dict)  # category -> USD
    cost_classes: dict = field(default_factory=dict)  # class / "output" -> USD
    cost_usd: float | None = None
    cli_cost_usd: float | None = None
    unpriced: list = field(default_factory=list)
    samples: list = field(default_factory=list)  # (growth, {feature: chars/4 estimate}) per main-thread call

    def models(self) -> list[str]:
        return sorted(set(self.per_call) | set(self.totals))

    # ---- derived -----------------------------------------------------------
    def input_by_category(self) -> dict[str, float]:
        return {c: sum(v.values()) for c, v in self.tokens.items()}

    def by_category(self) -> dict[str, float]:
        """Every token of the session by category, output included."""
        return {**self.input_by_category(), "output": float(self.output_tokens)}

    def total_input(self) -> float:
        return sum(self.input_by_category().values())

    def as_row(self) -> dict:
        """Compact fields for a results row."""
        r = lambda d: {k: round(v, 1) for k, v in d.items() if v}  # noqa: E731
        return {
            "attribution_method": self.method,
            "models": self.models(),
            "tokens_by_category": r(self.by_category()),
            "tokens_by_class": r({k: sum(v[k] for v in self.tokens.values()) for k in CLASSES}),
            "result_tokens_by_category": r(self.result_tokens),
            "estimated_tokens_by_category": r(self.estimated_tokens),
            "thinking_tokens": self.thinking_tokens,
            "attribution_residual": dict(self.residual),
            "cost_by_category": {k: round(v, 6) for k, v in self.cost.items() if v},
            "cost_by_class": {k: round(v, 6) for k, v in self.cost_classes.items() if v},
            "priced_cost_usd": None if self.cost_usd is None else round(self.cost_usd, 6),
        }


def _add_usage(acc: dict, model: str, u: dict) -> None:
    m = acc.setdefault(model, {"input_tokens": 0, "cache_creation_input_tokens": 0,
                               "cache_read_input_tokens": 0, "output_tokens": 0})
    for k in m:
        m[k] += int(u.get(k) or 0)


def _lay(att: Attribution, th: _Thread, cr: int, w5m: int, w1h: int, ctx: float) -> None:
    """Lay one call's usage over the thread's segments, in order."""
    # Longer-lived cache entries come first in a prompt (1-hour before 5-minute breakpoints).
    bounds = [("cache_read", cr), ("cache_write_1h", w1h), ("cache_write_5m", w5m)]
    edges, pos = [], 0.0
    for cls, n in bounds:
        edges.append((cls, pos, pos + n))
        pos += n
    edges.append(("uncached", pos, max(pos, ctx)))
    start = 0.0
    for cat, size in th.segments:
        end = start + size
        for cls, lo, hi in edges:
            overlap = min(end, hi) - max(start, lo)
            if overlap > 0:
                att.tokens[cat][cls] += overlap
        start = end


def attribute(path: str, prices: dict | None = None, rates: dict | None = None) -> Attribution:
    """Attribute the tokens of one stream-json transcript.

    ``rates`` (feature -> multiplier, from :func:`fit_rates`) re-weights the
    chars/4 estimates before a growth is split; default: all 1."""
    prices = PRICES if prices is None else prices
    rates = rates or {}
    att = Attribution()
    threads: dict = {}
    calls_by_id: dict[str, tuple[str, dict]] = {}  # tool_use id -> (name, input)
    seen_msgs: set = set()
    thinking_estimate: dict = {}  # thread -> CLI thinking-token estimate of the call in progress
    msg_thinking: dict = {}  # message id -> that estimate, used by the reply's (empty) thinking block
    call_classes: list = []  # (model, {class: tokens}) per call, for pricing
    result: dict = {}
    missing_usage = False

    def thread(key) -> _Thread:
        return threads.setdefault(key, _Thread())

    with open(path, encoding="utf-8", errors="replace") as fh:
        for line in fh:
            try:
                ev = json.loads(line)
            except ValueError:
                continue
            typ = ev.get("type")
            key = ev.get("parent_tool_use_id")
            if typ == "system" and ev.get("subtype") == "thinking_tokens":
                thinking_estimate[key] = max(thinking_estimate.get(key, 0), int(ev.get("estimated_tokens") or 0))
            elif typ == "system" and ev.get("subtype") == "hook_response":
                text = ""
                try:
                    out = json.loads(ev.get("output") or ev.get("stdout") or "{}")
                    text = str(((out or {}).get("hookSpecificOutput") or {}).get("additionalContext") or "")
                except (ValueError, AttributeError):
                    text = ""
                if text:
                    thread(None).pending.append(["hook", len(text) / CHARS_PER_TOKEN, "hook"])
            elif typ == "assistant":
                msg = ev.get("message") or {}
                th = thread(key)
                mid = msg.get("id") or f"anon-{len(seen_msgs)}"
                if mid not in seen_msgs:
                    seen_msgs.add(mid)
                    u = msg.get("usage") or {}
                    model = msg.get("model") or "?"
                    if u:
                        ctx, cr, w5m, w1h, _ = _usage_ctx(u)
                        _add_usage(att.per_call, model, u)
                    else:
                        missing_usage = True
                        ctx = (th.ctx or 0.0) + sum(w for _, w, _ in th.pending)
                        cr, w5m, w1h = 0, 0, 0
                    if th.ctx is None:
                        th.segments.append(["base", float(ctx)])
                        th.pending = []
                    else:
                        delta = ctx - th.ctx
                        if delta < 0:
                            att.shrinks += 1
                            scale = ctx / th.ctx if th.ctx else 0.0
                            for s in th.segments:
                                s[1] *= scale
                        elif delta > 0:
                            weights = [w * rates.get(f, 1.0) for _, w, f in th.pending]
                            weight = sum(weights)
                            if weight > 0:
                                for (cat, _, _), w in zip(th.pending, weights):
                                    th.segments.append([cat, delta * w / weight])
                                    att.result_tokens[cat] += delta * w / weight
                            else:
                                th.segments.append(["other", float(delta)])
                                att.result_tokens["other"] += delta
                        if u and key is None:  # a sample for the cross-check (aeval/tokens.fit_rates)
                            feats: dict = {}
                            for _, w, f in th.pending:
                                feats[f] = feats.get(f, 0.0) + w
                            att.samples.append((delta, feats))
                        for cat, w, _ in th.pending:
                            att.estimated_tokens[cat] += w
                    th.pending = [["assistant", 0.0, "reply"], ["assistant", 0.0, "thinking"]]
                    before = {c: dict(v) for c, v in att.tokens.items()}
                    _lay(att, th, cr, w5m, w1h, ctx)
                    classes = {k: sum(att.tokens[c][k] - before[c][k] for c in INPUT_CATEGORIES) for k in CLASSES}
                    call_classes.append((model, classes))
                    th.ctx = float(ctx)
                    th.calls += 1
                    att.calls += 1
                    msg_thinking[mid] = thinking_estimate.pop(key, 0)
                # The reply's own size: it is part of the next call's context.
                size, thought = 0.0, 0.0
                for b in msg.get("content") or []:
                    if b.get("type") == "text":
                        size += len(b.get("text") or "")
                    elif b.get("type") == "tool_use":
                        size += len(b.get("name") or "") + len(json.dumps(b.get("input") or {}))
                        calls_by_id[b.get("id")] = (b.get("name"), b.get("input") or {})
                    elif b.get("type") == "thinking":
                        text = b.get("thinking") or ""
                        thought += len(text) / CHARS_PER_TOKEN if text else msg_thinking.pop(mid, 0)
                if len(th.pending) >= 2 and th.pending[0][2] == "reply" and th.pending[1][2] == "thinking":
                    th.pending[0][1] += size / CHARS_PER_TOKEN
                    th.pending[1][1] += thought
            elif typ == "user":
                content = (ev.get("message") or {}).get("content")
                th = thread(key)
                if isinstance(content, list):
                    for b in content:
                        if not isinstance(b, dict):
                            continue
                        if b.get("type") == "tool_result":
                            name, inp = calls_by_id.get(b.get("tool_use_id"), ("?", {}))
                            cat = category_of(name, inp)
                            th.pending.append([cat, _text_len(b.get("content")) / CHARS_PER_TOKEN, cat])
                        elif b.get("type") == "text" and th.ctx is not None:
                            th.pending.append(["other", len(b.get("text") or "") / CHARS_PER_TOKEN, "other"])
                elif isinstance(content, str) and th.ctx is not None:
                    th.pending.append(["other", len(content) / CHARS_PER_TOKEN, "other"])
            elif typ == "result":
                result = ev

    # ---- session totals, residual, output -----------------------------------
    model_usage = result.get("modelUsage") or {}
    w1h_share: dict[str, float] = {}  # model -> share of its cache writes with the 1-hour TTL
    for model, mu in model_usage.items():
        att.totals[model] = {"input_tokens": int(mu.get("inputTokens") or 0),
                             "cache_creation_input_tokens": int(mu.get("cacheCreationInputTokens") or 0),
                             "cache_read_input_tokens": int(mu.get("cacheReadInputTokens") or 0),
                             "output_tokens": int(mu.get("outputTokens") or 0)}
        att.thinking_tokens += int(mu.get("thinkingTokens") or 0)
        cc = att.totals[model]["cache_creation_input_tokens"]
        w1h_share[model] = int(mu.get("cacheCreationEphemeral1hInputTokens") or 0) / cc if cc else 0.0
    if not model_usage and result.get("usage"):
        u = result["usage"]
        model = next(iter(att.per_call), "?")
        att.totals[model] = {k: int(u.get(k) or 0) for k in ("input_tokens", "cache_creation_input_tokens",
                                                             "cache_read_input_tokens", "output_tokens")}
        cc = att.totals[model]["cache_creation_input_tokens"]
        w1h = int(((u.get("cache_creation") or {}).get("ephemeral_1h_input_tokens")) or 0)
        w1h_share[model] = w1h / cc if cc else 0.0
    att.output_tokens = sum(t["output_tokens"] for t in att.totals.values())
    fields = ("input_tokens", "cache_creation_input_tokens", "cache_read_input_tokens")

    def classes_of(model: str, d: dict) -> dict:
        cc = d.get("cache_creation_input_tokens", 0)
        share = w1h_share.get(model, 0.0)
        return {"uncached": d.get("input_tokens", 0), "cache_write_5m": cc * (1 - share),
                "cache_write_1h": cc * share, "cache_read": d.get("cache_read_input_tokens", 0)}

    if missing_usage and att.totals:
        # chars/4: the estimated split, scaled to the session totals and priced
        # with the totals' classes; nothing is left to reconcile.
        att.method = "chars/4"
        call_classes = [(m, classes_of(m, t)) for m, t in att.totals.items()]
        _rescale_estimated(att, {k: sum(c[k] for _, c in call_classes) for k in CLASSES})
    elif missing_usage:
        att.method = "chars/4"
    residual_classes: list = []
    if att.totals and not missing_usage:  # without a result event there is nothing to reconcile against
        for f in fields:
            att.residual[f] = sum(t[f] for t in att.totals.values()) - sum(p[f] for p in att.per_call.values())
        for model in sorted(set(att.totals) | set(att.per_call)):
            tot = att.totals.get(model) or {}
            pc = att.per_call.get(model) or {}
            cls = classes_of(model, {f: tot.get(f, 0) - pc.get(f, 0) for f in fields})
            for k, v in cls.items():
                att.tokens["unattributed"][k] += v
            residual_classes.append((model, cls))

    # ---- cost ----------------------------------------------------------------
    att.cli_cost_usd = result.get("total_cost_usd")
    _price(att, call_classes + residual_classes, prices)
    return att


def _rescale_estimated(att: Attribution, by_class: dict) -> None:
    """chars/4 fallback: scale the estimated categories to the session's
    input total and split each over the classes in the totals' proportions."""
    total = sum(by_class.values())
    est = sum(sum(v.values()) for v in att.tokens.values())
    if not total or not est:
        return
    for cat, v in att.tokens.items():
        n = sum(v.values()) * total / est
        att.tokens[cat] = {k: n * by_class[k] / total for k in CLASSES}


def _price(att: Attribution, call_classes: list, prices: dict) -> None:
    """Price each call's classes and spread each call's cost over the
    categories in proportion to their tokens of that class (summed over the
    session; exact when every call uses one model)."""
    by_model_class: dict = {}
    for model, classes in call_classes:
        d = by_model_class.setdefault(model, {k: 0.0 for k in CLASSES})
        for k, v in classes.items():
            d[k] += v
    class_cost = {k: 0.0 for k in CLASSES}
    unpriced = set()
    for model, classes in by_model_class.items():
        p = price_of(model, prices)
        if p is None:
            if any(classes.values()):
                unpriced.add(model)
            continue
        for k, v in classes.items():
            class_cost[k] += v * p[CLASS_PRICE[k]] / 1e6
    out_cost = 0.0
    for model, tot in att.totals.items():
        p = price_of(model, prices)
        if p is None:
            if tot["output_tokens"]:
                unpriced.add(model)
            continue
        out_cost += tot["output_tokens"] * p["output"] / 1e6
    att.unpriced = sorted(unpriced)
    att.cost_classes = {**class_cost, "output": out_cost}
    totals_by_class = {k: sum(att.tokens[c][k] for c in INPUT_CATEGORIES) for k in CLASSES}
    att.cost = {c: 0.0 for c in CATEGORIES}
    for c in INPUT_CATEGORIES:
        for k in CLASSES:
            if totals_by_class[k]:
                att.cost[c] += class_cost[k] * att.tokens[c][k] / totals_by_class[k]
    att.cost["output"] = out_cost
    att.cost_usd = None if att.unpriced else sum(att.cost_classes.values())


# --------------------------------------------------------------------------
# Cross-check of the split
# --------------------------------------------------------------------------

FEATURES = ("reply", "thinking", "hook", "myceliums", "search", "read", "edit", "other")
MIN_NONZERO = 20  # a feature present in fewer calls is not fitted (rate 1)


def _solve(a: list[list[float]], b: list[float]) -> list[float] | None:
    """Gaussian elimination with partial pivoting; None when singular."""
    n = len(b)
    m = [row[:] + [b[i]] for i, row in enumerate(a)]
    for i in range(n):
        piv = max(range(i, n), key=lambda r: abs(m[r][i]))
        if abs(m[piv][i]) < 1e-12:
            return None
        m[i], m[piv] = m[piv], m[i]
        for r in range(n):
            if r != i and m[r][i]:
                f = m[r][i] / m[i][i]
                m[r] = [x - f * y for x, y in zip(m[r], m[i])]
    return [m[i][n] / m[i][i] for i in range(n)]


def fit_rates(samples: list[tuple[float, dict]]) -> dict:
    """Least-squares fit of each call's measured context growth on the
    chars/4 estimates of what arrived before it, one coefficient per feature
    plus a constant per call (#178).

    This checks the split independently of the reconciliation: the split
    divides a growth in proportion to the estimates, which is fair between two
    kinds of content (myceliums results and grep output, say) only if the
    estimate undercounts both by the same factor. A fitted coefficient is the
    measured tokens per estimated token of that feature (per character: divided
    by 4; for ``thinking`` the unit is the CLI's thinking-token estimate).
    Features present in fewer than :data:`MIN_NONZERO` calls, or constant
    over all calls, are left out.
    """
    counts = {f: sum(1 for _, x in samples if x.get(f)) for f in FEATURES}

    def varies(f: str) -> bool:  # a constant feature is indistinguishable from the constant term
        vals = [x.get(f, 0.0) for _, x in samples]
        return max(vals) - min(vals) > 1e-9

    feats = [f for f in FEATURES if counts[f] >= MIN_NONZERO and varies(f)]
    rows = [[x.get(f, 0.0) for f in feats] + [1.0] for _, x in samples]
    y = [d for d, _ in samples]
    k = len(feats) + 1
    out = {"n_calls": len(samples), "features": {}, "constant": None, "r2": None,
           "left_out": sorted(f for f in FEATURES if counts[f] and f not in feats)}
    if len(rows) <= k:
        return out
    ata = [[sum(r[i] * r[j] for r in rows) for j in range(k)] for i in range(k)]
    aty = [sum(r[i] * yy for r, yy in zip(rows, y)) for i in range(k)]
    coef = _solve(ata, aty)
    if coef is None:
        return out
    mean_y = sum(y) / len(y)
    ss_tot = sum((v - mean_y) ** 2 for v in y)
    ss_res = sum((yy - sum(c * v for c, v in zip(coef, r))) ** 2 for r, yy in zip(rows, y))
    for f, c in zip(feats, coef):
        out["features"][f] = {"coef": round(c, 4), "n_calls": counts[f],
                              **({"tokens_per_char": round(c / CHARS_PER_TOKEN, 4)} if f != "thinking" else {})}
    out["constant"] = round(coef[-1], 2)
    out["r2"] = round(1 - ss_res / ss_tot, 4) if ss_tot else None
    return out


def rates_of(fit: dict) -> dict[str, float]:
    """Multipliers for :func:`attribute` from a fit (unfitted features: 1)."""
    return {f: max(0.0, v["coef"]) for f, v in (fit.get("features") or {}).items()}
