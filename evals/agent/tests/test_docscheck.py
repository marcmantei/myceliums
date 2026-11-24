"""The docs check of #210: diff features, ranking, graph neighbourhood, statistics, commit list."""

import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))
sys.path.insert(0, str(ROOT / "groundtruth"))

from docscheck import change, corpus, evaluate, fuse, gitdata, graph, measure, prepare, search, select_commits, text  # noqa: E402

JAVA = """package a.b;

import java.util.List;

/**
 * Sends things. Never `class Fake {`.
 */
@SuppressWarnings({"unchecked", "x{y}"})
public class Sender<T extends List<String>> implements Runnable {

	private static final String NAME = "spring.sender.name";

	private int retries = 3, other;

	public Sender(int retries) {
		this.retries = retries;
	}

	/**
	 * The retries.
	 */
	@Deprecated(since = "1")
	public int getRetries() {
		if (retries > 0) {
			return retries;
		}
		return 0;
	}

	abstract void run();

	public static class Builder {
		Builder retries(int n) { return this; }
	}
}

@interface Mark {
	String value() default "";

	boolean lenient() default false;
}
"""


def names(decls, kind=None):
    return [d["name"] for d in decls if kind in (None, d["kind"])]


class Scanner(unittest.TestCase):
    def test_types_and_members(self):
        decls = change.spans(JAVA)
        self.assertEqual(names(decls, "type"), ["Sender", "Builder", "Mark"])
        for m in ("NAME", "retries", "Sender", "getRetries", "run", "value", "lenient"):
            self.assertIn(m, names(decls, "member"))
        get = next(d for d in decls if d["name"] == "getRetries")
        # A declaration starts at its doc comment and ends at its brace.
        self.assertLess(get["start"], JAVA[:JAVA.index("The retries")].count("\n") + 1)
        self.assertEqual(JAVA.splitlines()[get["end"] - 1].strip(), "}")
        self.assertEqual(get["owner"], "Sender")
        self.assertEqual(next(d for d in decls if d["kind"] == "member" and d["name"] == "lenient")["owner"], "Mark")
        self.assertTrue(next(d for d in decls if d["name"] == "Mark")["annotation"])

    def test_blank_keeps_positions(self):
        for src in (JAVA, "a /* \n */ b // c\n\"x//y\" 'z'", '"""\ntext block {"""'):
            b = change.blank(src)
            self.assertEqual(len(b), len(src))
            self.assertEqual(b.count("\n"), src.count("\n"))
        self.assertNotIn("Fake", change.blank(JAVA))

    def test_kotlin(self):
        src = "package a\n\nclass Foo {\n\tval bar: Int = 1\n\n\tfun baz(x: Int): Int {\n\t\treturn x\n\t}\n}\n\nfun top() {\n}\n"
        decls = change.spans(src)
        self.assertEqual(names(decls, "type"), ["Foo"])
        self.assertIn("baz", names(decls, "member"))
        self.assertIn("top", names(decls, "member"))


def hunk(os=0, ol=0, ns=0, nl=0, minus=(), plus=()):
    return {"os": os, "ol": ol, "ns": ns, "nl": nl, "minus": list(minus), "plus": list(plus)}


def java_file(old, new, hunks, path="spring-x/src/main/java/a/b/Sender.java"):
    return {"old": path, "new": path, "hunks": hunks, "old_text": old, "new_text": new, "kind": change.kind(path)}


