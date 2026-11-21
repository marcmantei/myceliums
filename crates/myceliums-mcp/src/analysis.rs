//! The full (re-)analysis behind the `analyze` tool.
//!
//! * **Off the async runtime.** The whole build — parsing, call resolution,
//!   embeddings, community detection, process tracing, the swap — runs on a
//!   blocking-pool thread ([`Rebuild::run`]), so a long analysis never stalls
//!   the other requests of the server. (The analyzer's parse phase already
//!   fanned out to rayon from `spawn_blocking`; the rest used to run on a
//!   runtime worker.)
//! * **Atomic.** The new index is built in a [`StagedIndex`] beside the live
//!   one and swapped in only when complete. A failed or cancelled analysis
//!   deletes the staging directory; tools keep answering from the previous
//!   index throughout. The graph cache is invalidated at the swap.
//! * **One at a time per repository.** A second `analyze` of a repository
//!   that is being analyzed — by this server or by another process such as
//!   `myc analyze` — is an `isError` result ([`RunningGuard`],
//!   [`AnalysisLock`]).
//! * **Progress.** When the client sent a `progressToken`,
//!   [`ProgressSink`] turns analysis phases into `notifications/progress`
//!   (0–100, increasing, with a message).

use crate::errors::ToolError;
use crate::output::AnalyzeOutput;
use myceliums_core::analyzer::Analyzer;
use myceliums_core::config::ProjectConfig;
use myceliums_core::{
    build_snapshot, AnalysisLock, AnalysisPhase, CommunityDetector, GraphService, LockOutcome,
    PreviousVectors, ProcessTracer, ProgressReporter, StagedIndex,
};
use myceliums_storage::{RepoInfo, RepoRegistry, Store};
use rmcp::model::{ProgressNotificationParam, ProgressToken};
use rmcp::service::{Peer, RequestContext, RoleServer};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

// ── One analysis per repository ──────────────────────────────────────

/// Repositories this process is analyzing right now.
static RUNNING: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Default::default);

/// Marks a repository as being analyzed by this process until dropped. The
/// file lock ([`AnalysisLock`]) covers other processes; this covers two
/// calls in one server deterministically.
pub(crate) struct RunningGuard {
    // Fields drop in order: the file lock goes before the in-process mark.
    _lock: AnalysisLock,
    _claim: Claim,
}

/// Why a repository could not be claimed.
#[derive(Debug)]
pub(crate) enum Busy {
    /// An analysis of it is running; the text says where.
    Running(String),
    Failed(ToolError),
}

impl Busy {
    /// The error of an `analyze` that found the repository busy.
    pub(crate) fn for_analyze(self, repo_id: &str) -> ToolError {
        match self {
            Busy::Running(holder) => ToolError::recoverable(
                format!("Analysis already running for {repo_id:?} ({holder})."),
                "wait until it finishes, then call analyze again (without force it reuses the \
                 fresh index); until then the tools answer from the previous index",
            ),
            Busy::Failed(e) => e,
        }
    }

    /// The error of a `delete` that found the repository busy.
    pub(crate) fn for_delete(self, repo_id: &str) -> ToolError {
        match self {
            Busy::Running(holder) => ToolError::recoverable(
                format!("Cannot delete {repo_id:?}: an analysis of it is running ({holder})."),
                "wait until the analysis finishes, then call delete again",
            ),
            Busy::Failed(e) => e,
        }
    }
}

