//! The background work of the dashboard.
//!
//! The graphical interface runs on the main thread and must never block:
//! fetching an upstream repository, asking the local decision model and
//! driving the three agents all take seconds. Every one of those jobs runs on
//! a background thread of a Tokio runtime and reports back through a channel
//! that the interface drains once per frame.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

use fsrs_acp::{
    AcpError, AgentConfig, ModelChoice, PipelineOutcome, PipelineRequest, RunEvent, discover_models,
};
use fsrs_ai::{CalibrationProfile, FoqClient};
use fsrs_core::git::CommitStats;
use fsrs_core::{Assessment, Repo, RunState, Store, UpstreamCommit, catalog, now_unix};

use crate::state::{agent_name, output_of, prompt_of, run_state_for};

/// A message produced by the background and consumed by the interface.
#[derive(Debug)]
pub enum WorkerMessage {
    /// The upstream branch was fetched and the missing commits are known.
    Synced {
        /// The repository that was synchronised.
        repo_id: i64,
        /// The commits that are missing from the fork.
        commits: Vec<UpstreamCommit>,
    },
    /// The synchronisation failed.
    SyncFailed {
        /// The repository that could not be synchronised.
        repo_id: i64,
        /// The reason of the failure.
        error: String,
    },
    /// The local decision model rated one commit.
    Assessed {
        /// The assessment of the commit.
        assessment: Assessment,
    },
    /// The local decision model could not rate one commit.
    AssessmentFailed {
        /// The repository of the commit.
        repo_id: i64,
        /// The commit that could not be rated.
        sha: String,
        /// The reason of the failure.
        error: String,
    },
    /// The server of the local decision model reported how many requests it
    /// serves at the same time.
    SlotsKnown {
        /// The number of the slots of the server.
        slots: usize,
    },
    /// The agent published its list of models.
    ModelsLoaded {
        /// The models the user can choose from.
        models: Vec<ModelChoice>,
    },
    /// The model list could not be loaded.
    ModelsFailed {
        /// The reason of the failure.
        error: String,
    },
    /// The pipeline reported a step.
    PipelineEvent {
        /// The event of the pipeline.
        event: RunEvent,
    },
    /// The pipeline finished.
    PipelineFinished {
        /// The result of the run.
        outcome: Result<PipelineOutcome, String>,
    },
}

/// The settings of the local decision model.
#[derive(Debug, Clone, PartialEq)]
pub struct FoqSettings {
    /// Base URL of the decision server.
    pub url: String,
    /// Alias of the model on that server.
    pub model: String,
    /// Calibration profile of the decisions.
    pub profile: CalibrationProfile,
    /// Confidence below which a decision is flagged for review.
    pub min_confidence: f64,
    /// Timeout of a single request to the decision server.
    pub timeout: std::time::Duration,
}

impl Default for FoqSettings {
    /// The settings of a model that is not configured yet.
    ///
    /// The first assessment carries the settings of the user and replaces
    /// them.
    fn default() -> Self {
        Self {
            url: "http://127.0.0.1:8089".to_string(),
            model: "foq".to_string(),
            profile: CalibrationProfile::bare(),
            min_confidence: 0.8,
            timeout: std::time::Duration::from_secs(120),
        }
    }
}

impl FoqSettings {
    /// Build a client for these settings.
    pub fn client(&self) -> FoqClient {
        FoqClient::new(&self.url)
            .with_model(&self.model)
            .with_profile(self.profile.clone())
            .with_min_confidence(self.min_confidence)
            .with_timeout(self.timeout)
    }
}

