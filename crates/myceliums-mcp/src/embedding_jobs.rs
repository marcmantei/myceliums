//! Embedding in the background (#157).
//!
//! `analyze` builds the graph and answers; the symbols it could not take a
//! vector over for from the previous index are embedded afterwards, here, on
//! a blocking-pool thread, a slice at a time ([`myceliums_core::embed_pending`]).
//! A first index with the default model — a 644 MB download and minutes of
//! inference — therefore never runs inside one tool call, whose client gives
//! up after a few minutes. While the job runs:
//!
//! * it holds the repository's analysis lock ([`RunningGuard`]), so no other
//!   analysis replaces the index under it;
//! * every graph result's Index line, the search warnings and `server_stats`
//!   say how far it is;
//! * semantic and hybrid search already cover the symbols embedded so far.
//!
//! Each slice is stored before the next one is embedded, so a job that stops
//! — the server exits, the model fails — loses at most one slice: the next
//! `analyze` resumes it.

use crate::analysis::RunningGuard;
use myceliums_core::{EmbedOptions, GraphService};
use myceliums_storage::Store;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

/// Where a job is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum JobState {
    Running,
    Finished,
    Failed(String),
}

/// One repository's background embedding, as last reported.
#[derive(Debug, Clone)]
pub(crate) struct Job {
    /// The embedding model.
    pub model: String,
    /// Pending symbols embedded so far.
    pub done: usize,
    /// Symbols the job started with.
    pub total: usize,
    started: Instant,
    finished: Option<Duration>,
    pub state: JobState,
}

impl Job {
    /// Time left, extrapolated from the rate so far (once there is one).
    fn eta(&self) -> Option<Duration> {
        if self.done == 0 || self.done >= self.total {
            return None;
        }
        let elapsed = self.started.elapsed().as_secs_f64();
        let left = elapsed * (self.total - self.done) as f64 / self.done as f64;
        Some(Duration::from_secs_f64(left))
    }

    /// One line on the job, for the Index line and `server_stats`.
    pub(crate) fn line(&self) -> String {
        match &self.state {
            JobState::Running => {
                let eta = self
                    .eta()
                    .map(|left| format!(", about {} left", minutes(left)))
                    .unwrap_or_default();
                format!(
                    "Embeddings: building in the background with {}, {}/{} symbols{eta}; \
                     semantic and hybrid search cover the symbols embedded so far.",
                    self.model, self.done, self.total
                )
            }
            JobState::Finished => format!(
                "Embeddings: built in the background with {}, {} symbols in {}.",
                self.model,
                self.done,
                minutes(self.finished.unwrap_or_default())
            ),
            JobState::Failed(error) => format!(
                "Embeddings: background embedding with {} stopped after {}/{} symbols: \
                 {error}. Call analyze again to resume it.",
                self.model, self.done, self.total
            ),
        }
    }
}

/// `"3 min"`, `"under a minute"`.
fn minutes(d: Duration) -> String {
    match d.as_secs() / 60 {
        0 => "under a minute".to_string(),
        n => format!("{n} min"),
    }
}

/// The jobs of this process, by repository id. A finished or failed job
/// stays until the repository's next job replaces it.
static JOBS: LazyLock<Mutex<HashMap<String, Job>>> = LazyLock::new(Default::default);

fn jobs() -> std::sync::MutexGuard<'static, HashMap<String, Job>> {
    JOBS.lock().unwrap_or_else(|e| e.into_inner())
}

/// The last report of `repo_id`'s job, if this process ran one.
pub(crate) fn job(repo_id: &str) -> Option<Job> {
    jobs().get(repo_id).cloned()
}

/// Whether `repo_id` is being embedded in the background by this process.
pub(crate) fn is_running(repo_id: &str) -> bool {
    job(repo_id).is_some_and(|j| j.state == JobState::Running)
}

