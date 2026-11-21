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
///
/// Lenient: text the lexer cannot tokenize is dropped. Use [`tokenize`] to
/// get an error for it instead; [`crate::parser::parse_cypher`] does.
pub fn lex(input: &str) -> Vec<(Token, std::ops::Range<usize>)> {
    let lexer = Token::lexer(input);
    lexer
        .spanned()
        .filter_map(|(tok, span)| tok.ok().map(|t| (t, span)))
        .collect()
}

/// Input the lexer cannot tokenize: an unterminated string literal, a
/// character outside the supported syntax, or an integer literal too large
/// for an `i64`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LexError {
    /// What is wrong, e.g. "unterminated string literal".
    pub message: String,
    /// 1-based character position (not byte offset) in the query where the
    /// offending text starts.
    pub position: usize,
}

impl std::fmt::Display for LexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} at position {}", self.message, self.position)
    }
}

impl std::error::Error for LexError {}

/// [`lex`], plus the first piece of input it could not tokenize.
///
/// Every token the lexer recognises is returned, including those after the
/// error, so a caller can still inspect the whole query (the parser rejects
/// write keywords before it reports a lexing error). A single `;` at the end
/// of the query is accepted and ignored.
pub fn tokenize(input: &str) -> (Vec<(Token, std::ops::Range<usize>)>, Option<LexError>) {
    let mut tokens = Vec::new();
    let mut error = None;
    for (tok, span) in Token::lexer(input).spanned() {
        match tok {
            Ok(t) => tokens.push((t, span)),
            Err(()) if error.is_none() => error = lex_error(input, span),
            Err(()) => {}
        }
    }
    (tokens, error)
}

/// The error for the untokenizable `span` of `input`; `None` for a trailing
/// `;`.
fn lex_error(input: &str, span: std::ops::Range<usize>) -> Option<LexError> {
    let rest = &input[span.start..];
    let position = input[..span.start].chars().count() + 1;
    let first = rest.chars().next()?;
    let message = match first {
        '\'' | '"' => format!("unterminated string literal (no closing {first})"),
        ';' if rest[1..].trim().is_empty() => return None,
        c if c.is_ascii_digit() => {
            let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
            format!("integer literal {digits} is out of range")
        }
        c => format!("unexpected character `{c}`"),
    };
    Some(LexError { message, position })
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

    /// #118: text after an unterminated quote was dropped silently; now it
    /// is an error that says where the literal starts.
    #[test]
    fn unterminated_string_literal_is_an_error_with_its_position() {
        let (_, error) = tokenize("MATCH (s) WHERE s.name = 'handler RETURN s");
        let error = error.expect("an unterminated quote is an error");
        assert_eq!(error.position, 26);
        assert!(
            error.message.contains("unterminated string literal"),
            "{error}"
        );
        assert_eq!(
            error.to_string(),
            "unterminated string literal (no closing ') at position 26"
        );

        let (_, error) = tokenize("MATCH (s) WHERE s.name = \"x");
        assert_eq!(error.expect("double quotes too").position, 26);
    }

    /// Positions count characters, not bytes.
    #[test]
    fn lex_error_positions_count_characters() {
        let (_, error) = tokenize("RETURN '日本' § 1");
        let error = error.expect("§ is not Cypher");
        assert_eq!(error.position, 13);
        assert_eq!(error.message, "unexpected character `§`");
    }

    #[test]
    fn integer_overflow_is_a_lex_error() {
        let (_, error) = tokenize("MATCH (s) RETURN s LIMIT 99999999999999999999");
        assert_eq!(
            error.expect("too large for i64").message,
            "integer literal 99999999999999999999 is out of range"
        );
    }

    #[test]
    fn a_trailing_semicolon_is_accepted() {
        let (tokens, error) = tokenize("MATCH (s) RETURN s;  \n");
        assert_eq!(error, None);
        assert_eq!(tokens.len(), 6);
        let (_, error) = tokenize("MATCH (s); RETURN s");
        assert_eq!(error.expect("not at the end").position, 10);
    }

    #[test]
    fn test_blocked_keywords() {
        let tokens = lex("CREATE DELETE SET");
        assert!(tokens.iter().any(|(t, _)| *t == Token::Create));
        assert!(tokens.iter().any(|(t, _)| *t == Token::Delete));
        assert!(tokens.iter().any(|(t, _)| *t == Token::Set));
    }
}
