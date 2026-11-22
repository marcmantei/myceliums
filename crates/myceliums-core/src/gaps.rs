//! What a graph walk did not follow, in words an agent acts on (#169).
//!
//! The #163 outcome study lost sessions where an agent answered from one
//! `trace_flow` result that was partial without saying so. Every walk that
//! can stop early — `trace_flow`, `find_callers`/`find_callees` with a
//! depth, `impact_of_change` — now names each place it stopped:
//!
//! * **depth limit**: symbols at the last depth whose edges lead further,
//!   and how many symbols one more hop would add;
//! * **unlinked calls made** by the walked symbols: call sites the resolver
//!   could not link though they may reach project code — more than
//!   [`crate::MAX_AMBIGUOUS_TARGETS`] candidates (the #139 cap), a
//!   project name nothing in scope binds, a call through a variable or
//!   callback. A flow may go on through them;
//! * **unlinked call sites named like** a walked symbol: callers no edge
//!   shows;
//! * a tool-specific cut (a page, chains longer than the listed window).
//!
//! The MCP tools and the `myc` navigation commands render the reasons with
//! the exact call that continues the walk, each in its own syntax. Nothing
//! here claims completeness when nothing was cut: the graph stays a
//! heuristic, and the agent guidance says so.

use crate::navigation::DepthCut;
use crate::resolver::{CallResolutionStats, UnlinkedCall, UnlinkedReason};
use myceliums_storage::CodeSymbol;
use std::collections::{BTreeMap, BTreeSet};

/// Examples listed per reason.
const EXAMPLES: usize = 5;

/// What a walk did not follow.
#[derive(Debug, Default)]
pub struct Gaps<'a> {
    /// Reasons that are specific to one tool, already worded.
    pub other: Vec<String>,
    /// Extra branches those reasons stand for.
    pub other_branches: usize,
    /// The depth limit and what it cut, when it cut anything.
    pub depth: Option<(u32, DepthCut<'a>)>,
    /// Calls made by walked symbols that got no edge, walk order.
    pub unlinked_out: Vec<(&'a CodeSymbol, &'a UnlinkedCall)>,
    /// Unlinked call sites named like walked symbols: name → count.
    pub unlinked_in: Vec<(String, usize)>,
    /// The index holds no record of unlinked calls (written before #169,
    /// or not at all): the calls this walk cannot follow are unknown.
    pub record_missing: bool,
    /// Walked symbols in files re-indexed incrementally since the last full
    /// analysis: their unlinked calls are unknown.
    pub unknown: BTreeSet<&'a str>,
}

impl<'a> Gaps<'a> {
    /// Records the depth cut of a walk limited to `depth`, if it cut
    /// anything.
    pub fn depth_cut(&mut self, depth: u32, cut: DepthCut<'a>) {
        if cut.is_cut() {
            self.depth = Some((depth, cut));
        }
    }

    /// Records the calls `symbols` make that got no edge.
    pub fn unlinked_out_of(
        &mut self,
        stats: Option<&'a CallResolutionStats>,
        symbols: impl IntoIterator<Item = &'a CodeSymbol>,
    ) {
        let Some(stats) = stats.filter(|s| s.records_unlinked_calls()) else {
            self.record_missing = true;
            return;
        };
        for symbol in symbols {
            if stats.is_stale(&symbol.file_path) {
                self.unknown.insert(&symbol.uid);
                continue;
            }
            for call in stats.unlinked_from(&symbol.file_path, &symbol.qualified_name) {
                self.unlinked_out.push((symbol, call));
            }
        }
    }

    /// Records the unlinked call sites named like `symbols` (once per
    /// language family and name).
    pub fn unlinked_into(
        &mut self,
        stats: Option<&CallResolutionStats>,
        symbols: impl IntoIterator<Item = &'a CodeSymbol>,
    ) {
        let Some(stats) = stats else {
            self.record_missing = true;
            return;
        };
        let mut seen: BTreeMap<(&str, &str), usize> = BTreeMap::new();
        for symbol in symbols {
            let Some(family) = crate::language_family(&symbol.file_path) else {
                continue;
            };
            let count = stats.unlinked_named(family, &symbol.name);
            if count > 0 {
                seen.insert((family, symbol.name.as_str()), count);
            }
        }
        let mut named: Vec<(String, usize)> = seen
            .into_iter()
            .map(|((_, name), count)| (name.to_string(), count))
            .collect();
        named.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        self.unlinked_in.extend(named);
    }