impl RunningGuard {
    /// Claims `repo_id`, or says that an analysis of it is running.
    pub(crate) fn acquire(data_dir: &std::path::Path, repo_id: &str) -> Result<Self, Busy> {
        if !RUNNING
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(repo_id.to_string())
        {
            return Err(Busy::Running("in this server".to_string()));
        }
        // From here on, dropping `claim` releases the in-process mark.
        let claim = Claim(repo_id.to_string());
        let lock = match AnalysisLock::acquire(data_dir, repo_id) {
            Ok(LockOutcome::Acquired(lock)) => lock,
            Ok(LockOutcome::AlreadyRunning { pid }) => {
                // pid 0: the lock file was still being written.
                let holder = match pid {
                    0 => "in another process".to_string(),
                    pid => format!("in process {pid}"),
                };
                return Err(Busy::Running(holder));
            }
            Err(e) => {
                return Err(Busy::Failed(ToolError::internal(
                    "cannot take the analysis lock",
                    e,
                )))
            }
        };
        Ok(Self {
            _lock: lock,
            _claim: claim,
        })
    }
}

/// An in-process mark, released on drop.
struct Claim(String);

impl Drop for Claim {
    fn drop(&mut self) {
        RUNNING
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.0);
    }
}

// ── Progress notifications ───────────────────────────────────────────

/// Forwards analysis progress to the client as `notifications/progress`.
/// Exists only when the request carried a `progressToken`.
pub(crate) struct ProgressSink {
    inner: Arc<SinkInner>,
    forwarder: tokio::task::JoinHandle<()>,
}

/// What the forwarder receives.
enum Forward {
    Progress(ProgressNotificationParam),
    /// Everything before this is sent; stop.
    Stop,
}

struct SinkInner {
    token: ProgressToken,
    tx: mpsc::UnboundedSender<Forward>,
    /// The last percentage sent: progress must increase. Also serializes
    /// sends, so `Stop` is the last message. `u32::MAX` once closed.
    last: Mutex<u32>,
}

impl SinkInner {
    fn send(&self, percent: u32, message: impl Into<String>) {
        let mut last = self.last.lock().unwrap_or_else(|e| e.into_inner());
        if *last >= percent {
            return;
        }
        *last = percent;
        let _ = self.tx.send(Forward::Progress(
            ProgressNotificationParam::new(self.token.clone(), f64::from(percent))
                .with_total(100.0)
                .with_message(message),
        ));
    }

    /// Sends nothing more after this.
    fn close(&self) {
        let mut last = self.last.lock().unwrap_or_else(|e| e.into_inner());
        *last = u32::MAX;
        let _ = self.tx.send(Forward::Stop);
    }

    fn report(&self, phase: &AnalysisPhase) {
        let share = |current: usize, total: usize, span: u32| {
            (current.min(total) as u64 * u64::from(span))
                .checked_div(total as u64)
                .unwrap_or(0) as u32
        };
        match phase {
            AnalysisPhase::Discovering => self.send(1, "discovering files"),
            AnalysisPhase::Parsing { current, total } => self.send(
                2 + share(*current, *total, 58),
                format!("parsing files: {current}/{total}"),
            ),
            AnalysisPhase::BuildingRelationships => {
                self.send(62, "resolving calls and imports, embedding symbols")
            }
            AnalysisPhase::GeneratingEmbeddings { current, total } => self.send(
                62 + share(*current, *total, 16),
                format!("embedding symbols: {current}/{total}"),
            ),
            AnalysisPhase::Complete { symbols, files } => self.send(
                80,
                format!("graph written: {symbols} symbols in {files} files"),
            ),
            AnalysisPhase::DetectingCommunities => self.send(85, "detecting communities"),
            AnalysisPhase::TracingProcesses => self.send(90, "tracing execution flows"),
        }
    }
}

impl ProgressSink {
    /// A sink for the request in `context`, or `None` when the client sent
    /// no `progressToken` (then no notification is sent).
    pub(crate) fn for_request(context: &RequestContext<RoleServer>) -> Option<Self> {
        let token = context.meta.get_progress_token()?;
        Some(Self::new(token, context.peer.clone()))
    }

