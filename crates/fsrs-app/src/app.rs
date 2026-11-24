//! The dashboard itself.
//!
//! The window is built from four areas:
//!
//! * a header with the state of the dashboard,
//! * the central list of the tracked repositories with the number of the
//!   updates that are waiting,
//! * the list of the upstream commits of the selected repository, with the
//!   criticality on the left, the description in the middle, the estimated
//!   complexity on the right and the button that starts the implementation,
//! * a terminal on the right that runs the OpenCode CLI, and a log of the
//!   pipeline at the bottom.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Instant;

use fsrs_acp::{ModelChoice, PipelineRequest};
use fsrs_core::{
    Assessment, Complexity, Config, Criticality, Repo, Run, RunState, Store, UpstreamCommit,
    now_unix,
};

use crate::state::{
    ModelPicker, PickerIntent, PipelineLog, RepoView, complexity_color, criticality_color, plural,
    run_state_for,
};
use crate::terminal::TerminalPanel;
use crate::worker::{FoqSettings, Worker, WorkerMessage};

/// The moment the process was started.
///
/// The time from the start to the first frame is what the user waits for
/// after the icon is clicked, so it is measured from the first line of the
/// program and not from the creation of the window.
static PROCESS_STARTED: OnceLock<Instant> = OnceLock::new();

/// The moment the process was started.
pub fn process_started() -> Instant {
    *PROCESS_STARTED.get_or_init(Instant::now)
}

/// How many commits are rated by the local model at the same time.
///
/// The decision model is a single process on a single GPU, so the dashboard
/// keeps the queue short instead of flooding it with requests.
const MAX_CONCURRENT_ASSESSMENTS: usize = 2;

/// The parts of a frame that are measured separately.
///
/// The order is the order in which the parts are drawn.
const FRAME_PARTS: usize = 6;

/// The height of one row of the table of the commits.
///
/// The rows of the table are drawn in a fixed height, so that the list can
/// be scrolled without knowing the height of every row: the list of the
/// pending commits of a fork is long, and only the rows that are visible are
/// drawn.
const ROW_HEIGHT: f32 = 48.0;

/// The height of the row of the names of the columns.
const HEADER_HEIGHT: f32 = 20.0;

/// The width of the column of a badge.
///
/// The column is wide enough for the name of the column itself, so that the
/// name stands above the badge.
const BADGE_COLUMN: f32 = 92.0;

/// The width of the column of the button.
const ACTION_COLUMN: f32 = 140.0;

/// The widths of the columns of the table of the commits.
///
/// The header of the table and its rows use the same widths, so that the
/// names of the columns stand above the columns themselves.
#[derive(Debug, Clone, Copy)]
struct CommitColumns {
    /// The column of the criticality.
    criticality: f32,
    /// The column of the description.
    description: f32,
    /// The column of the complexity.
    complexity: f32,
    /// The column of the button.
    action: f32,
    /// The gap between the columns.
    spacing: f32,
}

impl CommitColumns {
    /// The whole width of the table.
    fn total(self) -> f32 {
        self.criticality + self.description + self.complexity + self.action + self.spacing * 3.0
    }
}

/// Compute the widths of the columns of the table of the commits.
///
/// The description takes the rest of the width of the window, and never less
/// than a hundred and twenty points: a narrow window still shows the text of
/// the commit. The gaps between the columns are counted, so that the table
/// fits the window exactly and does not widen it.
fn commit_columns(ui: &egui::Ui, width: f32) -> CommitColumns {
    let spacing = ui.spacing().item_spacing.x;
    let fixed = BADGE_COLUMN * 2.0 + ACTION_COLUMN + spacing * 3.0;
    CommitColumns {
        criticality: BADGE_COLUMN,
        description: (width - fixed).max(120.0),
        complexity: BADGE_COLUMN,
        action: ACTION_COLUMN,
        spacing,
    }
}

/// Draw the name of one column of the table of the commits.
///
/// The column keeps the width it was given, even when the name of it is
/// narrower: the names of the columns have to stand above the very columns
/// of the rows.
fn commit_column(ui: &mut egui::Ui, width: f32, name: &str) {
    ui.allocate_ui_with_layout(
        egui::vec2(width, HEADER_HEIGHT),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.set_min_width(width);
            ui.label(egui::RichText::new(name));
        },
    );
}

/// The number of the ratings that may be in flight at the same time.
///
/// The server of the model serves one request at a time per slot, so the
/// number of the slots is the real limit. A server that reports no slots is
/// assumed to have one: a request that waits in the queue of the server waits
/// for the model anyway.
fn concurrent_assessment_limit(slots: usize) -> usize {
    MAX_CONCURRENT_ASSESSMENTS.min(slots.max(1))
}

/// One row of the prepared screenshot.
///
/// The rows carry the ratings of the local model, so that the picture of the
/// documentation shows the window with the data it shows in real life.
#[derive(Debug, Clone)]
pub struct ScreenshotRow {
    /// Summary of the commit.
    pub summary: String,
    /// Author of the commit.
    pub author: String,
    /// Criticality of the commit.
    pub criticality: Criticality,
    /// Complexity of the port.
    pub complexity: Complexity,
}

/// What the dashboard has to do once the user picked a model.
#[derive(Debug, Clone)]
enum PendingAction {
    /// Open a terminal with the OpenCode CLI.
    OpenTerminal { repo: usize, sha: String },
    /// Start the implementation pipeline.
    RunPipeline { repo: usize, sha: String },
}

/// Something the user clicked.
#[derive(Debug, Clone)]
enum Action {
    /// Select a repository.
    SelectRepo(usize),
    /// Open the terminal for a commit.
    OpenTerminal { repo: usize, sha: String },
    /// Start the implementation of a commit.
    Implement { repo: usize, sha: String },
    /// Close the terminal.
    CloseTerminal,
    /// Synchronise a repository again.
    Resync(usize),
    /// Rate the commits that are still unrated.
    AssessAll(usize),
}

