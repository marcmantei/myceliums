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

Semantic search uses vector embeddings to find code by meaning rather than exact keywords. Myceliums uses the all-MiniLM-L6-v2 model (384-dimensional vectors) to encode both the query and the indexed code.

**Characteristics:**
- Understands natural language queries
- Finds conceptually similar code, even when different words are used
- Requires embeddings (run `myc analyze --force` if you previously skipped them)
- First run downloads the embedding model (~100 MB, cached at `.fastembed_cache/`)

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
- Best overall result quality for most queries
- Requires embeddings
- Slightly slower than BM25 alone (runs both searches)
- Optional: add `--rerank` for cross-encoder reranking (highest quality, slowest)

**Best for:**
- General-purpose code search where you want the best results
- Queries that mix exact terms with natural language

**Example:**

```bash
# Standard hybrid search
myc search "user authentication middleware" --hybrid

# Hybrid with cross-encoder reranking for maximum precision
myc search "user authentication middleware" --hybrid --rerank
```

The `--rerank` flag adds a cross-encoder pass that re-scores the top results. This produces the highest quality ranking but takes longer.

## Decision Guide

| You want to... | Use | Needs embeddings? |
|----------------|-----|-------------------|
| Find exact function name | `myc search "functionName"` | No |
| Find code by description | `myc semantic-search "validates email"` | Yes |
| Best results overall | `myc search "query" --hybrid` | Yes |
| Highest precision | `myc search "query" --hybrid --rerank` | Yes |

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

When using myceliums through MCP (e.g., in Claude Code or Cursor), the same search modes are available as tools:

- `context_search` performs hybrid search by default
- `semantic_search` performs semantic search
- `hybrid_search` performs hybrid search with optional reranking

The AI agent selects the appropriate tool based on the query. If embeddings are not available, it falls back to BM25 automatically.

## How Symbols Are Embedded (and Truncated)

Semantic and hybrid search embed **one vector per symbol**. The document text
for a symbol is built as:

```
{kind} {name} {decorators} {signature} {return_type} {superclasses} {content_head}
```

The leading header (kind, name, signature, decorators, return type,
superclasses) is short and high-signal, so it is **always kept whole**. Only the
trailing `content_head` is truncated, and the truncation is **bounded by the
embedding model's token budget**, not an arbitrary byte count.

### Why truncation is bounded

Every embedding model has a maximum input length in tokens. Text longer than
that is silently discarded by the model's tokenizer. To keep index-time text
within that limit, myceliums derives a **content byte budget** from the model's
declared `max_input_tokens`:

```
content_bytes = max(256, (max_input_tokens − header_reserve) × bytes_per_token)
```

with `header_reserve = 64` tokens and a conservative `bytes_per_token = 4`
(subword tokenizers average ~3–4 characters per token on code; picking the high
end keeps us at or under the true limit rather than over it). For the default
`multilingual-e5-small` model (512 tokens) this is **1792 bytes** of content —
substantially more than the previous fixed 512-byte cut, and it scales with the
model:

| Model | `max_input_tokens` | Content budget (bytes) |
|-------|-------------------:|-----------------------:|
| all-minilm-l6-v2 | 256 | 768 |
| multilingual-e5-small / base / large | 512 | 1792 |
| jina-embeddings-v2-base-code | 8192 | 32512 |

### Known limitation: long symbols are still truncated

This is the **single-vector-per-symbol** design (issue #36, option 2). A symbol
whose content exceeds the model's budget still loses its tail from the semantic
index. The header and the first ~1.8 KB (default model) of the body are indexed;
a very long function's final branches are not directly embedded. BM25 still
indexes the **full** symbol text, so lexical search covers what semantic search
truncates, and hybrid search fuses the two.

The reranker uses the **same** builder as the retriever, so the cross-encoder
scores exactly the text that was indexed — there is no longer an asymmetry where
rerank saw content the retriever never embedded.

### Roadmap: multi-chunk embeddings

Full coverage of long symbols requires splitting them into overlapping chunks,
embedding each chunk as its own row, and de-duplicating to the best chunk per
symbol at query time (issue #36, option 1). That improves recall at the cost of
index growth and query-side dedup, and is tracked as a follow-up rather than
shipped here. For launch, the bounded single-vector approach above is the
documented, measured behaviour.
