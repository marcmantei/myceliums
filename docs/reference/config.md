# Configuration Reference

Myceliums uses two configuration files: a per-project file for project-specific settings and a global file for user-wide defaults.

## Project Configuration

**File:** `.myceliums.toml` (placed in the project root)

Created with `myc init`. All fields are optional. Omitted fields use their defaults.

### `[project]`

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `name` | string | directory name | Human-readable project name |
| `languages` | list of strings | `[]` (auto-detect) | Languages to analyze. Empty means auto-detect all supported languages |

### `[analysis]`

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `include` | list of strings | `[]` (include all) | Glob patterns for files/directories to include. Empty means include everything |
| `exclude` | list of strings | see below | Glob patterns for files/directories to exclude |
| `max_file_size_kb` | integer | `512` | Maximum file size in KB to analyze (0 = no limit) |
| `parse_timeout_secs` | integer | `30` | Parse timeout per file in seconds (0 = no timeout) |
| `max_line_length_bytes` | integer | `5120` | Maximum line length in bytes (0 = no limit). Lines exceeding this are skipped |
| `skip_patterns` | list of strings | see below | File name patterns to skip (substring match) |
| `batch_size` | integer | `500` | Number of items to buffer before flushing a batch to storage |
| `channel_buffer_size` | integer | `8` | Capacity of the async channel between producers and the batch writer |
| `use_dsl` | bool | `false` | Use DSL-driven parsing for supported languages (Python, Go). When false, uses hand-coded extractors |
| `ann_threshold` | integer | `10000` | Minimum number of symbols before creating an ANN index for vector search |
| `embedding_batch_size` | integer | `256` | Number of symbols to embed in a single batch. Smaller values reduce peak memory for large repositories |

**Default `exclude` patterns:**

```toml
exclude = [
    "node_modules/**",
    ".git/**",
    "target/**",
    "dist/**",
    "build/**",
    "__pycache__/**",
    ".venv/**",
    "venv/**",
]
```

**Default `skip_patterns`:**

```toml
skip_patterns = ["min.js", "min.css", "bundle.js", "map"]
```

### `[process]`

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `entry_points` | list of strings | `[]` | Named entry points for process tracing. If empty, entry points are auto-detected from exported functions and handlers |

### `[community]`

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `min_community_size` | integer | `3` | Minimum number of symbols for a community to be reported |
| `resolution` | float | `1.0` | Louvain resolution parameter. Higher values produce more (smaller) communities, lower values produce fewer (larger) communities |

### `[embedding]`

Selects the model that turns code into the vectors used for semantic search.
The chosen model shapes every stored vector, so this section is committed with
the project (shared by the team) rather than kept as per-user state.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `provider` | string | `"local"` | `"local"` (bundled ONNX models via fastembed) or `"openai-compatible"` (any server speaking the OpenAI embeddings API — Ollama, LM Studio, TEI, vLLM, or a cloud provider) |
| `model` | string | `"multilingual-e5-small"` | Registry id for `local`; the API model name for `openai-compatible` |
| `reranker` | string | `"bge-reranker-v2-m3"` | Cross-encoder used when `rerank` is requested at search time |
| `base_url` | string | — | Embeddings API base URL. **Required** for `openai-compatible` |
| `dim` | integer | — | Vector dimension. **Required** for `openai-compatible` (cannot be derived); ignored for `local` |
| `api_key_env` | string | `"MYCELIUMS_EMBEDDING_API_KEY"` | Name of the environment variable holding the API key. The key itself never goes into this file |
| `query_prefix` | string | — | Prefix prepended to search queries (e.g. `"query: "` for E5-style models). Ignored for `local` — the registry value wins |
| `passage_prefix` | string | — | Prefix prepended to indexed documents (e.g. `"passage: "`). Ignored for `local` — the registry value wins |

#### Which fields invalidate an existing index

Vectors are only comparable when they were produced by the same embedder, so an
index records the embedder it was built with (its **fingerprint**) and query
paths resolve the matching embedder from that record. Changing a field that
shapes the stored vectors makes the old vectors incomparable — the index must be
rebuilt with a full analysis (`myc analyze`) before the change takes effect.

