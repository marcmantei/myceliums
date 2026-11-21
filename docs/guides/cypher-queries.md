# Cypher Query Guide

Myceliums supports a subset of the Cypher query language for querying the code knowledge graph. You can use Cypher to explore relationships between functions, classes, modules, and other code structures. Every example in this guide is run against a test graph by the `myceliums-cypher` test suite, so it stays valid.

## Supported Syntax

The following Cypher clauses and operators are supported:

- `MATCH` / `RETURN` / `WHERE`
- `ORDER BY` / `LIMIT` / `SKIP`
- `CONTAINS` / `=` / `<>` / `<` / `>` / `<=` / `>=`
- `IS NULL` / `IS NOT NULL`
- `AND` / `OR` / `NOT`
- Aggregations `count`, `sum`, `avg`, `min`, `max`, `collect`; `toLower` / `toUpper`
- Path functions `shortestPath`, `allPaths`, `anyPath` (`MATCH (a), (b), p = shortestPath((a)-[*..5]->(b))`). Their endpoints come from other patterns, and comma-separated patterns are cross-joined, which exceeds the 10,000-row limit on all but tiny graphs: for call chains use the `trace_flow` MCP tool or `myc impact`.

Not supported: `WITH`, `DISTINCT`, inline property maps in patterns (`(n {name: "x"})` — use `WHERE`), and pattern predicates in `WHERE`. A query with text the lexer cannot read (an unterminated string, a character such as `$` or `|`) fails with its position instead of being cut short; a single trailing `;` is accepted.

**Write operations are blocked.** You cannot use `CREATE`, `DELETE`, `SET`, or `MERGE`. The graph is read-only through Cypher queries.

**Limits.** An expression may contain at most 64 nested `NOT`s, function calls, or aggregations (counted together, e.g. `NOT f(count(x))` uses 3); a 65th level fails with a parse error. `AND`/`OR` chains may be any length. `ORDER BY` sorts a column that mixes value types by type first, in openCypher order — maps < lists < strings < booleans < numbers < `null` — then by value within each type, so `null` comes last ascending and first with `DESC`.

## Node Labels

The graph has two node labels. Any other label (`:Function`, `:Class`, ...)
is rejected with `Unknown label`.

| Label | What it matches | Properties |
|-------|-----------------|------------|
| `CodeSymbol` | Every extracted symbol: functions, methods, classes, documents, e-mails, ... A node pattern without a label, `(s)`, matches symbols too. | `uid`, `name`, `qualified_name`, `kind`, `file_path`, `start_line`, `end_line`, `signature`, `metadata` |
| `File` | Indexed files. Only in a node pattern of its own: relationships connect symbols, so `(f:File)-[...]->(x)` matches nothing. | `uid`, `path`, `language`, `num_symbols` |

The kind of symbol is the `kind` property, filtered with `WHERE`:

| `kind` value | Meaning |
|--------------|---------|
| `Function` | Standalone functions |
| `Method` | Methods within classes |
| `Class` | Classes, structs and traits |
| `Interface` | Interfaces |
| `TypeAlias` | Type aliases |
| `Enum` | Enums |
| `Variable` / `Constant` | Module-level variables and constants |
| `Module` | Modules and namespaces |
| `Import` | Import statements |
| `Section` / `Document` | Sections and documents of Markdown and other text files |
| `Rationale` | Design rationale comments (`WHY:`, `NOTE:`, ...) |
| `Email` / `Conversation` / `Person` / `Attachment` | E-mail analysis |

## Relationship Types

| Relationship | Meaning |
|-------------|---------|
| `CALLS` | Function/method calls another |
| `IMPORTS` | A symbol imports another |
| `CONTAINED_BY` | Symbol is contained by another (a method in its class) |
| `MEMBER_OF` | Symbol is a member of a container |
| `STEP_IN_PROCESS` | Symbol is a step of a traced process |
| `REFERENCES` | A document links to another document or code file |
| `RATIONALE_FOR` | A rationale comment explains a symbol |
| `REPLY_TO` / `SENT_BY` / `RECEIVED_BY` / `HAS_ATTACHMENT` / `PART_OF_CONVERSATION` | E-mail structure |
| `MENTIONS` | An e-mail body mentions a symbol or person |

Communities and processes are not graph nodes; use `myc communities` /
`myc processes` or the MCP tools for them.

## Common Query Patterns

### 1. Find all functions

```cypher
MATCH (f:CodeSymbol)
WHERE f.kind = "Function"
RETURN f.name, f.file_path
LIMIT 20
```

### 2. Find callers of a function

Who calls the `authenticate` function?

```cypher
MATCH (caller)-[:CALLS]->(target)
WHERE target.name = "authenticate"
RETURN caller.name, caller.file_path
```

### 3. Find what a function calls

What does `handleRequest` call?

```cypher
MATCH (source)-[:CALLS]->(target)
WHERE source.name = "handleRequest"
RETURN target.name
```

### 4. Find files by language

```cypher
MATCH (f:File)
WHERE f.language = "rust"
RETURN f.path, f.num_symbols
ORDER BY f.num_symbols DESC
LIMIT 10
```

### 5. Find the methods of a class

```cypher
MATCH (m)-[:CONTAINED_BY]->(c)
WHERE c.name = "Router" AND c.kind = "Class"
RETURN m.name, m.start_line
ORDER BY m.start_line
```

### 6. Find functions by pattern

Find all functions with "test" in their name:

```cypher
MATCH (f:CodeSymbol)
WHERE f.kind = "Function" AND f.name CONTAINS "test"
RETURN f.name, f.file_path
ORDER BY f.name
LIMIT 10
```

### 7. Cross-module calls

Find calls from API code into database code:

```cypher
MATCH (a)-[:CALLS]->(b)
WHERE a.file_path CONTAINS "api" AND b.file_path CONTAINS "database"
RETURN a.name, b.name
```

## More Examples

### Call edges between functions

```cypher
MATCH (a:CodeSymbol)-[:CALLS]->(b:CodeSymbol)
WHERE a.kind = "Function" AND b.kind = "Function"
RETURN a.name, b.name
LIMIT 10
```

### Count symbols per file

```cypher
MATCH (s:CodeSymbol)
RETURN s.file_path, count(s) AS symbol_count
ORDER BY symbol_count DESC
LIMIT 10
```

### Count symbols per kind

```cypher
MATCH (s)
RETURN s.kind, count(s) AS total
ORDER BY total DESC
```

### Rationale comments and what they explain

```cypher
MATCH (r)-[:RATIONALE_FOR]->(s)
RETURN r.name, s.name, s.file_path
LIMIT 20
```

## Using Cypher via MCP

When myceliums is running as an MCP server, use the `cypher_query` tool:

```json
{
  "tool": "cypher_query",
  "arguments": {
    "query": "MATCH (f:CodeSymbol) WHERE f.kind = 'Function' AND f.name CONTAINS 'auth' RETURN f.name, f.file_path LIMIT 10"
  }
}
```

The results come back as structured JSON, making them easy for AI agents to process without reading raw source files.

## Tips

- **Start broad, then narrow.** Use `LIMIT` generously while exploring. You can always remove it once you know what you are looking for.
- **Use `CONTAINS` for fuzzy matching.** File paths and symbol names often have predictable substrings.
- **One pattern per query.** Comma-separated patterns in one `MATCH` are cross-joined and fail beyond 10,000 combined rows; a relationship pattern with `WHERE` filters on both ends is the usual way to connect two symbols.
- **Check node properties.** Symbols have `name`, `qualified_name`, `kind`, `file_path` and line numbers; files have `path`, `language` and `num_symbols`.
