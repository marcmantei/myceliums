"""What a shell command does, by the programs it runs (#178).

Two users, one parser:

* token attribution (:mod:`aeval.tokens`) and the search audit sort each
  ``Bash`` call into ``myceliums``, ``search``, ``edit``, ``read`` or
  ``other`` (:func:`classify`);
* arm ``D`` ("myceliums only") runs this file as its PreToolUse hook
  (``python3 -I bashcmd.py hook --allow PREFIX ...``) and lets a ``Bash``
  command through only when **every** program it runs is on an allowlist
  (:func:`allowed`). The file imports nothing outside the standard library.

Parsing. A command is split into lines, each line into words with
:mod:`shlex` (quotes respected, ``| ; & && || ( )`` as separators,
redirections and their targets dropped), and the words into simple commands.
A program is a simple command's first word after environment assignments and
wrapper words (``sudo``, ``env``, ``nice``, ``nohup``, ``time``,
``timeout DURATION``, ``stdbuf -oL``, ``xargs``, ``command``, ``exec``,
``builtin``), with a leading backslash and any directory removed. ``git``
global options (``-C DIR``, ``-c K=V``, ``--no-pager``, ...) are skipped, so
``git -C . grep`` is ``git grep``. ``bash -c``/``sh -c``/``zsh -c`` scripts
are parsed recursively.

Classification (attribution and audit): ``myc`` is myceliums; ``grep``,
``egrep``, ``fgrep``, ``rg``, ``ag``, ``ack``, ``ugrep``, ``find``, ``fd``,
``locate``, ``git grep``, ``git ls-files``, ``ls -R``, ``tree``, ``awk``,
``perl -n/-p/-e``, ``sed`` with ``-n`` and a ``/regex/`` address, and ``git
log -S/-G/--grep`` are search; ``sed -i`` is an edit; ``cat``, ``head``,
``tail``, ``nl``, ``less``, ``more``, ``bat``, other ``sed`` are reads. The
highest kind wins: myceliums > search > edit > read > other. Code run by an
interpreter (``python -c``, heredocs) is ``other``: its intent is not
visible; the audit counts such calls separately.

The allowlist of arm D (:data:`D_ALLOWED`): ``myc``; ``cd``, ``pwd``,
``true``; plain file operations an edit needs (``mkdir``, ``touch``, ``mv``,
``cp``, ``rm``, ``chmod``); ``git status``, ``git diff``, ``git add``,
``git commit``, ``git log``, ``git restore``, ``git show`` without any
search option and without global options that load configuration or another
repository (``-c``, ``--config-env``, ``--exec-path``, ``--git-dir``, ...);
and exactly the task's own build and test commands (:func:`task_rules`).
Everything else is denied, as is any command substitution (``$(...)``,
backticks, ``<(...)``), any environment assignment or wrapper in front of a
command, any argument that names a path inside ``.git``, ``git --output``,
and any command that does not parse (unbalanced quotes, a heredoc never
closed). File operands (of ``cd``, ``mkdir``, ``touch``, ``mv``, ``cp``,
``rm``, ``chmod``) and redirection targets must not contain glob or
expansion characters (``* ? [ { ~ $``), must not name ``.git`` or a git
config, and must stay inside the working copy (the hook's ``cwd``). The same
hook refuses ``Edit``/``Write`` outside the working copy, into ``.git`` and
to ``~/.gitconfig`` / ``~/.config/git``, and the harness pins git's
configuration through the session's environment (aeval/runner.py
``nosearch_git_env``). This guards a cooperative agent told not to search;
it is not a sandbox (see the README's threat model).
"""

from __future__ import annotations

import json
import os
import re
import shlex
import sys

SEARCH_PROGRAMS = frozenset({"grep", "egrep", "fgrep", "rg", "ag", "ack", "ugrep", "find", "fd", "locate", "tree",
                             "awk", "gawk", "mawk"})
READ_PROGRAMS = frozenset({"cat", "head", "tail", "sed", "nl", "less", "more", "bat", "view"})
SHELLS = frozenset({"bash", "sh", "zsh", "dash", "ksh"})
WRAPPERS = frozenset({"sudo", "time", "env", "nice", "nohup", "xargs", "command", "exec", "builtin", "stdbuf",
                      "timeout", "unbuffer"})