/// The dashboard.
pub struct FsrsApp {
    /// The configuration of the user.
    config: Config,
    /// The address of the configuration, ready to be shown.
    ///
    /// The address is shown in the header of the window in every frame, and
    /// it does not change while the program runs.
    config_label: String,
    /// The persistent state.
    store: Store,
    /// The background executor.
    worker: Worker,
    /// The tracked repositories.
    repos: Vec<RepoView>,
    /// The repository whose commits are shown.
    selected_repo: Option<usize>,
    /// The commit the user picked last.
    selected_commit: Option<String>,
    /// The models the agent offers.
    models: Vec<ModelChoice>,
    /// The model the user picked.
    selected_model: Option<String>,
    /// The open model picker.
    picker: Option<ModelPicker>,
    /// What has to happen after the model was picked.
    pending: Option<PendingAction>,
    /// The terminal that runs the OpenCode CLI.
    terminal: Option<TerminalPanel>,
    /// The title of the terminal.
    terminal_title: String,
    /// The log of the running pipeline.
    log: Vec<PipelineLog>,
    /// The run of the dashboard in the database.
    run_id: Option<i64>,
    /// The state of that run.
    run_state: Option<RunState>,
    /// The commits that still wait for their assessment.
    assessment_queue: VecDeque<(usize, String)>,
    /// The assessments that are currently running.
    assessment_in_flight: usize,
    /// The number of the slots of the decision server.
    ///
    /// The server of the model serves one request at a time per slot. The
    /// number is asked once, in the background, and bounds the number of the
    /// ratings that are in flight: a request that waits in the queue of the
    /// server waits for the model anyway.
    assessment_slots: usize,
    /// The last message for the user.
    status: String,
    /// The last error for the user.
    error: Option<String>,
    /// Whether the window shows prepared data instead of the live state.
    ///
    /// The prepared data is used for the screenshot of the documentation: it
    /// must survive the messages of the background worker.
    prepared: bool,
    /// When the process was started.
    ///
    /// The time from the start to the first frame is what the user waits for
    /// after the icon is clicked, so it is measured once.
    started: Instant,
    /// Whether the first frame was already drawn.
    first_frame_drawn: bool,
    /// The times of the last frames, in microseconds.
    ///
    /// The time of a frame is what the user feels: the window is drawn again
    /// and again while the pipeline runs, and every one of those frames costs
    /// the processor. The times of the last frames are kept, so that the cost
    /// of the window can be measured instead of guessed.
    frame_times: VecDeque<u64>,
    /// The time of every part of the last frames, in microseconds.
    ///
    /// The parts are the header, the list of the repositories, the terminal,
    /// the log, the table of the commits and the dialog of the model. The
    /// time of every part is measured separately, so that the expensive one
    /// is known instead of guessed.
    frame_parts: VecDeque<[u64; FRAME_PARTS]>,
    /// How many frames were drawn since the program started.
    frames_drawn: u64,
    /// Whether the window asks for a new frame as soon as it is ready.
    ///
    /// The program draws a frame only when something changed. The
    /// measurement of the frames needs frames that follow each other, so the
    /// environment variable `FSRS_CONTINUOUS_REPAINT` turns this on.
    continuous_repaint: bool,
}

impl FsrsApp {
    /// Build the dashboard and start the first synchronisation.
    pub fn new(config: Config, config_path: PathBuf, store: Store, worker: Worker) -> Self {
        Self::with_discovery(config, config_path, store, worker, true)
    }

    /// Build the dashboard without asking the agent for its models.
    ///
    /// The tests of the window use this: they put the models into the
    /// dashboard themselves and do not want to start an agent.
    pub fn without_discovery(
        config: Config,
        config_path: PathBuf,
        store: Store,
        worker: Worker,
    ) -> Self {
        Self::with_discovery(config, config_path, store, worker, false)
    }

    /// Build the dashboard.
    fn with_discovery(
        config: Config,
        config_path: PathBuf,
        store: Store,
        worker: Worker,
        discovery: bool,
    ) -> Self {
        let mut app = Self {
            config,
            config_label: config_path.display().to_string(),
            store,
            worker,
            repos: Vec::new(),
            selected_repo: None,
            selected_commit: None,
            models: Vec::new(),
            selected_model: None,
            picker: None,
            pending: None,
            terminal: None,
            terminal_title: String::new(),
            log: Vec::new(),
            run_id: None,
            run_state: None,
            assessment_queue: VecDeque::new(),
            assessment_in_flight: 0,
            assessment_slots: 1,
            status: String::new(),
            error: None,
            prepared: false,
            started: process_started(),
            first_frame_drawn: false,
            frame_times: VecDeque::new(),
            frame_parts: VecDeque::new(),
            frames_drawn: 0,
            continuous_repaint: std::env::var_os("FSRS_CONTINUOUS_REPAINT").is_some(),
        };
        app.load_repositories();
        if discovery {
            app.refresh_models();
        }
        // The number of the slots of the decision server bounds the number of
        // the ratings that are in flight. It is asked in the background, so
        // the window does not wait for the server.
        let settings = app.foq_settings();
        app.worker.probe_slots(settings);
        app
    }

    // ------------------------------------------------------------------
    // Accessors
    // ------------------------------------------------------------------

    /// The repositories the dashboard shows.
    pub fn repos(&self) -> &[RepoView] {
        &self.repos
    }

    /// The index of the repository whose commits are shown.
    pub fn selected_repo(&self) -> Option<usize> {
        self.selected_repo
    }

    /// The commit the user picked last.
    pub fn selected_commit(&self) -> Option<&str> {
        self.selected_commit.as_deref()
    }

    /// The models the agent offers.
    pub fn models(&self) -> &[ModelChoice] {
        &self.models
    }

    /// The model the user picked.
    pub fn selected_model(&self) -> Option<&str> {
        self.selected_model.as_deref()
    }

    /// The open model picker, if there is one.
    pub fn picker(&self) -> Option<&ModelPicker> {
        self.picker.as_ref()
    }

    /// The state of the last implementation run.
    pub fn run_state(&self) -> Option<RunState> {
        self.run_state
    }

    /// The log of the pipeline.
    pub fn log(&self) -> &[PipelineLog] {
        &self.log
    }

    /// The last message for the user.
    pub fn status(&self) -> &str {
        &self.status
    }

    /// The last error for the user.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Whether the dashboard is waiting for something.
    pub fn busy(&self) -> bool {
        self.is_busy()
    }

    // ------------------------------------------------------------------
    // Startup
    // ------------------------------------------------------------------

