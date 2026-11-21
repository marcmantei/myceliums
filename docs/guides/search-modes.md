# Search Modes Explained

Myceliums offers three search modes, each with different strengths. Choosing the right one depends on what you are looking for and whether you have generated embeddings.

## BM25 Search

**Command:** `myc search "query"`

BM25 is a keyword-based ranking algorithm. It scores documents by how well they match the search terms, accounting for term frequency and document length.

**Characteristics:**
- Fast, runs entirely on the local index
- No model download required
- Works without embeddings
- Matches whole tokens, with identifiers split on `snake_case`/`camelCase`

BM25 tokenizes both the query and the indexed text on word boundaries **and**
identifier boundaries, then lowercases. This means:

- A query token matches only a *whole* token in the document — searching `cat`
  does **not** match `concatenate`.
- Identifiers are split, so `get_user_name` indexes as `get`, `user`, `name`;
  the natural-language query `user name` matches it.
- Term frequency and document length are counted in tokens, not characters, so
  a symbol is not penalised merely for having long identifiers.

**Best for:**
- Finding a specific function or class by name
- Searching for exact identifiers (e.g., `handleAuth`, `UserRepository`)
- Quick lookups where you know the terminology used in the code

**Example:**

```bash
myc search "validateEmail"
```

This will find symbols and files containing the exact token `validateEmail`, ranked by relevance.

## Semantic Search

**Command:** `myc semantic-search "query"`

Semantic search uses vector embeddings to find code by meaning rather than exact keywords. By default Myceliums encodes the query and the indexed code with the `jina-embeddings-v2-base-code` model (768-dimensional vectors, trained on code and English text). Long symbols are embedded in several chunks. Other models can be selected in configuration (see the [configuration reference](../reference/config.md)). For queries in languages other than English, or for the fastest first index, use `multilingual-e5-small` with `chunking = false`.

**Characteristics:**
- Understands natural language queries
- Finds conceptually similar code, even when different words are used
- Requires embeddings (run `myc analyze --force` if you previously skipped them)
- First run downloads the embedding model (about 644 MB for the default model; sizes per model are in the README), cached in `FASTEMBED_CACHE_DIR`, else `HF_HOME`, else `~/.myceliums/models`
- The first index of a repository embeds every symbol and takes the longest (with the default model about 4.4× as long as with `multilingual-e5-small`); later runs reuse the vectors of unchanged symbols

**Best for:**
- Natural language queries ("function that validates user input")
- Finding code when you do not know the exact name
- Discovering similar patterns across the codebase

**Example:**

```bash
myc semantic-search "function that checks if a user has permission"
```

This will find authorization-related code even if it uses words like `authorize`, `checkAccess`, or `hasPermission` rather than the exact query terms.

## Hybrid Search

**Command:** `myc search "query" --hybrid`

Hybrid search combines BM25 and semantic search using Reciprocal Rank Fusion (RRF). Both search modes run independently, and their rankings are merged to produce a final result list that benefits from the strengths of each approach.