/// The background executor of the dashboard.
pub struct Worker {
    runtime: tokio::runtime::Runtime,
    sender: Sender<WorkerMessage>,
    receiver: Receiver<WorkerMessage>,
    /// The client of the local decision model and the settings it was built
    /// from.
    ///
    /// The client is shared by every assessment: it keeps the connection to
    /// the server open, it remembers the size of the context window and it
    /// remembers the prompts it already fitted. A client per commit threw
    /// all of that away and asked the server for its properties again.
    ///
    /// The settings of the user can change in the middle of the life of the
    /// dashboard, so the pair is behind a lock: the client is replaced only
    /// when the settings really changed.
    client: RwLock<(FoqSettings, Arc<FoqClient>)>,
    /// The statistics of the commits of every repository, by the identifier of
    /// the repository and the SHA of the commit.
    ///
    /// The statistics of a commit never change: the diff of a commit against
    /// its first parent is the same forever. The dashboard therefore keeps
    /// them between two synchronisations and hands them to the next one, so
    /// that a fork that did not move is listed without a single diff.
    ///
    /// The cache lives here and not in the interface, because the interface
    /// would have to read the statistics back from the database before every
    /// synchronisation — one query and one parsing of the list of the files of
    /// every commit.
    known: Arc<Mutex<HashMap<i64, HashMap<String, CommitStats>>>>,
    /// The database of the dashboard, written by the background.
    ///
    /// The window draws its frames while the background works, and a write of
    /// the database on the thread of the window is a pause of the window. The
    /// background therefore keeps its own connection to the database and
    /// writes what it produced: the list of the pending commits, the
    /// assessments and the events of the pipeline.
    ///
    /// The connection is behind a lock, because a SQLite connection belongs
    /// to one thread at a time.
    store: Option<Arc<Mutex<Store>>>,
}

impl Worker {
    /// Start the background executor without a database.
    ///
    /// The tests of the background use this: they do not write anything.
    pub fn new() -> anyhow::Result<Self> {
        Self::build(None)
    }

    /// Start the background executor that writes to the database at `path`.
    ///
    /// The database is written by the background and not by the window: the
    /// window must draw its frames while the background works.
    pub fn with_store(path: &Path) -> anyhow::Result<Self> {
        let store = Store::open(path)?;
        Self::build(Some(Arc::new(Mutex::new(store))))
    }

