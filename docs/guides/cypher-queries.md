# Cypher Query Guide

Myceliums supports a subset of the Cypher query language for querying the code knowledge graph. You can use Cypher to explore relationships between functions, classes, modules, and other code structures.

## Supported Syntax

The following Cypher clauses and operators are supported:

- `MATCH` / `RETURN` / `WHERE`
- `ORDER BY` / `LIMIT` / `SKIP`
- `CONTAINS` / `starts_with` / `ends_with`
- `IS NULL` / `IS NOT NULL`
- `AND` / `OR` / `NOT`

**Write operations are blocked.** You cannot use `CREATE`, `DELETE`, `SET`, or `MERGE`. The graph is read-only through Cypher queries.

## Node Types

These are the types of nodes in the knowledge graph:

| Node type | Description |
|-----------|-------------|
| `Function` | Standalone functions |
| `Class` | Class definitions |
| `Interface` | Interface definitions |
| `Method` | Methods within classes |
| `Variable` | Variables and constants |
| `Module` | Module-level entities |
| `File` | Source files |
| `Community` | Detected architectural clusters |
| `Process` | Detected workflows/processes |
| `Enum` | Enum definitions |
| `Struct` | Struct definitions |
| `Trait` | Trait definitions |
| `Type` | Type aliases and definitions |
| `Constant` | Named constants |
| `Property` | Object properties |
| `Namespace` | Namespace declarations |
| `Section` | Document sections |
| `Reference` | External references |
| `Person` | People (from email/communication analysis) |
| `EmailThread` | Email threads |
| `Rationale` | Design rationale entries |

## Relationship Types

| Relationship | Meaning |
|-------------|---------|
| `CALLS` | Function/method calls another |
| `IMPORTS` | File/module imports another |
| `CONTAINED_BY` | Symbol is contained within a file or class |
| `MEMBER_OF` | Symbol is a member of a class/module |
| `IMPLEMENTS` | Class implements an interface |
| `EXTENDS` | Class extends another class |
| `REFERENCES` | Symbol references another symbol |
| `BELONGS_TO_COMMUNITY` | Symbol belongs to an architectural community |
| `RATIONALE_FOR` | Rationale is associated with a symbol |
| `SENT` | Person sent an email |
| `IN_THREAD` | Email belongs to a thread |
| `REPLY_TO` | Email replies to another |
| `MENTIONS` | Entity mentions another |

## Common Query Patterns

### 1. Find all functions

```cypher
MATCH (f:Function)
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

### 4. Find imports in a file

What does a file containing "handler" in its path import?

```cypher
MATCH (f:File)-[:IMPORTS]->(target)
WHERE f.file_path CONTAINS "handler"
RETURN target.name
```

### 5. Find community members

What symbols belong to the "auth" community?

```cypher
MATCH (s)-[:BELONGS_TO_COMMUNITY]->(c:Community)
WHERE c.label = "auth"
RETURN s.name, s.kind
```

### 6. Find functions by pattern

Find all functions with "test" in their name:

```cypher
MATCH (f:Function)
WHERE f.name CONTAINS "test"
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

### Find classes that implement an interface

```cypher
MATCH (c:Class)-[:IMPLEMENTS]->(i:Interface)
WHERE i.name = "Repository"
RETURN c.name, c.file_path
```

### Find the inheritance chain

```cypher
MATCH (child:Class)-[:EXTENDS]->(parent:Class)
RETURN child.name, parent.name
```

### Find orphan functions (called by nothing)

```cypher
MATCH (f:Function)
WHERE NOT ()-[:CALLS]->(f)
RETURN f.name, f.file_path
ORDER BY f.name
```

### Count symbols per file

```cypher
MATCH (s)-[:CONTAINED_BY]->(f:File)
RETURN f.file_path, count(s) AS symbol_count
ORDER BY symbol_count DESC
LIMIT 10
```

### Find all communities

```cypher
MATCH (c:Community)
RETURN c.label, c.description
ORDER BY c.label
```

## Using Cypher via MCP

When myceliums is running as an MCP server, use the `cypher_query` tool:

```json
{
  "tool": "cypher_query",
  "arguments": {
    "query": "MATCH (f:Function) WHERE f.name CONTAINS 'auth' RETURN f.name, f.file_path LIMIT 10"
  }
}
```

The results come back as structured JSON, making them easy for AI agents to process without reading raw source files.

## Tips

- **Start broad, then narrow.** Use `LIMIT` generously while exploring. You can always remove it once you know what you are looking for.
- **Use `CONTAINS` for fuzzy matching.** File paths and symbol names often have predictable substrings.
- **Combine relationship traversals.** You can chain multiple `MATCH` clauses or use multi-hop patterns like `(a)-[:CALLS]->(b)-[:CALLS]->(c)` to trace call chains.
- **Check node properties.** Most nodes have `name`, `file_path`, and `kind` properties. Communities have `label` and `description`.
