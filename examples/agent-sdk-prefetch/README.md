# Prefetch a context pack for a Claude Agent SDK agent

Layer 5 of the [agent integration guide](../../docs/guides/agent-integration.md):
a custom harness asks myceliums for the symbols relevant to a task *before*
the model call and puts them in the prompt, instead of relying on the agent
to discover the myceliums tools.

[`prefetch.py`](prefetch.py) does three things:

1. runs `myc context --json --budget 1500 -- "<task>"` in the project;
2. renders the pack (symbols with signature, callers, callees, file:line) as
   a `<codebase_context>` block;
3. starts a [Claude Agent SDK](https://code.claude.com/docs/en/agent-sdk/python)
   session with that block in front of the task, read-only tools allowed, and
   prints the final answer.

If there is no index or `myc` fails, the task goes to the agent unchanged.

## Run it

```bash
cd your-project && myc analyze .          # once
pip install claude-agent-sdk              # Python 3.10+

# See the prompt the agent would get, without calling a model:
python3 examples/agent-sdk-prefetch/prefetch.py --repo your-project --dry-run \
  "Which callers of parseAccept handle the Accept-Language header?"

# Run the agent (uses your Claude Code credentials):
python3 examples/agent-sdk-prefetch/prefetch.py --repo your-project \
  "Which callers of parseAccept handle the Accept-Language header?"
```

Options: `--budget` (context size in estimated tokens, default 1500),
`--model` (default: the SDK's default), `--dry-run`.

## Notes

- The pack is built with lexical search plus the symbols the task names
  (`parseAccept`), which needs no embedding model and takes well under a
  second on an indexed project. `myc context --hybrid` adds vector search; it
  loads the embedding model, which takes seconds.
- `freshness.state` in the JSON says whether the index was built at the
  current HEAD. The example adds a note to the prompt when it is behind.
- Whether prefetching improves answers, and by how much, is not measured yet;
  see [#125](https://github.com/marcmantei/myceliums/issues/125).