class Features(unittest.TestCase):
    def setUp(self):
        lines = JAVA.split("\n")
        self.line = lines.index("\tpublic int getRetries() {") + 1

    def test_member_declaration(self):
        new = JAVA.replace("getRetries()", "getMaxRetries()")
        f = java_file(JAVA, new, [hunk(self.line, 1, self.line, 1, ["\tpublic int getRetries() {"], ["\tpublic int getMaxRetries() {"])])
        feat = change.features([f])
        self.assertEqual(feat["members"]["getRetries"], 2.0)
        self.assertEqual(feat["members"]["getMaxRetries"], 2.0)
        self.assertIn(("Sender", "getRetries"), feat["pairs"])
        self.assertEqual(feat["types"]["Sender"], 1.0)
        self.assertIn("a.b.Sender", feat["qualified"])

    def test_body_change_is_a_touched_member(self):
        f = java_file(JAVA, JAVA, [hunk(self.line + 1, 1, self.line + 1, 1, ["\t\tx"], ["\t\tif (retries > 1) {"])])
        feat = change.features([f])
        self.assertEqual(feat["members"]["getRetries"], 1.0)
        self.assertNotIn("getRetries", feat["declared"])

    def test_constructor_counts_as_its_type(self):
        line = JAVA.split("\n").index("\t\tthis.retries = retries;") + 1
        feat = change.features([java_file(JAVA, JAVA, [hunk(line, 1, line, 1, ["a"], ["b"])])])
        self.assertNotIn("Sender", feat["members"])
        self.assertIn("Sender", feat["types"])

    def test_annotation_attribute(self):
        line = JAVA.split("\n").index("\tboolean lenient() default false;") + 1
        feat = change.features([java_file(JAVA, JAVA, [hunk(line, 1, line, 1, ["a"], ["b"])])])
        self.assertEqual(feat["attributes"], ["lenient"])

    def test_keys_come_from_literals_of_main_code_only(self):
        ln = '\tString NAME = "spring.sender.max-retries"; a.b.C();'
        feat = change.features([java_file(JAVA, JAVA, [hunk(11, 1, 11, 1, [ln], [ln])])])
        self.assertIn("spring.sender.max-retries", feat["keys"])
        self.assertNotIn("a.b.C", feat["keys"])
        test = java_file(JAVA, JAVA, [hunk(11, 1, 11, 1, [], [ln])], "spring-x/src/test/java/a/b/SenderTests.java")
        self.assertEqual(change.features([test])["keys"], {})

    def test_imports_are_skipped_and_comments_apart(self):
        f = java_file(JAVA, JAVA, [hunk(3, 1, 3, 1, ["import java.util.Old;"], [" * Uses {@link Other}."])])
        feat = change.features([f])
        self.assertNotIn("Old", feat["idents"]["code"])
        self.assertIn("Other", feat["idents"]["comment"])

    def test_parse_diff(self):
        d = change.parse_diff("""diff --git a/x/A.java b/x/B.java
similarity index 90%
rename from x/A.java
rename to x/B.java
--- a/x/A.java
+++ b/x/B.java
@@ -3 +3,2 @@ class A {
-old
+new
+--- not a header
diff --git a/n.txt b/n.txt
new file mode 100644
--- /dev/null
+++ b/n.txt
@@ -0,0 +1 @@
+hello
""")
        self.assertEqual([(f["old"], f["new"]) for f in d], [("x/A.java", "x/B.java"), (None, "n.txt")])
        self.assertEqual(d[0]["hunks"][0]["plus"], ["new", "--- not a header"])
        self.assertEqual((d[1]["hunks"][0]["ol"], d[1]["hunks"][0]["nl"]), (0, 1))


def small_corpus(files):
    """A corpus without git."""
    c = corpus.Corpus.__new__(corpus.Corpus)
    c.units = {p: corpus.Unit(t, p.endswith(".adoc")) for p, t in sorted(files.items())}
    c.n = len(c.units)
    c.df = {k: {} for k in ("idents", "code_idents", "keys", "words")}
    for u in c.units.values():
        for k, df in c.df.items():
            for t in getattr(u, k):
                df[t] = df.get(t, 0) + 1
    c.sections = [(p, i) for p, u in c.units.items() for i in range(len(u.sections))]
    c.sdf = {}
    for u in c.units.values():
        for b in u.sections:
            for t in b:
                c.sdf[t] = c.sdf.get(t, 0) + 1
    c.avg_ident = sum(u.length for u in c.units.values()) / c.n
    c.avg_words = sum(u.wlen for u in c.units.values()) / c.n
    c.avg_sec = sum(sum(u.sec_len) for u in c.units.values()) / len(c.sections)
    c.paths = {p: corpus.path_words(p) for p in c.units}
    return c


P = "framework-docs/modules/ROOT/pages/"
DOCS = {
    P + "core/sending.adoc": "= Sending\n\n== Retries\nCall `Sender#getRetries()` or set `spring.sender.name`.\n",
    P + "core/common.adoc": "= Common\n\n`Sender` is a common word. Use `List` and more words.\n" + "filler text here\n" * 40,
    P + "web/other.adoc": "= Other\n\nNothing of interest: handlers and mappings.\n",
}