    fn new(token: ProgressToken, peer: Peer<RoleServer>) -> Self {
        let (tx, mut rx) = mpsc::unbounded_channel::<Forward>();
        // One forwarder keeps the notifications in order.
        let forwarder = tokio::spawn(async move {
            while let Some(Forward::Progress(param)) = rx.recv().await {
                if let Err(e) = peer.notify_progress(param).await {
                    tracing::debug!("cannot send a progress notification: {e}");
                }
            }
        });
        Self {
            inner: Arc::new(SinkInner {
                token,
                tx,
                last: Mutex::new(0),
            }),
            forwarder,
        }
    }

    /// Sends what is queued and stops, so no notification follows the
    /// response. Copies of the sink still held elsewhere send nothing more.
    pub(crate) async fn close(self) {
        self.inner.close();
        let _ = self.forwarder.await;
    }
}

/// The reporter handed to the analyzer: the progress sink and, in tests, an
/// observer.
struct Reporters {
    sink: Option<Arc<SinkInner>>,
    observer: Option<Arc<dyn ProgressReporter>>,
}

impl ProgressReporter for Reporters {
    fn report(&self, phase: AnalysisPhase) {
        if let Some(sink) = &self.sink {
            sink.report(&phase);
        }
        if let Some(observer) = &self.observer {
            observer.report(phase);
        }
    }
}

// ── The rebuild ──────────────────────────────────────────────────────

/// Everything a full analysis needs, owned, so it can move to a blocking
/// thread.
pub(crate) struct Rebuild {
    pub repo_id: String,
    pub repo_name: String,
    pub project: PathBuf,
    pub config: Option<ProjectConfig>,
    pub skip_embeddings: bool,
    pub data_dir: PathBuf,
    pub service: Arc<GraphService>,
    /// Recorded as the index time.
    pub started_at: String,
    pub analyzed_commit: Option<String>,
    pub guard: RunningGuard,
}

/// How a [`Rebuild`] reports and stops.
pub(crate) struct RunControl {
    pub sink: Option<ProgressSink>,
    /// Sees every phase the analyzer reports (tests).
    pub observer: Option<Arc<dyn ProgressReporter>>,
    /// Cancelled: stop before the swap and keep the previous index.
    pub ct: CancellationToken,
}

impl Rebuild {
    /// Runs the analysis on a blocking-pool thread and returns its summary.
    pub(crate) async fn run(self, control: RunControl) -> Result<AnalyzeOutput, ToolError> {
        let RunControl { sink, observer, ct } = control;
        let reporters = Reporters {
            sink: sink.as_ref().map(|s| s.inner.clone()),
            observer,
        };
        let handle = tokio::runtime::Handle::current();
        let outcome =
            tokio::task::spawn_blocking(move || handle.block_on(self.build(reporters, ct)))
                .await
                .map_err(|e| ToolError::internal("the analysis task failed", e));
        if let Some(sink) = sink {
            sink.close().await;
        }
        outcome?
    }