    /// Fill the repository list from the configuration and the database.
    fn load_repositories(&mut self) {
        let configured = self.config.repos.clone();
        let mut views = Vec::new();
        let mut known = Vec::new();

        for entry in configured {
            let path = entry.path.clone();
            let name = if entry.name.trim().is_empty() {
                path.file_name()
                    .map(|name| name.to_string_lossy().to_string())
                    .unwrap_or_else(|| path.to_string_lossy().to_string())
            } else {
                entry.name.clone()
            };
            let repo = Repo {
                id: 0,
                name,
                path: path.clone(),
                upstream_url: entry.upstream_url.clone(),
                upstream_web: entry.upstream_web.clone(),
                main_branch: entry.main_branch.clone(),
                pending_commits: 0,
                last_fetched_at: None,
            };
            let id = match self.store.upsert_repo(&repo) {
                Ok(id) => id,
                Err(error) => {
                    self.error = Some(format!("{}: {error}", path.display()));
                    continue;
                }
            };
            let mut view = RepoView::new(Repo { id, ..repo });
            if let Ok(commits) = self.store.list_commits(id) {
                view.set_commits(commits);
            }
            if let Ok(assessments) = self.store.list_assessments(id) {
                for assessment in assessments {
                    view.insert_assessment(assessment);
                }
            }
            view.syncing = true;
            // The statistics of the commits are read out of the commits that
            // were just loaded: the database is not asked for them a second
            // time.
            known.push(fsrs_core::git::stats_of(view.commits()));
            views.push(view);
        }

        self.repos = views;
        self.selected_repo = if self.repos.is_empty() { None } else { Some(0) };
        for (view, stats) in self.repos.iter().zip(known) {
            self.worker.sync_repo(view.repo.clone(), Some(stats));
        }
    }

    /// Ask the agent which models it offers.
    fn refresh_models(&mut self) {
        let root = self
            .repos
            .first()
            .map(|view| view.repo.path.clone())
            .unwrap_or_else(|| PathBuf::from("."));
        self.worker
            .discover_models(self.config.acp_command.clone(), root);
    }

    // ------------------------------------------------------------------
    // Background messages
    // ------------------------------------------------------------------

    /// Apply every message the background produced.
    ///
    /// The answer tells whether the state of the window changed: the window
    /// asks for a frame when something happened, and not otherwise. A
    /// message that does not change the number of the tasks that are in
    /// flight still changes the picture on the screen — the number of the
    /// slots of the server, for example — and the frame has to be drawn.
    fn handle_messages(&mut self) -> bool {
        let mut changed = false;
        for message in self.worker.drain() {
            if self.prepared {
                // The window shows prepared data; the live state of the
                // background must not replace it.
                continue;
            }
            changed = true;
            match message {
                WorkerMessage::Synced { repo_id, commits } => self.on_synced(repo_id, commits),
                WorkerMessage::SyncFailed { repo_id, error } => self.on_sync_failed(repo_id, error),
                WorkerMessage::Assessed { assessment } => self.on_assessed(assessment),
                WorkerMessage::AssessmentFailed {
                    repo_id,
                    sha,
                    error,
                } => {
                    self.assessment_in_flight = self.assessment_in_flight.saturating_sub(1);
                    self.error = Some(format!("оценка {sha}: {error}"));
                    let _ = repo_id;
                }
                WorkerMessage::SlotsKnown { slots } => self.assessment_slots = slots.max(1),
                WorkerMessage::ModelsLoaded { models } => {
                    self.models = models;
                    if let Some(picker) = self.picker.as_mut() {
                        picker.loading = false;
                        picker.error = None;
                    }
                }
                WorkerMessage::ModelsFailed { error } => {
                    if let Some(picker) = self.picker.as_mut() {
                        picker.loading = false;
                        picker.error = Some(error.clone());
                    }
                    self.error = Some(error);
                }
                WorkerMessage::PipelineEvent { event } => self.on_pipeline_event(event),
                WorkerMessage::PipelineFinished { outcome } => self.on_pipeline_finished(outcome),
            }
        }
        changed
    }

    /// The upstream branch was fetched.
    fn on_synced(&mut self, repo_id: i64, commits: Vec<UpstreamCommit>) {
        let Some(index) = self.repos.iter().position(|view| view.repo.id == repo_id) else {
            return;
        };
        {
            let view = &mut self.repos[index];
            view.syncing = false;
            view.error = None;
            view.set_commits(commits);
        }
        let count = self.repos[index].commits().len();
        let mut pending = Vec::new();
        for commit in self.repos[index].commits() {
            if self.repos[index].assessment(&commit.sha).is_none() {
                pending.push(commit.sha.clone());
            }
        }
        for sha in pending {
            self.assessment_queue.push_back((index, sha));
        }
        self.status = format!("{}: обновлений {}", self.repos[index].repo.name, count);
    }

    /// The synchronisation failed.
    fn on_sync_failed(&mut self, repo_id: i64, error: String) {
        if let Some(view) = self.repos.iter_mut().find(|view| view.repo.id == repo_id) {
            view.syncing = false;
            view.error = Some(error.clone());
        }
        self.error = Some(error);
    }

    /// Fill the window with prepared data, so that it can be shown without a
    /// repository and without the decision model.
    ///
    /// This is what the screenshot of the documentation uses: the window is
    /// the real one, only the data is prepared. Every row carries the summary
    /// of the commit, the author and the two ratings.
    ///
    /// The prepared data survives the messages of the background worker, so
    /// that a failed synchronisation does not empty the window again.
    pub fn prepare_screenshot(&mut self, rows: &[ScreenshotRow]) {
        self.prepared = true;
        self.fill_screenshot(rows);
    }