**Fields in the fingerprint — a change invalidates the index:**

| Field | Why it invalidates |
|-------|--------------------|
| `provider` | A different backend produces a different vector space |
| `model` | Different weights → different vectors, even at the same dimension |
| `dim` | Different vector length; the stored table is physically rebuilt |
| `base_url` (host-normalized) | The same model name behind a different endpoint can be a different model. Cosmetic differences — trailing slash, host case, an explicit default port (`:80`/`:443`) — are folded and do **not** invalidate |
| `query_prefix` / `passage_prefix` | E5-style prefixes change the text sent to the model, so they change the vectors |

**Fields *not* in the fingerprint — a change does not invalidate the index:**

| Field | Why it is safe |
|-------|----------------|
| `api_key_env` | Only the environment-variable *name* is stored, never the key. Rotating the key — or renaming the variable that holds it — does not change the model or its output |
| `reranker` | Reranking only reorders query results at search time; it never shapes the stored vectors |

**Incremental vs. full runs.** Incremental runs (`myc watch`, re-indexing
changed files) never switch embedders: they keep the index's recorded embedder
and warn if the config now resolves to a different fingerprint — mixing two
embedders in one index would silently corrupt search. A full analysis
(`myc analyze`) adopts the configured embedder and, when it differs from the
recorded one, wipes the stale vectors first so no mixed index can survive. This
wipe is enforced by the analyzer itself, not by call-site ordering, so it cannot
be forgotten.

**Metadata versioning.** Each index records a `meta_version`. When a release
changes which fields shape stored vectors (or how the fingerprint is computed),
this version is bumped; an index written by an older release is detected on the
next incremental run and reported as stale, with the same instruction to run a
full re-analysis. Records written before `meta_version` existed read back as
version `1`.

### Example: Minimal

```toml
[project]
name = "my-app"
```

### Example: Python Project

```toml
[project]
name = "ml-pipeline"
languages = ["python"]

[analysis]
include = ["src/**", "lib/**"]
exclude = [
    "node_modules/**",
    ".git/**",
    "__pycache__/**",
    ".venv/**",
    "venv/**",
    "data/**",
    "notebooks/**",
]
max_file_size_kb = 1024

[community]
min_community_size = 5
resolution = 1.2
```

### Example: TypeScript Monorepo

```toml
[project]
name = "acme-monorepo"
languages = ["typescript", "javascript"]

[analysis]
include = ["packages/**", "apps/**"]
exclude = [
    "node_modules/**",
    ".git/**",
    "dist/**",
    "build/**",
    "coverage/**",
    "*.test.ts",
    "*.spec.ts",
]
max_file_size_kb = 256
skip_patterns = ["min.js", "bundle.js", "chunk.js", "map"]
embedding_batch_size = 128

[community]
resolution = 0.8
```

### Example: Full Configuration

```toml
[project]
name = "my-project"
languages = ["rust", "python", "typescript"]

[analysis]
include = ["src/**", "crates/**"]
exclude = [
    "node_modules/**",
    ".git/**",
    "target/**",
    "dist/**",
    "build/**",
    "__pycache__/**",
    ".venv/**",
    "venv/**",
]
max_file_size_kb = 512
parse_timeout_secs = 30
max_line_length_bytes = 5120
skip_patterns = ["min.js", "min.css", "bundle.js", "map"]
batch_size = 500
channel_buffer_size = 8
use_dsl = false
ann_threshold = 10000
embedding_batch_size = 256

[process]
entry_points = ["main", "handleRequest", "app"]

[community]
min_community_size = 3
resolution = 1.0
```

---

## Global Configuration

**File:** `~/.myceliums/config.toml`

Managed with `myc configure`. Provides user-wide defaults that apply across all repositories. Created automatically on first use.