WRAPPER_OPTS_WITH_VALUE = {"nice": {"-n", "--adjustment"}, "timeout": {"-s", "-k", "--signal", "--kill-after"},
                           "env": {"-u", "-C", "-S", "--unset", "--chdir"}, "sudo": {"-u", "-g", "-C"},
                           "stdbuf": {"-i", "-o", "-e"},
                           "xargs": {"-I", "-n", "-P", "-d", "-L", "-s", "-E", "-a", "--max-args", "--max-procs"}}
GIT_OPTS_WITH_VALUE = frozenset({"-C", "-c", "--git-dir", "--work-tree", "--namespace", "--exec-path"})
GIT_SEARCH_OPTS = re.compile(r"^(--grep|-S|-G|--pickaxe|-L|--find-object)")
EDIT_TOOLS = frozenset({"Edit", "Write", "MultiEdit", "NotebookEdit"})
SEPARATORS = frozenset({"|", "||", "&", "&&", ";", "(", ")", "|&", ";;"})

D_ALLOWED_PROGRAMS = frozenset({"myc", "cd", "pwd", "true", "mkdir", "touch", "mv", "cp", "rm", "chmod"})
D_ALLOWED_GIT = frozenset({"status", "diff", "add", "commit", "log", "restore", "show"})
D_ALLOWED = sorted(D_ALLOWED_PROGRAMS) + [f"git {s}" for s in sorted(D_ALLOWED_GIT)]


class Unparsed(ValueError):
    """A command :mod:`shlex` cannot split (unbalanced quotes), or one whose
    heredoc is still open at the end."""


def _tokens(line: str) -> list[str]:
    lex = shlex.shlex(line, posix=True, punctuation_chars=True)
    lex.whitespace_split = True
    lex.commenters = ""  # `#` inside a word (`${f#*:}`) is not a comment
    try:
        toks = list(lex)
    except ValueError as exc:
        raise Unparsed(str(exc)) from exc
    for i, t in enumerate(toks):
        if t.startswith("#"):  # a comment starts at a word boundary
            return toks[:i]
    return toks


def simple_commands(command: str) -> list[list[str]]:
    """The word lists of every simple command in ``command``; heredoc bodies
    are skipped (raises :class:`Unparsed`)."""
    return parse(command)[0]


def parse(command: str) -> tuple[list[list[str]], list[tuple[int, str]]]:
    """``(simple commands, redirection targets)``; a target is ``(index of its
    simple command, word)``: the file of ``> f``, ``>> f``, ``< f``, ``&> f``,
    ``>& f`` (not an fd duplication like ``2>&1``, not a heredoc word)."""
    out: list[list[str]] = []
    targets: list[tuple[int, str]] = []
    heredoc: list[tuple[str, bool]] = []  # (delimiter, leading tabs stripped: `<<-`) still to be met
    for line in (command or "").splitlines():
        if heredoc:
            # As bash: the terminator is the delimiter alone on its line, after
            # leading tabs only for `<<-`.
            delim, tabs = heredoc[0]
            if (line.lstrip("\t") if tabs else line) == delim:
                heredoc.pop(0)
            continue
        if not line.strip() or line.lstrip().startswith("#"):
            continue
        cur: list[str] = []
        toks = _tokens(line)
        i = 0
        while i < len(toks):
            t = toks[i]
            if t in SEPARATORS:
                if cur:
                    out.append(cur)
                cur = []
            elif set(t) <= set("<>&|") and ("<" in t or ">" in t):
                # a redirection: drop its fd (`2>`) and its target (`/dev/null`, `1`, a heredoc word)
                if cur and cur[-1].isdigit():
                    cur.pop()
                if t in ("<<", "<<-") and i + 1 < len(toks):
                    word, tabs = toks[i + 1], t == "<<-"
                    if word == "-" and i + 2 < len(toks):  # `<<- EOF`
                        i, word, tabs = i + 1, toks[i + 2], True
                    elif word.startswith("-") and t == "<<":  # `<<-EOF` splits as `<<`, `-EOF`
                        word, tabs = word[1:], True
                    heredoc.append((word, tabs))
                elif t != "<<<" and i + 1 < len(toks):
                    word = toks[i + 1]
                    if not (t.endswith("&") and (word.isdigit() or word == "-")):
                        targets.append((len(out), word))
                i += 1
            else:
                cur.append(t)
            i += 1
        if cur:
            out.append(cur)
    if heredoc:
        raise Unparsed(f"heredoc {heredoc[0][0]!r} is never closed")
    return out, targets