    /// Put the prepared rows into the window.
    fn fill_screenshot(&mut self, rows: &[ScreenshotRow]) {
        let Some(index) = self.selected_repo else {
            return;
        };
        let Some(view) = self.repos.get_mut(index) else {
            return;
        };
        view.clear();
        view.syncing = false;
        // The prepared data replaces the result of the synchronisation, so the
        // error of the failed attempt is not shown any more.
        view.error = None;
        self.error = None;
        let repo_id = view.repo.id;
        let mut commits = Vec::with_capacity(rows.len());
        let mut assessments = Vec::with_capacity(rows.len());
        for (number, row) in rows.iter().enumerate() {
            // The object ids of the real commits, so that the picture looks
            // like the window of a real repository.
            const SEEDS: [&str; 5] = [
                "39886c0cb34453cda21163df0c0c76fc9c011a21",
                "b2a287b3993768e0ec52f0852916917f23fb2a38",
                "e5849a085aa56ca914333ab226de7a9b4fbfabb2",
                "edc62af8ada7409ad7eff5473069382d5f6da6a6",
                "3a8edbecfd8d1f8ffec390cf3993037016d64468",
            ];
            let sha = SEEDS[number % SEEDS.len()].to_string();
            let short_sha = sha[..8].to_string();
            commits.push(UpstreamCommit {
                repo_id,
                sha: sha.clone(),
                short_sha: short_sha.clone(),
                summary: row.summary.clone(),
                body: String::new(),
                author: row.author.clone(),
                author_email: String::new(),
                authored_at: 1_762_280_000,
                committed_at: 1_762_280_000,
                files: vec![
                    "src/cli.rs".to_string(),
                    "src/config.rs".to_string(),
                    "tests/tests.rs".to_string(),
                ],
                insertions: 42,
                deletions: 17,
                url: format!("https://example.com/commit/{sha}"),
            });
            assessments.push(Assessment {
                repo_id,
                sha,
                criticality: row.criticality,
                complexity: row.complexity,
                criticality_confidence: 0.46,
                complexity_confidence: 0.48,
                needs_review: true,
                rationale: String::new(),
                model: "foq".to_string(),
                created_at: 1_762_280_000,
            });
        }
        view.set_commits(commits);
        for assessment in assessments {
            view.insert_assessment(assessment);
        }
        view.repo.pending_commits = rows.len() as i64;
        self.status = format!("обновлений: {}", rows.len());
    }

    /// The local model rated one commit.
    ///
    /// The assessment is written to the database by the background, which
    /// produced it: the window does not wait for a write of the disk.
    fn on_assessed(&mut self, assessment: fsrs_core::Assessment) {
        self.assessment_in_flight = self.assessment_in_flight.saturating_sub(1);
        if let Some(view) = self
            .repos
            .iter_mut()
            .find(|view| view.repo.id == assessment.repo_id)
        {
            view.insert_assessment(assessment);
        }
    }

    /// The pipeline reported a step.
    fn on_pipeline_event(&mut self, event: fsrs_acp::RunEvent) {
        self.log.push(PipelineLog::new(now_unix(), event.clone()));

        if let Some(state) = run_state_for(&event) {
            self.run_state = Some(state);
        }
    }

    /// The pipeline finished.
    fn on_pipeline_finished(&mut self, outcome: Result<fsrs_acp::PipelineOutcome, String>) {
        match outcome {
            Ok(result) => {
                self.run_state = Some(RunState::Merged);
                self.status = format!(
                    "реализация завершена: {} → {} ({})",
                    result.branch,
                    result.main_branch,
                    plural(result.cycles as usize, "цикл", "цикла", "циклов")
                );
                self.refresh_repositories();
            }
            Err(error) => {
                self.run_state = Some(RunState::Failed);
                self.error = Some(format!("реализация не удалась: {error}"));
            }
        }
    }

    /// Synchronise every repository again.
    fn refresh_repositories(&mut self) {
        for view in &mut self.repos {
            view.syncing = true;
        }
        for view in &self.repos {
            // The statistics of the commits are kept by the worker: they are
            // handed to the background thread from there.
            self.worker.sync_repo(view.repo.clone(), None);
        }
    }

    /// Send the queued commits to the local decision model.
    fn dispatch_assessments(&mut self) {
        while self.assessment_in_flight < self.max_concurrent_assessments() {
            let Some((index, sha)) = self.assessment_queue.pop_front() else {
                break;
            };
            let Some(view) = self.repos.get(index) else {
                continue;
            };
            if view.assessment(&sha).is_some() {
                continue;
            }
            // The commit is found by its position, and not by a search
            // through the list: the list of the pending commits is long, and
            // every commit of it is looked up once.
            let Some(commit) = view.commit(&sha).cloned() else {
                continue;
            };
            let path = view.repo.path.clone();
            let settings = self.foq_settings();
            self.worker.assess(settings, path, commit);
            self.assessment_in_flight += 1;
        }
    }

    /// The number of the ratings that may be in flight at the same time.
    ///
    /// The limit is the number of the slots of the server, and never more
    /// than the constant of the program: a server with one slot is asked one
    /// question at a time, because the questions that wait in its queue wait
    /// for the model anyway.
    fn max_concurrent_assessments(&self) -> usize {
        concurrent_assessment_limit(self.assessment_slots)
    }

    /// The settings of the local decision model.
    fn foq_settings(&self) -> FoqSettings {
        FoqSettings {
            url: self.config.foq_url.clone(),
            model: self.config.foq_model.clone(),
            profile: self.config.foq_profile.into(),
            min_confidence: self.config.min_confidence,
            timeout: std::time::Duration::from_secs(self.config.foq_timeout_secs),
        }
    }

    // ------------------------------------------------------------------
    // Actions
    // ------------------------------------------------------------------

    /// Apply the action the user triggered.
    fn apply(&mut self, action: Action) {
        match action {
            Action::SelectRepo(index) => {
                self.selected_repo = Some(index);
                self.selected_commit = None;
            }
            Action::OpenTerminal { repo, sha } => {
                self.selected_commit = Some(sha.clone());
                self.pending = Some(PendingAction::OpenTerminal {
                    repo,
                    sha: sha.clone(),
                });
                self.open_picker(PickerIntent::Terminal);
            }
            Action::Implement { repo, sha } => {
                self.selected_commit = Some(sha.clone());
                self.pending = Some(PendingAction::RunPipeline {
                    repo,
                    sha: sha.clone(),
                });
                match self.selected_model.clone() {
                    Some(model) => self.start_pipeline(repo, &sha, &model),
                    None => self.open_picker(PickerIntent::Pipeline),
                }
            }
            Action::CloseTerminal => {
                self.terminal = None;
                self.terminal_title.clear();
            }
            Action::Resync(index) => {
                if let Some(view) = self.repos.get(index) {
                    let repo = view.repo.clone();
                    self.repos[index].syncing = true;
                    // The statistics of the commits are kept by the worker.
                    self.worker.sync_repo(repo, None);
                }
            }
            Action::AssessAll(index) => {
                if let Some(view) = self.repos.get(index) {
                    for commit in view.commits() {
                        if view.assessment(&commit.sha).is_none() {
                            self.assessment_queue.push_back((index, commit.sha.clone()));
                        }
                    }
                }
            }
        }
    }

