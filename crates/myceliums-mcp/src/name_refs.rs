//! The "Referenced by name" section of `find_callers`, `find_callees`,
//! `impact_of_change` and `trace_flow` answers (#179): the strings and
//! config files that name a symbol by its qualified name (`Class.forName`,
//! `spring.factories`, an XML bean's `class=`, a Django setting), which no
//! CALLS edge shows.
//!
//! The section is appended after the answer (and after its own
//! `Incomplete`/`Next` block): a count, the first [`SHOWN`] places grouped
//! by file, and how to list the rest. It is empty when there is nothing to
//! show, so answers without name references stay as they were.

use crate::navigation;
use crate::MyceliumsMcp;
use myceliums_core::name_refs::{self, NameRefHit};
use myceliums_storage::CodeSymbol;

/// Places listed before the line that says how to find the rest.
pub(crate) const SHOWN: usize = 8;

/// Which way the edges point, seen from the subject.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Way {
    /// Places that name the subject.
    Into,
    /// What the subject names.
    OutOf,
}

/// The section for `subjects` (for a type, its members too).
pub(crate) fn section(
    symbols: &[CodeSymbol],
    relationships: &[myceliums_storage::Relationship],
    subjects: &[&CodeSymbol],
    way: Way,
) -> String {
    let hits = name_refs::hits(symbols, relationships, subjects, way == Way::Into);
    render(&hits, subjects, way)
}

