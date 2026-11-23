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
    lex = shlex.shlex(strip_comment(line), posix=True, punctuation_chars=True)
    lex.whitespace_split = True
    lex.commenters = ""  # `#` inside a word (`${f#*:}`) or quoted (`'#method'`) is not a comment
    try:
        return list(lex)
    except ValueError as exc:
        raise Unparsed(str(exc)) from exc


def quoted_states(text: str) -> list[str] | None:
    """Per character: ``u`` unquoted, ``s`` in single quotes, ``d`` in double
    quotes, ``e`` escaped by a backslash, ``q`` a quote itself, ``b`` such a
    backslash; None when a quote stays open."""
    out, state, i = [], "u", 0
    while i < len(text):
        c = text[i]
        if state == "s":
            out.append("q" if c == "'" else "s")
            state = "u" if c == "'" else "s"
        elif c == "\\" and i + 1 < len(text) and (state == "u" or text[i + 1] in '$`"\\\n'):
            out += ["b", "e"]
            i += 1
        elif state == "d":
            out.append("q" if c == '"' else "d")
            state = "u" if c == '"' else "d"
        elif c in "'\"":
            out.append("q")
            state = "s" if c == "'" else "d"
        else:
            out.append("u")
        i += 1
    return out if state == "u" else None


def strip_comment(line: str) -> str:
    """``line`` without its shell comment: an unquoted ``#`` at a word start."""
    states = quoted_states(line)
    if states is None:
        return line
    for i, c in enumerate(line):
        if c == "#" and states[i] == "u" and (i == 0 or (states[i - 1] == "u" and line[i - 1] in " \t;&|()")):
            return line[:i]
    return line


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


# --- Isolated sessions (#183): one policy for every arm ---------------------
#
# The same hook runs in every arm of a repository marked `isolated`. All arms
# share arm D's allowlist, the Gradle rule and the output pipes; the arms with
# search additionally get SEARCH_SHELL below, the shell's search and file
# reading programs. Nothing else differs.
SEARCH_SHELL = frozenset({"grep", "egrep", "fgrep", "rg", "find", "ls", "cat", "head", "tail", "wc", "nl", "sed",
                          "awk", "sort", "uniq", "cut", "git grep", "git ls-files", "git blame"})
ISOLATED_TOOLS = frozenset({"Bash", "Read", "Edit", "Write", "Grep", "Glob", "ToolSearch", "TodoWrite"})
# `myc` subcommands that read the index; the others serve, write configuration or reach out (`email`).
MYC_READS = frozenset({"analyze", "callers", "callees", "communities", "context", "impact", "knowledge", "list",
                       "path", "processes", "query", "search", "semantic-search", "stats", "status"})
NETWORK_PROGRAMS = frozenset({"curl", "wget", "nc", "ncat", "netcat", "telnet", "ssh", "scp", "sftp", "rsync", "ftp",
                              "openssl", "dig", "nslookup", "host", "ping", "gh", "git fetch", "git pull", "git clone",
                              "git push", "git remote", "git ls-remote", "git submodule", "git archive", "npm", "npx",
                              "pip", "pip3", "brew", "mvn", "gradle", "docker", "open", "java", "javac", "jshell"})
_FILTER = re.compile(r"^(-n)?-?\d+$|^-n$|^-c$|^--lines=\d+$")  # `tail -50`, `head -n 20`: reads its stdin only
# Options that run a program, write a file, or read one named indirectly.
_UNSAFE_OPT = {"find": re.compile(r"^-(exec|execdir|ok|okdir|delete|fprint|fprint0|fprintf|fls|files0-from)$"),
               "rg": re.compile(r"^--(pre|pre-glob|hostname-bin|ignore-file|file|search-zip)\b|^-[a-zA-Z]*[fz]"),
               "grep": re.compile(r"^--(file|exclude-from)\b|^-[a-zA-Z]*f"),
               "sort": re.compile(r"^--(output|compress-program|files0-from|temporary-directory)\b|^-[a-zA-Z]*[oT]"),
               "uniq": re.compile(r"^[^-]"), "wc": re.compile(r"^--files0-from"),
               "git grep": re.compile(r"^-O|^--open-files-in-pager|^--no-index|^-f|^--file"),
               "git ls-files": re.compile(r"^-X|^--exclude-from"), "git blame": re.compile(r"^--(contents|ignore-revs-file)")}
