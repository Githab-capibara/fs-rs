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

use fsrs_acp::{ModelChoice, PipelineRequest};
use fsrs_core::{
    Assessment, Complexity, Config, Criticality, Repo, Run, RunState, Store, UpstreamCommit,
    now_unix,
};

use crate::state::{
    ModelPicker, PickerIntent, PipelineLog, RepoView, agent_name, complexity_color,
    criticality_color, format_time, output_of, plural, prompt_of, run_state_for,
};
use crate::terminal::TerminalPanel;
use crate::worker::{FoqSettings, Worker, WorkerMessage};

/// How many commits are rated by the local model at the same time.
///
/// The decision model is a single process on a single GPU, so the dashboard
/// keeps the queue short instead of flooding it with requests.
const MAX_CONCURRENT_ASSESSMENTS: usize = 2;

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
    /// Where the configuration was read from.
    config_path: PathBuf,
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
    /// The last message for the user.
    status: String,
    /// The last error for the user.
    error: Option<String>,
    /// Whether the window shows prepared data instead of the live state.
    ///
    /// The prepared data is used for the screenshot of the documentation: it
    /// must survive the messages of the background worker.
    prepared: bool,
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
            config_path,
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
            status: String::new(),
            error: None,
            prepared: false,
        };
        app.load_repositories();
        if discovery {
            app.refresh_models();
        }
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
                view.commits = commits;
            }
            if let Ok(assessments) = self.store.list_assessments(id) {
                for assessment in assessments {
                    view.assessments.insert(assessment.sha.clone(), assessment);
                }
            }
            view.syncing = true;
            views.push(view);
        }

        self.repos = views;
        self.selected_repo = if self.repos.is_empty() { None } else { Some(0) };
        for view in &self.repos {
            self.worker.sync_repo(view.repo.clone());
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
    fn handle_messages(&mut self) {
        for message in self.worker.drain() {
            if self.prepared {
                // The window shows prepared data; the live state of the
                // background must not replace it.
                continue;
            }
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
            view.commits = commits;
        }
        let commits = self.repos[index].commits.clone();
        if let Err(error) = self.store.replace_commits(repo_id, &commits) {
            self.error = Some(format!("сохранение коммитов: {error}"));
        }
        let mut pending = Vec::new();
        for commit in &commits {
            if !self.repos[index].assessments.contains_key(&commit.sha) {
                pending.push(commit.sha.clone());
            }
        }
        for sha in pending {
            self.assessment_queue.push_back((index, sha));
        }
        self.status = format!(
            "{}: обновлений {}",
            self.repos[index].repo.name,
            commits.len()
        );
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
        view.commits.clear();
        view.assessments.clear();
        view.syncing = false;
        // The prepared data replaces the result of the synchronisation, so the
        // error of the failed attempt is not shown any more.
        view.error = None;
        self.error = None;
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
            view.commits.push(UpstreamCommit {
                repo_id: view.repo.id,
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
            view.assessments.insert(
                sha.clone(),
                Assessment {
                    repo_id: view.repo.id,
                    sha,
                    criticality: row.criticality,
                    complexity: row.complexity,
                    criticality_confidence: 0.46,
                    complexity_confidence: 0.48,
                    needs_review: true,
                    rationale: String::new(),
                    model: "foq".to_string(),
                    created_at: 1_762_280_000,
                },
            );
        }
        view.repo.pending_commits = rows.len() as i64;
        self.status = format!("обновлений: {}", rows.len());
    }

    /// The local model rated one commit.
    fn on_assessed(&mut self, assessment: fsrs_core::Assessment) {
        self.assessment_in_flight = self.assessment_in_flight.saturating_sub(1);
        if let Some(view) = self
            .repos
            .iter_mut()
            .find(|view| view.repo.id == assessment.repo_id)
        {
            view.assessments
                .insert(assessment.sha.clone(), assessment.clone());
        }
        if let Err(error) = self.store.upsert_assessment(&assessment) {
            self.error = Some(format!("сохранение оценки: {error}"));
        }
    }

    /// The pipeline reported a step.
    fn on_pipeline_event(&mut self, event: fsrs_acp::RunEvent) {
        self.log.push(PipelineLog::new(now_unix(), event.clone()));

        if let Some(run_id) = self.run_id {
            if let Some(state) = run_state_for(&event) {
                self.run_state = Some(state);
                let _ = self.store.update_run_state(run_id, state);
            }
            let record = fsrs_core::RunEvent {
                run_id,
                agent: agent_name(&event).to_string(),
                prompt: prompt_of(&event).to_string(),
                output: output_of(&event),
                created_at: now_unix(),
            };
            let _ = self.store.insert_run_event(&record);
        }
    }

    /// The pipeline finished.
    fn on_pipeline_finished(&mut self, outcome: Result<fsrs_acp::PipelineOutcome, String>) {
        match outcome {
            Ok(result) => {
                self.run_state = Some(RunState::Merged);
                if let Some(run_id) = self.run_id {
                    let _ = self.store.update_run_state(run_id, RunState::Merged);
                }
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
                if let Some(run_id) = self.run_id {
                    let _ = self.store.update_run_state(run_id, RunState::Failed);
                }
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
            self.worker.sync_repo(view.repo.clone());
        }
    }

    /// Send the queued commits to the local decision model.
    fn dispatch_assessments(&mut self) {
        while self.assessment_in_flight < MAX_CONCURRENT_ASSESSMENTS {
            let Some((index, sha)) = self.assessment_queue.pop_front() else {
                break;
            };
            let Some(view) = self.repos.get(index) else {
                continue;
            };
            if view.assessments.contains_key(&sha) {
                continue;
            }
            let Some(commit) = view
                .commits
                .iter()
                .find(|commit| commit.sha == sha)
                .cloned()
            else {
                continue;
            };
            let path = view.repo.path.clone();
            let settings = self.foq_settings();
            self.worker.assess(settings, path, commit);
            self.assessment_in_flight += 1;
        }
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
                if let Some(repo) = self.repos.get(index).map(|view| view.repo.clone()) {
                    self.repos[index].syncing = true;
                    self.worker.sync_repo(repo);
                }
            }
            Action::AssessAll(index) => {
                if let Some(view) = self.repos.get(index) {
                    for commit in &view.commits {
                        if !view.assessments.contains_key(&commit.sha) {
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
        let Some(commit) = view
            .commits
            .iter()
            .find(|commit| commit.sha == sha)
            .cloned()
        else {
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
        match self.store.insert_run(&run) {
            Ok(id) => {
                self.run_id = Some(id);
                self.run_state = Some(RunState::Prepared);
            }
            Err(error) => {
                self.error = Some(format!("запуск не сохранён: {error}"));
                return;
            }
        }

        self.log.clear();
        self.status = format!(
            "реализация {} началась на {}",
            short_sha(sha),
            request.branch
        );
        self.error = None;
        self.worker.run_pipeline(request);
    }
}

/// Split a command line into the program and its arguments.
pub fn split_command(command: &str) -> (String, Vec<String>) {
    let mut parts = command.split_whitespace();
    let program = parts.next().unwrap_or("opencode").to_string();
    (program, parts.map(str::to_string).collect())
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
        self.handle_messages();
        self.dispatch_assessments();

        self.header(ui);
        self.repository_strip(ui);
        self.terminal_panel(ui);
        self.log_panel(ui);
        self.commit_table(ui);
        self.model_picker(ui.ctx());

        if self.is_busy() {
            ui.ctx().request_repaint();
        }
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

    /// The header of the window.
    fn header(&mut self, ui: &mut egui::Ui) {
        let model = self.selected_model_label();
        let status = self.status.clone();
        let error = self.error.clone();
        let run = self
            .run_state
            .map(|state| format!("{state:?}"))
            .unwrap_or_else(|| "—".to_string());
        let config_path = self.config_path.display().to_string();

        egui::Panel::top("header").exact_size(34.0).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.heading("fs-rs");
                ui.separator();
                ui.label(format!("Модель: {model}"));
                ui.separator();
                ui.label(format!("Реализация: {run}"));
                ui.separator();
                ui.label(status);
                ui.separator();
                ui.label(egui::RichText::new(config_path).small().weak().monospace());
                if let Some(error) = error {
                    ui.colored_label(egui::Color32::from_rgb(220, 90, 80), error);
                }
            });
        });
    }

    /// The label of the model the user picked.
    fn selected_model_label(&self) -> String {
        let Some(id) = self.selected_model.as_deref() else {
            return "не выбрана".to_string();
        };
        self.models
            .iter()
            .find(|model| model.id == id)
            .map(|model| model.label().to_string())
            .unwrap_or_else(|| id.to_string())
    }

    /// The central list of the tracked repositories.
    fn repository_strip(&mut self, ui: &mut egui::Ui) {
        let mut action: Option<Action> = None;

        egui::Panel::top("repositories")
            .exact_size(104.0)
            .show(ui, |ui| {
                egui::ScrollArea::horizontal().show(ui, |ui| {
                    ui.horizontal(|ui| {
                        for (index, view) in self.repos.iter().enumerate() {
                            let selected = self.selected_repo == Some(index);
                            let name = view.repo.name.clone();
                            let path = view.repo.path.to_string_lossy().to_string();
                            let pending = view.pending();
                            let unassessed = view.unassessed();
                            let syncing = view.syncing;
                            let error = view.error.clone();

                            egui::Frame::group(ui.style())
                                .inner_margin(8.0)
                                .show(ui, |ui| {
                                    ui.set_min_width(240.0);
                                    if ui
                                        .selectable_label(
                                            selected,
                                            egui::RichText::new(&name).strong().size(15.0),
                                        )
                                        .clicked()
                                    {
                                        action = Some(Action::SelectRepo(index));
                                    }
                                    ui.label(egui::RichText::new(path).small().weak().monospace());
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
                                                .small()
                                                .weak(),
                                            );
                                        }
                                    });
                                    if let Some(error) = error {
                                        ui.label(
                                            egui::RichText::new(error)
                                                .small()
                                                .color(egui::Color32::from_rgb(220, 90, 80)),
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
                        if self.repos.is_empty() {
                            ui.label(
                                egui::RichText::new(
                                    "Нет репозиториев. Добавьте их в ~/.config/fs-rs/config.toml",
                                )
                                .weak(),
                            );
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
            .default_size(620.0)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.heading(&title);
                    if let Some(terminal) = self.terminal.as_mut() {
                        if let Some(program_title) = terminal.title() {
                            ui.label(egui::RichText::new(program_title).weak());
                        }
                        if terminal.has_exited() {
                            ui.label(egui::RichText::new("процесс завершился").weak());
                        }
                        if let Some(pid) = terminal.process_id() {
                            ui.label(
                                egui::RichText::new(format!("pid {pid}"))
                                    .small()
                                    .weak()
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
    fn log_panel(&mut self, ui: &mut egui::Ui) {
        let entries: Vec<(String, String, String, String)> = self
            .log
            .iter()
            .map(|entry| {
                (
                    format_time(entry.at),
                    entry
                        .role()
                        .map(|role| role.label().to_string())
                        .unwrap_or_else(|| "pipeline".to_string()),
                    entry.summary(),
                    prompt_of(&entry.event).to_string(),
                )
            })
            .collect();

        egui::Panel::bottom("log")
            .resizable(true)
            .default_size(220.0)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.heading("Журнал реализации");
                    if let Some(state) = self.run_state {
                        ui.label(format!("{state:?}"));
                    }
                    ui.label(format!("событий: {}", entries.len()));
                });
                ui.separator();
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for (time, role, summary, mut prompt) in entries {
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(time).small().monospace().weak());
                            ui.label(egui::RichText::new(role).strong());
                            ui.label(summary);
                        });
                        if !prompt.is_empty() {
                            egui::CollapsingHeader::new("Видимый промпт")
                                .default_open(false)
                                .show(ui, |ui| {
                                    ui.add(
                                        egui::TextEdit::multiline(&mut prompt)
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
    fn commit_table(&mut self, ui: &mut egui::Ui) {
        let mut actions: Vec<Action> = Vec::new();

        let Some(index) = self.selected_repo else {
            egui::CentralPanel::default().show(ui, |ui| {
                ui.centered_and_justified(|ui| {
                    ui.label("Выберите репозиторий");
                });
            });
            return;
        };

        let title = self.repos[index].repo.name.clone();
        let count = self.repos[index].commits.len();

        egui::CentralPanel::default().show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.heading(format!("{title} — коммиты ({count})"));
            });
            ui.separator();

            if self.repos[index].commits.is_empty() {
                ui.label(egui::RichText::new("Нет новых коммитов").weak());
                return;
            }

            egui::ScrollArea::vertical().show(ui, |ui| {
                let view = &self.repos[index];
                egui::Grid::new("commits")
                    .num_columns(4)
                    .striped(true)
                    .spacing([16.0, 6.0])
                    .show(ui, |ui| {
                        ui.label(egui::RichText::new("Критичность").strong());
                        ui.label(egui::RichText::new("Описание").strong());
                        ui.label(egui::RichText::new("Сложность").strong());
                        ui.label(egui::RichText::new("Действие").strong());
                        ui.end_row();

                        for commit in &view.commits {
                            let selected =
                                self.selected_commit.as_deref() == Some(commit.sha.as_str());
                            let assessment = view.assessment(&commit.sha);

                            // The criticality of the commit, on the left.
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

                            // The description of the commit.
                            ui.vertical(|ui| {
                                if ui
                                    .selectable_label(
                                        selected,
                                        egui::RichText::new(&commit.summary).strong(),
                                    )
                                    .clicked()
                                {
                                    actions.push(Action::OpenTerminal {
                                        repo: index,
                                        sha: commit.sha.clone(),
                                    });
                                }
                                ui.horizontal(|ui| {
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "{} · {} · +{} −{}",
                                            commit.author,
                                            commit.short_sha,
                                            commit.insertions,
                                            commit.deletions
                                        ))
                                        .small()
                                        .weak()
                                        .monospace(),
                                    );
                                    if !commit.files.is_empty() {
                                        ui.label(
                                            egui::RichText::new(plural(
                                                commit.files.len(),
                                                "файл",
                                                "файла",
                                                "файлов",
                                            ))
                                            .small()
                                            .weak(),
                                        );
                                    }
                                });
                            });

                            // The estimated complexity, on the right.
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

                            // The button that starts the implementation.
                            if ui.button("РЕАЛИЗОВАТЬ").clicked() {
                                actions.push(Action::Implement {
                                    repo: index,
                                    sha: commit.sha.clone(),
                                });
                            }
                            ui.end_row();
                        }
                    });
            });
        });

        for action in actions {
            self.apply(action);
        }
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
                        ui.colored_label(egui::Color32::from_rgb(220, 90, 80), error);
                    }

                    let filtered = picker.filtered(models);
                    egui::ScrollArea::vertical()
                        .max_height(360.0)
                        .show(ui, |ui| {
                            if filtered.is_empty() {
                                ui.label(egui::RichText::new("Ничего не найдено").weak());
                            }
                            for model in filtered {
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
    egui::Frame::group(ui.style())
        .fill(color)
        .corner_radius(4.0)
        .inner_margin(egui::Margin::symmetric(6, 2))
        .show(ui, |ui| {
            ui.label(
                egui::RichText::new(letter)
                    .strong()
                    .color(egui::Color32::WHITE),
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
}
