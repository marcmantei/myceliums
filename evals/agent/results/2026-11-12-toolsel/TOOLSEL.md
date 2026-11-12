# Tool-selection eval: 2026-11-12-toolsel

- **Date:** 2026-11-12
- **Model:** `haiku` (resolved `claude-haiku-5-5`), one turn, Claude Code 2.1.322 (Claude Code)
- **Tools offered:** the live `tools/list` of `myc 0.3.2` (54 tools, sha256 `5703072ff242ae1d…`), as native tool definitions, no built-in tools, tool search off
- **Intents:** 54 natural-language requests, each with one or more acceptable tools (`toolsel/intents.json`); a pick is correct if it is one of them

| Pass | Intents | Correct | Accuracy |
|---:|---:|---:|---:|
| 1 | 54 | 53 | 98 % |
| 2 | 54 | 53 | 98 % |

The two passes chose the same tool for 51 of 54 intents.

1 pick(s) were written out as text instead of being made as a tool call; they count as misses: `emails-search` (pass 1: `search_emails`, right tool).

## Most frequent confusions

| Expected | Chosen instead | Times |
|---|---|---:|
| `search_emails` | `(no tool call)` | 1 |
| `cypher_query` | `context_search` | 1 |

## Per intent

| Intent | Expected | Chosen (per pass) |
|---|---|---|
| Find the function that parses the configuration file. | `context_search`, `hybrid_search`, `semantic_search`, `search_documents` | ✓ `context_search`; ✓ `context_search` |
| Where is user authentication handled in this codebase? | `hybrid_search`, `semantic_search`, `context_search`, `search_documents`, `isolate_intent` | ✓ `context_search`; ✓ `context_search` |
| Look up the class named RequestHandler. | `context_search`, `get_symbol_definition`, `symbol_context`, `search_documents`, `hybrid_search` | ✓ `context_search`; ✓ `context_search` |
| Show me code that deals with retrying failed network calls, even if it never uses the word 'retry'. | `semantic_search`, `hybrid_search` | ✓ `semantic_search`; ✓ `semantic_search` |
| Show me the source code of the function normalize_path. | `get_symbol_definition`, `symbol_context` | ✓ `get_symbol_definition`; ✓ `get_symbol_definition` |
| Before I change save_user, show me its code together with who calls it and what it calls. | `symbol_context` | ✓ `symbol_context`; ✓ `symbol_context` |
| List every function and class defined in src/server/router.ts. | `get_file_symbols` | ✓ `get_file_symbols`; ✓ `get_file_symbols` |
| What calls validate_token? | `get_callers`, `symbol_context` | ✓ `get_callers`; ✓ `get_callers` |
| Which functions does process_order call? | `get_callees`, `symbol_context` | ✓ `get_callees`; ✓ `get_callees` |
| Show the call tree below main, three levels deep. | `get_callees` | ✓ `get_callees`; ✓ `get_callees` |
| Which code depends on encode_frame, including callers of its callers? | `get_callers` | ✓ `get_callers`; ✓ `get_callers` |
| I have uncommitted changes. What else in the codebase could they break? | `detect_impact`, `get_review_context` | ✓ `detect_impact`; ✓ `detect_impact` |
| Give me a compact summary of my current diff for code review: changed functions with their callers and callees. | `get_review_context`, `detect_impact` | ✓ `get_review_context`; ✓ `get_review_context` |
| What questions should a reviewer ask about my current changes? | `get_suggested_questions` | ✓ `get_suggested_questions`; ✓ `get_suggested_questions` |
| How is handle_login connected to write_audit_log? | `find_path` | ✓ `find_path`; ✓ `find_path` |
| Show how a request flows from the HTTP entry point down to the database layer. | `get_processes` | ✓ `get_processes`; ✓ `get_processes` |
| What are the main execution flows in this project? | `get_processes` | ✓ `get_processes`; ✓ `get_processes` |
| How is the code organised into clusters of related modules? | `get_communities`, `architecture_view` | ✓ `get_communities`; ✓ `architecture_view` |
| List all members of community 12 and its entry points. | `get_community_detail` | ✓ `get_community_detail`; ✓ `get_community_detail` |
| Draw a service-level architecture diagram of this repository. | `architecture_view`, `export_mermaid` | ✓ `architecture_view`; ✓ `architecture_view` |
| Export the call graph as a Mermaid flowchart. | `export_mermaid` | ✓ `export_mermaid`; ✓ `export_mermaid` |
| Which functions have the most incoming and outgoing calls? | `get_god_nodes`, `get_centrality_report` | ✓ `get_god_nodes`; ✓ `get_god_nodes` |
| Rank functions by betweenness so I can find bottlenecks. | `get_centrality_report` | ✓ `get_centrality_report`; ✓ `get_centrality_report` |
| How cleanly are the detected modules separated? I want a modularity score. | `get_community_metrics` | ✓ `get_community_metrics`; ✓ `get_community_metrics` |
| Are there import or call cycles between modules? | `detect_circular_dependencies`, `architecture_lint` | ✓ `detect_circular_dependencies`; ✓ `detect_circular_dependencies` |
| Run architecture quality checks on this repository. | `architecture_lint` | ✓ `architecture_lint`; ✓ `architecture_lint` |
| Which files does src/db/pool.go import, directly and transitively, and which files import it? | `get_dependencies` | ✓ `get_dependencies`; ✓ `get_dependencies` |
| Compute afferent and efferent coupling and instability for each directory. | `get_module_coupling` | ✓ `get_module_coupling`; ✓ `get_module_coupling` |
| Find calls between parts of the code that normally have nothing to do with each other. | `get_surprising_connections` | ✓ `get_surprising_connections`; ✓ `get_surprising_connections` |
| What symbols and relationships were added or removed since the last snapshot? | `get_graph_diff`, `detect_architecture_drift`, `snapshot_diff` | ✓ `get_graph_diff`; ✓ `get_graph_diff` |
| Give me a single drift score comparing the current graph with the last snapshot. | `detect_architecture_drift` | ✓ `detect_architecture_drift`; ✓ `detect_architecture_drift` |
| Find functions that nothing calls anymore. | `find_dead_code` | ✓ `find_dead_code`; ✓ `find_dead_code` |
| Which functions are not called by any test? | `get_knowledge_gaps` | ✓ `get_knowledge_gaps`; ✓ `get_knowledge_gaps` |
| Which code is both central and frequently changed, so it is the best refactoring target? | `quality_hotspots` | ✓ `quality_hotspots`; ✓ `quality_hotspots` |
| How many functions, classes and files does this repository have, per language? | `get_stats` | ✓ `get_stats`; ✓ `get_stats` |
| What properties does a Function node have in the graph schema? | `get_schema` | ✓ `get_schema`; ✓ `get_schema` |
| Run this graph query: MATCH (f)-[:CALLS]->(g) WHERE g.name = 'log' RETURN f.name | `cypher_query` | ✓ `cypher_query`; ✓ `cypher_query` |
| Find all classes that implement the Storage interface. | `cypher_query` | ✓ `cypher_query`; ✗ `context_search` |
| Preview renaming getUser to fetchUser across the codebase. | `rename_symbol` | ✓ `rename_symbol`; ✓ `rename_symbol` |
| Who last modified apply_discount, and how many commits have touched it? | `get_git_context` | ✓ `get_git_context`; ✓ `get_git_context` |
| Who owns src/billing according to CODEOWNERS? | `get_ownership` | ✓ `get_ownership`; ✓ `get_ownership` |
| Why was retry_backoff written this way? Show the NOTE and HACK comments about it. | `get_rationale` | ✓ `get_rationale`; ✓ `get_rationale` |
| Find documents and emails that mention the TokenBucket class. | `query_knowledge` | ✓ `query_knowledge`; ✓ `query_knowledge` |
| Search my indexed emails for messages from Alice about the release. | `search_emails` | ✗ `None`; ✓ `search_emails` |
| Show me the whole email thread for conversation UID conv-1234. | `get_conversation` | ✓ `get_conversation`; ✓ `get_conversation` |
| Record an architecture decision: we use PostgreSQL for persistence. | `record_decision` | ✓ `record_decision`; ✓ `record_decision` |
| List the architecture decision records of this repository. | `get_decisions` | ✓ `get_decisions`; ✓ `get_decisions` |
| Call community 7 'billing-service' from now on. | `map_service` | ✓ `map_service`; ✓ `map_service` |
| Match the OpenAPI endpoints in this repo to their handler functions. | `get_contracts` | ✓ `get_contracts`; ✓ `get_contracts` |
| Collect all code that implements rate limiting in this repository. | `isolate_intent`, `hybrid_search`, `semantic_search` | ✓ `hybrid_search`; ✓ `hybrid_search` |
| Compare how repository A and repository B implement authentication. | `differentiate_intent` | ✓ `differentiate_intent`; ✓ `differentiate_intent` |
| Plan the steps to port the caching approach of repository A to repository B. | `plan_adaptation` | ✓ `plan_adaptation`; ✓ `plan_adaptation` |
| Index this project so I can query it. | `analyze` | ✓ `analyze`; ✓ `analyze` |
| Remove the stored analysis data for repository foo-1234. | `delete` | ✓ `delete`; ✓ `delete` |