_SED_ADDR = r"(?:\d+|\$|/(?:[^/\\\n]|\\[^\n])*/)"
_SED_PRINT = re.compile(rf"^{_SED_ADDR}(,{_SED_ADDR})?p(;\s*{_SED_ADDR}(,{_SED_ADDR})?p)*$")
_AWK_UNSAFE = re.compile(r"system|getline|\||print[^}]*>|close\s*\(|ENVIRON|@")


def unaccounted(command: str) -> str | None:
    """What in ``command`` the parser above does not model, so that the whole
    command is refused: any expansion, continuation lines, odd characters, a
    quote that stays open on its line."""
    if any(ord(c) < 32 and c not in "\n\t" or ord(c) in (0x85, 0xA0, 0x2028, 0x2029) for c in command):
        return "control characters"
    for line in command.split("\n"):
        st = quoted_states(line)
        if st is None:
            return "a quote that is open at the end of a line"
        for i, c in enumerate(line):
            if c in "$`" and st[i] in "ud":
                return "an expansion (`$`, backticks)"
            if c in "{}" and st[i] == "u":
                return "a brace expansion"
            if c == "!" and st[i] in "ud" and line[i + 1:i + 2].strip():
                return "a history expansion"
            if c == "\\" and st[i] == "u":
                return "a backslash at the end of a line"
            if c in "<>" and st[i] == "u" and line[i + 1:i + 2] == "(":
                return "process substitution"
    return None


def outside(word: str, here: str, root: str) -> bool:
    """Does ``word`` name, or could the shell make it name, a path outside
    ``root``? Absolute, ``~``, ``..`` and ``$VAR`` forms, also after ``=`` or
    ``:`` (``--file=/x``, ``HEAD:/x``)."""
    if word in DEVICES:
        return False
    if re.match(r"^-[A-Za-z0-9]+[/~]", word):  # a path attached to a short option: `-e/etc/x`
        return True
    if re.search(r"(^|[/=:,{])\.\.(/|$|[,}])", word):  # no `..` at all: a `cd` before it may not have happened
        return True
    if re.search(r"[{,]\s*[/~]", word):  # alternatives of a glob: `{/etc/x,a}`
        return True
    for part in {word, *re.split(r"[=:,]", word)}:
        if re.search(r"\$[A-Za-z_{]", part) or part.startswith("~"):
            return True
        if (part.startswith("/") or ".." in part.split("/")) and not _inside(part, here, root):
            return True
        if part and not part.startswith("-") and os.path.lexists(os.path.join(here, part)) \
                and not _inside(part, here, root):  # a symlink out
            return True
    return False


def _pattern_operands(prog: str, args: list[str]) -> set[int]:
    """Indexes of the arguments that are search patterns, not paths."""
    if prog in ("awk", "sed"):  # the program text
        return next(({i} for i, a in enumerate(args) if not a.startswith("-") and (i == 0 or args[i - 1] not in ("-F", "-v"))),
                    set())
    if prog not in ("grep", "egrep", "fgrep", "rg", "git grep"):
        return set()
    out, i = set(), 0
    while i < len(args):
        if args[i] in ("-e", "--regexp") and i + 1 < len(args):
            out.add(i + 1)
            i += 1
        elif args[i].startswith("--regexp="):
            out.add(i)
        i += 1
    if not out:
        for i, a in enumerate(args):
            if not a.startswith("-") and (i == 0 or args[i - 1] not in ("-A", "-B", "-C", "-m", "-g", "--glob", "-t",
                                                                          "--type", "--include", "--exclude")):
                return {i}
    return out