### `[defaults]`

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `max_results` | integer | `20` | Default maximum search results |
| `log_level` | string | `"warn"` | Log verbosity. Valid values: `trace`, `debug`, `info`, `warn`, `error` |

### `[analysis]`

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `default_exclude` | list of strings | see below | Default exclude patterns applied when no project config exists |
| `max_file_size_kb` | integer | `500` | Default maximum file size in KB |

**Default `default_exclude`:**

```toml
default_exclude = ["node_modules", "__pycache__", ".git", "dist", "build", "target"]
```

### `[llm]`

Configuration for optional LLM-based features (semantic mentions extraction).

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `provider` | string | `"ollama"` | LLM provider. Valid values: `"ollama"`, `"openai"` |
| `model` | string | `"qwen2.5:7b"` | Model name (e.g., `"qwen2.5:7b"` for Ollama, `"gpt-3.5-turbo"` for OpenAI) |
| `base_url` | string | `"http://localhost:11434"` | Base URL for the provider API |
| `api_key` | string | none | Optional API key (used by OpenAI-compatible providers) |
| `enable_mentions` | bool | `false` | Enable LLM-based semantic mentions extraction. Disabled by default for cost control |
| `mentions_max_content_chars` | integer | `4000` | Maximum content length sent to the LLM for mentions extraction |
| `mentions_max_symbols` | integer | `100` | Maximum symbols included in the mention extraction registry |
| `mentions_min_confidence` | float | `0.7` | Minimum confidence threshold for LLM mentions (0.0 to 1.0) |

### `[setup]`

Preferences saved by the setup wizard. These are typically set automatically, not manually.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `completed` | bool | `false` | Whether the setup wizard has been completed at least once |
| `instructions_enabled` | bool | `false` | Whether AI instructions are enabled in editor configs |
| `analysis_mode` | string | `"session-start"` | Analysis mode preference: `"session-start"`, `"watch"`, or `"manual"` |
| `configured_editors` | list of strings | `[]` | List of editor names that were configured during setup |

### Configurable Keys via CLI

The `myc configure --set` command supports these dot-separated keys:

| Key | Type | Description |
|-----|------|-------------|
| `defaults.max_results` | integer | Default maximum search results |
| `defaults.log_level` | string | Log level (`trace`, `debug`, `info`, `warn`, `error`) |
| `analysis.max_file_size_kb` | integer | Default maximum file size |
| `analysis.default_exclude` | comma-separated list | Default exclude patterns |
| `llm.provider` | string | `"ollama"` or `"openai"` |
| `llm.model` | string | Model name |
| `llm.base_url` | string | Provider API base URL |
| `llm.api_key` | string | API key (empty string to clear) |
| `llm.enable_mentions` | bool | `true` or `false` |
| `llm.mentions_max_content_chars` | integer | Max content length for mentions |
| `llm.mentions_max_symbols` | integer | Max symbols for mentions |
| `llm.mentions_min_confidence` | float | 0.0 to 1.0 |

```bash
# Examples
myc configure --set defaults.max_results=50
myc configure --set defaults.log_level=info
myc configure --set llm.provider=openai
myc configure --set llm.model=gpt-4o-mini
myc configure --set llm.api_key=sk-...
myc configure --set llm.enable_mentions=true
myc configure --set analysis.default_exclude="node_modules,dist,build,.git"
```

### Example: Full Global Configuration

```toml
[defaults]
max_results = 30
log_level = "info"

[analysis]
default_exclude = ["node_modules", "__pycache__", ".git", "dist", "build", "target"]
max_file_size_kb = 500

[llm]
provider = "ollama"
model = "qwen2.5:7b"
base_url = "http://localhost:11434"
enable_mentions = false
mentions_max_content_chars = 4000
mentions_max_symbols = 100
mentions_min_confidence = 0.7

[setup]
completed = true
instructions_enabled = true
analysis_mode = "session-start"
configured_editors = ["claude", "vscode", "zed"]
```

### Precedence

When both project and global configurations exist, project-level settings take precedence over global defaults for overlapping fields (e.g., `max_file_size_kb`, exclude patterns).
