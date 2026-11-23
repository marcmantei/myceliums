"""What a shell command does, by the programs it runs (#178).

One classifier serves two purposes:

* token attribution (:mod:`aeval.tokens`) sorts each ``Bash`` result into a
  category by the programs the command runs;
* arm ``D`` ("myceliums only") blocks search in ``Bash``: this file is also
  the PreToolUse hook the session runs (``python3 -I bashcmd.py hook``), so
  it imports nothing outside the standard library and the hook and the
  attribution can never disagree about what counts as search.

A command is split into simple commands at ``;``, ``&&``, ``||``, ``|``,
newlines, ``(``, ``)``, backticks and ``$(``. The program of a simple command
is its first word after environment assignments and wrapper words (``sudo``,
``time``, ``env``, ``nice``, ``xargs``, ``command``, ``exec``); ``git grep``
counts as ``grep``. A command that runs several kinds of program gets the
highest-ranked kind: ``myceliums`` > ``search`` > ``edit`` > ``read`` >
``other``. So ``sed -n 1,9p a.py; grep -n x b.py`` is search and
``myc callers x | head`` is myceliums.

The search programs are the ones #178 names (``grep``, ``rg``, ``find``,
``ag``, ``git grep``) plus their close relatives (``egrep``, ``fgrep``,
``ack``, ``fd``, ``locate``). Listing a directory (``ls``, ``tree``) is not
search. The hook cannot see inside an interpreter (``python -c "..."``); it
blocks the programs named here, which is what an agent reaches for.
"""

from __future__ import annotations

import json
import re
import sys

SEARCH_PROGRAMS = frozenset({"grep", "egrep", "fgrep", "rg", "ag", "ack", "find", "fd", "locate"})
READ_PROGRAMS = frozenset({"cat", "head", "tail", "sed", "nl", "less", "more", "bat", "view"})
WRAPPERS = frozenset({"sudo", "time", "env", "nice", "nohup", "xargs", "command", "exec", "builtin"})

_SPLIT = re.compile(r"\$\(|&&|\|\||[;|&\n()`]")
_ASSIGN = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*=")


def _words(simple: str) -> list[str]:
    return [w.strip("'\"") for w in simple.split() if w.strip("'\"")]


def programs(command: str) -> list[tuple[str, list[str]]]:
    """``(program, args)`` of every simple command in ``command``."""
    out = []
    for simple in _SPLIT.split(command or ""):
        words = _words(simple)
        while words and (_ASSIGN.match(words[0]) or words[0] in WRAPPERS):
            if words[0] in WRAPPERS:
                words = words[1:]
                # options of the wrapper (`xargs -0 grep`, `env -i grep`)
                while words and words[0].startswith("-"):
                    words = words[1:]
            else:
                words = words[1:]
        if not words:
            continue
        prog = words[0].rsplit("/", 1)[-1]
        args = words[1:]
        if prog == "git" and args[:1] == ["grep"]:
            prog, args = "grep", args[1:]
        out.append((prog, args))
    return out


def search_programs(command: str) -> list[str]:
    """The search programs ``command`` runs (empty: none)."""
    return [p for p, _ in programs(command) if p in SEARCH_PROGRAMS]


def classify(command: str) -> str:
    """``myceliums``, ``search``, ``edit``, ``read`` or ``other``."""
    kinds = set()
    for prog, args in programs(command):
        if prog == "myc":
            kinds.add("myceliums")
        elif prog in SEARCH_PROGRAMS:
            kinds.add("search")
        elif prog == "sed" and any(a == "-i" or a.startswith("-i") for a in args):
            kinds.add("edit")
        elif prog in READ_PROGRAMS:
            kinds.add("read")
    for kind in ("myceliums", "search", "edit", "read"):
        if kind in kinds:
            return kind
    return "other"


BLOCK_MESSAGE = ("Searching is not available in this session: {progs} cannot run here (Grep, Glob and search "
                 "commands in Bash are disabled). Use the myceliums tools to find code, and Read to read files.")


def hook(stdin=sys.stdin, stderr=sys.stderr) -> int:
    """PreToolUse hook for ``Bash``: exit 2 (block, reason on stderr) when the
    command runs a search program, else 0. Claude Code feeds the stderr of an
    exit-2 PreToolUse hook back to the model as the reason the call failed."""
    try:
        event = json.load(stdin)
    except ValueError:
        return 0
    if event.get("tool_name") != "Bash":
        return 0
    found = search_programs(str((event.get("tool_input") or {}).get("command") or ""))
    if not found:
        return 0
    stderr.write(BLOCK_MESSAGE.format(progs=", ".join(f"`{p}`" for p in dict.fromkeys(found))) + "\n")
    return 2


if __name__ == "__main__":
    if sys.argv[1:] == ["hook"]:
        sys.exit(hook())
    for line in sys.stdin:
        print(classify(line), search_programs(line))