    /// Whether anything was left out.
    pub fn is_empty(&self) -> bool {
        self.other.is_empty()
            && self.depth.is_none()
            && self.unlinked_out.is_empty()
            && self.unlinked_in.is_empty()
            && !self.record_missing
            && self.unknown.is_empty()
    }

    /// Places the walk did not follow: symbols one more hop would add,
    /// call sites without an edge, and what the tool-specific reasons count.
    pub fn branches(&self) -> usize {
        self.other_branches
            + self.depth.as_ref().map_or(0, |(_, cut)| cut.beyond)
            + self.unlinked_out.len()
            + self.unlinked_in.iter().map(|(_, n)| n).sum::<usize>()
    }

    /// One line per reason. `noun` names what the walk lists (`callers`,
    /// `dependents`, `callees`, `symbols`).
    pub fn reasons(&self, noun: &str) -> Vec<String> {
        let mut out = self.other.clone();
        if let Some((depth, cut)) = &self.depth {
            let names: Vec<String> = cut
                .frontier
                .iter()
                .take(EXAMPLES)
                .map(|s| format!("`{}`", s.name))
                .collect();
            let more = more(cut.frontier.len());
            out.push(format!(
                "depth limit {depth}: {} more {noun} one step further, through {} symbol(s) at depth {depth}: {}{more}",
                cut.beyond,
                cut.frontier.len(),
                names.join(", ")
            ));
        }
        if !self.unlinked_out.is_empty() {
            let mut by_reason: BTreeMap<UnlinkedReason, usize> = BTreeMap::new();
            for (_, call) in &self.unlinked_out {
                *by_reason.entry(call.reason).or_default() += 1;
            }
            let counts: Vec<String> = by_reason
                .iter()
                .map(|(reason, n)| format!("{n} {}", reason.describe()))
                .collect();
            let examples: Vec<String> = self
                .unlinked_out
                .iter()
                .take(EXAMPLES)
                .map(|(caller, call)| {
                    format!(
                        "`{}` -> `{}` ({}:{})",
                        caller.name, call.name, caller.file_path, call.line
                    )
                })
                .collect();
            out.push(format!(
                "{} call(s) made here got no CALLS edge ({}), so the flow may continue unseen: {}{}",
                self.unlinked_out.len(),
                counts.join(", "),
                examples.join(", "),
                more(self.unlinked_out.len())
            ));
        }
        if !self.unlinked_in.is_empty() {
            let total: usize = self.unlinked_in.iter().map(|(_, n)| n).sum();
            let names: Vec<String> = self
                .unlinked_in
                .iter()
                .take(EXAMPLES)
                .map(|(name, n)| format!("`{name}` ({n})"))
                .collect();
            out.push(format!(
                "{total} call site(s) named like a symbol here got no CALLS edge (more than {} candidates, or not bound in scope), so callers may be missing: {}{}",
                crate::MAX_AMBIGUOUS_TARGETS,
                names.join(", "),
                more(self.unlinked_in.len())
            ));
        }
        if self.record_missing {
            out.push(
                "the calls this walk cannot follow are unknown: the index predates their record \
                 (a full analysis records them)"
                    .to_string(),
            );
        }
        if !self.unknown.is_empty() {
            out.push(format!(
                "unlinked calls unknown for {} symbol(s) in files changed since the last full \
                 analysis (a full analysis records them)",
                self.unknown.len()
            ));
        }
        out
    }

    /// Whether only a full analysis can say what this walk could not follow
    /// ([`record_missing`](Self::record_missing) or
    /// [`unknown`](Self::unknown)).
    pub fn needs_full_analysis(&self) -> bool {
        self.record_missing || !self.unknown.is_empty()
    }

    /// The `Incomplete:` block of a text result, with `next` as the last
    /// line; empty when nothing was cut.
    pub fn block(&self, noun: &str, next: &str) -> String {
        if self.is_empty() {
            return String::new();
        }
        let mut out = format!(
            "Incomplete ({} branch(es) not followed):\n",
            self.branches()
        );
        for reason in self.reasons(noun) {
            out.push_str(&format!("  - {reason}\n"));
        }
        out.push_str(&format!("Next: {next}\n"));
        out
    }