class Ranking(unittest.TestCase):
    def setUp(self):
        self.c = small_corpus(DOCS)
        lines = JAVA.split("\n")
        n = lines.index("\tpublic int getRetries() {") + 1
        self.f = change.features([java_file(JAVA, JAVA, [hunk(n, 1, n, 1, [lines[n - 1]], [lines[n - 1]])])])

    def test_every_configuration_finds_the_page(self):
        for fn, grid in ((text.grep, text.GREP_GRID), (text.bm25, text.BM25_GRID)):
            self.assertEqual(len(grid), 48)
            for cfg in grid:
                self.assertEqual(fuse.ranked(fn(self.c, self.f, cfg))[0], P + "core/sending.adoc", cfg)

    def test_grids_have_the_registered_size(self):
        self.assertEqual((len(graph.GRID), len(search.GRID), len(fuse.GRID)), (48, 24, 24))
        for grid in (text.GREP_GRID, text.BM25_GRID, graph.GRID, search.GRID, fuse.GRID):
            self.assertEqual(len({json.dumps(c, sort_keys=True) for c in grid}), len(grid))

    def test_unrelated_page_is_not_shown_by_grep(self):
        self.assertNotIn(P + "web/other.adoc", text.grep(self.c, self.f, text.GREP_GRID[0]))

    def test_words(self):
        self.assertEqual(corpus.words_of("getMaxRetries"), ("get", "max", "retry", "getmaxretries"))
        self.assertEqual(corpus.words_of("XMLReaders"), ("xml", "reader", "xmlreaders"))

    def test_ties_by_path(self):
        self.assertEqual(fuse.ranked({"b": 1.0, "a": 1.0, "c": 2.0}), ["c", "a", "b"])

    def test_fusion(self):
        a, b = {"x": 10.0, "y": 1.0}, {"y": 0.9, "z": 0.1}
        for cfg in fuse.GRID:
            self.assertEqual(set(fuse.fuse(a, b, cfg)), {"x", "y", "z"})
        self.assertEqual(fuse.fuse(a, {}, {"how": "max", "w": 0.5})["x"], 0.5)
        rrf = fuse.fuse(a, b, {"how": "rrf", "k": 2, "w": 0.5})
        self.assertEqual(fuse.ranked(rrf)[0], "y")

    def test_measure(self):
        m = measure.score(["a", "b", "c", "d", "e", "f"], ["c", "f", "zz"])
        self.assertAlmostEqual(m["recall@5"], 1 / 3)
        self.assertAlmostEqual(m["recall@10"], 2 / 3)
        self.assertAlmostEqual(m["precision@5"], 1 / 5)
        self.assertAlmostEqual(m["mrr"], 1 / 3)
        self.assertEqual(measure.score(["a"], ["a"])["shown@5"], 1.0)
        self.assertEqual(measure.score([], ["a"])["mrr"], 0.0)