    /// Start the background executor.
    fn build(store: Option<Arc<Mutex<Store>>>) -> anyhow::Result<Self> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()?;
        let (sender, receiver) = channel();
        // The default settings are replaced by the first assessment, which
        // carries the configuration of the user.
        let settings = FoqSettings::default();
        let client = Arc::new(settings.client());
        Ok(Self {
            runtime,
            sender,
            receiver,
            client: RwLock::new((settings, client)),
            known: Arc::new(Mutex::new(HashMap::new())),
            store,
        })
    }

    /// The client of the local decision model, rebuilt when the settings
    /// change.
    fn client(&self, settings: &FoqSettings) -> Arc<FoqClient> {
        let mut slot = self
            .client
            .write()
            .unwrap_or_else(|error| error.into_inner());
        if &slot.0 != settings {
            slot.0 = settings.clone();
            slot.1 = Arc::new(settings.client());
        }
        Arc::clone(&slot.1)
    }

    /// Take every message that arrived since the last call.
    pub fn drain(&self) -> Vec<WorkerMessage> {
        let mut messages = Vec::new();
        while let Ok(message) = self.receiver.try_recv() {
            messages.push(message);
        }
        messages
    }

    /// Fetch the upstream branch of a repository and list the missing commits.
    ///
    /// The statistics of the commits that were listed before are taken from
    /// the cache of the worker and handed to the background thread: a fork
    /// that did not move since the last synchronisation lists its pending
    /// commits without computing a single diff. The caller may hand them over
    /// itself — that is what the first synchronisation does, with the
    /// statistics that were read from the store — and the worker keeps them
    /// for the next one.
    ///
    /// The store itself is not shared — a SQLite connection is bound to one
    /// thread.
    pub fn sync_repo(&self, repo: Repo, known: Option<HashMap<String, CommitStats>>) {
        let sender = self.sender.clone();
        let cache = Arc::clone(&self.known);
        let store = self.store.clone();
        self.runtime.spawn_blocking(move || {
            let repo_id = repo.id;
            let started = Instant::now();
            let mut known = known.unwrap_or_else(|| {
                cache
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .remove(&repo_id)
                    .unwrap_or_default()
            });
            match catalog::sync_repo(&repo, &mut known) {
                Ok(commits) => {
                    tracing::info!(
                        repo_id,
                        commits = commits.len(),
                        elapsed_ms = started.elapsed().as_millis(),
                        "sync finished"
                    );
                    // The list of the commits is written by the background: the
                    // window does not wait for a write of the disk.
                    write_to(&store, |store| store.replace_commits(repo_id, &commits));
                    // The statistics of the commits that are pending are kept
                    // for the next synchronisation. They are read out of the
                    // commits themselves, so the database is not asked for
                    // them again.
                    let stats = fsrs_core::git::stats_of(&commits);
                    cache
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .insert(repo_id, stats);
                    let _ = sender.send(WorkerMessage::Synced { repo_id, commits });
                }
                Err(error) => {
                    tracing::warn!(
                        repo_id,
                        elapsed_ms = started.elapsed().as_millis(),
                        %error,
                        "sync failed"
                    );
                    // The statistics that were not used are kept: the next
                    // synchronisation does not have to compute them again.
                    cache
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .insert(repo_id, known);
                    let _ = sender.send(WorkerMessage::SyncFailed {
                        repo_id,
                        error: error.to_string(),
                    });
                }
            }
        });
    }

    /// Rate one commit with the local decision model.
    ///
    /// The diff of the commit is read from the working copy of the fork: the
    /// objects of the upstream branch are there after the fetch, so the model
    /// sees the change it has to judge.
    pub fn assess(&self, settings: FoqSettings, repo_path: PathBuf, commit: UpstreamCommit) {
        let sender = self.sender.clone();
        let client = self.client(&settings);
        let store = self.store.clone();
        self.runtime.spawn_blocking(move || {
            let started = Instant::now();
            let patch = patch_of(&repo_path, &commit.sha);
            match fsrs_ai::assess_commit(&client, &commit, patch.as_deref()) {
                Ok(assessment) => {
                    tracing::info!(
                        sha = %commit.sha,
                        elapsed_ms = started.elapsed().as_millis(),
                        "the commit was assessed"
                    );
                    // The assessment is written by the background: the window
                    // does not wait for a write of the disk.
                    write_to(&store, |store| store.upsert_assessment(&assessment));
                    let _ = sender.send(WorkerMessage::Assessed { assessment });
                }
                Err(error) => {
                    let _ = sender.send(WorkerMessage::AssessmentFailed {
                        repo_id: commit.repo_id,
                        sha: commit.sha.clone(),
                        error: error.to_string(),
                    });
                }
            }
        });
    }

    /// Ask the server of the local decision model how many requests it serves
    /// at the same time.
    ///
    /// The answer bounds the number of the ratings that are in flight: a
    /// request that waits in the queue of the server waits for the model
    /// anyway, so asking more of the server at once only makes the run
    /// slower.
    pub fn probe_slots(&self, settings: FoqSettings) {
        let sender = self.sender.clone();
        let client = self.client(&settings);
        self.runtime.spawn_blocking(move || {
            let slots = client.slots();
            tracing::info!(slots, "the decision server reported its slots");
            let _ = sender.send(WorkerMessage::SlotsKnown { slots });
        });
    }

    /// Ask the agent which models it offers.
    pub fn discover_models(&self, command: String, root: PathBuf) {
        let sender = self.sender.clone();
        self.runtime.spawn(async move {
            let config = AgentConfig::new(command, root);
            match discover_models(&config).await {
                Ok(models) => {
                    let _ = sender.send(WorkerMessage::ModelsLoaded { models });
                }
                Err(error) => {
                    let _ = sender.send(WorkerMessage::ModelsFailed {
                        error: describe(&error),
                    });
                }
            }
        });
    }

    /// Run the three-agent pipeline and report every step.
    ///
    /// The diff of the upstream commit is read from the working copy before
    /// the agents start, so that the implementer receives the change itself
    /// and not only its description.
    ///
    /// The events of the pipeline and the state of the run are written by the
    /// background: the window draws its frames while the pipeline works.
    pub fn run_pipeline(&self, request: PipelineRequest, run_id: i64) {
        let sender = self.sender.clone();
        let store = self.store.clone();
        self.runtime.spawn(async move {
            let started = Instant::now();
            let repo_path = request.repo_path.clone();
            let sha = request.commit.sha.clone();
            let patch =
                tokio::task::spawn_blocking(move || patch_of(&repo_path, &sha).unwrap_or_default())
                    .await
                    .unwrap_or_default();
            let request = PipelineRequest { patch, ..request };

            let (events, receiver) = tokio::sync::mpsc::unbounded_channel::<RunEvent>();
            let forwarder = Self::forward_events(&sender, receiver, run_id, store.clone());
            let outcome = fsrs_acp::run_pipeline(request, events).await;
            let _ = forwarder.await;
            let elapsed_ms = started.elapsed().as_millis();
            match &outcome {
                Ok(_) => tracing::info!(elapsed_ms, "the pipeline finished"),
                Err(error) => tracing::warn!(elapsed_ms, %error, "the pipeline failed"),
            }
            // The state of the run is the last thing that is written.
            let state = match &outcome {
                Ok(_) => RunState::Merged,
                Err(_) => RunState::Failed,
            };
            write_to(&store, |store| store.update_run_state(run_id, state));
            let _ = sender.send(WorkerMessage::PipelineFinished {
                outcome: outcome.map_err(|error| error.to_string()),
            });
        });
    }

    /// Forward the events of the pipeline to the interface.
    ///
    /// Every event is written to the database on the way: the window shows
    /// the log of the pipeline and does not wait for a write of the disk.
    ///
    /// The events that arrive together are written in one transaction, and
    /// not one transaction per event: the disk is asked to commit once for a
    /// burst of events. The state of the run is written in the same
    /// transaction as the events that produced it.
    async fn forward_events(
        sender: &Sender<WorkerMessage>,
        mut receiver: tokio::sync::mpsc::UnboundedReceiver<RunEvent>,
        run_id: i64,
        store: Option<Arc<Mutex<Store>>>,
    ) {
        /// The number of the events that are written in one transaction.
        const BATCH: usize = 64;

        let mut batch: Vec<fsrs_core::RunEvent> = Vec::new();
        let mut state: Option<RunState> = None;
        while let Some(event) = receiver.recv().await {
            Self::collect_event(&mut batch, &mut state, run_id, &event);
            let _ = sender.send(WorkerMessage::PipelineEvent { event });

            // Everything that is already there is taken too: a burst of the
            // events of a pipeline is one transaction, and not one
            // transaction per event.
            while batch.len() < BATCH {
                match receiver.try_recv() {
                    Ok(event) => {
                        Self::collect_event(&mut batch, &mut state, run_id, &event);
                        let _ = sender.send(WorkerMessage::PipelineEvent { event });
                    }
                    Err(_) => break,
                }
            }

            if !batch.is_empty() {
                write_to(&store, |store| {
                    store.insert_run_events_with_state(run_id, &batch, state.take())
                });
                batch.clear();
            }
        }
        // The last events of the pipeline are written when it is over.
        if !batch.is_empty() {
            write_to(&store, |store| {
                store.insert_run_events_with_state(run_id, &batch, state.take())
            });
        }
    }

    /// Put one event of the pipeline into the batch that is written.
    ///
    /// The event is remembered in the shape that the database wants, and the
    /// state of the run is the newest one that the events produced.
    fn collect_event(
        batch: &mut Vec<fsrs_core::RunEvent>,
        state: &mut Option<RunState>,
        run_id: i64,
        event: &RunEvent,
    ) {
        if let Some(new_state) = run_state_for(event) {
            *state = Some(new_state);
        }
        batch.push(fsrs_core::RunEvent {
            run_id,
            agent: agent_name(event).to_string(),
            prompt: prompt_of(event).to_string(),
            output: output_of(event),
            created_at: now_unix(),
        });
    }
}