    /// Open the dialog that asks the user for a model.
    fn open_picker(&mut self, intent: PickerIntent) {
        self.picker = Some(ModelPicker::new(intent, self.selected_model.clone()));
        let root = self
            .repos
            .first()
            .map(|view| view.repo.path.clone())
            .unwrap_or_else(|| PathBuf::from("."));
        self.worker
            .discover_models(self.config.acp_command.clone(), root);
    }

    /// The user picked a model: do what the dialog was opened for.
    fn confirm_picker(&mut self, model: String) {
        self.selected_model = Some(model.clone());
        self.picker = None;
        match self.pending.take() {
            Some(PendingAction::OpenTerminal { repo, sha }) => {
                self.open_terminal(repo, &sha, &model)
            }
            Some(PendingAction::RunPipeline { repo, sha }) => {
                self.start_pipeline(repo, &sha, &model)
            }
            None => {}
        }
    }

    /// Open the terminal that runs the OpenCode CLI.
    fn open_terminal(&mut self, repo: usize, sha: &str, model: &str) {
        let Some(view) = self.repos.get(repo) else {
            return;
        };
        let cwd = view.repo.path.clone();
        let short = sha.chars().take(8).collect::<String>();
        let title = format!("{} · {short}", view.repo.name);

        let (program, mut arguments) = split_command(&self.config.terminal_command);
        arguments.push("--model".to_string());
        arguments.push(model.to_string());

        match TerminalPanel::spawn(&program, &arguments, &cwd, &title) {
            Ok(panel) => {
                self.terminal = Some(panel);
                self.terminal_title = title;
                self.status = format!("терминал открыт: {program} --model {model}");
                self.error = None;
            }
            Err(error) => {
                self.error = Some(format!("терминал не запущен: {error}"));
            }
        }
    }

    /// Start the three-agent pipeline for one commit.
    fn start_pipeline(&mut self, repo: usize, sha: &str, model: &str) {
        let Some(view) = self.repos.get(repo) else {
            return;
        };
        let Some(commit) = view.commit(sha).cloned() else {
            return;
        };
        let tracked = view.repo.clone();

        let request = PipelineRequest::new(
            tracked.path.clone(),
            commit.clone(),
            String::new(),
            tracked.main_branch.clone(),
            model,
        )
        .with_agent_command(self.config.acp_command.clone());

        let run = Run {
            id: 0,
            repo_id: tracked.id,
            sha: sha.to_string(),
            branch: request.branch.clone(),
            state: RunState::Prepared,
            cycles: 0,
            model: model.to_string(),
            created_at: now_unix(),
            updated_at: now_unix(),
        };
        let run_id = match self.store.insert_run(&run) {
            Ok(id) => {
                self.run_id = Some(id);
                self.run_state = Some(RunState::Prepared);
                id
            }
            Err(error) => {
                self.error = Some(format!("запуск не сохранён: {error}"));
                return;
            }
        };

        self.log.clear();
        self.status = format!(
            "реализация {} началась на {}",
            short_sha(sha),
            request.branch
        );
        self.error = None;
        self.worker.run_pipeline(request, run_id);
    }
}

/// Split a command line into the program and its arguments.
pub fn split_command(command: &str) -> (String, Vec<String>) {
    let mut parts = command.split_whitespace();
    let program = parts.next().unwrap_or("opencode").to_string();
    (program, parts.map(str::to_string).collect())
}

/// The label of the model the user picked.
///
/// The label is borrowed from the list of the models, so that the header of
/// the window does not copy it in every frame.
fn model_label<'a>(models: &'a [ModelChoice], selected: Option<&'a str>) -> &'a str {
    let Some(id) = selected else {
        return "не выбрана";
    };
    models
        .iter()
        .find(|model| model.id == id)
        .map(|model| model.label())
        .unwrap_or(id)
}

/// The label of the state of the last run.
///
/// The label is a constant of the program, so that the header of the window
/// does not format the state in every frame.
fn run_state_label(state: Option<RunState>) -> &'static str {
    match state {
        None => "—",
        Some(RunState::Prepared) => "Prepared",
        Some(RunState::Implementing) => "Implementing",
        Some(RunState::Reviewing) => "Reviewing",
        Some(RunState::Verifying) => "Verifying",
        Some(RunState::Rework) => "Rework",
        Some(RunState::Approved) => "Approved",
        Some(RunState::Merged) => "Merged",
        Some(RunState::Failed) => "Failed",
    }
}

/// The short form of a commit id.
fn short_sha(sha: &str) -> String {
    sha.chars().take(8).collect()
}

// ----------------------------------------------------------------------
// The interface
// ----------------------------------------------------------------------

impl eframe::App for FsrsApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let started = Instant::now();
        if !self.first_frame_drawn {
            self.first_frame_drawn = true;
            tracing::info!(
                elapsed_ms = self.started.elapsed().as_millis(),
                "the first frame of the window was drawn"
            );
        }
        let before = self.work_in_flight();
        let changed = self.handle_messages();
        self.dispatch_assessments();

        // Every part of the frame is measured on its own: the table of the
        // commits is the part that grows with the size of the backlog, and the
        // numbers say where the time of a frame goes.
        let mut parts = [0u64; FRAME_PARTS];
        let mark = Instant::now();
        self.header(ui);
        parts[0] = mark.elapsed().as_micros() as u64;
        let mark = Instant::now();
        self.repository_strip(ui);
        parts[1] = mark.elapsed().as_micros() as u64;
        let mark = Instant::now();
        self.terminal_panel(ui);
        parts[2] = mark.elapsed().as_micros() as u64;
        let mark = Instant::now();
        self.log_panel(ui);
        parts[3] = mark.elapsed().as_micros() as u64;
        let mark = Instant::now();
        self.commit_table(ui);
        parts[4] = mark.elapsed().as_micros() as u64;
        let mark = Instant::now();
        self.model_picker(ui.ctx());
        parts[5] = mark.elapsed().as_micros() as u64;

        // A frame is drawn only when something changed: the background moved
        // forward, the user clicked, or the terminal produced output (the
        // terminal widget asks for its own frames). While the dashboard is
        // idle the CPU is not spun at all.
        if self.continuous_repaint {
            // The measurement of the frames needs frames that follow each
            // other.
            ui.ctx().request_repaint();
        } else if changed || self.work_in_flight() != before {
            // The background moved forward: the window has to show it.
            ui.ctx().request_repaint();
        } else if self.is_busy() {
            // Busy without new data (the model reads the diff): poll slowly
            // instead of as fast as the window server allows.
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(250));
        }
        self.record_frame(started.elapsed(), parts);
    }
}

