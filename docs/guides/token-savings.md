# How Token Savings Work

AI coding tools like Claude Code, Cursor, and Copilot typically send entire source files to the model for every query. A single file might cost 2,000-10,000 tokens. When the model needs context from multiple files, the token cost multiplies quickly.

Myceliums solves this by providing **structural summaries** instead of full source code.

## The Problem

When an AI agent needs to understand what a function does, it usually reads the entire file. For a 200-line file, that might look like:

- Full file read: ~4,000 tokens
- 3 related files for context: ~12,000 tokens
- Total per question: ~16,000 tokens

Most of those tokens are boilerplate, imports, and unrelated code that the model reads but does not need.

## The Solution: `get_review_context`

The key tool is `get_review_context`, available via MCP. Given a git diff (or the current uncommitted changes), it returns:

- Changed symbol signatures (function names, parameters, return types)
- Affected callers (what calls the changed code)
- Affected callees (what the changed code calls)
- Touched communities (which architectural clusters are involved)

A typical response looks something like this:

```
Changed symbols:
  - authenticate(username: str, password: str) -> Token  [Function, auth/login.py]
  - validate_credentials(creds: Credentials) -> bool     [Function, auth/validators.py]

Callers affected:
  - handle_login_request  [api/routes.py:45]
  - refresh_token          [api/routes.py:82]

Callees affected:
  - hash_password          [auth/crypto.py:12]
  - create_session         [auth/session.py:30]

Communities touched:
  - auth-core (5 members)
```

This is shorter than the files it summarises, because it leaves out bodies, imports and unrelated code. How much shorter depends on the files involved. Each response ends with a `Token estimate` line that sets the size of the summary against the size of the indexed symbols in the affected files (characters divided by four for both). That line is a size estimate, not a measured token saving: there is no measurement yet of how many tokens an agent spends on a review with and without myceliums, or of whether it reaches the same conclusion from the summary alone.

## Token Savings Are Not Automatic

This is an important point: myceliums does not intercept or modify the AI agent's behavior. The agent must **choose** to call myceliums tools instead of reading files directly. If the agent reads the file anyway, you get no savings.

## How to Encourage the AI to Use Myceliums

### Step 1: Run the Setup Wizard

```bash
myc setup
```

In Step 2 of the wizard, enable AI instructions. This configures the SessionStart hook to include guidance telling the AI agent to prefer myceliums tools for structural queries.

### Step 2: Verify the Hook Is Active

After setup, start a new session. You should see a message from myceliums in the session initialization indicating the knowledge graph is available. The AI agent will then prefer calling `context_search`, `symbol_context`, and `get_review_context` over reading raw files.

### Step 3: For Cursor Users

Cursor uses a rules system instead of hooks. Consider adding a rule file in `~/.cursor/rules/` or in your project's `.cursor/rules/` directory that instructs the AI to use myceliums MCP tools for code understanding tasks.

## What Works Without Embeddings

Not all features require the embedding model (which is a ~100 MB download on first use). Here is what works out of the box:

**No embeddings needed:**
- BM25 text search (keyword matching)
- Cypher graph queries
- `get_review_context` (structural summaries)
- Community detection
- Process tracing
- Impact analysis

**Embeddings required:**
- Semantic search (meaning-based similarity)
- Hybrid search (BM25 + vector ranking)

If you want to minimize setup overhead, run `myc analyze --skip-embeddings`. You get the full knowledge graph, structural summaries, and keyword search. Add embeddings later if you need semantic search.

## Summary

| Approach | What the model receives | Setup needed |
|----------|-------------------------|--------------|
| Raw file reads | Whole files: bodies, imports and unrelated code included | None |
| Myceliums structural summary | Signatures, callers, callees and touched communities | `myc setup` + analysis |

There is no measured figure for the saving yet. The `Token estimate` line tells you roughly how much smaller one summary is than the source behind it; it cannot tell you how many tokens an agent would have spent otherwise, because that depends on what the agent would have read instead.
