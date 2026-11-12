//! Lex and parse arbitrary input as Cypher. Any panic or stack overflow is a bug:
//! the parser must answer every query with `Ok` or `Err`.
#![no_main]

use libfuzzer_sys::fuzz_target;
use myceliums_cypher::{lexer::lex, parser::parse_cypher};

fuzz_target!(|data: &[u8]| {
    let Ok(input) = std::str::from_utf8(data) else {
        return;
    };
    for (_, span) in lex(input) {
        assert!(span.start < span.end && span.end <= input.len());
        assert!(input.is_char_boundary(span.start) && input.is_char_boundary(span.end));
    }
    let _ = parse_cypher(input);
});