KEYWORDS = frozenset({"do", "then", "else", "elif", "if", "while", "until", "!", "{", "time"})
HEADERS = frozenset({"for", "select", "case", "done", "fi", "esac", "}", "function"})


def program(words: list[str]) -> tuple[str, list[str]]:
    """``(program, args)`` of one simple command, wrappers, shell keywords
    and git options removed; ``git grep`` -> ``("git grep", args)``; a loop
    header (``for f in ...``) or a closing keyword -> ``("", [])``."""
    w = list(words)
    if w and w[0] in HEADERS:
        return "", []
    while w:
        head = w[0]
        if head in KEYWORDS:
            w = w[1:]
        elif re.match(r"^[A-Za-z_][A-Za-z0-9_]*=", head):
            w = w[1:]
        elif head.lstrip("\\").rsplit("/", 1)[-1] in WRAPPERS:
            name = head.lstrip("\\").rsplit("/", 1)[-1]
            w = w[1:]
            while w and w[0].startswith("-"):
                opt = w.pop(0)
                if opt in WRAPPER_OPTS_WITH_VALUE.get(name, ()) and w:
                    w.pop(0)
            if name == "timeout" and w and re.match(r"^\d+(\.\d+)?[smhd]?$", w[0]):
                w = w[1:]
        else:
            break
    if not w:
        return "", []
    prog = w[0].lstrip("\\").rsplit("/", 1)[-1]
    args = w[1:]
    if prog == "git":
        while args and args[0].startswith("-"):
            opt = args.pop(0)
            if opt in GIT_OPTS_WITH_VALUE and args:
                args.pop(0)
        if args:
            return f"git {args[0]}", args[1:]
    return prog, args


def programs(command: str) -> list[tuple[str, list[str]]]:
    """``(program, args)`` of every simple command, ``sh -c`` scripts expanded;
    an unparsable line yields ``("?", [line])``."""
    out = []
    try:
        cmds = simple_commands(command)
    except Unparsed:
        # Unbalanced quotes: fall back to splitting at every separator and space.
        cmds = [ln.split() for ln in re.split(r"\$\(|&&|\|\||[;|&\n()`]", command or "") if ln.split()]
    for words in cmds:
        for w in words:  # command substitutions inside a word: `"== $(grep -l x)"`
            for inner in w.split("$(")[1:]:
                out.extend(programs(inner.rstrip(")")))
            for inner in w.split("`")[1::2]:
                out.extend(programs(inner))
        prog, args = program(words)
        if not prog:
            continue
        if prog in SHELLS and "-c" in args[:-1]:
            out.extend(programs(args[args.index("-c") + 1]))
            continue
        if prog == "eval":
            out.extend(programs(" ".join(args)))
            continue
        out.append((prog, args))
    return out


def _kind(prog: str, args: list[str]) -> str:
    if prog == "myc":
        return "myceliums"
    if prog in SEARCH_PROGRAMS or prog in ("git grep", "git ls-files"):
        return "search"
    if prog == "ls" and any(a.startswith("-") and "R" in a for a in args):
        return "search"
    if prog in ("git log", "git diff", "git show") and any(GIT_SEARCH_OPTS.match(a) for a in args):
        return "search"
    if prog == "perl" and any(a.startswith("-") and set(a[1:]) & set("nep") for a in args):
        return "search"
    if prog == "sed":
        if any(a == "-i" or a.startswith("-i") or a.startswith("--in-place") for a in args):
            return "edit"
        if "-n" in args and any(re.search(r"(^|[^\\])/.+/", a) for a in args if not a.startswith("-")):
            return "search"
        return "read"
    if prog in READ_PROGRAMS:
        return "read"
    return "other"


def classify(command: str) -> str:
    """``myceliums``, ``search``, ``edit``, ``read`` or ``other``."""
    kinds = {_kind(p, a) for p, a in programs(command)}
    for kind in ("myceliums", "search", "edit", "read"):
        if kind in kinds:
            return kind
    return "other"


