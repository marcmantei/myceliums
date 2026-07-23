//! Code-aware tokenization for lexical search.
//!
//! Splits text into lowercase tokens on word boundaries **and** on the internal
//! boundaries of programming identifiers (`snake_case`, `camelCase`,
//! `PascalCase`, `SCREAMING_SNAKE`, and digit runs). This lets a query like
//! `"user name"` match the identifier `get_user_name`, while keeping tokens
//! whole so `"cat"` no longer matches `concatenate`.
//!
//! The tokenizer is deliberately dependency-free and deterministic: the same
//! input always yields the same token sequence.

/// Split `text` into lowercase tokens.
///
/// A token is a maximal run of ASCII/Unicode alphanumerics. Identifier-internal
/// case and digit boundaries additionally split a run into sub-tokens, so
/// `get_user_name` yields `["get", "user", "name"]` and `parseHTTPResponse2`
/// yields `["parse", "http", "response", "2"]`.
///
/// Returns tokens in order of appearance. Empty input yields an empty vector.
pub fn tokenize(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut word = String::new();

    for ch in text.chars() {
        if ch.is_alphanumeric() {
            word.push(ch);
        } else if !word.is_empty() {
            split_identifier(&word, &mut tokens);
            word.clear();
        }
    }
    if !word.is_empty() {
        split_identifier(&word, &mut tokens);
    }
    tokens
}

/// Split a single word run on `camelCase`, `PascalCase`, and alpha/digit
/// boundaries, pushing lowercased sub-tokens onto `out`.
///
/// Boundaries recognised (between adjacent chars `a` then `b`):
/// - lower→upper: `userName`  -> `user`, `name`
/// - upper→upper→lower: `HTTPServer` -> `http`, `server`
/// - letter↔digit: `utf8`     -> `utf`, `8`; `2fa` -> `2`, `fa`
fn split_identifier(word: &str, out: &mut Vec<String>) {
    let chars: Vec<char> = word.chars().collect();
    let mut start = 0;

    for i in 1..chars.len() {
        let prev = chars[i - 1];
        let cur = chars[i];
        let next = chars.get(i + 1).copied();

        let boundary = is_boundary(prev, cur, next);
        if boundary {
            out.push(chars[start..i].iter().collect::<String>().to_lowercase());
            start = i;
        }
    }
    out.push(chars[start..].iter().collect::<String>().to_lowercase());
}

/// Whether a split should occur between `prev` and `cur` (with `next` for the
/// acronym-boundary lookahead).
fn is_boundary(prev: char, cur: char, next: Option<char>) -> bool {
    let prev_digit = prev.is_numeric();
    let cur_digit = cur.is_numeric();

    // Letter <-> digit transitions are always boundaries: utf8 -> utf, 8.
    if prev_digit != cur_digit {
        return true;
    }
    // Within digits, no further splitting.
    if cur_digit {
        return false;
    }

    let prev_upper = prev.is_uppercase();
    let cur_upper = cur.is_uppercase();

    // lower -> Upper: userName -> user | Name
    if !prev_upper && cur_upper {
        return true;
    }
    // Upper -> Upper -> lower: HTTPServer -> HTTP | Server. Split before the
    // last uppercase that begins a new lowercase word.
    if prev_upper && cur_upper && matches!(next, Some(n) if n.is_lowercase()) {
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(s: &str) -> Vec<String> {
        tokenize(s)
    }

    #[test]
    fn plain_words_lowercased() {
        assert_eq!(toks("Hello World"), vec!["hello", "world"]);
    }

    #[test]
    fn snake_case_splits() {
        assert_eq!(toks("get_user_name"), vec!["get", "user", "name"]);
    }

    #[test]
    fn screaming_snake_splits() {
        assert_eq!(toks("MAX_RETRY_COUNT"), vec!["max", "retry", "count"]);
    }

    #[test]
    fn camel_case_splits() {
        assert_eq!(toks("getUserName"), vec!["get", "user", "name"]);
    }

    #[test]
    fn pascal_case_splits() {
        assert_eq!(toks("HttpResponse"), vec!["http", "response"]);
    }

    #[test]
    fn acronym_boundary() {
        assert_eq!(toks("parseHTTPResponse"), vec!["parse", "http", "response"]);
    }

    #[test]
    fn digit_boundaries() {
        assert_eq!(toks("utf8"), vec!["utf", "8"]);
        assert_eq!(toks("2fa"), vec!["2", "fa"]);
        assert_eq!(toks("sha256sum"), vec!["sha", "256", "sum"]);
    }

    #[test]
    fn punctuation_separates() {
        assert_eq!(toks("a.b(c, d)"), vec!["a", "b", "c", "d"]);
    }

    #[test]
    fn empty_input() {
        assert!(toks("").is_empty());
        assert!(toks("   ").is_empty());
    }

    #[test]
    fn cat_does_not_split_concatenate() {
        // The whole word is one token; a "cat" query token cannot match it.
        assert_eq!(toks("concatenate"), vec!["concatenate"]);
    }

    #[test]
    fn unicode_word() {
        assert_eq!(toks("naïveimpl"), vec!["naïveimpl"]);
    }
}