impl FsrsApp {
    /// Whether the dashboard is waiting for something.
    fn is_busy(&self) -> bool {
        self.assessment_in_flight > 0
            || !self.assessment_queue.is_empty()
            || self.repos.iter().any(|view| view.syncing)
            || self.run_state.is_some_and(RunState::is_active)
    }

    /// Remember how long the frame took and report it.
    ///
    /// The times of the last frames are kept, and every few hundred frames
    /// the median, the ninety-fifth percentile and the slowest frame are
    /// written to the log. The number of the frames and the number of the
    /// commits that are shown are in the same line, so the cost of a frame
    /// can be read together with the work it did.
    ///
    /// The time of every part of the frame is reported with it: the part that
    /// takes the time is the one that has to be made cheaper.
    fn record_frame(&mut self, elapsed: std::time::Duration, parts: [u64; FRAME_PARTS]) {
        const WINDOW: usize = 512;
        const REPORT_EVERY: u64 = 256;

        self.frames_drawn += 1;
        self.frame_times.push_back(elapsed.as_micros() as u64);
        self.frame_parts.push_back(parts);
        while self.frame_times.len() > WINDOW {
            self.frame_times.pop_front();
            self.frame_parts.pop_front();
        }
        if !self.frames_drawn.is_multiple_of(REPORT_EVERY) {
            return;
        }
        let mut times: Vec<u64> = self.frame_times.iter().copied().collect();
        times.sort_unstable();
        let median = times[times.len() / 2];
        let p95 = times[times.len() * 95 / 100];
        let slowest = times[times.len() - 1];
        // The median of every part of the frame.
        let mut parts_median = [0u64; FRAME_PARTS];
        for (index, part) in parts_median.iter_mut().enumerate() {
            let mut values: Vec<u64> = self.frame_parts.iter().map(|parts| parts[index]).collect();
            values.sort_unstable();
            *part = values[values.len() / 2];
        }
        let commits = self
            .selected_repo
            .and_then(|index| self.repos.get(index))
            .map_or(0, |view| view.commits().len());
        tracing::info!(
            frames = self.frames_drawn,
            commits,
            median_us = median,
            p95_us = p95,
            max_us = slowest,
            header_us = parts_median[0],
            repositories_us = parts_median[1],
            terminal_us = parts_median[2],
            log_us = parts_median[3],
            commits_us = parts_median[4],
            picker_us = parts_median[5],
            "the frames of the window"
        );
    }

    /// A number that changes whenever the background state of the window moves.
    ///
    /// The frame before the background is drained is compared with the one
    /// after, so a frame is drawn only when the work of the background has
    /// actually changed what the window shows.
    fn work_in_flight(&self) -> usize {
        self.assessment_in_flight
            + self.assessment_queue.len()
            + self.repos.iter().filter(|view| view.syncing).count()
            + self.log.len()
            + usize::from(self.run_state.is_some_and(RunState::is_active))
    }