def search_programs(command: str) -> list[str]:
    """The search programs ``command`` runs (empty: none)."""
    return [p for p, a in programs(command) if _kind(p, a) == "search"]


def interpreted(command: str) -> bool:
    """True when ``command`` runs code through an interpreter (its intent is not visible)."""
    return any(p in ("python", "python3", "node", "ruby", "perl", "php") or p.startswith("python3.")
               for p, _ in programs(command))


# git options that load configuration, code or another repository: `-c
# core.fsmonitor='grep ...'` runs any command, so none of them is allowed in D.
GIT_UNSAFE_OPTS = ("-c", "--config-env", "--exec-path", "--git-dir", "--work-tree", "--namespace",
                   "--super-prefix", "--attr-source", "--list-cmds")
_DOT_GIT = re.compile(r"(^|[/=])\.git(/|$)")


def touches_dot_git(path: str) -> bool:
    """True when ``path`` has a ``.git`` component (``.git/config``, ``x/.git``)."""
    return bool(_DOT_GIT.search(path or ""))


def task_rules(extra) -> list:
    """The task's own commands as rules: a string allows each simple command
    of it exactly (same words); ``{"prefix": "npx vitest run", "args": REGEX}``
    allows the prefix followed by arguments that, joined by spaces, match REGEX
    in full."""
    rules = []
    for e in extra or ():
        if isinstance(e, str):
            if e.strip():
                rules.extend(("exact", words, None) for words in simple_commands(e))
        else:
            rules.append(("pattern", str(e["prefix"]).split(), re.compile(str(e["args"]))))
    return rules


def _task_allows(words: list[str], rules: list) -> bool:
    for kind, w, rx in rules:
        if kind == "exact" and words == w:
            return True
        if kind == "pattern" and words[:len(w)] == w and rx.fullmatch(" ".join(words[len(w):])):
            return True
    return False


PATH_PROGRAMS = frozenset({"cd", "mkdir", "touch", "mv", "cp", "rm", "chmod"})
EXPANSION_CHARS = set("*?[{~$")  # globs, brace, tilde and variable expansion: the shell may turn them into .git
DEVICES = frozenset({"/dev/null", "/dev/stdout", "/dev/stderr"})


def _inside(path: str, cwd: str, root: str) -> bool:
    full = os.path.realpath(os.path.join(cwd, path))
    root = os.path.realpath(root)
    return full == root or full.startswith(root.rstrip("/") + "/")


def path_problem(word: str, cwd: str | None, root: str | None) -> str | None:
    """Why ``word`` may not be used as a file operand or a write target in D:
    an expansion character, a ``.git`` path, a home git config, or (with the
    session's working copy ``root``) a path outside it; None when fine."""
    if word in DEVICES:
        return None
    if set(word) & EXPANSION_CHARS:
        return f"`{word}` (globs and expansions are not allowed in file operands)"
    if touches_dot_git(word) or touches_home_git_config(word):
        return "a path inside .git or a git config"
    if root and not _inside(word, cwd or root, root):
        return f"`{word}` (outside the working copy)"
    return None


def touches_home_git_config(path: str) -> bool:
    home = os.path.expanduser("~")
    full = os.path.realpath(os.path.expanduser(path)) if path else ""
    return (os.path.basename(path or "") == ".gitconfig"
            or full == os.path.join(home, ".gitconfig")
            or full.startswith(os.path.join(home, ".config", "git") + os.sep)
            or "/.config/git/" in (path or "") + "/")


