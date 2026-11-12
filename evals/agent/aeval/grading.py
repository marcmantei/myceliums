"""Answer extraction and set-based grading.

An agent ends its reply with a fenced JSON block ``{"answer": [...]}``. The
items are matched one-to-one against the task's gold items (maximum bipartite
matching, so one answer item can never count for two gold items), then scored
with precision, recall and F1.

Matching is deliberately lenient about *formatting* and strict about
*identity*: ``fastapi.routing.serialize_response``, ``serialize_response()``
and ``routing.py:serialize_response`` all name the gold item
``serialize_response``, but ``Pattern.AsString`` never matches ``Item.AsString``.
A bare method name (``dumpItem``) is accepted for ``Terminal.dumpItem``; the
lenience is the same in both conditions.

Answer items fall into three buckets:

* true positive  - matched to a gold item;
* neutral        - names an optional item (a defensible extra the task lists
                   explicitly), or is a second spelling of a gold item that is
                   already matched;
* false positive - everything else.

Precision is TP / (TP + FP); neutral items are ignored. Recall is TP / |gold|.
"""

from __future__ import annotations

import json
import re
from dataclasses import dataclass, field

# --------------------------------------------------------------------------
# Answer extraction
# --------------------------------------------------------------------------

_FENCE_RE = re.compile(r"```(?:json|JSON)?\s*\n(.*?)```", re.DOTALL)


def extract_answer(text: str | None) -> list[str] | None:
    """Return the answer list from the last JSON block in ``text``.

    Looks at fenced code blocks first (last one wins), then falls back to the
    last ``{"answer": ...}`` object anywhere in the text. Returns ``None`` when
    no parseable answer exists, which the grader scores as F1 = 0.
    """
    if not text:
        return None
    candidates = [m.group(1) for m in _FENCE_RE.finditer(text)]
    # Fallback: bare JSON objects that contain an "answer" key.
    for m in re.finditer(r"\{[^{}]*\"answer\"\s*:\s*\[[^\]]*\][^{}]*\}", text, re.DOTALL):
        candidates.append(m.group(0))
    for raw in reversed(candidates):
        try:
            obj = json.loads(raw.strip())
        except (json.JSONDecodeError, ValueError):
            continue
        if isinstance(obj, dict) and isinstance(obj.get("answer"), list):
            return [str(x) for x in obj["answer"] if isinstance(x, (str, int, float))]
        if isinstance(obj, list):
            return [str(x) for x in obj if isinstance(x, (str, int, float))]
    return None


# --------------------------------------------------------------------------
# Normalisation
# --------------------------------------------------------------------------

_SOURCE_EXT = r"(?:py|pyi|go|ts|tsx|js|jsx|mjs|rs|java|rb)"


def normalize_symbol(raw: str) -> str:
    """Canonical, case-folded dotted form of a symbol answer item."""
    s = raw.strip().strip("`'\" ")
    # "src/terminal.go:5719 Terminal.dumpItem" / "routing.py:serialize_response"
    s = re.sub(r"^.*?\.%s(?::\d+)*\s*(?:::|:|#|\s|-)\s*" % _SOURCE_EXT, "", s)
    s = re.sub(r"^func\s+", "", s)
    # Go method declaration forms: "(t *Terminal) dumpItem", "(*Terminal).dumpItem"
    s = re.sub(r"^\(\s*\w+\s+\*?\s*([\w.]+)\s*\)\s*\.?\s*", r"\1.", s)
    s = re.sub(r"\(\s*\*?\s*([\w.]+)\s*\)\s*\.", r"\1.", s)
    # Strip call parentheses / signatures: "foo(x, y)" -> "foo", "foo()" -> "foo"
    s = re.sub(r"\(.*$", "", s)
    # Generic parameters: "ConcurrentSet[T].ForEach", "Foo<T>.bar"
    s = re.sub(r"\[[^\]]*\]", "", s)
    s = re.sub(r"<[^>]*>", "", s)
    s = s.replace("::", ".").replace(".prototype.", ".")
    s = s.replace(".#", ".").replace("#", ".")
    s = s.lstrip("*&.").rstrip(".")
    s = re.sub(r"\.\*", ".", s)
    s = re.sub(r"\s+", "", s)
    return s.lower()


def normalize_file(raw: str, repo: str) -> str:
    """Repository-relative path of a file answer item.

    Absolute paths are cut at the *first* ``/<repo>/`` component, which is the
    checkout root: in ``/tmp/aeval/fastapi/fastapi/routing.py`` the second
    ``fastapi/`` is the package directory and must survive.
    """
    s = raw.strip().strip("`'\" ")
    s = re.sub(r"#L\d+.*$", "", s)
    s = re.sub(r"(:\d+)+$", "", s)
    s = s.replace("\\", "/")
    marker = f"/{repo}/"
    if s.startswith("/") and marker in s:
        s = s[s.index(marker) + len(marker):]
    while s.startswith("./"):
        s = s[2:]
    return s.lstrip("/")


