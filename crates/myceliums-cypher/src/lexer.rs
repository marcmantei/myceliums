use logos::Logos;

#[derive(Logos, Debug, PartialEq, Clone)]
#[logos(skip r"[ \t\r\n]+")]
pub enum Token {
    // Keywords
    #[token("MATCH", ignore(ascii_case))]
    Match,
    #[token("RETURN", ignore(ascii_case))]
    Return,
    #[token("WHERE", ignore(ascii_case))]
    Where,
    #[token("WITH", ignore(ascii_case))]
    With,
    #[token("ORDER", ignore(ascii_case))]
    Order,
    #[token("BY", ignore(ascii_case))]
    By,
    #[token("LIMIT", ignore(ascii_case))]
    Limit,
    #[token("SKIP", ignore(ascii_case))]
    Skip,
    #[token("AS", ignore(ascii_case))]
    As,
    #[token("AND", ignore(ascii_case))]
    And,
    #[token("OR", ignore(ascii_case))]
    Or,
    #[token("NOT", ignore(ascii_case))]
    Not,
    #[token("IN", ignore(ascii_case))]
    In,
    #[token("CONTAINS", ignore(ascii_case))]
    Contains,
    #[token("IS", ignore(ascii_case))]
    Is,
    #[token("NULL", ignore(ascii_case))]
    Null,
    #[token("TRUE", ignore(ascii_case))]
    True,
    #[token("FALSE", ignore(ascii_case))]
    False,
    #[token("ASC", ignore(ascii_case))]
    Asc,
    #[token("DESC", ignore(ascii_case))]
    Desc,
    #[token("DISTINCT", ignore(ascii_case))]
    Distinct,

    // Blocked keywords (will cause parse errors)
    #[token("CREATE", ignore(ascii_case))]
    Create,
    #[token("DELETE", ignore(ascii_case))]
    Delete,
    #[token("SET", ignore(ascii_case))]
    Set,
    #[token("MERGE", ignore(ascii_case))]
    Merge,
    #[token("DROP", ignore(ascii_case))]
    Drop,
    #[token("ALTER", ignore(ascii_case))]
    Alter,

    // Symbols
    #[token("(")]
    LParen,
    #[token(")")]
    RParen,
    #[token("[")]
    LBracket,
    #[token("]")]
    RBracket,
    #[token("{")]
    LBrace,
    #[token("}")]
    RBrace,
    #[token(":")]
    Colon,
    #[token(",")]
    Comma,
    #[token(".")]
    Dot,
    #[token("->")]
    Arrow,
    #[token("<-")]
    LeftArrow,
    #[token("-")]
    Dash,
    #[token("=")]
    Eq,
    #[token("<>")]
    Neq,
    #[token("!=")]
    NotEq,
    #[token("<")]
    Lt,
    #[token(">")]
    Gt,
    #[token("<=")]
    Lte,
    #[token(">=")]
    Gte,
    #[token("*")]
    Star,

    // Literals
    #[regex(r"[a-zA-Z_][a-zA-Z0-9_]*", |lex| lex.slice().to_string())]
    Ident(String),

    #[regex(r"'[^']*'", |lex| {
        let s = lex.slice();
        s[1..s.len()-1].to_string()
    })]
    StringLit(String),

    #[regex(r#""[^"]*""#, |lex| {
        let s = lex.slice();
        s[1..s.len()-1].to_string()
    })]
    QuotedString(String),

    #[regex(r"[0-9]+", |lex| lex.slice().parse::<i64>().ok())]
    IntLit(i64),

    #[regex(r"[0-9]+\.[0-9]+", |lex| lex.slice().parse::<f64>().ok())]
    FloatLit(f64),
}

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
