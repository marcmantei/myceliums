"""#182: the post-hoc audit finds paths outside the working copy and network use."""

import json
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import outside_audit as oa  # noqa: E402

ROOTS = ("/private/tmp/w/spring", "/tmp/w/spring")


class Outside(unittest.TestCase):
    def out(self, text):
        return oa.outside_paths(text, ROOTS, ROOTS[0])

    def test_inside(self):
        for cmd in ("grep -rn x /private/tmp/w/spring/a", "cd /tmp/w/spring; ls a", "ls a/../b 2>/dev/null",
                    "cd spring-core/src && cat ../README.md", "grep -n 'a/b' c"):
            self.assertEqual(self.out(cmd), [], cmd)

    def test_outside(self):
        for cmd in ("cat /Users/x/gold.json", "ls ~/.m2", "ls $HOME", "cat ../other/f", "cd spring-core; cat ../../x",
                    "find / -name gold.json", "ls /opt/homebrew", "cat /tmp/w/spring2/f", "cat ${HOME}/x",
                    "cat a/../../x", "python3 -c 'open(\"/etc/hosts\")'", "ls /tmp/w/spring/../x"):
            self.assertTrue(self.out(cmd), cmd)

    def test_transcript_kinds(self):
        def use(name, **inp):
            return {"type": "assistant", "message": {"content": [{"type": "tool_use", "name": name, "input": inp}]}}
        evs = [use("Read", file_path="/Users/x/tasks.json"), use("Bash", command="curl -s example.org"),
               use("Bash", command="git -C . fetch origin"), use("Bash", command="git log -3; git show HEAD"),
               use("WebSearch", query="q"), use("Grep", pattern="x", path="/private/tmp/w/spring"),
               {"type": "user", "message": {"content": [{"type": "tool_result", "content": "a gold_method b"}]}}]
        with tempfile.TemporaryDirectory() as tmp:
            p = Path(tmp) / "t.jsonl"
            p.write_text("".join(json.dumps(e) + "\n" for e in evs))
            kinds = [h["kind"] for h in oa.audit_transcript(p, ROOTS)]
        self.assertEqual(kinds, ["outside", "network", "network", "network", "marker"])


if __name__ == "__main__":
    unittest.main()