class Graph(unittest.TestCase):
    def node(self, i, kind, qn, start, end, path="spring-x/src/main/java/a/b/Sender.java"):
        return {"id": i, "kind": kind, "name": qn.split(".")[-1], "qualified_name": qn, "file_path": path,
                "start_line": start, "end_line": end}

    def setUp(self):
        page = P + "core/sending.adoc"
        sample = "framework-docs/src/main/java/org/springframework/docs/Use.java"
        base = "spring-x/src/main/java/a/b/Base.java"
        lines = JAVA.split("\n")
        self.get = lines.index("\tpublic int getRetries() {") + 1
        nodes = [
            self.node("T", "Class", "Sender", 9, 35), self.node("C", "Method", "Sender.Sender", 15, 17),
            self.node("M", "Method", "Sender.getRetries", self.get, self.get + 5),
            self.node("O", "Method", "Sender.run", self.get + 7, self.get + 7),
            self.node("B", "Class", "Base", 1, 9, base), self.node("BM", "Method", "Base.getRetries", 2, 4, base),
            self.node("S", "Section", "Retries", 3, 4, page), self.node("S2", "Section", "Base", 3, 4, P + "core/base.adoc"),
            self.node("U", "Method", "Use.use", 1, 9, sample),
        ]
        self.nodes = {n["id"]: n for n in nodes}
        self.edges = {"MENTIONS": [("S", "T"), ("S", "M"), ("S2", "BM")], "OVERRIDES": [("M", "BM")],
                      "CALLS": [("U", "M")],
                      # What the index really has: members of a class with a constructor point to the constructor.
                      "MEMBER_OF": [("M", "C"), ("O", "C")]}
        self.files = [java_file(JAVA, JAVA, [hunk(self.get + 1, 1, self.get + 1, 1, ["\t\tx"], ["\t\ty"])])]

    def test_neighbourhood(self):
        g = graph.neighbourhood(self.nodes, self.edges, self.files, "o")
        k = "spring-x/src/main/java/a/b/Sender.java:"
        # The scanner also touches the innermost type; a constructor is its type.
        self.assertIn(k + "Sender.getRetries", g["relations"]["touched"])
        self.assertEqual(g["relations"]["owner"], [k + "Sender"])
        self.assertIn("spring-x/src/main/java/a/b/Base.java:Base.getRetries", g["relations"]["dispatch"])
        self.assertIn(k + "Sender.run", g["relations"]["member"])
        self.assertEqual(g["sample"], {"framework-docs/src/main/java/org/springframework/docs/Use.java": 1})
        self.assertEqual(g["units"][k + "Sender.getRetries"], {P + "core/sending.adoc": 1})

    def test_mentions_ranks_the_direct_page_first_and_hops_only_by_profile(self):
        raw = graph.neighbourhood(self.nodes, self.edges, self.files, "o")
        g = graph.merge(raw, raw)
        c = small_corpus({**DOCS, P + "core/base.adoc": "= Base\n"})
        for cfg in graph.GRID:
            r = fuse.ranked(graph.mentions(c, g, cfg))
            self.assertEqual(r[0], P + "core/sending.adoc", cfg)
            self.assertEqual(P + "core/base.adoc" in r, cfg["profile"] in ("hop", "all"))

    def test_merge_keeps_both_sides(self):
        a = {"relations": {"touched": ["x"]}, "units": {"x": {"p": 1}}, "sample": {"s": 1}, "declared": ["x"]}
        b = {"relations": {"touched": ["y"]}, "units": {"x": {"p": 3, "q": 1}}, "sample": {}, "declared": ["y"]}
        m = graph.merge(a, b)
        self.assertEqual(m["relations"]["touched"], ["x", "y"])
        self.assertEqual(m["units"]["x"], {"p": 3, "q": 1})


class SearchAnswers(unittest.TestCase):
    ANSWER = """Search results for 'x' in docs-only:

  1. Retries (Section) — modules/ROOT/pages/core/sending.adoc:3-4  [score: 8.1]
     == Retries
  2. Using (the) thing (Document) — modules/ROOT/pages/core/common.adoc:1-40  [score: 7.0]
  3. More (Section) — modules/ROOT/pages/core/sending.adoc:5-9  [score: 6.0]

  ... and 9 more results
"""

    def test_parse(self):
        self.assertEqual(search.parse(self.ANSWER), [P + "core/sending.adoc", P + "core/common.adoc", P + "core/sending.adoc"])

    def test_aggregate(self):
        for agg in ("max", "sum", "rrf", "first"):
            s = search.aggregate([search.parse(self.ANSWER)], agg)
            self.assertEqual(fuse.ranked(s)[0], P + "core/sending.adoc")
        self.assertEqual(search.aggregate([], "max"), {})

    def test_queries_come_from_the_diff(self):
        c = small_corpus(DOCS)
        f = change.features([java_file(JAVA, JAVA, [hunk(11, 1, 11, 1, ["x"], ["y"])])])
        q = search.queries(c, f)
        self.assertEqual(set(q), set(search.QUERIES))
        self.assertTrue(all(qs and all(qs) for qs in q.values()))


