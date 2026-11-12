//! Char-boundary-safe string shortening for display output.
//!
//! Slicing a `&str` by byte index (`&s[..n]`) panics when `n` falls inside a
//! multi-byte character, and symbol names, comments and email bodies are full
//! of umlauts and emoji. Every place that shortens text for output goes
//! through the helpers here, which count **characters**, never bytes.

/// The longest prefix of `s` that holds at most `max_chars` characters.
///
/// Returns `s` unchanged when it is already short enough. Never splits a
/// character, so the result is always valid UTF-8.
pub fn prefix_chars(s: &str, max_chars: usize) -> &str {
    if s.len() <= max_chars {
        // Byte length within the limit implies char length within it too.
        return s;
    }
    match s.char_indices().nth(max_chars) {
        Some((byte_idx, _)) => &s[..byte_idx],
        None => s,
    }
}

/// Shortens `s` to at most `max_chars` characters, marking a cut with `...`.
///
/// When `s` is longer than `max_chars`, the result is the first
/// `max_chars - 3` characters followed by `...`, so it is exactly
/// `max_chars` characters long. If `max_chars` is 3 or less there is no room
/// for the marker and the text is cut hard.
pub fn truncate_ellipsis(s: &str, max_chars: usize) -> String {
    let kept = prefix_chars(s, max_chars);
    if kept.len() == s.len() {
        return s.to_string();
    }
    if max_chars > 3 {
        format!("{}...", prefix_chars(s, max_chars - 3))
    } else {
        kept.to_string()
    }
}

/// Keeps the first `max_chars` characters of `s` and appends `...` when
/// anything was cut. Unlike [`truncate_ellipsis`], the marker is added on top
/// of the budget, which suits body snippets ("first 200 characters").
pub fn snippet(s: &str, max_chars: usize) -> String {
    let kept = prefix_chars(s, max_chars);
    if kept.len() == s.len() {
        s.to_string()
    } else {
        format!("{kept}...")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_chars_counts_characters_not_bytes() {
        assert_eq!(prefix_chars("hello", 3), "hel");
        assert_eq!(prefix_chars("hello", 10), "hello");
        assert_eq!(prefix_chars("", 3), "");
        assert_eq!(prefix_chars("abc", 0), "");
        // 'ü' and 'ö' are two bytes each; a byte slice at 2 would panic.
        assert_eq!(prefix_chars("Grüße", 3), "Grü");
        assert_eq!(prefix_chars("öäü", 2), "öä");
        // 4-byte emoji.
        assert_eq!(prefix_chars("🦀🦀🦀", 2), "🦀🦀");
        assert_eq!(prefix_chars("a🦀b", 2), "a🦀");
    }

    #[test]
    fn truncate_ellipsis_fits_budget_in_characters() {
        assert_eq!(truncate_ellipsis("short", 10), "short");
        assert_eq!(truncate_ellipsis("hello world", 8), "hello...");
        assert_eq!(truncate_ellipsis("hello", 5), "hello");
        assert_eq!(truncate_ellipsis("hello", 2), "he");
        let cut = truncate_ellipsis("Übergrößenträger", 8);
        assert_eq!(cut, "Überg...");
        assert_eq!(cut.chars().count(), 8);
        let emoji = truncate_ellipsis("🦀🦀🦀🦀🦀🦀🦀🦀🦀🦀", 6);
        assert_eq!(emoji, "🦀🦀🦀...");
        // A string whose byte length exceeds the budget but whose char
        // length does not stays whole.
        assert_eq!(truncate_ellipsis("ääää", 4), "ääää");
    }

    #[test]
    fn snippet_appends_marker_only_when_cut() {
        assert_eq!(snippet("abc", 3), "abc");
        assert_eq!(snippet("abcd", 3), "abc...");
        assert_eq!(snippet("Grüße aus Köln", 5), "Grüße...");
        assert_eq!(snippet("😀😀😀", 1), "😀...");
    }

    #[test]
    fn no_panic_on_any_cut_point() {
        let s = "aä€😀b";
        for n in 0..=s.chars().count() + 1 {
            let _ = prefix_chars(s, n);
            let _ = truncate_ellipsis(s, n);
            let _ = snippet(s, n);
        }
    }
}