    /// The header of the window.
    ///
    /// The texts of the header are borrowed, and not copied: the header is
    /// drawn in every frame, and a copy of the status and of the address of
    /// the configuration is an allocation in every frame.
    fn header(&mut self, ui: &mut egui::Ui) {
        let FsrsApp {
            status,
            error,
            run_state,
            models,
            selected_model,
            config_label,
            ..
        } = self;
        let model = model_label(models, selected_model.as_deref());
        let run = run_state_label(*run_state);
        let status = status.as_str();
        let config_label = config_label.as_str();

        egui::Panel::top("header")
            .min_size(28.0)
            .max_size(80.0)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("fs-rs").size(16.0));
                    ui.separator();
                    ui.label(format!("Модель: {model}"));
                    ui.separator();
                    ui.label(format!("Реализация: {run}"));
                    ui.separator();
                    ui.label(status);
                    ui.separator();
                    ui.label(egui::RichText::new(config_label).small().monospace());
                    if let Some(error) = error {
                        ui.label(
                            egui::RichText::new(error.clone()).small().weak(),
                        );
                    }
                });
            });
    }

    /// The central list of the tracked repositories.
    ///
    /// The names and the addresses of the repositories are borrowed, and not
    /// copied: the strip is drawn in every frame, and a copy of the address
    /// of every repository is an allocation in every frame.
    fn repository_strip(&mut self, ui: &mut egui::Ui) {
        let mut action: Option<Action> = None;
        let FsrsApp {
            repos,
            selected_repo,
            ..
        } = self;

        egui::Panel::top("repositories")
            .min_size(80.0)
            .max_size(240.0)
            .show(ui, |ui| {
                egui::ScrollArea::horizontal().show(ui, |ui| {
                    ui.horizontal(|ui| {
                        for (index, view) in repos.iter().enumerate() {
                            let selected = *selected_repo == Some(index);
                            let name = &view.repo.name;
                            let path = view.repo.path.to_string_lossy();
                            let pending = view.pending();
                            let unassessed = view.unassessed();
                            let syncing = view.syncing;

                            egui::Frame::default()
                                .inner_margin(8.0)
                                .show(ui, |ui| {
                                    ui.set_min_width(240.0);
                                    if ui
                                        .selectable_label(
                                            selected,
                                            egui::RichText::new(name).size(15.0),
                                        )
                                        .clicked()
                                    {
                                        action = Some(Action::SelectRepo(index));
                                    }
                                    ui.label(
                                        egui::RichText::new(path.as_ref()).small().monospace(),
                                    );
                                    ui.horizontal(|ui| {
                                        if syncing {
                                            ui.spinner();
                                        }
                                        ui.label(plural(
                                            pending,
                                            "обновление",
                                            "обновления",
                                            "обновлений",
                                        ));
                                        if unassessed > 0 {
                                            ui.label(
                                                egui::RichText::new(format!(
                                                    "без оценки: {}",
                                                    plural(
                                                        unassessed,
                                                        "коммит",
                                                        "коммита",
                                                        "коммитов",
                                                    )
                                                ))
                                                .small(),
                                            );
                                        }
                                    });
                                    if let Some(error) = &view.error {
                                        ui.label(
                                            egui::RichText::new(error)
                                                .small()
                                                .color(ui.visuals().error_fg_color),
                                        );
                                    }
                                    ui.horizontal(|ui| {
                                        if ui.small_button("Обновить").clicked() {
                                            action = Some(Action::Resync(index));
                                        }
                                        if ui.small_button("Оценить").clicked() {
                                            action = Some(Action::AssessAll(index));
                                        }
                                    });
                                });
                        }
                        if repos.is_empty() {
                            ui.label(egui::RichText::new(
                                "Нет репозиториев. Добавьте их в ~/.config/fs-rs/config.toml",
                            ));
                        }
                    });
                });
            });

        if let Some(action) = action {
            self.apply(action);
        }
    }

    /// The terminal that runs the OpenCode CLI.
    fn terminal_panel(&mut self, ui: &mut egui::Ui) {
        if self.terminal.is_none() {
            return;
        }
        let title = self.terminal_title.clone();
        let mut close = false;

        egui::Panel::right("terminal")
            .resizable(true)
            .min_size(250.0)
            .max_size(800.0)
            .default_size(620.0)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(&title).size(16.0));
                    if let Some(terminal) = self.terminal.as_mut() {
                        if let Some(program_title) = terminal.title() {
                            ui.label(egui::RichText::new(program_title));
                        }
                        if terminal.has_exited() {
                            ui.label(egui::RichText::new("процесс завершился"));
                        }
                        if let Some(pid) = terminal.process_id() {
                            ui.label(
                                egui::RichText::new(format!("pid {pid}"))
                                    .small()
                                    .monospace(),
                            );
                        }
                        ui.label(
                            egui::RichText::new(terminal.command().to_string())
                                .small()
                                .weak()
                                .monospace(),
                        );
                    }
                    if ui.button("Закрыть").clicked() {
                        close = true;
                    }
                });
                ui.separator();
                if let Some(terminal) = self.terminal.as_mut() {
                    terminal.ui(ui);
                }
            });

        if close {
            self.apply(Action::CloseTerminal);
        }
    }

    /// The log of the pipeline.
    ///
    /// The strings of every line are built when the event arrives, so the
    /// panel only draws them.
    fn log_panel(&mut self, ui: &mut egui::Ui) {
        let FsrsApp { log, .. } = self;
        let count = log.len();

        egui::Panel::bottom("log")
            .resizable(true)
            .min_size(80.0)
            .default_size(220.0)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Журнал реализации").size(16.0));
                    ui.label(format!("событий: {count}"));
                });
                ui.separator();
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for entry in log.iter_mut() {
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(entry.time()).small().monospace());
                            ui.label(egui::RichText::new(entry.role_label()));
                            ui.label(entry.summary());
                        });
                        if !entry.prompt().is_empty() {
                            egui::CollapsingHeader::new("Видимый промпт")
                                .default_open(false)
                                .show(ui, |ui| {
                                    ui.add(
                                        egui::TextEdit::multiline(entry.prompt_mut())
                                            .desired_width(f32::INFINITY)
                                            .font(egui::TextStyle::Monospace)
                                            .interactive(true),
                                    );
                                });
                        }
                        ui.separator();
                    }
                });
            });
    }

    /// The list of the upstream commits of the selected repository.
    ///
    /// The table draws only the rows that are visible: a fork that is behind
    /// its upstream by a thousand commits has a thousand rows, and the window
    /// draws them again and again while the pipeline runs.
    fn commit_table(&mut self, ui: &mut egui::Ui) {
        let mut actions: Vec<Action> = Vec::new();

        let Some(index) = self.selected_repo else {
            egui::CentralPanel::default().show(ui, |ui| {
                ui.centered_and_justified(|ui| {
                    ui.vertical_centered(|ui| {
                        ui.label("Выберите репозиторий");
                        ui.label(egui::RichText::new("Кликните карточку репозитория в полосе выше").weak());
                    });
                });
            });
            return;
        };

        let title = &self.repos[index].repo.name;
        let count = self.repos[index].commits().len();

        egui::CentralPanel::default().show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(format!("{title} — коммиты ({count})")).size(16.0));
            });
            ui.separator();

            if count == 0 {
                ui.label(egui::RichText::new("Нет новых коммитов"));
                return;
            }

            // The names of the columns stay above the list, so that the reader
            // knows what the columns mean while the list is scrolled. They
            // stand above the very columns of the rows, so the two use the
            // same widths.
            let columns = commit_columns(ui, ui.available_width());
            self.commit_header(ui, columns);
            ui.separator();

            let scroll = egui::ScrollArea::vertical();
            scroll.show_rows(ui, ROW_HEIGHT, count, |ui, rows| {
                for position in rows {
                    self.commit_row(ui, index, position, &mut actions);
                }
            });
        });

        for action in actions {
            self.apply(action);
        }
    }

    /// Draw the names of the columns of the table of the commits.
    ///
    /// The names stand above the very columns of the rows, so the header and
    /// the rows use the same widths and the same gaps.
    fn commit_header(&self, ui: &mut egui::Ui, columns: CommitColumns) {
        ui.allocate_ui_with_layout(
            egui::vec2(columns.total(), HEADER_HEIGHT),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                commit_column(ui, columns.criticality, "Критичность");
                commit_column(ui, columns.description, "Описание");
                commit_column(ui, columns.complexity, "Сложность");
                commit_column(ui, columns.action, "Действие");
            },
        );
    }

    /// Draw one row of the table of the commits.
    ///
    /// The row is drawn in a fixed height, so that the list of the rows can
    /// be scrolled without knowing the height of every one of them.
    fn commit_row(
        &self,
        ui: &mut egui::Ui,
        repo: usize,
        position: usize,
        actions: &mut Vec<Action>,
    ) {
        let view = &self.repos[repo];
        let Some(commit) = view.commits().get(position) else {
            return;
        };
        let row = view.row(position);
        let assessment = view.assessment(&commit.sha);
        let selected = self.selected_commit.as_deref() == Some(commit.sha.as_str());
        let width = ui.available_width();
        let columns = commit_columns(ui, width);

        ui.allocate_ui_with_layout(
            egui::vec2(columns.total(), ROW_HEIGHT),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                // The criticality of the commit, on the left.
                ui.allocate_ui_with_layout(
                    egui::vec2(columns.criticality, ROW_HEIGHT),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.set_min_width(columns.criticality);
                        match assessment {
                            Some(assessment) => {
                                badge(
                                    ui,
                                    assessment.criticality.badge(),
                                    criticality_color(assessment.criticality),
                                );
                            }
                            None => {
                                ui.label(egui::RichText::new("…").weak());
                            }
                        }
                    },
                );

                // The description of the commit.
                ui.allocate_ui_with_layout(
                    egui::vec2(columns.description, ROW_HEIGHT),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_min_width(columns.description);
                        if ui
                            .selectable_label(
                                selected,
                                egui::RichText::new(&commit.summary),
                            )
                            .clicked()
                        {
                            actions.push(Action::OpenTerminal {
                                repo,
                                sha: commit.sha.clone(),
                            });
                        }
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(&row.detail).small().monospace());
                            if let Some(files) = &row.files {
                                ui.label(egui::RichText::new(files).small());
                            }
                        });
                    },
                );

                // The estimated complexity, on the right.
                ui.allocate_ui_with_layout(
                    egui::vec2(columns.complexity, ROW_HEIGHT),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.set_min_width(columns.complexity);
                        match assessment {
                            Some(assessment) => {
                                badge(
                                    ui,
                                    assessment.complexity.badge(),
                                    complexity_color(assessment.complexity),
                                );
                            }
                            None => {
                                ui.label(egui::RichText::new("…").weak());
                            }
                        }
                    },
                );

                // The button that starts the implementation.
                ui.allocate_ui_with_layout(
                    egui::vec2(columns.action, ROW_HEIGHT),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.set_min_width(columns.action);
                        if ui.button("Реализовать").clicked() {
                            actions.push(Action::Implement {
                                repo,
                                sha: commit.sha.clone(),
                            });
                        }
                    },
                );
            },
        );
    }

    /// The dialog that asks the user for a model.
    fn model_picker(&mut self, ctx: &egui::Context) {
        if self.picker.is_none() {
            return;
        }

        let decision = {
            let FsrsApp { picker, models, .. } = self;
            let picker = picker.as_mut().expect("the picker exists");
            egui::Modal::new(egui::Id::new("model_picker"))
                .show(ctx, |ui| {
                    let mut decision: Option<PickerOutcome> = None;

                    ui.heading(picker.intent.title());
                    // The line is always there, so that the dialog does not
                    // jump under the pointer when a model is picked.
                    let chosen = picker
                        .chosen(models)
                        .map(|model| model.label().to_string())
                        .unwrap_or_else(|| "не выбрано".to_string());
                    ui.label(egui::RichText::new(format!("Выбрано: {chosen}")).weak());
                    ui.horizontal(|ui| {
                        ui.label("Поиск:");
                        ui.text_edit_singleline(&mut picker.filter);
                        if picker.loading {
                            ui.spinner();
                        }
                    });
                    if let Some(error) = &picker.error {
                        ui.colored_label(ui.visuals().error_fg_color, error);
                    }

                    let filtered = picker.filtered(models);
                    egui::ScrollArea::vertical()
                        .max_height(360.0)
                        .show(ui, |ui| {
                            if filtered.is_empty() {
                                ui.label(egui::RichText::new("Ничего не найдено").weak());
                            }
                            for position in filtered.iter() {
                                let model = &models[*position];
                                let selected =
                                    picker.selected.as_deref() == Some(model.id.as_str());
                                let label = format!("{}  ·  {}", model.label(), model.id);
                                if ui.selectable_label(selected, label).clicked() {
                                    picker.select(&model.id);
                                }
                            }
                        });

                    ui.separator();
                    ui.horizontal(|ui| {
                        if ui.button("Отмена").clicked() {
                            decision = Some(PickerOutcome::Cancelled);
                        }
                        let ready = picker.selected.is_some();
                        let response =
                            ui.add_enabled(ready, egui::Button::new(picker.intent.confirm_label()));
                        if response.clicked() {
                            decision = Some(PickerOutcome::Confirmed(
                                picker.selected.clone().unwrap_or_default(),
                            ));
                        }
                    });

                    decision
                })
                .inner
        };

        match decision {
            Some(PickerOutcome::Confirmed(model)) => self.confirm_picker(model),
            Some(PickerOutcome::Cancelled) => {
                self.picker = None;
                self.pending = None;
            }
            None => {}
        }
    }
}