def _symbol_matches(answer: str, name: str) -> bool:
    """True when a normalised answer item names the normalised gold ``name``."""
    if not answer or not name:
        return False
    if answer == name:
        return True
    if answer.endswith("." + name):  # more qualified answer
        return True
    if "." in answer and name.endswith("." + answer):  # partially qualified answer
        return True
    if "." not in answer and name.rsplit(".", 1)[-1] == answer:  # bare method name
        return True
    return False


def _file_matches(answer: str, name: str) -> bool:
    return bool(answer) and (answer == name or answer.endswith("/" + name))


# --------------------------------------------------------------------------
# Grading
# --------------------------------------------------------------------------


@dataclass
class Grade:
    precision: float
    recall: float
    f1: float
    tp: int
    fp: int
    neutral: int
    n_gold: int
    n_answer: int
    parsed: bool
    matched: list[str] = field(default_factory=list)
    missed: list[str] = field(default_factory=list)
    false_positives: list[str] = field(default_factory=list)

    def as_dict(self) -> dict:
        return dict(self.__dict__)


def _names(item: dict) -> list[str]:
    return [item["name"], *item.get("aliases", [])]


def _max_matching(adj: list[list[int]], n_right: int) -> list[int]:
    """Kuhn's augmenting-path maximum bipartite matching.

    ``adj[i]`` lists the right-hand vertices answer ``i`` may match. Returns
    ``match_right`` where ``match_right[j]`` is the answer index matched to
    gold item ``j`` (or -1).
    """
    match_right = [-1] * n_right

    def augment(i: int, seen: list[bool]) -> bool:
        for j in adj[i]:
            if seen[j]:
                continue
            seen[j] = True
            if match_right[j] == -1 or augment(match_right[j], seen):
                match_right[j] = i
                return True
        return False

    for i in range(len(adj)):
        augment(i, [False] * n_right)
    return match_right


def grade(task: dict, answer: list[str] | None, success_threshold: float = 0.8) -> Grade:
    """Score ``answer`` against ``task["gold"]`` (and ``task.get("optional")``)."""
    gold = task["gold"]
    optional = task.get("optional", [])
    kind = task["answer_kind"]
    repo = task["repo"]
    if answer is None:
        return Grade(0.0, 0.0, 0.0, 0, 0, 0, len(gold), 0, False,
                     missed=[g["name"] for g in gold])

    if kind == "files":
        norm = lambda s: normalize_file(s, repo)  # noqa: E731
        match = _file_matches
    else:
        norm = normalize_symbol
        match = _symbol_matches

    # De-duplicate answers after normalisation, keeping the first spelling.
    seen: set[str] = set()
    items: list[tuple[str, str]] = []
    for raw in answer:
        n = norm(raw)
        if n and n not in seen:
            seen.add(n)
            items.append((raw, n))

    gold_names = [[norm(x) for x in _names(g)] for g in gold]
    opt_names = [[norm(x) for x in _names(o)] for o in optional]

    adj = [[j for j, names in enumerate(gold_names) if any(match(n, g) for g in names)]
           for _, n in items]
    match_right = _max_matching(adj, len(gold))
    matched_answers = {i for i in match_right if i != -1}

    tp = len(matched_answers)
    fp = 0
    neutral = 0
    false_positives: list[str] = []
    for i, (raw, n) in enumerate(items):
        if i in matched_answers:
            continue
        if adj[i] or any(match(n, o) for names in opt_names for o in names):
            neutral += 1  # second spelling of a matched gold item, or optional
        else:
            fp += 1
            false_positives.append(raw)

    precision = tp / (tp + fp) if (tp + fp) else 0.0
    recall = tp / len(gold) if gold else 0.0
    f1 = 2 * precision * recall / (precision + recall) if (precision + recall) else 0.0
    return Grade(
        precision=precision,
        recall=recall,
        f1=f1,
        tp=tp,
        fp=fp,
        neutral=neutral,
        n_gold=len(gold),
        n_answer=len(items),
        parsed=True,
        matched=[gold[j]["name"] for j, i in enumerate(match_right) if i != -1],
        missed=[gold[j]["name"] for j, i in enumerate(match_right) if i == -1],
        false_positives=false_positives,
    )


def is_success(f1: float, threshold: float) -> bool:
    return f1 >= threshold - 1e-9