    /// The generic next step for unlinked calls: read them.
    pub fn read_the_source(&self) -> Option<String> {
        if !self.unlinked_out.is_empty() {
            return Some("read the source at the lines listed above".to_string());
        }
        let (name, _) = self.unlinked_in.first()?;
        Some(format!(
            "search the source for `{name}(` (grep): those call sites may call it"
        ))
    }
}

/// `, +N more` when a list of `len` items was cut to [`EXAMPLES`].
fn more(len: usize) -> String {
    if len > EXAMPLES {
        format!(", +{} more", len - EXAMPLES)
    } else {
        String::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myceliums_storage::SymbolKind;

    fn sym(name: &str, file: &str) -> CodeSymbol {
        CodeSymbol {
            uid: name.into(),
            name: name.into(),
            qualified_name: name.into(),
            kind: SymbolKind::Function,
            file_path: file.into(),
            start_line: 1,
            end_line: 9,
            signature: String::new(),
            content: String::new(),
            repo_id: "r".into(),
            metadata: None,
        }
    }

    #[test]
    fn a_missing_or_stale_record_is_a_reason_not_silence() {
        let a = sym("a", "src/a.ts");
        let mut gaps = Gaps::default();
        gaps.unlinked_out_of(None, [&a]);
        assert!(gaps.needs_full_analysis() && !gaps.is_empty());
        assert!(gaps.reasons("callees")[0].contains("the index predates their record"));

        let old = CallResolutionStats::default();
        let mut gaps = Gaps::default();
        gaps.unlinked_out_of(Some(&old), [&a]);
        assert!(gaps.record_missing, "a record from before #169");

        let mut stale = CallResolutionStats {
            version: myceliums_storage::schema::CALL_RECORD_VERSION,
            ..Default::default()
        };
        stale.forget_files(["src/a.ts"], []);
        let mut gaps = Gaps::default();
        gaps.unlinked_out_of(Some(&stale), [&a]);
        assert!(!gaps.record_missing);
        assert_eq!(
            gaps.reasons("callees"),
            ["unlinked calls unknown for 1 symbol(s) in files changed since the last full analysis (a full analysis records them)"]
        );
    }

    #[test]
    fn nothing_cut_says_nothing() {
        let gaps = Gaps::default();
        assert!(gaps.is_empty());
        assert_eq!(gaps.block("callers", "x"), "");
    }

    #[test]
    fn every_reason_is_counted_and_named() {
        let (a, b) = (sym("a", "src/a.ts"), sym("b", "src/b.ts"));
        let mut stats = CallResolutionStats {
            version: myceliums_storage::schema::CALL_RECORD_VERSION,
            ..Default::default()
        };
        stats
            .unlinked_by_caller
            .entry("src/a.ts".into())
            .or_default()
            .insert(
                "a".into(),
                vec![
                    UnlinkedCall {
                        name: "cb".into(),
                        line: 4,
                        reason: UnlinkedReason::ThroughVariable,
                    },
                    UnlinkedCall {
                        name: "get".into(),
                        line: 6,
                        reason: UnlinkedReason::TooManyCandidates,
                    },
                ],
            );
        stats
            .unlinked_by_name
            .entry("js".into())
            .or_default()
            .insert("b".into(), 3);
        let mut gaps = Gaps::default();
        gaps.depth_cut(
            2,
            DepthCut {
                frontier: vec![&b],
                beyond: 4,
            },
        );
        gaps.unlinked_out_of(Some(&stats), [&a, &b]);
        gaps.unlinked_into(Some(&stats), [&a, &b]);
        assert_eq!(gaps.branches(), 4 + 2 + 3);
        let reasons = gaps.reasons("callees");
        assert_eq!(
            reasons[0],
            "depth limit 2: 4 more callees one step further, through 1 symbol(s) at depth 2: `b`"
        );
        assert!(
            reasons[1].starts_with("2 call(s) made here got no CALLS edge (1 more than 2 candidates, 1 through a variable or callback)"),
            "{}",
            reasons[1]
        );
        assert!(
            reasons[1].contains("`a` -> `cb` (src/a.ts:4)"),
            "{}",
            reasons[1]
        );
        assert!(
            reasons[2].starts_with("3 call site(s) named like"),
            "{}",
            reasons[2]
        );
        let block = gaps.block("callees", "find_callees(symbol_name=\"a\", depth=3)");
        assert!(block.starts_with("Incomplete (9 branch(es) not followed):\n  - depth limit 2"));
        assert!(block.ends_with("Next: find_callees(symbol_name=\"a\", depth=3)\n"));
    }
}
