//! The "Docs mentioning" section of `impact_of_change` and `find_callers`
//! answers (#180): which docs and doc comments name the symbols, so an agent
//! changing them updates those too.
//!
//! The section is kept small: the count, the first [`SHOWN`] docs grouped by
//! file, a `Next (docs):` call that lists them all, and the mentions of the
//! name the index left unlinked because several symbols carry it. It is
//! appended after the answer's own `Incomplete`/`Next` block, whose `Next:`
//! stays the walk's continuation.

use myceliums_core::doc_mentions::{self, DocHit, DocMentionStats};
use myceliums_storage::{CodeSymbol, Relationship, Store, SymbolKind};

/// Docs listed before the call that lists them all.
pub(crate) const SHOWN: usize = 5;

/// What the section is about.
#[derive(Clone, Copy)]
pub(crate) enum Subject<'a> {
    /// One symbol (and, for a type, its members).
    Symbol(&'a CodeSymbol),
    /// Several symbols: a file's, or a diff's.
    Several(&'a str),
}

/// The section for `roots`, read from the index; empty for an index built
/// before #180 or for a repository whose docs name no symbol.
pub(crate) async fn section(
    store: &Store,
    symbols: &[CodeSymbol],
    relationships: &[Relationship],
    roots: &[&CodeSymbol],
    subject: Subject<'_>,
    repo_id: &str,
) -> String {
    let stats = match DocMentionStats::load(store).await {
        Ok(stats) => stats,
        Err(e) => {
            tracing::warn!("Failed to load doc mention statistics: {e}");
            None
        }
    };
    let targets = doc_mentions::mention_targets(symbols, roots);
    let hits = doc_mentions::docs_mentioning(symbols, relationships, &targets);
    render(&hits, stats.as_ref(), subject, repo_id)
}

/// The section's text (see the module docs).
pub(crate) fn render(
    hits: &[DocHit<'_>],
    stats: Option<&DocMentionStats>,
    subject: Subject<'_>,
    repo_id: &str,
) -> String {
    let Some(stats) = stats else {
        return String::new();
    };
    let ambiguous = match subject {
        Subject::Symbol(s) => stats.ambiguous_named(&s.name).map(|a| (s.name.as_str(), a)),
        Subject::Several(_) => None,
    };
    let label = match subject {
        Subject::Symbol(s) => s.qualified_name.as_str(),
        Subject::Several(label) => label,
    };
    // Nothing linked says nothing: no line, unless unlinked mentions follow.
    let mut out = String::new();
    if !hits.is_empty() {
        let comments = hits.iter().filter(|h| h.in_comment()).count();
        out.push_str(&format!(
            "\nDocs mentioning {label} ({}: {} in docs, {comments} in doc comments):\n",
            hits.len(),
            hits.len() - comments
        ));
        out.push_str(&crate::format::group_by_file(
            hits.iter()
                .take(SHOWN)
                .map(|h| (h.source.file_path.as_str(), hit_line(h, subject))),
        ));
        if hits.len() > SHOWN {
            let next = match subject {
                Subject::Symbol(s) => format!(
                    "query_knowledge(repo_id={repo_id:?}, query={:?}) lists every mention",
                    crate::repo::located_name(s),
                ),
                Subject::Several(_) => format!(
                    "query_knowledge(repo_id={repo_id:?}, query=\"<file>:<symbol>\") lists one symbol's"
                ),
            };
            out.push_str(&format!("Next (docs): {next}\n"));
        }
    } else if ambiguous.is_some() {
        out.push('\n');
    }
    if let Some((name, a)) = ambiguous {
        out.push_str(&format!(
            "Not linked: {} doc mention(s) of `{name}` that several symbols could be, e.g. {}\n",
            a.count,
            a.at.join(", ")
        ));
    }
    out
}

/// `:line where [symbol]`: the line of the first mention, the section or
/// documented symbol, and the symbol named when it is not the subject.
fn hit_line(hit: &DocHit<'_>, subject: Subject<'_>) -> String {
    let place = match hit.source.kind {
        SymbolKind::Section => format!("section \"{}\"", hit.source.name),
        SymbolKind::Document => "document".to_string(),
        _ => format!("doc comment of {}", hit.source.qualified_name),
    };
    let mut line = format!(":{} {place}", hit.line);
    let is_subject = matches!(subject, Subject::Symbol(s) if s.uid == hit.target.uid);
    if !is_subject {
        line.push_str(&format!(" [{}]", hit.target.qualified_name));
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;
    use myceliums_core::doc_mentions::AmbiguousName;

    fn sym(uid: &str, file: &str, qualified: &str, kind: SymbolKind, line: u32) -> CodeSymbol {
        CodeSymbol {
            uid: uid.to_string(),
            name: qualified.rsplit('.').next().unwrap().to_string(),
            qualified_name: qualified.to_string(),
            kind,
            file_path: file.to_string(),
            start_line: line,
            end_line: line + 5,
            signature: String::new(),
            content: String::new(),
            repo_id: "r".to_string(),
            metadata: None,
        }
    }

    #[test]
    fn the_section_lists_a_few_docs_and_the_call_for_the_rest() {
        let store = sym("s", "src/Store.java", "Store", SymbolKind::Class, 1);
        let open = sym("o", "src/Store.java", "Store.open", SymbolKind::Method, 9);
        let sections: Vec<CodeSymbol> = (0..6)
            .map(|i| {
                sym(
                    &format!("d{i}"),
                    "docs/store.adoc",
                    &format!("Part {i}"),
                    SymbolKind::Section,
                    i * 10 + 1,
                )
            })
            .collect();
        let user = sym("u", "src/User.java", "User.run", SymbolKind::Method, 3);
        let mut hits: Vec<DocHit> = sections
            .iter()
            .map(|s| DocHit {
                source: s,
                target: &store,
                line: s.start_line + 2,
            })
            .collect();
        hits.push(DocHit {
            source: &user,
            target: &open,
            line: 2,
        });
        let mut stats = DocMentionStats {
            linked: 9,
            ..Default::default()
        };
        stats.ambiguous_by_name.insert(
            "Store".to_string(),
            AmbiguousName {
                count: 2,
                at: vec!["docs/other.md:4".to_string()],
            },
        );
        let text = render(&hits, Some(&stats), Subject::Symbol(&store), "repo");
        assert_eq!(
            text,
            "\nDocs mentioning Store (7: 6 in docs, 1 in doc comments):\n\
             docs/store.adoc\n  :3 section \"Part 0\"\n  :13 section \"Part 1\"\n  :23 section \"Part 2\"\n  :33 section \"Part 3\"\n  :43 section \"Part 4\"\n\
             Next (docs): query_knowledge(repo_id=\"repo\", query=\"src/Store.java:Store\") lists every mention\n\
             Not linked: 2 doc mention(s) of `Store` that several symbols could be, e.g. docs/other.md:4\n"
        );
        // Only doc comments beyond the first five: the member is named.
        let text = render(&hits[5..], Some(&stats), Subject::Symbol(&store), "repo");
        assert!(
            text.contains("src/User.java\n  :2 doc comment of User.run [Store.open]\n"),
            "{text}"
        );
    }

    #[test]
    fn an_index_without_doc_mentions_adds_nothing() {
        let store = sym("s", "src/Store.java", "Store", SymbolKind::Class, 1);
        assert_eq!(render(&[], None, Subject::Symbol(&store), "r"), "");
        let none = DocMentionStats::default();
        assert_eq!(render(&[], Some(&none), Subject::Symbol(&store), "r"), "");
        let some = DocMentionStats {
            linked: 3,
            ..Default::default()
        };
        // Nothing linked: no line at all.
        assert_eq!(
            render(&[], Some(&some), Subject::Several("src/Store.java"), "r"),
            ""
        );
    }
}
