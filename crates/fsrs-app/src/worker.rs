//! The background work of the dashboard.
//!
//! The graphical interface runs on the main thread and must never block:
//! fetching an upstream repository, asking the local decision model and
//! driving the three agents all take seconds. Every one of those jobs runs on
//! a background thread of a Tokio runtime and reports back through a channel
//! that the interface drains once per frame.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Instant;

use fsrs_acp::{
    AcpError, AgentConfig, ModelChoice, PipelineOutcome, PipelineRequest, RunEvent, discover_models,
};
use fsrs_ai::{CalibrationProfile, FoqClient};
use fsrs_core::git::CommitStats;
use fsrs_core::{Assessment, Repo, UpstreamCommit, catalog};

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
    /// The client of the local decision model.
    ///
    /// The client is shared by every assessment: it keeps the connection to
    /// the server open, it remembers the size of the context window and it
    /// remembers the prompts it already fitted. A client per commit threw
    /// all of that away and asked the server for its properties again.
    client: Arc<FoqClient>,
    /// The settings the client was built from.
    ///
    /// A change of the settings builds a new client.
    settings: FoqSettings,
}

impl Worker {
    /// Start the background executor.
    pub fn new() -> anyhow::Result<Self> {
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
            client,
            settings,
        })
    }

    /// The client of the local decision model, rebuilt when the settings
    /// change.
    fn client(&self, settings: &FoqSettings) -> Arc<FoqClient> {
        if &self.settings == settings {
            return Arc::clone(&self.client);
        }
        Arc::new(settings.client())
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
    /// The statistics of the commits that were listed before are read from the
    /// store on the caller's thread and handed to the background thread: a
    /// fork that did not move since the last synchronisation lists its pending
    /// commits without computing a single diff. The store itself is not shared
    /// — a SQLite connection is bound to one thread.
    pub fn sync_repo(&self, repo: Repo, known: HashMap<String, CommitStats>) {
        let sender = self.sender.clone();
        self.runtime.spawn_blocking(move || {
            let repo_id = repo.id;
            let started = Instant::now();
            match catalog::sync_repo(&repo, &known) {
                Ok(commits) => {
                    tracing::info!(
                        repo_id,
                        commits = commits.len(),
                        elapsed_ms = started.elapsed().as_millis(),
                        "sync finished"
                    );
                    let _ = sender.send(WorkerMessage::Synced { repo_id, commits });
                }
                Err(error) => {
                    tracing::warn!(
                        repo_id,
                        elapsed_ms = started.elapsed().as_millis(),
                        %error,
                        "sync failed"
                    );
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
    pub fn run_pipeline(&self, request: PipelineRequest) {
        let sender = self.sender.clone();
        self.runtime.spawn(async move {
            let repo_path = request.repo_path.clone();
            let sha = request.commit.sha.clone();
            let patch =
                tokio::task::spawn_blocking(move || patch_of(&repo_path, &sha).unwrap_or_default())
                    .await
                    .unwrap_or_default();
            let request = PipelineRequest { patch, ..request };

            let (events, receiver) = tokio::sync::mpsc::unbounded_channel::<RunEvent>();
            let forwarder = Self::forward_events(&sender, receiver);
            let outcome = fsrs_acp::run_pipeline(request, events).await;
            let _ = forwarder.await;
            let _ = sender.send(WorkerMessage::PipelineFinished {
                outcome: outcome.map_err(|error| error.to_string()),
            });
        });
    }

    /// Forward the events of the pipeline to the interface.
    async fn forward_events(
        sender: &Sender<WorkerMessage>,
        mut receiver: tokio::sync::mpsc::UnboundedReceiver<RunEvent>,
    ) {
        while let Some(event) = receiver.recv().await {
            let _ = sender.send(WorkerMessage::PipelineEvent { event });
        }
    }
}

/// Describe an agent error for the user interface.
fn describe(error: &AcpError) -> String {
    error.to_string()
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
        worker.sync_repo(repo, Default::default());
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
}