/// The line the Index line and the search warnings add: a running or failed
/// job; nothing once it finished.
pub(crate) fn status_line(repo_id: &str) -> Option<String> {
    job(repo_id)
        .filter(|j| j.state != JobState::Finished)
        .map(|j| j.line())
}

/// Every job of this process, for `server_stats`.
pub(crate) fn summary() -> Option<String> {
    let jobs = jobs();
    if jobs.is_empty() {
        return None;
    }
    let mut repos: Vec<_> = jobs.iter().collect();
    repos.sort_by(|a, b| a.0.cmp(b.0));
    let lines: Vec<String> = repos
        .into_iter()
        .map(|(repo, job)| format!("  {repo}: {}", job.line()))
        .collect();
    Some(format!("Background embedding:\n{}", lines.join("\n")))
}

fn update(repo_id: &str, f: impl FnOnce(&mut Job)) {
    if let Some(job) = jobs().get_mut(repo_id) {
        f(job);
    }
}

/// A background embedding to start.
pub(crate) struct PendingJob {
    pub repo_id: String,
    /// The live index.
    pub db_path: PathBuf,
    /// The embedding model, for the status lines.
    pub model: String,
    /// Symbols without a vector when the job starts.
    pub pending: usize,
    pub options: EmbedOptions,
    pub service: Arc<GraphService>,
    /// Held until the job ends: no analysis replaces the index meanwhile.
    pub guard: RunningGuard,
}

/// Starts `job` on a blocking-pool thread and returns at once.
pub(crate) fn start(job: PendingJob) {
    jobs().insert(
        job.repo_id.clone(),
        Job {
            model: job.model.clone(),
            done: 0,
            total: job.pending,
            started: Instant::now(),
            finished: None,
            state: JobState::Running,
        },
    );
    let handle = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || handle.block_on(job.run()));
}

impl PendingJob {
    async fn run(self) {
        let repo_id = self.repo_id.clone();
        let progress = |done: usize, total: usize| {
            update(&repo_id, |job| {
                job.done = done;
                job.total = total;
            })
        };
        let outcome = match Store::open_existing(&self.db_path, &self.repo_id).await {
            Ok(store) => {
                myceliums_core::embed_pending(&store, self.options, Some(&progress), &|| false)
                    .await
                    .map_err(|e| e.to_string())
            }
            Err(e) => Err(format!("cannot open the index: {e}")),
        };
        update(&repo_id, |job| {
            job.finished = Some(job.started.elapsed());
            match outcome {
                Ok(outcome) => {
                    job.done = outcome.embedded;
                    job.state = JobState::Finished;
                }
                Err(e) => {
                    tracing::warn!("background embedding of {repo_id} stopped: {e}");
                    job.state = JobState::Failed(e);
                }
            }
        });
        // Reads from here on see every stored vector.
        self.service.invalidate(&self.repo_id);
        drop(self.guard);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(done: usize, total: usize, state: JobState) -> Job {
        Job {
            model: "jina-embeddings-v2-base-code".to_string(),
            done,
            total,
            started: Instant::now() - Duration::from_secs(120),
            finished: Some(Duration::from_secs(300)),
            state,
        }
    }

    #[test]
    fn the_status_line_says_how_far_the_job_is() {
        let running = job(100, 400, JobState::Running).line();
        assert!(running.contains("100/400 symbols"), "{running}");
        assert!(running.contains("about 6 min left"), "{running}");
        assert!(
            running.contains("jina-embeddings-v2-base-code"),
            "{running}"
        );
        let starting = job(0, 400, JobState::Running).line();
        assert!(!starting.contains("left"), "no rate yet: {starting}");
        let failed = job(256, 400, JobState::Failed("model download failed".into())).line();
        assert!(failed.contains("256/400"), "{failed}");
        assert!(failed.contains("Call analyze again to resume"), "{failed}");
        let finished = job(400, 400, JobState::Finished).line();
        assert!(finished.contains("400 symbols in 5 min"), "{finished}");
    }
}