/// What the model picker returned.
enum PickerOutcome {
    /// The user picked a model and confirmed.
    Confirmed(String),
    /// The user cancelled the dialog.
    Cancelled,
}

/// Draw a coloured badge with one letter.
fn badge(ui: &mut egui::Ui, letter: &str, color: egui::Color32) {
    egui::Frame::default()
        .fill(color)
        .corner_radius(8.0)
        .inner_margin(egui::Margin::symmetric(6, 2))
        .show(ui, |ui| {
            ui.label(
                egui::RichText::new(letter).color(egui::Color32::WHITE),
            );
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_line_is_split_into_program_and_arguments() {
        let (program, arguments) = split_command("opencode mini");
        assert_eq!(program, "opencode");
        assert_eq!(arguments, vec!["mini".to_string()]);

        let (program, arguments) = split_command("opencode");
        assert_eq!(program, "opencode");
        assert!(arguments.is_empty());
    }

    #[test]
    fn an_empty_command_line_falls_back_to_opencode() {
        let (program, arguments) = split_command("   ");
        assert_eq!(program, "opencode");
        assert!(arguments.is_empty());
    }

    #[test]
    fn a_commit_id_is_shortened() {
        assert_eq!(short_sha("0123456789abcdef"), "01234567");
        assert_eq!(short_sha("abc"), "abc");
    }

    #[test]
    fn the_number_of_the_ratings_follows_the_slots_of_the_server() {
        // One slot: one rating at a time.
        assert_eq!(concurrent_assessment_limit(1), 1);
        // More slots than the constant of the program: the constant wins.
        assert_eq!(concurrent_assessment_limit(8), MAX_CONCURRENT_ASSESSMENTS);
        // A server that reports nothing is assumed to have one slot.
        assert_eq!(concurrent_assessment_limit(0), 1);
    }
}
