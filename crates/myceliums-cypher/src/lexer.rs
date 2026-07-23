use logos::Logos;

/// A lexical token in the supported read-only Cypher subset.
#[derive(Logos, Debug, PartialEq, Clone)]
#[logos(skip r"[ \t\r\n]+")]
pub enum Token {
    // Keywords
    /// `MATCH` keyword.
    #[token("MATCH", ignore(ascii_case))]
    Match,
    /// `RETURN` keyword.
    #[token("RETURN", ignore(ascii_case))]
    Return,
    /// `WHERE` keyword.
    #[token("WHERE", ignore(ascii_case))]
    Where,
    /// `WITH` keyword.
    #[token("WITH", ignore(ascii_case))]
    With,
    /// `ORDER` keyword (part of `ORDER BY`).
    #[token("ORDER", ignore(ascii_case))]
    Order,
    /// `BY` keyword (part of `ORDER BY`).
    #[token("BY", ignore(ascii_case))]
    By,
    /// `LIMIT` keyword.
    #[token("LIMIT", ignore(ascii_case))]
    Limit,
    /// `SKIP` keyword.
    #[token("SKIP", ignore(ascii_case))]
    Skip,
    /// `AS` keyword (column aliasing).
    #[token("AS", ignore(ascii_case))]
    As,
    /// `AND` logical operator.
    #[token("AND", ignore(ascii_case))]
    And,
    /// `OR` logical operator.
    #[token("OR", ignore(ascii_case))]
    Or,
    /// `NOT` logical operator.
    #[token("NOT", ignore(ascii_case))]
    Not,
    /// `IN` membership operator.
    #[token("IN", ignore(ascii_case))]
    In,
    /// `CONTAINS` substring operator.
    #[token("CONTAINS", ignore(ascii_case))]
    Contains,
    /// `IS` keyword (used in `IS NULL` / `IS NOT NULL`).
    #[token("IS", ignore(ascii_case))]
    Is,
    /// `NULL` literal.
    #[token("NULL", ignore(ascii_case))]
    Null,
    /// `TRUE` boolean literal.
    #[token("TRUE", ignore(ascii_case))]
    True,
    /// `FALSE` boolean literal.
    #[token("FALSE", ignore(ascii_case))]
    False,
    /// `ASC` sort direction.
    #[token("ASC", ignore(ascii_case))]
    Asc,
    /// `DESC` sort direction.
    #[token("DESC", ignore(ascii_case))]
    Desc,
    /// `DISTINCT` keyword.
    #[token("DISTINCT", ignore(ascii_case))]
    Distinct,

    // Blocked keywords (will cause parse errors)
    /// `CREATE` — a blocked write keyword; parsing rejects it.
    #[token("CREATE", ignore(ascii_case))]
    Create,
    /// `DELETE` — a blocked write keyword; parsing rejects it.
    #[token("DELETE", ignore(ascii_case))]
    Delete,
    /// `SET` — a blocked write keyword; parsing rejects it.
    #[token("SET", ignore(ascii_case))]
    Set,
    /// `MERGE` — a blocked write keyword; parsing rejects it.
    #[token("MERGE", ignore(ascii_case))]
    Merge,
    /// `DROP` — a blocked write keyword; parsing rejects it.
    #[token("DROP", ignore(ascii_case))]
    Drop,
    /// `ALTER` — a blocked write keyword; parsing rejects it.
    #[token("ALTER", ignore(ascii_case))]
    Alter,

    // Symbols
    /// Left parenthesis `(`.
    #[token("(")]
    LParen,
    /// Right parenthesis `)`.
    #[token(")")]
    RParen,
    /// Left bracket `[`.
    #[token("[")]
    LBracket,
    /// Right bracket `]`.
    #[token("]")]
    RBracket,
    /// Left brace `{`.
    #[token("{")]
    LBrace,
    /// Right brace `}`.
    #[token("}")]
    RBrace,
    /// Colon `:`.
    #[token(":")]
    Colon,
    /// Comma `,`.
    #[token(",")]
    Comma,
    /// Dot `.` (property access).
    #[token(".")]
    Dot,
    /// Right arrow `->`.
    #[token("->")]
    Arrow,
    /// Left arrow `<-`.
    #[token("<-")]
    LeftArrow,
    /// Dash `-` (relationship edge).
    #[token("-")]
    Dash,
    /// Equality `=`.
    #[token("=")]
    Eq,
    /// Inequality `<>`.
    #[token("<>")]
    Neq,
    /// Inequality `!=`.
    #[token("!=")]
    NotEq,
    /// Less-than `<`.
    #[token("<")]
    Lt,
    /// Greater-than `>`.
    #[token(">")]
    Gt,
    /// Less-than-or-equal `<=`.
    #[token("<=")]
    Lte,
    /// Greater-than-or-equal `>=`.
    #[token(">=")]
    Gte,
    /// Star `*` (used in variable-length paths).
    #[token("*")]
    Star,

    // Literals
    /// An identifier (variable, label, or property name).
    #[regex(r"[a-zA-Z_][a-zA-Z0-9_]*", |lex| lex.slice().to_string())]
    Ident(String),

    /// A single-quoted string literal (quotes stripped).
    #[regex(r"'[^']*'", |lex| {
        let s = lex.slice();
        s[1..s.len()-1].to_string()
    })]
    StringLit(String),

    /// A double-quoted string literal (quotes stripped).
    #[regex(r#""[^"]*""#, |lex| {
        let s = lex.slice();
        s[1..s.len()-1].to_string()
    })]
    QuotedString(String),

    /// An integer literal.
    #[regex(r"[0-9]+", |lex| lex.slice().parse::<i64>().ok())]
    IntLit(i64),

    /// A floating-point literal.
    #[regex(r"[0-9]+\.[0-9]+", |lex| lex.slice().parse::<f64>().ok())]
    FloatLit(f64),
}

/// Tokenizes a Cypher query string into `(token, byte-span)` pairs,
/// discarding whitespace and unrecognized input.
pub fn lex(input: &str) -> Vec<(Token, std::ops::Range<usize>)> {
    let lexer = Token::lexer(input);
    lexer
        .spanned()
        .filter_map(|(tok, span)| tok.ok().map(|t| (t, span)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lex_simple_match() {
        let tokens = lex("MATCH (s:CodeSymbol) RETURN s.name LIMIT 5");
        let kinds: Vec<_> = tokens.iter().map(|(t, _)| t.clone()).collect();
        assert_eq!(kinds[0], Token::Match);
        assert_eq!(kinds[1], Token::LParen);
        assert!(matches!(kinds[2], Token::Ident(_)));
        assert_eq!(kinds[3], Token::Colon);
        assert!(matches!(kinds[4], Token::Ident(_)));
        assert_eq!(kinds[5], Token::RParen);
        assert_eq!(kinds[6], Token::Return);
    }

    #[test]
    fn test_lex_string_literal() {
        let tokens = lex("WHERE s.name = 'handler'");
        assert!(tokens
            .iter()
            .any(|(t, _)| matches!(t, Token::StringLit(s) if s == "handler")));
    }

    #[test]
    fn test_blocked_keywords() {
        let tokens = lex("CREATE DELETE SET");
        assert!(tokens.iter().any(|(t, _)| *t == Token::Create));
        assert!(tokens.iter().any(|(t, _)| *t == Token::Delete));
        assert!(tokens.iter().any(|(t, _)| *t == Token::Set));
    }
}