/// The section's text (see the module docs).
pub(crate) fn render(hits: &[NameRefHit<'_>], subjects: &[&CodeSymbol], way: Way) -> String {
    if hits.is_empty() {
        return String::new();
    }
    let single = match subjects {
        [one] => Some(one.uid.as_str()),
        _ => None,
    };
    let mut out = match way {
        Way::Into => format!(
            "\nReferenced by name ({}: strings and config files naming it):\n",
            hits.len()
        ),
        Way::OutOf => format!(
            "\nNames by qualified name ({}: strings that name symbols):\n",
            hits.len()
        ),
    };
    let shown = hits.iter().take(SHOWN);
    match way {
        Way::Into => {
            out.push_str(&crate::format::group_by_file(shown.map(|h| {
                let mut line = match h.meta.line {
                    0 => ":0 (file name)".to_string(),
                    n => format!(":{n}"),
                };
                match (h.source, h.meta.line) {
                    (Some(source), 0) => line.push_str(&format!(" in {}", source.qualified_name)),
                    (Some(source), _) => {
                        line.push_str(&format!(" in {}: {}", source.qualified_name, h.meta.text))
                    }
                    (None, 0) => {}
                    (None, _) => line.push_str(&format!(" {}", h.meta.text)),
                }
                if single != Some(h.target.uid.as_str()) {
                    line.push_str(&format!(" → {}", h.target.qualified_name));
                }
                (h.meta.file.as_str(), line)
            })));
        }
        Way::OutOf => {
            out.push_str(&crate::format::group_by_file(shown.map(|h| {
                let mut line = format!(
                    "{} {} {} (named at {}:{}",
                    h.target.qualified_name,
                    h.target.kind,
                    crate::format::line_range(h.target.start_line, h.target.end_line),
                    h.meta.file,
                    h.meta.line
                );
                if let (Some(source), None) = (h.source, single) {
                    line.push_str(&format!(" in {}", source.qualified_name));
                }
                line.push(')');
                (h.target.file_path.as_str(), line)
            })));
        }
    }
    if hits.len() > SHOWN {
        let name = hits[0].meta.name.split(['#', '$']).next().unwrap_or("");
        out.push_str(&format!(
            "{} more not shown. Next (by name): grep -rnF {name:?} lists the places that name {}\n",
            hits.len() - SHOWN,
            hits[0].target.name
        ));
    }
    out
}

impl MyceliumsMcp {
    /// The section for the symbol `name` resolves to in `repo_id`; empty
    /// when it cannot be read (the answer it is appended to says why).
    pub(crate) async fn name_refs_of(&self, repo_id: &str, name: &str, way: Way) -> String {
        let Ok(graph) = self.graph(repo_id).await else {
            return String::new();
        };
        let (Ok(symbols), Ok(relationships)) = (graph.symbols().await, graph.relationships().await)
        else {
            return String::new();
        };
        match navigation::resolve(&symbols, name) {
            Ok(subject) => section(&symbols, &relationships, &[subject], way),
            Err(_) => String::new(),
        }
    }

    /// The section of an `impact_of_change` answer: places that name the
    /// symbol, or any symbol of the file. None for a diff.
    pub(crate) async fn name_refs_of_impact(&self, params: &crate::ImpactOfChangeParams) -> String {
        let Ok(graph) = self.graph_for(params.repo_id.as_deref()).await else {
            return String::new();
        };
        let (Ok(symbols), Ok(relationships)) = (graph.symbols().await, graph.relationships().await)
        else {
            return String::new();
        };
        let subjects: Vec<&CodeSymbol> = match (&params.symbol_name, &params.file_path) {
            (Some(name), _) => match navigation::resolve(&symbols, name) {
                Ok(subject) => vec![subject],
                Err(_) => return String::new(),
            },
            (None, Some(path)) => navigation::symbols_in_file(&symbols, path),
            (None, None) => return String::new(),
        };
        section(&symbols, &relationships, &subjects, Way::Into)
    }
}

/// The sections of a `trace_flow` answer: what the symbols on the chains
/// (or `from`, when there is none) name, and the places that name `to`.
pub(crate) fn flow_sections(
    symbols: &[CodeSymbol],
    relationships: &[myceliums_storage::Relationship],
    on_flow: &[&CodeSymbol],
    to: &CodeSymbol,
) -> String {
    let named = section(symbols, relationships, on_flow, Way::OutOf);
    let naming = section(symbols, relationships, &[to], Way::Into);
    named + &naming
}

#[cfg(test)]
mod tests {
    use super::*;
    use myceliums_core::name_refs::NameRefMetadata;
    use myceliums_storage::{Relationship, RelationshipKind, SymbolKind};

    fn sym(uid: &str, file: &str, qname: &str, kind: SymbolKind, lines: (u32, u32)) -> CodeSymbol {
        CodeSymbol {
            uid: uid.to_string(),
            name: qname.rsplit('.').next().unwrap().to_string(),
            qualified_name: qname.to_string(),
            kind,
            file_path: file.to_string(),
            start_line: lines.0,
            end_line: lines.1,
            signature: String::new(),
            content: String::new(),
            repo_id: "r".to_string(),
            metadata: None,
        }
    }

    fn edge(source: &str, target: &str, file: &str, line: u32, text: &str) -> Relationship {
        Relationship {
            uid: format!("{source}->{target}"),
            source_uid: source.to_string(),
            target_uid: target.to_string(),
            kind: RelationshipKind::References,
            repo_id: "r".to_string(),
            metadata: serde_json::to_string(&NameRefMetadata {
                extraction_method: name_refs::METHOD.to_string(),
                file: file.to_string(),
                line,
                name: "org.acme.Plugin".to_string(),
                text: text.to_string(),
                origin: "string".to_string(),
                count: 1,
            })
            .unwrap(),
        }
    }

    #[test]
    fn places_that_name_a_symbol_are_listed_by_file() {
        let symbols = vec![
            sym(
                "p",
                "src/org/acme/Plugin.java",
                "Plugin",
                SymbolKind::Class,
                (3, 20),
            ),
            sym(
                "s",
                "src/org/acme/Plugin.java",
                "Plugin.start",
                SymbolKind::Method,
                (5, 9),
            ),
            sym(
                "l",
                "src/org/acme/Loader.java",
                "Loader.load",
                SymbolKind::Method,
                (4, 12),
            ),
        ];
        let mut relationships = vec![
            edge(
                "l",
                "p",
                "src/org/acme/Loader.java",
                6,
                "Class.forName(\"org.acme.Plugin\");",
            ),
            edge(
                "file-uid",
                "p",
                "res/META-INF/spring.factories",
                2,
                "org.acme.Plugin",
            ),
            edge(
                "l",
                "s",
                "src/org/acme/Loader.java",
                8,
                "run(\"org.acme.Plugin#start\");",
            ),
            Relationship {
                uid: "m".into(),
                source_uid: "s".into(),
                target_uid: "p".into(),
                kind: RelationshipKind::MemberOf,
                repo_id: "r".into(),
                metadata: String::new(),
            },
        ];
        let plugin = &symbols[0];
        let text = section(&symbols, &relationships, &[plugin], Way::Into);
        assert_eq!(
            text,
            "\nReferenced by name (3: strings and config files naming it):\n\
             res/META-INF/spring.factories\n  :2 org.acme.Plugin\n\
             src/org/acme/Loader.java\n  :6 in Loader.load: Class.forName(\"org.acme.Plugin\");\n  \
             :8 in Loader.load: run(\"org.acme.Plugin#start\"); → Plugin.start\n"
        );
        let load = &symbols[2];
        let out = section(&symbols, &relationships, &[load], Way::OutOf);
        assert!(
            out.contains("src/org/acme/Plugin.java\n  Plugin Class :3-20 (named at src/org/acme/Loader.java:6)\n"),
            "{out}"
        );
        // Nothing to show: nothing appended.
        relationships.clear();
        assert_eq!(section(&symbols, &relationships, &[plugin], Way::Into), "");
    }

    #[test]
    fn a_long_list_says_how_to_find_the_rest() {
        let symbols = vec![sym(
            "p",
            "src/org/acme/Plugin.java",
            "Plugin",
            SymbolKind::Class,
            (3, 20),
        )];
        let relationships: Vec<Relationship> = (1..=SHOWN as u32 + 2)
            .map(|i| {
                let mut e = edge("f", "p", &format!("conf/{i:02}.xml"), i, "<bean/>");
                e.uid = format!("e{i}");
                e
            })
            .collect();
        let text = section(&symbols, &relationships, &[&symbols[0]], Way::Into);
        assert!(text.starts_with("\nReferenced by name (10: "), "{text}");
        assert!(
            text.ends_with(
                "2 more not shown. Next (by name): grep -rnF \"org.acme.Plugin\" lists the places that name Plugin\n"
            ),
            "{text}"
        );
    }
}
