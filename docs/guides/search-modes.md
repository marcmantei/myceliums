# Search Modes Explained

Myceliums offers three search modes, each with different strengths. Choosing the right one depends on what you are looking for and whether you have generated embeddings.

## BM25 Search

**Command:** `myc search "query"`

BM25 is a keyword-based ranking algorithm. It scores documents by how well they match the search terms, accounting for term frequency and document length.

**Characteristics:**
- Fast, runs entirely on the local index
- No model download required
- Works without embeddings
- Matches exact tokens (identifiers, function names, variable names)

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