    async fn build(
        self,
        reporters: Reporters,
        ct: CancellationToken,
    ) -> Result<AnalyzeOutput, ToolError> {
        let Rebuild {
            repo_id,
            repo_name,
            project,
            config,
            skip_embeddings,
            data_dir,
            service,
            started_at,
            analyzed_commit,
            guard,
        } = self;
        let cancelled = || {
            ct.is_cancelled().then(|| {
                ToolError::recoverable(
                    "analyze was cancelled before the new index was complete; the previous \
                     index is unchanged.",
                    "call analyze again to re-index",
                )
            })
        };
        let sink = reporters.sink.clone();
        let reporters: Arc<dyn ProgressReporter> = Arc::new(reporters);
        let step = |phase: AnalysisPhase| reporters.report(phase);

        let live = RepoRegistry::repo_db_path(&data_dir, &repo_id);
        // The vectors of the index being replaced: unchanged symbols keep
        // them, so a run that does not embed (skip_embeddings, or a model
        // that fails) does not drop the embeddings (#151). Only an index the
        // registry knows, at the current vector geometry. Unreadable: the
        // run embeds what it can, as without a previous index.
        let geometry = crate::repo::registry()?
            .get(&repo_id)
            .map(|info| info.vector_geometry_version);
        let previous = match (geometry, Store::open_existing(&live, &repo_id).await) {
            (Some(geometry), Ok(store)) => PreviousVectors::load(&store, geometry)
                .await
                .unwrap_or_else(|e| {
                    tracing::warn!("cannot read the previous index's vectors: {e}");
                    None
                }),
            _ => None,
        };
        let staged = StagedIndex::create(&live)
            .map_err(|e| ToolError::internal("cannot create the staging directory", e))?;

        let store = Store::open(staged.path(), &repo_id)
            .await
            .map_err(|e| ToolError::internal("cannot open the new index", e))?;
        let analyzer = match config {
            Some(cfg) => Analyzer::with_config(store, project.clone(), cfg),
            None => Analyzer::new(store, project.clone()),
        }
        .set_skip_embeddings(skip_embeddings)
        .with_previous_vectors(previous)
        .with_progress(reporters.clone())
        .with_cancel({
            let ct = ct.clone();
            Arc::new(move || ct.is_cancelled())
        });
        let result = analyzer.analyze().await;
        drop(analyzer);
        let result = match result {
            Ok(result) => result,
            Err(e) if e.is_cancelled() => return Err(cancelled().expect("cancelled")),
            Err(e) => return Err(ToolError::internal("analysis failed", e)),
        };
        if let Some(e) = cancelled() {
            return Err(e);
        }

        let store = Store::open(staged.path(), &repo_id)
            .await
            .map_err(|e| ToolError::internal("cannot open the new index", e))?;
        let symbols = store
            .get_symbols()
            .await
            .map_err(|e| ToolError::internal("cannot read symbols", e))?;
        let relationships = store
            .get_relationships()
            .await
            .map_err(|e| ToolError::internal("cannot read relationships", e))?;

        step(AnalysisPhase::DetectingCommunities);
        let communities = CommunityDetector::detect(&symbols, &relationships, &repo_id)
            .map_err(|e| ToolError::internal("community detection failed", e))?;
        let community_count = store
            .store_communities(&communities)
            .await
            .map_err(|e| ToolError::internal("cannot store communities", e))?;
        if let Some(e) = cancelled() {
            return Err(e);
        }

        step(AnalysisPhase::TracingProcesses);
        let processes = ProcessTracer::trace(&symbols, &relationships, &repo_id)
            .map_err(|e| ToolError::internal("process tracing failed", e))?;
        let process_count = store
            .store_processes(&processes)
            .await
            .map_err(|e| ToolError::internal("cannot store processes", e))?;
        drop(store);
        if let Some(e) = cancelled() {
            return Err(e);
        }

        // The swap. Cached handles point at the directory being replaced.
        if let Some(sink) = &sink {
            sink.send(95, "replacing the previous index");
        }
        service.invalidate(&repo_id);
        staged
            .commit()
            .map_err(|e| ToolError::internal("cannot replace the previous index", e))?;

        let mut registry = crate::repo::registry()?;
        registry.register(RepoInfo {
            id: repo_id.clone(),
            name: repo_name,
            path: project.to_string_lossy().to_string(),
            analyzed_at: started_at,
            symbol_count: result.symbol_count as u32,
            file_count: result.file_count as u32,
            analyzed_commit: analyzed_commit.clone(),
            vector_geometry_version: myceliums_storage::schema::VECTOR_GEOMETRY_VERSION,
            classification_version: myceliums_storage::schema::SYMBOL_CLASSIFICATION_VERSION,
        });
        registry
            .save()
            .map_err(|e| ToolError::internal("cannot save the repository registry", e))?;
        // Reads from here on see the new index and its registry entry.
        service.invalidate(&repo_id);
        drop(guard);

        // Record this analysis for detect_architecture_drift. Best effort: a
        // failed write costs the drift history, not the index.
        let snapshot = build_snapshot(&repo_id, &symbols, &relationships);
        if let Err(e) =
            myceliums_core::record_snapshot(&data_dir, &snapshot, analyzed_commit.as_deref())
        {
            tracing::warn!("cannot record the graph snapshot: {e}");
        }

        if let Some(sink) = &sink {
            sink.send(100, "done");
        }
        let embedding_stats = result.embedding_stats();
        Ok(AnalyzeOutput {
            repo_id,
            symbols: result.symbol_count,
            files: result.file_count,
            relationships: result.relationship_count,
            communities: community_count,
            processes: process_count,
            symbols_embedded: embedding_stats.symbols_embedded,
            symbols_total: embedding_stats.symbols_total,
            embedding_failures: embedding_stats.embedding_failures,
            embeddings_reused: result.embeddings_reused,
            calls: Some(result.call_resolution.summary()),
            skipped: result.skip_summary(),
            cached: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sink() -> (Arc<SinkInner>, mpsc::UnboundedReceiver<Forward>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let inner = Arc::new(SinkInner {
            token: ProgressToken(rmcp::model::NumberOrString::Number(7)),
            tx,
            last: Mutex::new(0),
        });
        (inner, rx)
    }

    #[test]
    fn progress_only_increases_and_ends_at_the_total() {
        let (sink, mut rx) = sink();
        sink.report(&AnalysisPhase::Discovering);
        sink.report(&AnalysisPhase::Parsing {
            current: 0,
            total: 10,
        });
        sink.report(&AnalysisPhase::Parsing {
            current: 5,
            total: 10,
        });
        // The poller repeats itself; a repeat sends nothing.
        sink.report(&AnalysisPhase::Parsing {
            current: 5,
            total: 10,
        });
        sink.report(&AnalysisPhase::Parsing {
            current: 0,
            total: 0,
        });
        sink.report(&AnalysisPhase::BuildingRelationships);
        sink.report(&AnalysisPhase::Complete {
            symbols: 3,
            files: 1,
        });
        sink.report(&AnalysisPhase::DetectingCommunities);
        sink.report(&AnalysisPhase::TracingProcesses);
        sink.send(100, "done");
        sink.close();
        sink.send(100, "after close");
        sink.report(&AnalysisPhase::Discovering);

        let mut seen = Vec::new();
        let mut stopped = false;
        while let Ok(msg) = rx.try_recv() {
            assert!(!stopped, "nothing follows Stop");
            let p = match msg {
                Forward::Progress(p) => p,
                Forward::Stop => {
                    stopped = true;
                    continue;
                }
            };
            assert_eq!(p.total, Some(100.0));
            assert_eq!(
                p.progress_token,
                ProgressToken(rmcp::model::NumberOrString::Number(7))
            );
            seen.push((p.progress, p.message.unwrap()));
        }
        let values: Vec<f64> = seen.iter().map(|(v, _)| *v).collect();
        assert_eq!(values, vec![1.0, 2.0, 31.0, 62.0, 80.0, 85.0, 90.0, 100.0]);
        assert_eq!(seen[2].1, "parsing files: 5/10");
        assert!(stopped);
    }

    #[test]
    fn a_repository_is_claimed_once_per_process() {
        let data = tempfile::tempdir().unwrap();
        let first = RunningGuard::acquire(data.path(), "guard-test").expect("first claim");
        let second = RunningGuard::acquire(data.path(), "guard-test")
            .err()
            .expect("second claim is refused")
            .for_analyze("guard-test");
        let message = second.message();
        assert!(message.contains("Analysis already running"), "{message}");
        assert!(matches!(second, ToolError::Recoverable { .. }));
        drop(first);
        RunningGuard::acquire(data.path(), "guard-test").expect("released on drop");
    }
}
