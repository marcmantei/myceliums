//! Request handlers, one module per API area.

pub(crate) mod analytics;
pub(crate) mod assets;
pub(crate) mod graph;
pub(crate) mod graph_v1;
pub(crate) mod knowledge;
pub(crate) mod llm_config;

use crate::error::ApiError;
use myceliums_core::{TestPolicy, TestRanking};
use myceliums_storage::RepoInfo;
use serde::Deserialize;

/// The `tests` query parameter of the search endpoints (#150): how test
/// code ranks, as in `search_code` and `myc search`.
#[derive(Deserialize, Default)]
pub(crate) struct TestsParam {
    tests: Option<String>,
}

impl TestsParam {
    /// `demote` (the default), `include` or `prefer`; anything else is a
    /// `400`.
    pub(crate) fn policy(&self) -> Result<TestPolicy, ApiError> {
        test_policy(self.tests.as_deref())
    }
}

/// Parses a `tests` parameter: `demote` (the default when absent),
/// `include` or `prefer`; anything else is a `400`.
pub(crate) fn test_policy(tests: Option<&str>) -> Result<TestPolicy, ApiError> {
    let ranking = match tests {
        None => TestRanking::default(),
        Some(value) => value.parse().map_err(|e: String| {
            ApiError::bad_request(
                e,
                "pass tests=demote (the default), tests=include or tests=prefer",
            )
        })?,
    };
    Ok(TestPolicy::new(ranking))
}

/// Said when the index predates the test/product classification (#146) and
/// the request asked for test code to be reordered: nothing is reordered.
pub(crate) fn unclassified_notice(info: &RepoInfo, tests: &TestPolicy) -> Option<String> {
    let stale =
        info.classification_version < myceliums_storage::schema::SYMBOL_CLASSIFICATION_VERSION;
    (stale && !tests.is_neutral()).then(|| {
        format!(
            "this index was built before test code was classified, so tests rank like product \
             code; run `myc analyze {}` to rank them below it",
            info.path
        )
    })
}