/// Describe an agent error for the user interface.
fn describe(error: &AcpError) -> String {
    error.to_string()
}

/// Write to the database of the background, if it has one.
///
/// The write happens on the thread of the background, and the window does not
/// wait for it. A write that fails is written to the log of the program: the
/// window has nothing to do about it, and the run of the program continues.
fn write_to(store: &Option<Arc<Mutex<Store>>>, write: impl FnOnce(&Store) -> anyhow::Result<()>) {
    let Some(store) = store.as_ref() else {
        return;
    };
    let store = store.lock().unwrap_or_else(|error| error.into_inner());
    if let Err(error) = write(&store) {
        tracing::warn!(%error, "the database could not be written");
    }
}

/// Read the diff of one commit from the working copy.
///
/// The objects of the upstream branch are in the object database of the fork
/// after the fetch, so the patch can be built without another network round
/// trip. A commit that is not reachable returns `None`.
fn patch_of(repo_path: &std::path::Path, sha: &str) -> Option<String> {
    let git = fsrs_core::git::GitRepo::open(repo_path).ok()?;
    git.commit_patch(sha).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_worker_starts_and_drains_empty() {
        let worker = Worker::new().expect("the runtime should start");
        assert!(worker.drain().is_empty());
    }

    #[test]
    fn the_client_is_kept_while_the_settings_do_not_change() {
        let worker = Worker::new().expect("the runtime should start");
        let settings = FoqSettings::default();
        let first = worker.client(&settings);
        let second = worker.client(&settings);
        assert!(
            Arc::ptr_eq(&first, &second),
            "the client of the decision server has to be shared"
        );

        let other = FoqSettings {
            url: "http://127.0.0.1:8090".to_string(),
            ..settings
        };
        let third = worker.client(&other);
        assert!(
            !Arc::ptr_eq(&first, &third),
            "a change of the settings has to build a new client"
        );
    }

    #[test]
    fn the_worker_reports_the_slots_of_the_decision_server() {
        let worker = Worker::new().expect("the runtime should start");
        worker.probe_slots(FoqSettings::default());
        let mut messages = Vec::new();
        for _ in 0..100 {
            messages.extend(worker.drain());
            if !messages.is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        // A server that does not answer is assumed to have one slot, so the
        // number is always known.
        assert!(matches!(
            messages.first(),
            Some(WorkerMessage::SlotsKnown { slots }) if *slots >= 1
        ));
    }

    #[test]
    fn the_foq_client_is_built_from_the_settings() {
        let settings = FoqSettings {
            url: "http://127.0.0.1:8089".to_string(),
            model: "foq".to_string(),
            profile: CalibrationProfile::with_lora(),
            min_confidence: 0.8,
            timeout: std::time::Duration::from_secs(120),
        };
        let client = settings.client();
        assert_eq!(client.base_url(), "http://127.0.0.1:8089");
        assert_eq!(client.model(), "foq");
        assert_eq!(
            client.profile().temperature,
            CalibrationProfile::with_lora().temperature
        );
    }

    #[test]
    fn a_synchronisation_reports_the_commits() {
        let worker = Worker::new().expect("the runtime should start");
        let repo = Repo {
            id: 7,
            name: "demo".to_string(),
            path: std::env::temp_dir(),
            upstream_url: String::new(),
            upstream_web: String::new(),
            main_branch: "main".to_string(),
            pending_commits: 0,
            last_fetched_at: None,
        };
        worker.sync_repo(repo, None);
        // The directory is not a repository, so the job has to fail — but it
        // has to report the failure instead of hanging.
        let mut messages = Vec::new();
        for _ in 0..100 {
            messages.extend(worker.drain());
            if !messages.is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(matches!(
            messages.first(),
            Some(WorkerMessage::SyncFailed { .. })
        ));
    }

    #[test]
    fn the_statistics_of_the_commits_are_kept_for_the_next_synchronisation() {
        let worker = Worker::new().expect("the runtime should start");
        let temp = tempfile::tempdir().unwrap();
        let upstream = temp.path().join("upstream");
        let fork = temp.path().join("fork");
        let git = |dir: &std::path::Path, args: &[&str]| {
            let output = std::process::Command::new("git")
                .args(args)
                .current_dir(dir)
                .output()
                .expect("running git");
            assert!(
                output.status.success(),
                "git {args:?} failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        std::fs::create_dir_all(&upstream).unwrap();
        git(&upstream, &["init", "-q", "-b", "main"]);
        git(&upstream, &["config", "user.email", "upstream@example.com"]);
        git(&upstream, &["config", "user.name", "Upstream"]);
        std::fs::write(upstream.join("a.txt"), "one\n").unwrap();
        git(&upstream, &["add", "."]);
        git(&upstream, &["commit", "-q", "-m", "First"]);
        git(
            temp.path(),
            &[
                "clone",
                "-q",
                upstream.to_str().unwrap(),
                fork.to_str().unwrap(),
            ],
        );
        std::fs::write(upstream.join("b.txt"), "two\n").unwrap();
        git(&upstream, &["add", "."]);
        git(&upstream, &["commit", "-q", "-m", "Second"]);

        let repo = Repo {
            id: 9,
            name: "demo".to_string(),
            path: fork,
            upstream_url: upstream.to_string_lossy().to_string(),
            upstream_web: String::new(),
            main_branch: "main".to_string(),
            pending_commits: 0,
            last_fetched_at: None,
        };

        // The first synchronisation computes the statistics of the commit.
        worker.sync_repo(repo.clone(), None);
        let commits = wait_for_sync(&worker);
        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].summary, "Second");
        let cache = worker.known.lock().unwrap();
        let cached = cache
            .get(&repo.id)
            .expect("the statistics of the commits are kept");
        assert_eq!(cached.len(), 1);
        assert_eq!(
            cached.get(&commits[0].sha).map(|stats| stats.files.clone()),
            Some(vec!["b.txt".to_string()])
        );
        drop(cache);

        // The second one finds them in the cache: the worker is asked for no
        // statistics at all, and the commit is listed all the same.
        worker.sync_repo(repo, None);
        let commits = wait_for_sync(&worker);
        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].sha, commits[0].sha);
    }

    /// Wait for the result of a synchronisation and return its commits.
    fn wait_for_sync(worker: &Worker) -> Vec<UpstreamCommit> {
        for _ in 0..200 {
            for message in worker.drain() {
                if let WorkerMessage::Synced { commits, .. } = message {
                    return commits;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        panic!("the synchronisation did not report anything");
    }

    /// Build a fork that misses the last commit of its upstream.
    fn lagging_fork(root: &std::path::Path) -> (Repo, PathBuf) {
        let upstream = root.join("upstream");
        let fork = root.join("fork");
        let git = |dir: &std::path::Path, args: &[&str]| {
            let output = std::process::Command::new("git")
                .args(args)
                .current_dir(dir)
                .output()
                .expect("running git");
            assert!(
                output.status.success(),
                "git {args:?} failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        std::fs::create_dir_all(&upstream).unwrap();
        git(&upstream, &["init", "-q", "-b", "main"]);
        git(&upstream, &["config", "user.email", "upstream@example.com"]);
        git(&upstream, &["config", "user.name", "Upstream"]);
        std::fs::write(upstream.join("a.txt"), "one\n").unwrap();
        git(&upstream, &["add", "."]);
        git(&upstream, &["commit", "-q", "-m", "First"]);
        git(
            root,
            &[
                "clone",
                "-q",
                upstream.to_str().unwrap(),
                fork.to_str().unwrap(),
            ],
        );
        std::fs::write(upstream.join("b.txt"), "two\n").unwrap();
        git(&upstream, &["add", "."]);
        git(&upstream, &["commit", "-q", "-m", "Second"]);

        let repo = Repo {
            id: 11,
            name: "demo".to_string(),
            path: fork,
            upstream_url: upstream.to_string_lossy().to_string(),
            upstream_web: String::new(),
            main_branch: "main".to_string(),
            pending_commits: 0,
            last_fetched_at: None,
        };
        (repo, upstream)
    }

    #[test]
    fn the_background_writes_the_commits_to_the_database() {
        let temp = tempfile::tempdir().unwrap();
        let database = temp.path().join("fsrs.db");
        let (repo, _upstream) = lagging_fork(temp.path());

        // The row of the repository has to exist before the commits of it.
        let store = Store::open(&database).expect("the database");
        let id = store.upsert_repo(&repo).expect("the repository");
        drop(store);
        let repo = Repo { id, ..repo };

        let worker = Worker::with_store(&database).expect("the runtime should start");
        worker.sync_repo(repo.clone(), None);
        let commits = wait_for_sync(&worker);
        assert_eq!(commits.len(), 1);

        // The background wrote the commits while the window was drawing its
        // frames: the database holds them without the window knowing.
        let store = Store::open(&database).expect("the database");
        let stored = store.list_commits(repo.id).expect("the commits");
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].sha, commits[0].sha);
        assert_eq!(stored[0].summary, "Second");
    }

    /// The events of a run reach the database, and the state of the run is
    /// written with them.
    ///
    /// The events arrive one by one, but a burst of them belongs to one
    /// transaction: the disk is asked to commit once for the whole batch, and
    /// not once for every event.
    #[test]
    fn the_events_of_the_run_are_written_in_one_batch() {
        let temp = tempfile::tempdir().unwrap();
        let database = temp.path().join("fsrs.db");
        let store = Store::open(&database).expect("the database");
        let repo_id = store
            .upsert_repo(&Repo {
                id: 0,
                name: "demo".to_string(),
                path: std::path::PathBuf::from("/tmp/demo"),
                upstream_url: String::new(),
                upstream_web: String::new(),
                main_branch: "main".to_string(),
                pending_commits: 0,
                last_fetched_at: None,
            })
            .expect("the repository");
        let run_id = store
            .insert_run(&fsrs_core::Run {
                id: 0,
                repo_id,
                sha: "abc12345".to_string(),
                branch: "fsrs/implement/abc12345".to_string(),
                state: RunState::Prepared,
                cycles: 0,
                model: "mock/model".to_string(),
                created_at: now_unix(),
                updated_at: now_unix(),
            })
            .expect("the run");

        // Three events of the pipeline, and the channel is closed after them.
        let (events, receiver) = tokio::sync::mpsc::unbounded_channel::<RunEvent>();
        events
            .send(RunEvent::Started {
                sha: "abc12345".to_string(),
                branch: "fsrs/implement/abc12345".to_string(),
                model: "mock/model".to_string(),
            })
            .expect("the event");
        events
            .send(RunEvent::PromptSent {
                role: fsrs_acp::AgentRole::Implementer,
                prompt: "implement it".to_string(),
            })
            .expect("the event");
        events
            .send(RunEvent::AgentMessage {
                role: fsrs_acp::AgentRole::Implementer,
                text: "done".to_string(),
            })
            .expect("the event");
        drop(events);

        let store = Some(Arc::new(Mutex::new(store)));
        let (sender, _receiver) = channel();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("the runtime");
        runtime.block_on(Worker::forward_events(
            &sender,
            receiver,
            run_id,
            store.clone(),
        ));

        // Every event of the run is in the database, and the state of the run
        // is the one that the events produced.
        let store = store.as_ref().expect("the store").lock().unwrap();
        let written = store.list_run_events(run_id).expect("the events");
        assert_eq!(written.len(), 3);
        assert_eq!(written[0].agent, "pipeline");
        assert_eq!(written[1].agent, "implementer");
        assert_eq!(written[1].prompt, "implement it");
        assert_eq!(written[2].output, "done");
        let run = store.run(run_id).expect("the run").expect("the run");
        assert_eq!(run.state, RunState::Implementing);
    }
}
