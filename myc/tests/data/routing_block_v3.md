<!-- BEGIN myceliums -->
<!-- v3, written by `myc setup-claude`: edits inside these markers are overwritten; `myc setup-claude --uninstall` removes them -->
## Code navigation: myceliums
Structural questions go to the myceliums code graph first, not grep/rg:
- callers / callees of X: `myc callers X`, `myc callees X` (`--depth 3` for chains)
- what breaks if X or a file changes: `myc impact X`, `myc impact path/to/file`
- where or how something is implemented: `myc context "<question>"`
The myceliums MCP tools and the `code-explorer` subagent answer the same questions.
Graph answers are heuristic: when completeness matters, or a result says `Incomplete` or lists calls without an edge, read the source before answering.
grep/rg stay right for string literals, config values, log messages and non-code files.
<!-- END myceliums -->