**Characteristics:**
- Matches exact terms and meaning together. On the [fixture benchmark](../../crates/myceliums-benchmarks/benchmarks/METHODOLOGY.md#recorded-results) it clearly beats BM25, but not semantic search alone
- Requires embeddings
- Slightly slower than BM25 alone (runs both searches)
- Optional: add `--rerank` for cross-encoder reranking (slowest; downloads a ~2.2 GB reranker on first use)

**Best for:**
- Queries that mix exact identifiers with natural-language words, when you want both kinds of match in one list
- Queries that mix exact terms with natural language

**Example:**

```bash
# Standard hybrid search
myc search "user authentication middleware" --hybrid

# Hybrid with cross-encoder reranking for maximum precision
myc search "user authentication middleware" --hybrid --rerank
```

The `--rerank` flag adds a cross-encoder pass that re-scores the top 20 results. It is meant to sharpen the ordering and takes noticeably longer; on the [fixture benchmark](../../crates/myceliums-benchmarks/benchmarks/METHODOLOGY.md#recorded-results) its gain over plain hybrid was not distinguishable from zero.

## Test Code Ranks Lower

Every mode ranks test code — symbols in test files and directories by the
language's convention, Rust `#[cfg(test)]` modules and `#[test]` functions —
below product code by default. It is a down-weight, not a filter: a test that
clearly matches better still shows, only later. Use `--tests include` to rank
tests like everything else, or `--tests prefer` when you are looking for the
tests:

```bash
myc search "cookie jar" --hybrid --tests prefer
```

The rules, the weight and how it was measured: [Test code](../reference/mcp-tools.md#test-code).

## Decision Guide

| You want to... | Use | Needs embeddings? |
|----------------|-----|-------------------|
| Find exact function name | `myc search "functionName"` | No |
| Find code by description | `myc semantic-search "validates email"` | Yes |
| Exact terms and meaning together | `myc search "query" --hybrid` | Yes |
| Re-score the top results (slower) | `myc search "query" --hybrid --rerank` | Yes |
| Find the tests for something | `myc search "query" --hybrid --tests prefer` | Yes |

## Do I Need Embeddings?

If you are unsure whether to generate embeddings, consider your usage:

**Skip embeddings if:**
- You primarily search by exact symbol names
- You want the fastest possible analysis
- You are on a memory-constrained machine
- You only use Cypher queries and `get_review_context`

**Generate embeddings if:**
- You search by describing what code does (natural language)
- You want the best search quality overall
- You use hybrid or semantic search regularly

To generate embeddings after an initial analysis without them:

```bash
myc analyze . --force
```

To analyze without embeddings:

```bash
myc analyze . --skip-embeddings
```

## MCP Integration

When using myceliums through MCP (e.g., in Claude Code or Cursor), the same search modes are available through one tool, `search_code`, and its `mode` parameter:

- `mode=auto` (the default) runs hybrid search when the index has embeddings, and keyword search otherwise (the result says so)
- `mode=keyword` ranks by names, qualified names and signatures and needs no model
- `mode=semantic` performs semantic search
- `mode=hybrid` performs hybrid search, with optional reranking (`rerank=true`)
- `tests=demote` (default), `include` or `prefer` places test code below, among or above product code

The older `context_search`, `semantic_search`, `hybrid_search` and `search_documents` tools remain as deprecated aliases with `--toolsets all` for one release.

## How Symbols Are Embedded

Semantic search and the vector leg of hybrid search compare the query's vector
with vectors computed from each symbol at index time. What a symbol is
embedded *as* is versioned and recorded in the index, so an index never mixes
two ways of building vectors (#92, #157).

### The document: two templates

Template 2 is a structured document, with the most telling lines first. It is
the default: it is used whenever chunking is on (the default) and by the models
evaluated in #157 (`jina-embeddings-v2-base-code`, the default model,
`granite-embedding-small-english-r2`, `coderankembed`, `gte-modernbert-base`):

```text
python method src/requests/sessions.py::SessionRedirectMixin.resolve_redirects
session redirect mixin resolve redirects
def resolve_redirects(self, resp, req, stream=False, timeout=None, ...)
Receives a Response. Returns a generator of Responses or Requests.
hist = []  # keep track of history
...
```

1. language, kind and `file::qualified.name`;
2. the qualified name split into words (`getUserById` → `get user by id`);
3. decorators and the signature;
4. the doc comment written above the symbol (`///`, `/** */`, `//`, `#` lines,
   extracted at parse time) or the Python docstring;
5. the body, without the signature line and docstring it would repeat, every
   line trimmed and blank lines dropped — indentation costs tokens and means
   nothing to retrieval.

The model's window (512 tokens for every registry model but the legacy
`all-minilm-l6-v2`, 256) cuts the end of the document, never the header.

Template 1 — `{kind} {name} {decorators} {signature} {return_type}
{superclasses} {content_head}`, the body cut to a byte budget — is what the e5
family embeds with `chunking = false`, the fast option and the previous
default. Remote (`openai-compatible`) models also embed with template 1.
Measured on the real-repository set, template 2 did not improve
`multilingual-e5-small` (held-out −0.013 [−0.047, +0.021]).

### Long symbols: chunking

With chunking (`[embedding] chunking`, on by default for local models), a symbol longer
than the window is embedded as its header chunk — the document cut at the window, the
symbol's own vector — plus body chunks: consecutive 512-token windows over the
rest of the body, overlapping by 64 tokens, each starting with the symbol's
`language kind file::qualified.name` line, at most 16 vectors per symbol.
Boundaries are counted in the model's own tokens. A symbol's similarity is the
best of its vectors, and results stay one row per symbol: vector search fetches
chunk vectors until no deeper chunk could change the top results, so it stays
exact. Classes, interfaces, enums and modules are not chunked — their bodies are
their members, which have vectors of their own. Chunk vectors live in their own
table and are removed with their file.

Measured on the 60 real-repository queries whose answers include a long
symbol, chunking raised the objective by +0.026 to +0.050 for four of five
models: +0.028 [+0.011, +0.047] for `jina-embeddings-v2-base-code`, +0.033
[+0.003, +0.066] for `multilingual-e5-small`. Over all queries its effect is
within noise. It is on by default because the default configuration, jina with
chunking, was the one whose gain held on the held-out queries. It is cheap: on
the four evaluation repositories, jina stored 254 chunk vectors for 14,795
symbols (2% more vectors) and indexed about 6% longer than without chunking.
`chunking = false` turns it off.

BM25 always indexes the full symbol text, chunked or not.

### Indexing speed, threads and memory

Texts are sorted by token count and batched up to a token budget
(`[analysis] embedding_batch_tokens`, default 16,384 — 32 full windows), so a
batch pads to a length its texts share instead of to the longest of 256 random
symbols. Texts are tokenized and embedded 4,096 at a time, and a text longer
than the window is cut at a word boundary just past it before it is
tokenized, so the memory tokenization takes does not grow with the
repository. `MYCELIUMS_EMBEDDING_THREADS` caps the CPU threads of an embedding
session (default: all logical cores). `myc analyze` shows embedding progress.
The MCP `analyze` tool builds the graph, answers, and embeds in the
background, storing what it embedded every 1,024 symbols or 20 seconds: the
Index line of every result shows the progress, search by meaning covers the
symbols embedded so far, and an interrupted embedding resumes on the next
`analyze`.
A re-index takes over the vectors of unchanged symbols — their chunk vectors
included — and of unchanged body chunks of changed symbols (#151).

The reranker scores the template-1 document of each candidate, whatever the
index's template.
