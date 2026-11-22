//! How a graph walk says it is not the whole answer (#169): the shared
//! [`Gaps`] of core, and its structured form for the list tools.

pub(crate) use myceliums_core::gaps::Gaps;

/// The structured form of `gaps` for list tools; `None` when nothing was
/// cut.
pub(crate) fn structured(
    gaps: &Gaps,
    noun: &str,
    next: String,
) -> Option<crate::output::Incomplete> {
    (!gaps.is_empty()).then(|| crate::output::Incomplete {
        reasons: gaps.reasons(noun),
        branches_cut: gaps.branches() as u32,
        next,
    })
}
