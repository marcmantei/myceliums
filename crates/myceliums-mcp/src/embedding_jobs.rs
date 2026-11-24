//! Embedding in the background (#157).
//!
//! `analyze` builds the graph and answers; the symbols it could not take a
//! vector over for from the previous index are embedded afterwards, here, on
//! a blocking-pool thread ([`myceliums_core::embed_pending`]). A first index
//! with the default model — a 644 MB download and minutes of inference —
//! therefore never runs inside one tool call, whose client gives up after a
//! few minutes. While the job runs:
//!
//! * it holds the repository's analysis lock ([`RunningGuard`]), so no other
//!   analysis replaces the index under it;
//! * every graph result's Index line, the search warnings and `server_stats`
//!   say how far it is (the model download included);
//! * semantic and hybrid search already cover the symbols embedded so far,
//!   and the cached graph stays: the job writes no table the graph is read
//!   from until it moves its vectors in at the end.
//!
//! A job stops between two model batches when the server stops serving
//! ([`stop_all`]) or when an `analyze` needs the repository ([`cancel`]); it
//! keeps what it stored, and the next `analyze` resumes it. A job that
//! panics ends as failed, never as running forever. A job is reported as
//! ended only once its lock is released and the cached graph is dropped, so
//! whoever waited for it ([`wait_until_stopped`]) can claim the repository.

use crate::analysis::RunningGuard;
use myceliums_core::{EmbedOptions, GraphService, PendingOutcome};
use myceliums_storage::Store;
use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