def isolated_bash(command: str, extra=(), here: str | None = None, root: str | None = None,
                  search: bool = False) -> tuple[bool, list[str]]:
    """The Bash policy of an isolated session: ``(ok, what is denied)``.
    ``here`` is the shell's directory, ``root`` the working copy; ``search``
    adds :data:`SEARCH_SHELL` (every arm but D)."""
    here, root = here or root or "/", root or here or "/"
    if not _inside(here, root, root):
        return False, ["a shell outside the working copy"]
    if why := unaccounted(command):
        return False, [why]
    try:
        cmds, targets = parse(command)
    except Unparsed:
        return False, ["unparsable command"]
    rules, denied = task_rules(extra), []
    for k, words in enumerate(cmds):
        bad = [why for idx, t in targets if idx == k for why in [path_problem(t, here, root)] if why]
        if bad:
            denied.extend("redirection to " + b for b in bad)
            continue
        if _task_allows(words, rules):
            if os.path.realpath(here) != os.path.realpath(root):
                denied.append("Gradle outside the working copy's top directory")
            continue
        prog, args = program(words)
        if not prog:
            if not words or (words[0] not in HEADERS and not all(w in KEYWORDS for w in words)):
                denied.append(f"`{words[0] if words else ''}`")  # `env`, a bare assignment
            elif any(outside(w, here, root) for w in words[1:]):
                denied.append("a path outside the working copy")
            continue
        if prog in ("gradlew", "gradle") or (prog in SHELLS and any("gradlew" in a for a in args)):
            denied.append("Gradle without the required flags, or with an option or task that is not allowed")
            continue
        if prog in NETWORK_PROGRAMS:
            denied.append(prog + " (network)")
            continue
        name = words[0]
        if name != prog.split(" ")[0]:  # a wrapper (`xargs cat` reads names from its input), assignment, keyword, path
            if name in HEADERS or name in KEYWORDS:
                name = prog.split(" ")[0]
            else:
                denied.append(f"`{name}` in front of a command")
                continue
        if "/" in name:
            denied.append(f"`{name}` (a program by path)")
            continue
        if prog in ("head", "tail") and args and all(_FILTER.match(a) for a in args) or prog in ("head", "tail") and not args:
            continue  # an output pipe, in every arm
        if prog.startswith("git "):  # its global options too: `git -C /elsewhere log`
            pre = words[1:words.index(prog[4:], 1)] if prog[4:] in words[1:] else []
            if pre and pre != ["--no-pager"]:
                denied.append("git with global options")
                continue
        patterns = _pattern_operands(prog, args)
        if any(i not in patterns and outside(a, here, root) for i, a in enumerate(args)):
            denied.append("a path outside the working copy")
            continue
        if any(i not in patterns and touches_dot_git(a) for i, a in enumerate(args)):
            denied.append("a path inside .git")
            continue
        if prog in PATH_PROGRAMS and any("gradlew" in a or "gradle/wrapper" in a for a in args):
            denied.append("the Gradle wrapper's files")
            continue
        if prog == "myc" and (not args or args[0] not in MYC_READS and args[0] not in ("--help", "--version")):
            denied.append("that `myc` subcommand")
            continue
        if prog.startswith("git ") and any(a in ("--ext-diff", "--textconv") for a in args):
            denied.append(prog + " with a helper program")
            continue
        if search and prog in SEARCH_SHELL:
            rx = _UNSAFE_OPT.get("grep" if prog in ("egrep", "fgrep") else prog)
            if rx and any(i not in patterns and rx.search(a) for i, a in enumerate(args)):
                denied.append(f"`{prog}` with that option")
            elif prog == "sed" and not (len(args) >= 2 and args[0] in ("-n", "-nE", "-En") and _SED_PRINT.match(args[1])
                                        and not any(a.startswith("-") for a in args[2:])):
                denied.append("`sed` other than printing (`sed -n 'A,Bp'`, `sed -n '/re/p'`)")
            elif prog == "awk" and (any(_AWK_UNSAFE.search(a) for a in args) or any(
                    re.match(r"^-(f|-file|E|l|-load|i|-include)", a) for a in args)):
                denied.append("`awk` with system, getline, a pipe, a redirection or a program file")
            elif prog == "tail" and any(re.match(r"^-[a-zA-Z]*[fF]|^--follow|^--retry", a) for a in args):
                denied.append("`tail -f`")
            else:
                continue
            continue
        ok, why = allowed(shlex.join(words), extra, root if prog in PATH_PROGRAMS and here == root else here)
        if prog in PATH_PROGRAMS and here != root:
            why = [w for a in args if not a.startswith("-") for w in [path_problem(a, here, root)] if w]
            ok = not why
        if not ok:
            denied.extend(why)
        elif prog == "cd":
            target = os.path.realpath(os.path.join(here, ([a for a in args if not a.startswith("-")] or ["."])[0]))
            if not os.path.isdir(target):
                denied.append("`cd` to a directory that does not exist")
            here = target
    return not denied, denied