class Statistics(unittest.TestCase):
    def test_bootstrap(self):
        b = evaluate.bootstrap([0.2] * 30)
        self.assertAlmostEqual(b["diff"], 0.2)
        self.assertAlmostEqual(b["lo"], 0.2)
        self.assertEqual(b["p"], 1 / evaluate.BOOT)
        zero = evaluate.bootstrap([0.0] * 30)
        self.assertEqual(zero["p"], 1.0)
        self.assertEqual(evaluate.bootstrap([0.1, -0.1, 0.3]), evaluate.bootstrap([0.1, -0.1, 0.3]))

    def test_holm(self):
        h = evaluate.holm({"a": 0.01, "b": 0.04})
        self.assertAlmostEqual(h["a"], 0.02)
        self.assertAlmostEqual(h["b"], 0.04)
        h = evaluate.holm({"a": 0.04, "b": 0.03})
        self.assertAlmostEqual(h["a"], 0.06)

    def result(self, added, alone, text_r=0.8, myc_r=0.7, both_r=0.85):
        def cmp(clear):
            return {"diff": 0.12 if clear else 0.02, "lo": 0.05 if clear else -0.01, "hi": 0.2, "p_holm": 0.01 if clear else 0.5}
        return evaluate.decide({"text+myc - text": cmp(added), "myc - text": cmp(alone)},
                               {m: {"recall@5": r} for m, r in (("text", text_r), ("myc", myc_r), ("text+myc", both_r))})

    def test_decision_table(self):
        self.assertEqual(self.result(False, False), "grep")
        self.assertEqual(self.result(True, False), "text with myceliums")
        self.assertEqual(self.result(False, True), "myceliums")
        self.assertEqual(self.result(True, True, myc_r=0.95, both_r=0.95), "myceliums")
        self.assertEqual(self.result(True, True, myc_r=0.9, both_r=0.95), "text with myceliums")
        self.assertEqual(self.result(False, False, text_r=0.4), "none")

    def test_clearly_better_needs_all_three(self):
        ok = {"diff": 0.10, "lo": 0.01, "p_holm": 0.05}
        self.assertTrue(evaluate.clearly_better(ok))
        for k, v in (("diff", 0.09), ("lo", 0.0), ("p_holm", 0.06)):
            self.assertFalse(evaluate.clearly_better({**ok, k: v}))


class HeldOutIsGuarded(unittest.TestCase):
    def test_evaluate(self):
        with mock.patch.object(sys, "argv", ["evaluate", "held-out", "--hist", "x", "--work", "y", "--out", "z"]):
            with self.assertRaisesRegex(SystemExit, "independent review"):
                evaluate.main()

    def test_once(self):
        with tempfile.TemporaryDirectory() as d:
            out = Path(d) / "h.json"
            out.write_text("{}")
            with mock.patch.object(sys, "argv", ["evaluate", "held-out", "--hist", "x", "--work", "y", "--out", str(out),
                                                 "--i-have-the-review"]):
                with self.assertRaisesRegex(SystemExit, "scored once"):
                    evaluate.main()

    def test_prepare(self):
        with mock.patch.object(sys, "argv", ["prepare", "--hist", "x", "--work", "y", "--myc", "m", "--models", "n",
                                             "--split", "held-out"]):
            with self.assertRaisesRegex(SystemExit, "independent review"):
                prepare.main()

    def test_remove_stays_below_its_root(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d) / "work"
            inside, outside = root / "home", Path(d) / "other"
            for p in (inside, outside):
                p.mkdir(parents=True)
            prepare.remove(inside, root)
            self.assertFalse(inside.exists())
            for p in (outside, root, root / ".." / "other"):
                with self.assertRaises(SystemExit):
                    prepare.remove(p, root)
            self.assertTrue(outside.exists())