def allowed(command: str, extra=(), cwd: str | None = None) -> tuple[bool, list[str]]:
    """Arm D's allowlist: ``(ok, denied programs)``. ``extra`` are the task's
    own build/test commands (:func:`task_rules`); ``cwd`` is the session's
    working copy (the hook passes it): file operands and write targets must
    stay inside it."""
    if "$(" in command or "`" in command or "<(" in command or ">(" in command:
        return False, ["command substitution"]
    try:
        cmds, targets = parse(command)
    except Unparsed:
        return False, ["unparsable command"]
    rules = task_rules(extra)
    denied = []
    here = cwd
    for k, words in enumerate(cmds):
        # Redirection targets, of every command (a task command included): `git show :x > .git/config`.
        bad = [why for idx, t in targets if idx == k for why in [path_problem(t, here, cwd)] if why]
        if bad:
            denied.extend("redirection to " + b for b in bad)
            continue
        if _task_allows(words, rules):
            continue
        head = words[0] if words else ""
        if re.match(r"^[A-Za-z_][A-Za-z0-9_]*=", head):
            denied.append("environment assignment")  # `GIT_EXTERNAL_DIFF=... git diff`
            continue
        if head.lstrip("\\").rsplit("/", 1)[-1] in WRAPPERS:
            denied.append(head)
            continue
        if any(touches_dot_git(w) for w in words):
            denied.append("a path inside .git")
            continue
        prog, args = program(words)
        if not prog:
            continue
        if prog in PATH_PROGRAMS:
            operands = [a for a in args if not a.startswith("-")] or (["~"] if prog == "cd" else [])
            bad = [why for a in operands for why in [path_problem(a, here, cwd)] if why]
            if bad:
                denied.extend(bad)
                continue
            if prog == "cd" and here:
                here = os.path.realpath(os.path.join(here, operands[0]))
        if prog in D_ALLOWED_PROGRAMS:
            continue
        if prog.startswith("git "):
            if any(a.startswith("--output") for a in args):
                denied.append(prog + " --output")
                continue
            pre = words[1:len(words) - len(args) - 1]  # global options before the subcommand
            if any(o == u or o.startswith(u + "=") or (u == "-c" and o.startswith("-c"))
                   for o in pre for u in GIT_UNSAFE_OPTS):
                denied.append("git " + next(o for o in pre if o.startswith("-")).split("=")[0])
                continue
            if prog[4:] in D_ALLOWED_GIT and not any(GIT_SEARCH_OPTS.match(a) for a in args):
                continue
        denied.append(prog)
    return not denied, denied


DENY_MESSAGE = ("This session has no search and only a few shell commands: {progs} cannot run here. Bash "
                "allows {allowed}{extra}; nothing else. Use the myceliums tools to find code, and Read to "
                "read a file.")


def hook(argv: list[str], stdin=sys.stdin, stderr=sys.stderr) -> int:
    """PreToolUse hook for ``Bash`` in arm D: exit 2 (block; the reason on
    stderr, which Claude Code shows the model) unless the command passes
    :func:`allowed`; else 0."""
    extra = []
    for i, a in enumerate(argv[:-1]):
        if a == "--allow":
            extra.append(argv[i + 1])
        elif a == "--allow-json":
            extra.extend(json.loads(argv[i + 1]))
    try:
        event = json.load(stdin)
    except ValueError:
        stderr.write("unreadable hook input\n")
        return 2
    tool, inp = event.get("tool_name"), event.get("tool_input") or {}
    cwd = event.get("cwd") or None
    if tool in EDIT_TOOLS:
        # Writes stay in the working copy and out of .git and git config: planted config could make an
        # allowed git command run anything.
        path = str(inp.get("file_path") or inp.get("notebook_path") or "")
        if touches_dot_git(path) or touches_home_git_config(path) or (cwd and not _inside(path, cwd, cwd)):
            stderr.write(f"Writing there is not allowed in this session (only files of the working copy, "
                         f"not inside .git): {path}\n")
            return 2
        return 0
    if tool != "Bash":
        return 0
    ok, denied = allowed(str(inp.get("command") or ""), extra, cwd)
    if ok:
        return 0
    shown = [e if isinstance(e, str) else f"{e['prefix']} <{e['args']}>" for e in extra]
    stderr.write(DENY_MESSAGE.format(
        progs=", ".join(f"`{p}`" for p in dict.fromkeys(denied)),
        allowed=", ".join(f"`{p}`" for p in D_ALLOWED),
        extra=(" and exactly the task's own commands " + ", ".join(f"`{p}`" for p in shown)) if shown else "")
        + "\n")
    return 2


if __name__ == "__main__":
    if sys.argv[1:2] == ["hook"]:
        sys.exit(hook(sys.argv[2:]))
    for line in sys.stdin:
        print(classify(line), search_programs(line), allowed(line))