def isolated_tool(tool: str, inp: dict, root: str, here: str | None = None, extra=(), search: bool = False,
                  readable=()) -> tuple[str, str] | None:
    """Why a tool call is refused in an isolated session, as ``(kind,
    reason)`` (``outside``, ``network``, ``gradle`` or ``other``), or None.
    ``readable``: directories outside the working copy `Read` may use (the
    session's own saved tool results)."""
    here = here or root
    if tool == "Bash":
        ok, denied = isolated_bash(str(inp.get("command") or ""), extra, here, root, search)
        if ok:
            return None
        text = ", ".join(dict.fromkeys(denied))
        kind = ("network" if "(network)" in text else "gradle" if "Gradle" in text
                else "outside" if "outside" in text else "other")
        return kind, text
    if tool in ("WebFetch", "WebSearch"):
        return "network", tool
    if not (tool in ISOLATED_TOOLS or tool in EDIT_TOOLS or tool.startswith("mcp__myceliums__")):
        return "other", f"the tool {tool}"
    if tool in EDIT_TOOLS:
        path = str(inp.get("file_path") or inp.get("notebook_path") or "")
        if touches_dot_git(path) or touches_home_git_config(path) or not _inside(path, here, root):
            return "outside", f"writing {path}"
        rel = os.path.relpath(os.path.realpath(os.path.join(here, path)), os.path.realpath(root))
        if rel in ("gradlew", "gradlew.bat") or rel.startswith("gradle/wrapper/"):
            return "gradle", "writing the Gradle wrapper's files"
        return None
    if tool == "Read":
        path = str(inp.get("file_path") or "")
        if touches_dot_git(path):
            return "outside", f"reading {path}"
        if _inside(path, here, root) or any(_inside(path, r, r) for r in readable):
            return None
        return "outside", f"reading {path}"
    if tool in ("Grep", "Glob"):
        vals = [str(inp.get(k) or "") for k in (("path", "glob") if tool == "Grep" else ("path", "pattern"))]
        bad = [v for v in vals if v and (outside(v, here, root) or touches_dot_git(v) or "$" in v)]
        return ("outside", f"{tool} in {bad[0]}") if bad else None
    if tool.startswith("mcp__"):
        def strings(v):
            if isinstance(v, str):
                yield v
            elif isinstance(v, dict):
                for x in v.values():
                    yield from strings(x)
            elif isinstance(v, list):
                for x in v:
                    yield from strings(x)
        bad = [v for v in strings(inp) if (v.startswith(("/", "~")) or v.startswith("..") or "/../" in v)
               and not _inside(os.path.expanduser(v), here, root)]
        return ("outside", f"{tool} on {bad[0]}") if bad else None
    return None


ISOLATED_MESSAGE = ("Not allowed in this session: {why}. Work only on files of the working copy {root}; there is no "
                    "network; Bash allows {progs}, and Gradle only as `./gradlew --offline --no-scan "
                    "--no-build-cache ...` from the top directory.")
ISOLATED_MARKER = "Not allowed in this session:"


def isolated_hook(argv: list[str], stdin=sys.stdin, stdout=sys.stdout) -> int:
    """PreToolUse hook of an isolated session, for every tool, in every arm."""
    opts = dict(zip(argv[::2], argv[1::2]))
    try:
        event = json.load(stdin)
        root, search = opts["--root"], opts["--search"] == "yes"
        tp, sid = str(event.get("transcript_path") or ""), str(event.get("session_id") or "")
        readable = [os.path.join(os.path.dirname(tp), sid, "tool-results")] if tp and sid else []
        extra = json.loads(opts.get("--allow-json") or "[]")
        why = isolated_tool(str(event.get("tool_name") or ""), event.get("tool_input") or {}, root,
                            event.get("cwd") or root, extra, search, readable)
    except Exception as exc:  # noqa: BLE001 - a hook that cannot decide refuses
        return _deny(stdout, f"{ISOLATED_MARKER} the check failed ({type(exc).__name__})")
    if why is None:
        return 0
    progs = sorted(D_ALLOWED) + ["head/tail as pipes"] + (sorted(SEARCH_SHELL) if search else [])
    return _deny(stdout, ISOLATED_MESSAGE.format(why=why[1], root=root, progs=", ".join(progs)))


def _deny(stdout, reason: str) -> int:
    """Refuse with the hook's JSON answer: unlike exit 2, Claude Code then
    shows the model the reason alone, not the hook's command line (its paths
    and the Gradle pattern)."""
    json.dump({"hookSpecificOutput": {"hookEventName": "PreToolUse", "permissionDecision": "deny",
                                      "permissionDecisionReason": reason}}, stdout)
    return 0


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
    if sys.argv[1:2] == ["isolated-hook"]:
        sys.exit(isolated_hook(sys.argv[2:]))
    for line in sys.stdin:
        print(classify(line), search_programs(line), allowed(line))
