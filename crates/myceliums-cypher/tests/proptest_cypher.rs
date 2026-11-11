//! Property tests for the Cypher lexer, parser, and executor (#91).
//!
//! Queries reach the engine straight from agents (the `cypher_query` MCP
//! tool), so the whole pipeline must turn any input into either rows or an
//! error — never a panic or a stack overflow. Alongside that, a few
//! invariants are checked on generated well-formed queries: clause values
//! round-trip into the AST, keyword case does not matter, write clauses are
//! always rejected, and `LIMIT`/`SKIP` bound the result.
//!
//! Case counts are kept modest so `cargo test` stays fast; the cargo-fuzz
//! targets in `fuzz/` explore the same surface for much longer.

mod support;

use myceliums_cypher::lexer::{lex, Token};
use myceliums_cypher::parser::{parse_cypher, ParseError, MAX_NESTING_DEPTH};
use proptest::prelude::*;
use support::small_graph::small_graph_executor;

/// Fragments of the supported Cypher subset plus a few hostile extras.
/// Random sequences of these form near-valid queries that reach every parser
/// branch, unlike uniformly random strings.
const CYPHER_FRAGMENTS: &[&str] = &[
    "MATCH ",
    "RETURN ",
    "WHERE ",
    "WITH ",
    "ORDER BY ",
    "LIMIT ",
    "SKIP ",
    "AS ",
    "AND ",
    "OR ",
    "NOT ",
    "IN ",
    "CONTAINS ",
    "IS ",
    "NULL ",
    "TRUE ",
    "FALSE ",
    "ASC ",
    "DESC ",
    "DISTINCT ",
    "(",
    ")",
    "[",
    "]",
    "{",
    "}",
    ":",
    ",",
    ".",
    "->",
    "<-",
    "-",
    "=",
    "<>",
    "!=",
    "<",
    ">",
    "<=",
    ">=",
    "*",
    "..",
    "n",
    "m",
    "p",
    "path",
    "name",
    "kind",
    "file_path",
    "start_line",
    "metadata",
    "CodeSymbol",
    "File",
    "Function",
    "Class",
    "CALLS",
    "IMPORTS",
    "count",
    "sum",
    "avg",
    "min",
    "max",
    "collect",
    "starts_with",
    "ends_with",
    "shortestPath",
    "allPaths",
    "anyPath",
    "'main'",
    "'x'",
    "\"leaf\"",
    "''",
    "0",
    "1",
    "3",
    "10",
    "99999999999999999999",
    "1.5",
    " ",
    "\n",
    "'unterminated",
    "§",
    "日本",
];

fn cypher_soup() -> impl Strategy<Value = String> {
    prop::collection::vec(prop::sample::select(CYPHER_FRAGMENTS), 0..40).prop_map(|f| f.concat())
}

/// A well-formed `MATCH … RETURN …` query plus the SKIP/LIMIT it carries.
fn well_formed_query() -> impl Strategy<Value = (String, Option<i64>, Option<i64>)> {
    let label = prop::sample::select(vec!["", ":CodeSymbol"]);
    let prop_name = prop::sample::select(vec!["name", "kind", "file_path", "start_line"]);
    let op = prop::sample::select(vec!["=", "<>", "<", ">", "<=", ">=", "CONTAINS"]);
    let literal = prop_oneof![
        "[a-z]{0,6}".prop_map(|s| format!("'{s}'")),
        (0i64..100).prop_map(|n| n.to_string()),
    ];
    let order = prop::sample::select(vec!["", " ORDER BY n.name", " ORDER BY n.start_line DESC"]);
    (
        label,
        prop::option::of((prop_name.clone(), op, literal)),
        prop_name,
        order,
        prop::option::of(0i64..20),
        prop::option::of(0i64..20),
    )
        .prop_map(|(label, filter, ret, order, skip, limit)| {
            let mut q = format!("MATCH (n{label})");
            if let Some((p, op, lit)) = filter {
                q.push_str(&format!(" WHERE n.{p} {op} {lit}"));
            }
            q.push_str(&format!(" RETURN n.{ret}{order}"));
            if let Some(s) = skip {
                q.push_str(&format!(" SKIP {s}"));
            }
            if let Some(l) = limit {
                q.push_str(&format!(" LIMIT {l}"));
            }
            (q, skip, limit)
        })
}