class Selection(unittest.TestCase):
    def commit(self, subject="Add X", date="2024-01-01T00:00:00+00:00", files=None, **kw):
        files = files or [("M", "spring-core/src/main/java/a/A.java"), ("M", P + "core/a.adoc")]
        return {"sha": "s", "parent": "p", "date": date, "subject": subject, "issues": [],
                "files": [{"status": s, "path": p, "old": "1", "new": "2"} for s, p in files], **kw}

    def kept(self, **kw):
        return len(select_commits.by_paths([self.commit(**kw)])[0])

    def test_path_rules(self):
        main = ("M", "spring-core/src/main/java/a/A.java")
        page = ("M", P + "core/a.adoc")
        self.assertEqual(self.kept(), 1)
        self.assertEqual(self.kept(date="2023-04-24T23:59:59+00:00"), 0)
        self.assertEqual(self.kept(subject="Polish A"), 0)
        self.assertEqual(self.kept(files=[page]), 0)
        self.assertEqual(self.kept(files=[main]), 0)
        self.assertEqual(self.kept(files=[main, ("A", P + "core/new.adoc")]), 0)
        self.assertEqual(self.kept(files=[("M", "spring-core/src/test/java/a/ATests.java"), page]), 0)
        self.assertEqual(self.kept(files=[main] + [("M", P + f"{i}.adoc") for i in range(11)]), 0)
        self.assertEqual(self.kept(files=[main, page] + [("M", f"spring-core/src/test/java/T{i}.java") for i in range(39)]), 0)

    def test_day_is_utc(self):
        self.assertEqual(select_commits.day("2023-04-25T01:30:00+02:00").isoformat(), "2023-04-24")

    def test_units(self):
        self.assertTrue(select_commits.is_unit(P + "core/a.adoc"))
        self.assertTrue(select_commits.is_unit("framework-docs/modules/ROOT/partials/x.adoc"))
        self.assertTrue(select_commits.is_unit("framework-docs/src/main/kotlin/a/B.kt"))
        self.assertFalse(select_commits.is_unit("framework-docs/modules/ROOT/nav.adoc"))
        self.assertFalse(select_commits.is_unit("framework-docs/framework-docs.gradle"))

    def test_neighbours(self):
        r = {"sha": "r", "date": "2026-01-10T00:00:00+00:00", "issues": ["100"],
             "files": [{"path": "spring-core/src/main/java/a/A.java"}]}
        c = self.commit(date="2026-01-20T00:00:00+00:00")
        self.assertIn("main file", select_commits.neighbours(c, [r]))
        self.assertIsNone(select_commits.neighbours({**c, "date": "2026-02-20T00:00:00+00:00"}, [r]))
        self.assertIn("issue", select_commits.neighbours({**c, "date": "2026-05-20T00:00:00+00:00", "issues": ["100"]}, [r]))
        self.assertEqual(select_commits.neighbours({**c, "sha": "r"}, [r]), "replayed")

    def test_committed_list(self):
        doc = json.loads((ROOT / "docscheck" / "data" / "commits.json").read_text())
        cs = doc["commits"]
        replayed = select_commits.replayed_shas()
        self.assertEqual(len(replayed), 29)
        self.assertEqual(doc["counts"]["split"], {s: sum(c["split"] == s for c in cs) for s in ("dev", "held-out", "excluded-183")})
        self.assertEqual(len({c["sha"] for c in cs}), len(cs))
        self.assertEqual(len({c["subject"] for c in cs}), len(cs))
        for c in cs:
            self.assertTrue(c["gold"] and all(select_commits.is_unit(g) for g in c["gold"]), c["sha"])
            self.assertLessEqual(len(c["gold"]), select_commits.MAX_GOLD)
            self.assertEqual(c["split"] == "dev", c["date"] < select_commits.SPLIT)
            if c["split"] == "held-out":
                self.assertNotIn(c["sha"], replayed)
        noise = json.loads((ROOT / "docscheck" / "data" / "gold_noise.json").read_text())
        dev = {c["sha"][:10]: c for c in cs if c["split"] == "dev"}
        for sha, pages in noise["verdicts"].items():
            self.assertEqual(sorted(P + p for p in pages), dev[sha]["gold"], sha)


class GitAccess(unittest.TestCase):
    def test_tree_and_blobs(self):
        with tempfile.TemporaryDirectory() as d:
            repo = Path(d)
            run = lambda *a: subprocess.run(["git", "-C", d, *a], check=True, capture_output=True)  # noqa: E731
            run("init", "-q")
            (repo / "a.txt").write_text("one\n")
            (repo / "b c.txt").write_bytes(b"x\nmissing\n\ny")
            run("add", "-A")
            run("-c", "user.email=t@example.invalid", "-c", "user.name=t", "-c", "commit.gpgsign=false", "-c", "core.hooksPath=/dev/null",
                "commit", "-q", "-m", "m")
            tree = gitdata.tree(repo, "HEAD")
            self.assertEqual(set(tree), {"a.txt", "b c.txt"})
            got = gitdata.blobs(repo, tree.values())
            self.assertEqual(got[tree["b c.txt"]], b"x\nmissing\n\ny")
            self.assertEqual(gitdata.missing(repo, list(tree.values()) + ["1" * 40]), ["1" * 40])
            with self.assertRaises(RuntimeError):
                gitdata.blobs(repo, ["1" * 40])


if __name__ == "__main__":
    unittest.main()