/// Where a job is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum JobState {
    Running,
    Finished,
    /// Asked to stop (server exit, or an `analyze` that needed the lock).
    Stopped,
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
    /// Set to stop the job between two model batches.
    cancel: Arc<AtomicBool>,
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
                let download = (self.done == 0)
                    .then(myceliums_core::download_status)
                    .flatten()
                    .filter(|d| d.model == self.model)
                    .map(|d| {
                        let mb = |b: u64| b / 1_000_000;
                        match d.total {
                            Some(total) => format!(
                                " (downloading the model: {} {}/{} MB)",
                                d.file,
                                mb(d.done),
                                mb(total)
                            ),
                            None => {
                                format!(" (downloading the model: {} {} MB)", d.file, mb(d.done))
                            }
                        }
                    })
                    .unwrap_or_default();
                let eta = self
                    .eta()
                    .map(|left| format!(", about {} left", minutes(left)))
                    .unwrap_or_default();
                format!(
                    "Embeddings: building in the background with {}, {}/{} symbols{eta}{download}; \
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
            JobState::Stopped => format!(
                "Embeddings: background embedding with {} stopped after {}/{} symbols; call \
                 analyze to resume it.",
                self.model, self.done, self.total
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

/// The jobs of this process, by repository id. An ended job stays until the
/// repository's next job replaces it.
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

/// The line the Index line and the search warnings add: a running, stopped
/// or failed job; nothing once it finished.
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

/// Asks `repo_id`'s running job to stop between two model batches; whether
/// one was running.
pub(crate) fn cancel(repo_id: &str) -> bool {
    match jobs().get(repo_id) {
        Some(job) if job.state == JobState::Running => {
            job.cancel.store(true, Ordering::SeqCst);
            true
        }
        _ => false,
    }
}

/// Waits until `repo_id`'s job no longer runs, at most `timeout`; whether it
/// ended.
pub(crate) async fn wait_until_stopped(repo_id: &str, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while is_running(repo_id) {
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    true
}

/// Stops every job of this process, a model download included: the server
/// stopped serving. Jobs end within a model batch; what they stored stays,
/// and the next `analyze` resumes them.
pub(crate) fn stop_all() {
    myceliums_core::stop_embedding();
    for job in jobs().values() {
        job.cancel.store(true, Ordering::SeqCst);
    }
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
    let cancel = Arc::new(AtomicBool::new(false));
    let (db_path, repo_id, options) = (job.db_path.clone(), job.repo_id.clone(), job.options);
    let stop = cancel.clone();
    start_with(job, cancel, async move {
        let store = Store::open_existing(&db_path, &repo_id)
            .await
            .map_err(|e| format!("cannot open the index: {e}"))?;
        let progress = |done: usize, total: usize| {
            update(&repo_id, |job| {
                job.done = done;
                job.total = total;
            })
        };
        myceliums_core::embed_pending(&store, options, Some(&progress), &|| {
            stop.load(Ordering::SeqCst)
        })
        .await
        .map_err(|e| e.to_string())
    });
}

/// Registers `job` as running and runs `work` for it on a blocking-pool
/// thread, under [`Supervisor`].
fn start_with(
    job: PendingJob,
    cancel: Arc<AtomicBool>,
    work: impl Future<Output = Result<PendingOutcome, String>> + Send + 'static,
) {
    jobs().insert(
        job.repo_id.clone(),
        Job {
            model: job.model.clone(),
            done: 0,
            total: job.pending,
            started: Instant::now(),
            finished: None,
            state: JobState::Running,
            cancel: cancel.clone(),
        },
    );
    let handle = tokio::runtime::Handle::current();
    let supervisor = Supervisor {
        repo_id: job.repo_id,
        service: job.service,
        cancel,
        guard: Some(job.guard),
        outcome: None,
    };
    tokio::task::spawn_blocking(move || handle.block_on(supervisor.run(work)));
}

/// Ends a job however its work ends: with the outcome, as failed on an
/// error, and — when the work panics — as failed from `Drop`, during the
/// unwinding, so a job never stays "running".
///
/// The order matters (#201): first the cached graph goes, so reads see the
/// vectors the job moved in, then the lock, then the job is reported as
/// ended. Reported earlier, an `analyze` that stopped the job and waited for
/// its end would find the repository still locked and fail.
struct Supervisor<G> {
    repo_id: String,
    service: Arc<GraphService>,
    /// The job's cancel flag, which also tells it from a later job of the
    /// repository.
    cancel: Arc<AtomicBool>,
    /// The [`RunningGuard`] (tests use others).
    guard: Option<G>,
    outcome: Option<Result<PendingOutcome, String>>,
}

impl<G> Supervisor<G> {
    async fn run(mut self, work: impl Future<Output = Result<PendingOutcome, String>>) {
        self.outcome = Some(work.await);
    }
}

impl<G> Drop for Supervisor<G> {
    fn drop(&mut self) {
        let outcome = self
            .outcome
            .take()
            .unwrap_or_else(|| Err("the embedding task panicked; see the server log".to_string()));
        if let Err(e) = &outcome {
            tracing::warn!("background embedding of {} stopped: {e}", self.repo_id);
        }
        self.service.invalidate(&self.repo_id);
        drop(self.guard.take());
        let mut jobs = jobs();
        // With the lock free, another job of the repository may have started
        // already; its report is not ours to change.
        let Some(job) = jobs
            .get_mut(&self.repo_id)
            .filter(|job| Arc::ptr_eq(&job.cancel, &self.cancel))
        else {
            return;
        };
        job.finished = Some(job.started.elapsed());
        match outcome {
            Ok(outcome) => {
                job.done = outcome.embedded;
                job.state = if outcome.stopped {
                    JobState::Stopped
                } else {
                    JobState::Finished
                };
            }
            Err(e) => job.state = JobState::Failed(e),
        }
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
            cancel: Arc::default(),
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
        let stopped = job(256, 400, JobState::Stopped).line();
        assert!(stopped.contains("call analyze to resume"), "{stopped}");
        let finished = job(400, 400, JobState::Finished).line();
        assert!(finished.contains("400 symbols in 5 min"), "{finished}");
    }

    /// A job whose work panics ends as failed, releases the repository's
    /// analysis lock, and is no longer reported as running.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_panicking_job_ends_as_failed() {
        let data = tempfile::tempdir().unwrap();
        let repo_id = "panicking-job";
        let guard = RunningGuard::acquire(data.path(), repo_id).expect("claim");
        let job = PendingJob {
            repo_id: repo_id.to_string(),
            db_path: data.path().join("db"),
            model: "fake".to_string(),
            pending: 10,
            options: EmbedOptions::default(),
            service: Arc::new(GraphService::new(data.path().to_path_buf())),
            guard,
        };
        start_with(job, Arc::default(), async {
            panic!("model exploded");
        });
        assert!(
            wait_until_stopped(repo_id, Duration::from_secs(30)).await,
            "the job is still reported running"
        );
        let job = super::job(repo_id).unwrap();
        assert!(
            matches!(&job.state, JobState::Failed(e) if e.contains("panicked")),
            "{:?}",
            job.state
        );
        RunningGuard::acquire(data.path(), repo_id).expect("the lock was released");
    }

    /// Runs `f` when dropped: a stand-in for the analysis lock.
    struct OnRelease<F: FnMut()>(F);

    impl<F: FnMut()> Drop for OnRelease<F> {
        fn drop(&mut self) {
            (self.0)();
        }
    }

    /// Registers a running job of `repo_id` and ends it with `outcome`
    /// (`None`: its work panicked); `on_release` runs when its lock goes.
    fn end_job(
        repo_id: &str,
        outcome: Option<Result<PendingOutcome, String>>,
        on_release: impl FnMut(),
    ) {
        let data = tempfile::tempdir().unwrap();
        let running = job(0, 10, JobState::Running);
        let cancel = running.cancel.clone();
        jobs().insert(repo_id.to_string(), running);
        drop(Supervisor {
            repo_id: repo_id.to_string(),
            service: Arc::new(GraphService::new(data.path().to_path_buf())),
            cancel,
            guard: Some(OnRelease(on_release)),
            outcome,
        });
    }

    /// #201: a job is reported as ended only after its lock is released, so
    /// an `analyze` that waited for the end can take the lock.
    #[test]
    fn the_lock_is_released_before_the_end_is_reported() {
        let stopped = PendingOutcome {
            symbols_total: 10,
            pending: 10,
            embedded: 3,
            stopped: true,
        };
        for (name, outcome) in [
            ("panicked", None),
            ("failed", Some(Err("no model".to_string()))),
            ("stopped", Some(Ok(stopped))),
        ] {
            let repo_id = format!("release-order-{name}");
            let mut running_at_release = None;
            end_job(&repo_id, outcome, || {
                running_at_release = Some(is_running(&repo_id));
            });
            assert_eq!(running_at_release, Some(true), "{name}");
            assert!(!is_running(&repo_id), "{name}");
        }
    }

    /// A job of the same repository that starts as soon as the lock is free
    /// keeps its own report.
    #[test]
    fn an_ending_job_leaves_the_report_of_the_next_one_alone() {
        let repo_id = "next-job";
        end_job(repo_id, Some(Err("no model".to_string())), || {
            jobs().insert(repo_id.to_string(), job(0, 99, JobState::Running));
        });
        let next = super::job(repo_id).unwrap();
        jobs().remove(repo_id);
        assert_eq!((next.total, next.state), (99, JobState::Running));
    }

    /// A cancelled job is asked to stop and reported as stopped.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_cancelled_job_stops() {
        let data = tempfile::tempdir().unwrap();
        let repo_id = "cancelled-job";
        let guard = RunningGuard::acquire(data.path(), repo_id).expect("claim");
        let flag = Arc::new(AtomicBool::new(false));
        let seen = flag.clone();
        let job = PendingJob {
            repo_id: repo_id.to_string(),
            db_path: data.path().join("db"),
            model: "fake".to_string(),
            pending: 10,
            options: EmbedOptions::default(),
            service: Arc::new(GraphService::new(data.path().to_path_buf())),
            guard,
        };
        start_with(job, flag, async move {
            while !seen.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            Ok(PendingOutcome {
                symbols_total: 10,
                pending: 10,
                embedded: 3,
                stopped: true,
            })
        });
        assert!(is_running(repo_id));
        assert!(cancel(repo_id), "a running job was cancelled");
        assert!(wait_until_stopped(repo_id, Duration::from_secs(30)).await);
        let job = super::job(repo_id).unwrap();
        assert_eq!(job.state, JobState::Stopped);
        assert_eq!(job.done, 3);
    }
}