/// Randomise the ASCII case of every character.
fn scramble_case(s: &str, mask: &[bool]) -> String {
    s.chars()
        .zip(mask.iter().cycle())
        .map(|(c, upper)| {
            if *upper {
                c.to_ascii_uppercase()
            } else {
                c.to_ascii_lowercase()
            }
        })
        .collect()
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 512,
        failure_persistence: None,
        .. ProptestConfig::default()
    })]

    /// The parser returns `Ok` or `Err` for any Unicode string.
    #[test]
    fn parser_never_panics_on_arbitrary_strings(input in any::<String>()) {
        let _ = parse_cypher(&input);
    }

    /// … and for near-valid token soup, which reaches deep parser branches.
    #[test]
    fn parser_never_panics_on_cypher_soup(input in cypher_soup()) {
        let _ = parse_cypher(&input);
    }

    /// Lexer spans are in bounds, on char boundaries, ordered, and disjoint.
    #[test]
    fn lexer_spans_are_well_formed(input in prop_oneof![any::<String>(), cypher_soup()]) {
        let mut prev_end = 0;
        for (_, span) in lex(&input) {
            prop_assert!(span.start < span.end);
            prop_assert!(span.start >= prev_end);
            prop_assert!(span.end <= input.len());
            prop_assert!(input.is_char_boundary(span.start));
            prop_assert!(input.is_char_boundary(span.end));
            prev_end = span.end;
        }
    }

    /// Parsing is a pure function of its input.
    #[test]
    fn parser_is_deterministic(input in cypher_soup()) {
        let a = format!("{:?}", parse_cypher(&input));
        let b = format!("{:?}", parse_cypher(&input));
        prop_assert_eq!(a, b);
    }

    /// The executor turns any query into rows or an error — never a panic.
    #[test]
    fn executor_never_panics_on_cypher_soup(input in cypher_soup()) {
        let _ = small_graph_executor().execute(&input);
    }
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 256,
        failure_persistence: None,
        .. ProptestConfig::default()
    })]

    /// A write keyword that the lexer recognises as a token is always
    /// rejected, whatever surrounds it and however it is cased. (Text the
    /// lexer cannot tokenise — e.g. everything after an unterminated quote —
    /// is dropped, so a keyword inside it never becomes a token at all.)
    #[test]
    fn write_clauses_are_always_blocked(
        prefix in cypher_soup(),
        suffix in cypher_soup(),
        keyword in prop::sample::select(vec!["CREATE", "DELETE", "SET", "MERGE", "DROP", "ALTER"]),
        mask in prop::collection::vec(any::<bool>(), 1..8),
    ) {
        let input = format!("{prefix} {} {suffix}", scramble_case(keyword, &mask));
        let has_write_token = lex(&input).iter().any(|(t, _)| {
            matches!(t, Token::Create | Token::Delete | Token::Set | Token::Merge | Token::Drop | Token::Alter)
        });
        // Without quotes in the prefix the keyword is always tokenised.
        if !prefix.contains(['\'', '"']) {
            prop_assert!(has_write_token, "{input:?} lost its write keyword");
        }
        if has_write_token {
            prop_assert!(
                matches!(parse_cypher(&input), Err(ParseError::BlockedOperation(_))),
                "{input:?} was not blocked"
            );
            prop_assert!(small_graph_executor().execute(&input).is_err());
        }
    }

    /// Well-formed queries parse, SKIP/LIMIT round-trip into the AST, and the
    /// executor honours LIMIT.
    #[test]
    fn well_formed_queries_round_trip_and_respect_limit(
        (query, skip, limit) in well_formed_query(),
    ) {
        let ast = parse_cypher(&query);
        prop_assert!(ast.is_ok(), "{query:?} failed: {ast:?}");
        let ast = ast.unwrap();
        prop_assert!(ast.match_clause.is_some());
        prop_assert_eq!(ast.skip, skip);
        prop_assert_eq!(ast.limit, limit);
        prop_assert_eq!(ast.return_clause.items.len(), 1);

        let rows = small_graph_executor().execute(&query);
        prop_assert!(rows.is_ok(), "{query:?} failed: {rows:?}");
        let rows = rows.unwrap();
        if let Some(l) = limit {
            prop_assert!(rows.len() as i64 <= l);
        }
        // SKIP and LIMIT act on the full result: |rows| = min(|all| - skip, limit).
        let mut unbounded = query.clone();
        if let Some(s) = skip {
            unbounded = unbounded.replace(&format!(" SKIP {s}"), "");
        }
        if let Some(l) = limit {
            unbounded = unbounded.replace(&format!(" LIMIT {l}"), "");
        }
        let all = small_graph_executor().execute(&unbounded).unwrap().len();
        let expected = all.saturating_sub(skip.unwrap_or(0) as usize);
        let expected = limit.map_or(expected, |l| expected.min(l as usize));
        prop_assert_eq!(rows.len(), expected);
    }

    /// Keywords are case-insensitive: re-casing them yields the same AST.
    #[test]
    fn keyword_case_does_not_change_the_ast(
        (query, _, _) in well_formed_query(),
        mask in prop::collection::vec(any::<bool>(), 1..8),
    ) {
        const KEYWORDS: &[&str] = &[
            "MATCH", "WHERE", "RETURN", "ORDER", "BY", "DESC", "SKIP", "LIMIT", "CONTAINS",
        ];
        let recased: String = query
            .split(' ')
            .map(|w| if KEYWORDS.contains(&w) { scramble_case(w, &mask) } else { w.to_string() })
            .collect::<Vec<_>>()
            .join(" ");
        prop_assert_eq!(
            format!("{:?}", parse_cypher(&query)),
            format!("{:?}", parse_cypher(&recased))
        );
    }

    /// Any nesting depth fails gracefully: within the limit it parses, beyond
    /// it the parser reports `TooDeeplyNested` instead of overflowing.
    #[test]
    fn nesting_depth_is_bounded(
        depth in 0usize..(4 * MAX_NESTING_DEPTH),
        wrapper in prop::sample::select(vec!["NOT ", "f(", "count(", "collect("]),
    ) {
        let close = if wrapper.ends_with('(') { ")" } else { "" };
        let query = format!(
            "MATCH (n) RETURN {}n.name{}",
            wrapper.repeat(depth),
            close.repeat(depth)
        );
        match parse_cypher(&query) {
            Ok(_) => prop_assert!(depth < MAX_NESTING_DEPTH),
            Err(ParseError::TooDeeplyNested(_)) => prop_assert!(depth >= MAX_NESTING_DEPTH - 1),
            Err(e) => prop_assert!(false, "unexpected error at depth {depth}: {e}"),
        }
        let _ = small_graph_executor().execute(&query);
    }
}
