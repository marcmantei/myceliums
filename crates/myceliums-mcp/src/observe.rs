//! Per-call tracing and statistics, applied once at the dispatch layer.
//!
//! [`Observed`] wraps the rmcp router: every `tools/call` request runs inside
//! one `tool_call` span (target `myceliums_mcp::calls`) carrying the tool,
//! its toolset, the repository it was asked about, and — once it answers —
//! the duration, the size of the result text, an approximate token count
//! (bytes / 4) and the outcome (`ok`, `is_error`, `protocol_error`, or
//! `cancelled` when the request is dropped unanswered). The span ends with
//! one INFO event, so a server started by `myc mcp` logs a line per call on
//! stderr. The same numbers feed [`CallStats`]. Handlers do nothing for it.

use crate::stats::{approx_tokens, CallStats, Outcome};
use rmcp::model::{
    CallToolRequestParams, CallToolResult, ClientNotification, ClientRequest, ErrorData,
    RawContent, ServerInfo, ServerResult,
};
use rmcp::service::{NotificationContext, RequestContext, RoleServer, Service};
use std::sync::Arc;
use std::time::Instant;
use tracing::field::{display, Empty};
use tracing::Instrument;

/// The tracing target of the per-call spans and events.
pub(crate) const CALLS_TARGET: &str = "myceliums_mcp::calls";

/// A service that traces and counts every tool call before handing it to
/// `inner` (the rmcp router).
pub struct Observed<S> {
    inner: S,
    stats: Arc<CallStats>,
}

impl<S> Observed<S> {
    pub(crate) fn new(inner: S, stats: Arc<CallStats>) -> Self {
        Self { inner, stats }
    }
}

impl<S: Service<RoleServer>> Service<RoleServer> for Observed<S> {
    async fn handle_request(
        &self,
        request: ClientRequest,
        context: RequestContext<RoleServer>,
    ) -> Result<ServerResult, ErrorData> {
        let ClientRequest::CallToolRequest(call) = request else {
            return self.inner.handle_request(request, context).await;
        };
        let mut observed = ToolCall::start(&call.params, &self.stats);
        let span = observed.span.clone();
        let result = self
            .inner
            .handle_request(ClientRequest::CallToolRequest(call), context)
            .instrument(span)
            .await;
        match &result {
            Ok(ServerResult::CallToolResult(r)) => observed.finish_result(r),
            Ok(_) => observed.finish(Outcome::Ok, 0),
            Err(e) => observed.finish(Outcome::ProtocolError, e.message.len()),
        }
        result
    }

    async fn handle_notification(
        &self,
        notification: ClientNotification,
        context: NotificationContext<RoleServer>,
    ) -> Result<(), ErrorData> {
        self.inner.handle_notification(notification, context).await
    }

    fn get_info(&self) -> ServerInfo {
        self.inner.get_info()
    }
}

/// One tool call in flight. Dropped unfinished, it records `cancelled`.
struct ToolCall {
    tool: String,
    span: tracing::Span,
    started: Instant,
    stats: Arc<CallStats>,
    finished: bool,
}

impl ToolCall {
    fn start(params: &CallToolRequestParams, stats: &Arc<CallStats>) -> Self {
        let tool = params.name.to_string();
        let toolset = crate::toolsets::toolset_of(&tool).map_or("-", |t| t.name());
        let span = tracing::info_span!(
            target: CALLS_TARGET,
            "tool_call",
            tool = %tool,
            toolset = %toolset,
            repo = Empty,
            duration_ms = Empty,
            result_bytes = Empty,
            approx_tokens = Empty,
            outcome = Empty,
        );
        if let Some(repo) = repo_of(params) {
            span.record("repo", display(repo));
        }
        Self {
            tool,
            span,
            started: Instant::now(),
            stats: stats.clone(),
            finished: false,
        }
    }

    fn finish_result(&mut self, result: &CallToolResult) {
        let outcome = if result.is_error == Some(true) {
            Outcome::IsError
        } else {
            Outcome::Ok
        };
        self.finish(outcome, result_bytes(result));
    }

    fn finish(&mut self, outcome: Outcome, bytes: usize) {
        self.finished = true;
        let elapsed = self.started.elapsed();
        let duration_ms = (elapsed.as_secs_f64() * 100_000.0).round() / 100.0;
        self.span.record("duration_ms", duration_ms);
        self.span.record("result_bytes", bytes as u64);
        self.span
            .record("approx_tokens", approx_tokens(bytes) as u64);
        self.span.record("outcome", display(outcome.as_str()));
        self.span.in_scope(|| match outcome {
            Outcome::Ok | Outcome::IsError => {
                tracing::info!(target: CALLS_TARGET, "tool call finished")
            }
            Outcome::ProtocolError | Outcome::Cancelled => {
                tracing::warn!(target: CALLS_TARGET, "tool call failed")
            }
        });
        self.stats.record(&self.tool, outcome, elapsed, bytes);
    }
}

impl Drop for ToolCall {
    fn drop(&mut self) {
        if !self.finished {
            self.finish(Outcome::Cancelled, 0);
        }
    }
}

/// The repository a call is about: its `repo_id` argument, or for `analyze`
/// the project `path`. `None` when the tool falls back to the working
/// directory's repository.
fn repo_of(params: &CallToolRequestParams) -> Option<&str> {
    let args = params.arguments.as_ref()?;
    ["repo_id", "path"]
        .iter()
        .find_map(|key| args.get(*key)?.as_str())
        .filter(|s| !s.is_empty())
}

/// Bytes of result text the agent reads: the text blocks, or the JSON of
/// `structuredContent` when there are none.
fn result_bytes(result: &CallToolResult) -> usize {
    let text: usize = result
        .content
        .iter()
        .map(|c| match &c.raw {
            RawContent::Text(t) => t.text.len(),
            _ => 0,
        })
        .sum();
    if text > 0 {
        return text;
    }
    result
        .structured_content
        .as_ref()
        .map_or(0, |v| v.to_string().len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::model::Content;

    #[test]
    fn result_bytes_count_the_text_an_agent_reads() {
        let r = CallToolResult::success(vec![Content::text("abcd"), Content::text("ef")]);
        assert_eq!(result_bytes(&r), 6);
        let mut structured = CallToolResult::success(vec![]);
        structured.structured_content = Some(serde_json::json!({"a": 1}));
        assert_eq!(result_bytes(&structured), 7);
    }

    #[test]
    fn repo_comes_from_repo_id_or_path() {
        let with = |args: serde_json::Value| {
            let serde_json::Value::Object(args) = args else {
                unreachable!()
            };
            CallToolRequestParams::new("t").with_arguments(args)
        };
        assert_eq!(
            repo_of(&with(serde_json::json!({"repo_id": "r1"}))),
            Some("r1")
        );
        assert_eq!(
            repo_of(&with(serde_json::json!({"path": "/p"}))),
            Some("/p")
        );
        assert_eq!(repo_of(&with(serde_json::json!({"query": "x"}))), None);
        assert_eq!(repo_of(&CallToolRequestParams::new("t")), None);
    }
}
