import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from aeval.grading import extract_answer, grade, is_success, normalize_file, normalize_symbol  # noqa: E402


def task(gold, kind="symbols", optional=(), repo="fzf"):
    return {
        "id": "t", "repo": repo, "answer_kind": kind,
        "gold": [g if isinstance(g, dict) else {"name": g} for g in gold],
        "optional": [o if isinstance(o, dict) else {"name": o} for o in optional],
    }


class ExtractAnswer(unittest.TestCase):
    def test_last_fenced_block_wins(self):
        text = 'draft:\n```json\n{"answer": ["a"]}\n```\nfinal:\n```json\n{"answer": ["b", "c"]}\n```'
        self.assertEqual(extract_answer(text), ["b", "c"])

    def test_unlabelled_fence_and_bare_object(self):
        self.assertEqual(extract_answer('```\n{"answer": ["x"]}\n```'), ["x"])
        self.assertEqual(extract_answer('So: {"answer": ["y", "z"]} done.'), ["y", "z"])

    def test_missing_or_broken_answer(self):
        self.assertIsNone(extract_answer(None))
        self.assertIsNone(extract_answer("no json here"))
        self.assertIsNone(extract_answer('```json\n{"answer": [unquoted]}\n```'))

    def test_empty_answer_is_an_answer(self):
        self.assertEqual(extract_answer('```json\n{"answer": []}\n```'), [])


class Normalisation(unittest.TestCase):
    def test_symbol_spellings(self):
        for raw in ["Terminal.dumpItem", "(*Terminal).dumpItem", "(t *Terminal) dumpItem",
                    "func (t *Terminal) dumpItem(i *Item) StatusItem", "`Terminal.dumpItem()`",
                    "src/terminal.go:8890 Terminal.dumpItem", "src.(*Terminal).dumpItem"]:
            self.assertTrue(normalize_symbol(raw).endswith("terminal.dumpitem"), raw)

    def test_python_and_typescript_spellings(self):
        self.assertEqual(normalize_symbol("fastapi/routing.py:serialize_response"), "serialize_response")
        self.assertEqual(normalize_symbol("serialize_response(field, response_content)"), "serialize_response")
        self.assertEqual(normalize_symbol("Hono.#dispatch"), "hono.dispatch")
        self.assertEqual(normalize_symbol("HonoBase#dispatch"), "honobase.dispatch")
        self.assertEqual(normalize_symbol("Hono.prototype.fetch"), "hono.fetch")
        self.assertEqual(normalize_symbol("ConcurrentSet[T].ForEach"), "concurrentset.foreach")

    def test_file_paths(self):
        self.assertEqual(normalize_file("./src/core.go:90", "fzf"), "src/core.go")
        self.assertEqual(normalize_file("/private/tmp/aeval/fzf/src/core.go", "fzf"), "src/core.go")
        # The package directory has the same name as the repository.
        self.assertEqual(normalize_file("/tmp/aeval/fastapi/fastapi/routing.py", "fastapi"), "fastapi/routing.py")
        self.assertEqual(normalize_file("`src/x.ts#L12`", "hono"), "src/x.ts")


class Grade(unittest.TestCase):
    def test_exact_answer(self):
        g = grade(task(["Terminal.captureLine", "Terminal.Loop"]), ["Terminal.Loop", "Terminal.captureLine"])
        self.assertEqual((g.precision, g.recall, g.f1), (1.0, 1.0, 1.0))

    def test_no_answer_scores_zero(self):
        g = grade(task(["a"]), None)
        self.assertEqual(g.f1, 0.0)
        self.assertFalse(g.parsed)

    def test_wrong_receiver_is_a_false_positive(self):
        g = grade(task(["Item.AsString"]), ["Pattern.AsString"])
        self.assertEqual((g.tp, g.fp), (0, 1))

    def test_bare_method_name_and_qualified_name_match(self):
        g = grade(task(["Terminal.dumpItem", "replacePlaceholder"]),
                  ["dumpItem", "github.com/junegunn/fzf/src.replacePlaceholder"])
        self.assertEqual(g.f1, 1.0)

    def test_one_answer_cannot_cover_two_gold_items(self):
        g = grade(task(["HTTPBase.__call__", "HTTPBearer.__call__"]), ["__call__"])
        self.assertEqual((g.tp, g.recall), (1, 0.5))

    def test_bipartite_matching_prefers_a_full_assignment(self):
        # "Hono.dispatch" could match either gold item; the matching must give
        # it to #dispatch so that "compose" still matches compose.
        t = task([{"name": "Hono.#dispatch"}, {"name": "compose", "aliases": ["dispatch"]}])
        g = grade(t, ["Hono.dispatch", "compose"])
        self.assertEqual(g.f1, 1.0)

    def test_alias_duplicate_and_optional_are_neutral(self):
        t = task([{"name": "get_request_handler", "aliases": ["_serialize_data"]}, "serialize_response"],
                 optional=["jsonable_encoder"])
        g = grade(t, ["get_request_handler", "_serialize_data", "serialize_response", "jsonable_encoder", "other"])
        self.assertEqual((g.tp, g.fp, g.neutral), (2, 1, 2))
        self.assertAlmostEqual(g.precision, 2 / 3)

    def test_duplicates_after_normalisation_count_once(self):
        g = grade(task(["Run"]), ["Run", "Run()", "`Run`"])
        self.assertEqual((g.tp, g.fp, g.n_answer), (1, 0, 1))

    def test_files(self):
        t = task(["src/core.go", "src/tui/light_unix.go"], kind="files")
        g = grade(t, ["src/core.go:90", "/tmp/aeval/fzf/src/tui/light_unix.go", "src/util/util.go"])
        self.assertEqual((g.tp, g.fp), (2, 1))

    def test_partial_file_name_does_not_match(self):
        t = task(["src/tui/light.go"], kind="files")
        self.assertEqual(grade(t, ["light.go"]).tp, 0)

    def test_success_threshold(self):
        # 3 gold, 2 found, nothing wrong: P = 1, R = 2/3, F1 = 0.8 exactly.
        g = grade(task(["a", "b", "c"]), ["a", "b"])
        self.assertAlmostEqual(g.f1, 0.8)
        self.assertTrue(is_success(g.f1, 0.8))
        self.assertFalse(is_success(0.79, 0.8))


if __name__ == "__main__":
    unittest.main()
