//! The state of the dashboard and the logic that does not need a window.
//!
//! Everything in this module is plain data or a pure function, so it can be
//! tested without starting a graphical interface.

use std::collections::HashMap;
use std::sync::Arc;

use fsrs_acp::{AgentRole, ModelChoice, RunEvent};
use fsrs_core::{Assessment, Repo, RunState, UpstreamCommit};

/// What the user opened the model picker for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickerIntent {
    /// Open a terminal that runs the OpenCode CLI.
    Terminal,
    /// Start the implementation pipeline.
    Pipeline,
}

impl PickerIntent {
    /// The title of the picker dialog.
    pub fn title(self) -> &'static str {
        match self {
            PickerIntent::Terminal => "Модель для терминала",
            PickerIntent::Pipeline => "Модель для реализации",
        }
    }

    /// The label of the button that confirms the choice.
    pub fn confirm_label(self) -> &'static str {
        match self {
            PickerIntent::Terminal => "Открыть терминал",
            PickerIntent::Pipeline => "Запустить реализацию",
        }
    }
}

/// The Russian plural of a noun.
///
/// The Russian language has three forms: `1 файл`, `2 файла` and `5 файлов`.
/// The form is chosen by the last digit of the number, and by the teens for
/// the numbers that end in 11, 12, 13 and 14.
pub fn plural(count: usize, one: &str, few: &str, many: &str) -> String {
    let last_two = count % 100;
    let last = count % 10;
    let form = if (11..=14).contains(&last_two) {
        many
    } else if last == 1 {
        one
    } else if (2..=4).contains(&last) {
        few
    } else {
        many
    };
    format!("{count} {form}")
}

/// One prepared row of the table of the commits.
///
/// The strings of a row are built when the list of the commits changes, and
/// not when the row is drawn: the window draws the table again and again,
/// and a row that is built every time costs an allocation per line.
#[derive(Debug, Clone)]
pub struct CommitRow {
    /// The line under the description: the author, the short object id and
    /// the number of the lines that the commit adds and removes.
    pub detail: String,
    /// The number of the files of the commit, when it touches any.
    pub files: Option<String>,
}

/// One repository of the central list.
///
/// The view keeps three things that the window would otherwise build again
/// for every frame: the position of every commit, the strings of every row
/// of the table, and the number of the commits that wait for their
/// assessment. They are built when the list of the commits changes.
#[derive(Debug, Clone)]
pub struct RepoView {
    /// The stored repository.
    pub repo: Repo,
    /// The upstream commits that are missing from the fork.
    commits: Vec<UpstreamCommit>,
    /// The assessments of those commits, by commit id.
    assessments: HashMap<String, Assessment>,
    /// The position of every commit, by its object id.
    ///
    /// The lookup of a commit by its object id is asked for every commit that
    /// is rated and for every commit that is implemented, and the list of the
    /// commits is long.
    positions: HashMap<String, usize>,
    /// The prepared rows of the table.
    rows: Vec<CommitRow>,
    /// The number of the commits that wait for their assessment.
    unassessed: usize,
    /// Whether a synchronisation is currently running.
    pub syncing: bool,
    /// The last error of this repository.
    pub error: Option<String>,
}

impl RepoView {
    /// Build an empty view of a repository.
    pub fn new(repo: Repo) -> Self {
        Self {
            repo,
            commits: Vec::new(),
            assessments: HashMap::new(),
            positions: HashMap::new(),
            rows: Vec::new(),
            unassessed: 0,
            syncing: false,
            error: None,
        }
    }

    /// The upstream commits that are missing from the fork.
    pub fn commits(&self) -> &[UpstreamCommit] {
        &self.commits
    }

    /// The prepared row of the table at the given position.
    pub fn row(&self, index: usize) -> &CommitRow {
        &self.rows[index]
    }

    /// The commit with the given object id.
    pub fn commit(&self, sha: &str) -> Option<&UpstreamCommit> {
        self.positions
            .get(sha)
            .and_then(|index| self.commits.get(*index))
    }

    /// The assessment of one commit, when it is already known.
    pub fn assessment(&self, sha: &str) -> Option<&Assessment> {
        self.assessments.get(sha)
    }

    /// The assessments of the commits, by commit id.
    pub fn assessments(&self) -> &HashMap<String, Assessment> {
        &self.assessments
    }

    /// Replace the list of the missing commits.
    ///
    /// The position of every commit, the strings of every row of the table
    /// and the number of the commits that wait for their assessment are built
    /// here, once, and not again for every frame.
    pub fn set_commits(&mut self, commits: Vec<UpstreamCommit>) {
        self.positions.clear();
        self.rows.clear();
        self.rows.reserve(commits.len());
        for (index, commit) in commits.iter().enumerate() {
            self.positions.insert(commit.sha.clone(), index);
            self.rows.push(CommitRow {
                detail: format!(
                    "{} · {} · +{} −{}",
                    commit.author, commit.short_sha, commit.insertions, commit.deletions
                ),
                files: (!commit.files.is_empty())
                    .then(|| plural(commit.files.len(), "файл", "файла", "файлов")),
            });
        }
        self.unassessed = commits
            .iter()
            .filter(|commit| !self.assessments.contains_key(&commit.sha))
            .count();
        self.commits = commits;
    }

    /// Remember the assessment of one commit.
    ///
    /// The number of the commits that wait for their assessment falls by one
    /// when the commit was not rated yet.
    pub fn insert_assessment(&mut self, assessment: Assessment) {
        let known = self.assessments.contains_key(&assessment.sha);
        if !known && self.positions.contains_key(&assessment.sha) {
            self.unassessed = self.unassessed.saturating_sub(1);
        }
        self.assessments.insert(assessment.sha.clone(), assessment);
    }

    /// Forget the commits and the assessments of the repository.
    pub fn clear(&mut self) {
        self.commits.clear();
        self.assessments.clear();
        self.positions.clear();
        self.rows.clear();
        self.unassessed = 0;
    }

    /// The number of upstream commits that are still missing.
    pub fn pending(&self) -> usize {
        self.commits.len()
    }

    /// The number of commits that still wait for an assessment.
    pub fn unassessed(&self) -> usize {
        self.unassessed
    }
}

/// The dialog that asks the user which model to use.
#[derive(Debug, Clone)]
pub struct ModelPicker {
    /// What the picker was opened for.
    pub intent: PickerIntent,
    /// The search text of the user.
    pub filter: String,
    /// The identifier of the model the user picked.
    pub selected: Option<String>,
    /// Whether the model list is still being loaded.
    pub loading: bool,
    /// The error of the last attempt to load the model list.
    pub error: Option<String>,
    /// The result of the last filter.
    ///
    /// The dialog is drawn in every frame, and the filter of every model of
    /// the list is computed in every frame. The result is kept, and it is
    /// built again when the text of the user or the list of the models
    /// changes.
    filtered: Option<Filtered>,
}

/// The result of the filter of the model list.
#[derive(Debug, Clone)]
struct Filtered {
    /// The text of the user that produced the result.
    filter: String,
    /// The number of the models that were filtered.
    models: usize,
    /// The positions of the models that match.
    positions: Arc<[usize]>,
}

impl ModelPicker {
    /// Open the picker for one intent.
    pub fn new(intent: PickerIntent, selected: Option<String>) -> Self {
        Self {
            intent,
            filter: String::new(),
            selected,
            loading: true,
            error: None,
            filtered: None,
        }
    }

    /// The positions of the models that match the search text of the user.
    ///
    /// The result is kept until the text of the user or the list of the
    /// models changes. The result is handed out as a shared slice, so that
    /// the dialog can use it and change the choice of the user at the same
    /// time.
    pub fn filtered(&mut self, models: &[ModelChoice]) -> Arc<[usize]> {
        let stale = self
            .filtered
            .as_ref()
            .is_none_or(|cached| cached.filter != self.filter || cached.models != models.len());
        if stale {
            let needle = self.filter.trim().to_lowercase();
            let positions: Vec<usize> = models
                .iter()
                .enumerate()
                .filter(|(_, model)| {
                    if needle.is_empty() {
                        return true;
                    }
                    model.id.to_lowercase().contains(&needle)
                        || model.name.to_lowercase().contains(&needle)
                })
                .map(|(index, _)| index)
                .collect();
            self.filtered = Some(Filtered {
                filter: self.filter.clone(),
                models: models.len(),
                positions: positions.into(),
            });
        }
        Arc::clone(
            &self
                .filtered
                .as_ref()
                .expect("the filter was built")
                .positions,
        )
    }

    /// Forget the result of the last filter.
    ///
    /// The list of the models changed under the dialog, so the result of the
    /// filter is not the result of the new list any more.
    pub fn invalidate(&mut self) {
        self.filtered = None;
    }

    /// Remember the model the user picked.
    pub fn select(&mut self, id: &str) {
        self.selected = Some(id.to_string());
    }

    /// The model the user picked, if it is still in the list.
    pub fn chosen<'a>(&self, models: &'a [ModelChoice]) -> Option<&'a ModelChoice> {
        let selected = self.selected.as_deref()?;
        models.iter().find(|model| model.id == selected)
    }
}

/// One line of the pipeline log.
///
/// The strings of the line are built when the event arrives, and not when the
/// line is drawn: the window draws the log in every frame.
#[derive(Debug, Clone)]
pub struct PipelineLog {
    /// Unix timestamp of the event.
    pub at: i64,
    /// The event itself.
    event: RunEvent,
    /// The time of the event, ready to be shown.
    time: String,
    /// The name of the agent, ready to be shown.
    role: String,
    /// The summary of the event, ready to be shown.
    summary: String,
    /// The prompt of the event, ready to be shown.
    prompt: String,
}

impl PipelineLog {
    /// Build a log entry.
    pub fn new(at: i64, event: RunEvent) -> Self {
        let role = event
            .role()
            .map(|role| role.label().to_string())
            .unwrap_or_else(|| "pipeline".to_string());
        let summary = event.summary();
        let prompt = prompt_of(&event).to_string();
        Self {
            at,
            event,
            time: format_time(at),
            role,
            summary,
            prompt,
        }
    }

    /// The agent the event belongs to, when it has one.
    pub fn role(&self) -> Option<AgentRole> {
        self.event.role()
    }

    /// The name of the agent, ready to be shown.
    pub fn role_label(&self) -> &str {
        &self.role
    }

    /// The time of the event, ready to be shown.
    pub fn time(&self) -> &str {
        &self.time
    }

    /// The one-line summary shown in the log.
    pub fn summary(&self) -> &str {
        &self.summary
    }

    /// The prompt of the event, ready to be shown.
    pub fn prompt(&self) -> &str {
        &self.prompt
    }

    /// The prompt of the event, ready to be changed.
    pub fn prompt_mut(&mut self) -> &mut String {
        &mut self.prompt
    }
}

/// The state of the pipeline the event belongs to.
pub fn run_state_for(event: &RunEvent) -> Option<RunState> {
    match event {
        RunEvent::Started { .. } => Some(RunState::Implementing),
        RunEvent::PromptSent { role, .. } => match role {
            AgentRole::Implementer => Some(RunState::Implementing),
            AgentRole::Reviewer => Some(RunState::Reviewing),
            AgentRole::Verifier => Some(RunState::Verifying),
        },
        RunEvent::Rejected { .. } => Some(RunState::Rework),
        RunEvent::Approved { .. } => Some(RunState::Approved),
        RunEvent::Merged { .. } => Some(RunState::Merged),
        RunEvent::Failed { .. } => Some(RunState::Failed),
        _ => None,
    }
}

/// The name of the agent an event belongs to, as stored in the database.
pub fn agent_name(event: &RunEvent) -> &'static str {
    match event.role() {
        Some(role) => role.as_str(),
        None => "pipeline",
    }
}

/// The prompt of an event, if it carries one.
pub fn prompt_of(event: &RunEvent) -> &str {
    match event {
        RunEvent::PromptSent { prompt, .. } => prompt,
        _ => "",
    }
}

/// The answer of an event, if it carries one.
pub fn output_of(event: &RunEvent) -> String {
    match event {
        RunEvent::AgentMessage { text, .. }
        | RunEvent::AgentThought { text, .. }
        | RunEvent::AgentFinished { text, .. } => text.clone(),
        RunEvent::ToolCall { title, status, .. } => format!("{title} ({status})"),
        RunEvent::Permission { title, outcome, .. } => format!("{outcome}: {title}"),
        RunEvent::Rejected { feedback, .. } => feedback.clone(),
        RunEvent::Failed { message } => message.clone(),
        other => other.summary(),
    }
}

/// The colour of a criticality badge.
///
/// Every colour keeps at least a 4.5:1 contrast against the white letter of
/// the badge, so the badge stays readable in both the light and the dark
/// theme of egui.
pub fn criticality_color(criticality: fsrs_core::Criticality) -> egui::Color32 {
    use fsrs_core::Criticality;
    match criticality {
        Criticality::Critical => egui::Color32::from_rgb(170, 40, 40),
        Criticality::High => egui::Color32::from_rgb(180, 80, 30),
        Criticality::Medium => egui::Color32::from_rgb(150, 100, 30),
        Criticality::Low => egui::Color32::from_rgb(50, 120, 50),
        Criticality::Trivial => egui::Color32::from_rgb(90, 90, 100),
    }
}

/// The colour of a complexity badge.
///
/// Every colour keeps at least a 4.5:1 contrast against the white letter of
/// the badge, so the badge stays readable in both the light and the dark
/// theme of egui.
pub fn complexity_color(complexity: fsrs_core::Complexity) -> egui::Color32 {
    use fsrs_core::Complexity;
    match complexity {
        Complexity::Expert => egui::Color32::from_rgb(140, 40, 170),
        Complexity::Hard => egui::Color32::from_rgb(50, 80, 170),
        Complexity::Medium => egui::Color32::from_rgb(30, 120, 160),
        Complexity::Easy => egui::Color32::from_rgb(40, 120, 70),
        Complexity::Trivial => egui::Color32::from_rgb(90, 90, 100),
    }
}

/// Format a unix timestamp as `YYYY-MM-DD HH:MM`.
pub fn format_time(unix: i64) -> String {
    chrono_like(unix)
}

/// Convert a unix timestamp into a readable date and time.
///
/// The conversion is done with the civil calendar algorithm of Howard
/// Hinnant, so the crate does not need a date-time dependency.
fn chrono_like(unix: i64) -> String {
    let days = unix.div_euclid(86_400);
    let seconds_of_day = unix.rem_euclid(86_400);
    let hour = seconds_of_day / 3600;
    let minute = (seconds_of_day % 3600) / 60;
    let second = seconds_of_day % 60;

    // Shift the epoch to 1970-03-01 so that the leap day is the last day of
    // the "year" and the regular algorithm applies.
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    let year = year + if month <= 2 { 1 } else { 0 };

    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use fsrs_core::{Complexity, Criticality};

    fn model(id: &str, name: &str) -> ModelChoice {
        ModelChoice::new(id, name)
    }

    #[test]
    fn the_russian_plural_follows_the_number() {
        assert_eq!(plural(1, "файл", "файла", "файлов"), "1 файл");
        assert_eq!(plural(2, "файл", "файла", "файлов"), "2 файла");
        assert_eq!(plural(5, "файл", "файла", "файлов"), "5 файлов");
        assert_eq!(plural(11, "файл", "файла", "файлов"), "11 файлов");
        assert_eq!(plural(14, "файл", "файла", "файлов"), "14 файлов");
        assert_eq!(plural(21, "файл", "файла", "файлов"), "21 файл");
        assert_eq!(plural(22, "файл", "файла", "файлов"), "22 файла");
        assert_eq!(plural(25, "файл", "файла", "файлов"), "25 файлов");
        assert_eq!(plural(0, "файл", "файла", "файлов"), "0 файлов");
    }

    #[test]
    fn the_filter_matches_the_identifier_and_the_name() {
        let models = vec![
            model("hnc/MiniMax-M3.1-Flash", "MiniMax M3.1 Flash"),
            model("opencode/gpt-5.3-codex", "GPT 5.3 Codex"),
        ];
        let mut picker = ModelPicker::new(PickerIntent::Terminal, None);
        picker.filter = "minimax".to_string();
        let filtered = picker.filtered(&models);
        assert_eq!(filtered.len(), 1);
        assert_eq!(models[filtered[0]].id, "hnc/MiniMax-M3.1-Flash");

        picker.filter = "codex".to_string();
        let filtered = picker.filtered(&models);
        assert_eq!(filtered.len(), 1);
        assert_eq!(models[filtered[0]].id, "opencode/gpt-5.3-codex");

        picker.filter = String::new();
        assert_eq!(picker.filtered(&models).len(), 2);
    }

    #[test]
    fn the_filter_is_case_insensitive() {
        let models = vec![model("hnc/MiniMax-M3.1-Flash", "MiniMax M3.1 Flash")];
        let mut picker = ModelPicker::new(PickerIntent::Pipeline, None);
        picker.filter = "MINIMAX".to_string();
        assert_eq!(picker.filtered(&models).len(), 1);
    }

    #[test]
    fn the_filter_is_not_built_again_for_every_frame() {
        let models = vec![
            model("a/1", "One"),
            model("b/2", "Two"),
            model("c/3", "Three"),
        ];
        let mut picker = ModelPicker::new(PickerIntent::Pipeline, None);
        let first = picker.filtered(&models);
        let second = picker.filtered(&models);
        // The same result is handed out: the dialog does not filter the list
        // again in every frame.
        assert!(Arc::ptr_eq(&first, &second));

        // The text of the user changes the result.
        picker.filter = "two".to_string();
        let third = picker.filtered(&models);
        assert!(!Arc::ptr_eq(&first, &third));
        assert_eq!(third.len(), 1);
        assert_eq!(models[third[0]].id, "b/2");

        // The list of the models changes the result as well.
        picker.invalidate();
        let fourth = picker.filtered(&models);
        assert!(!Arc::ptr_eq(&third, &fourth));
    }

    #[test]
    fn the_chosen_model_is_found_again() {
        let models = vec![model("a/1", "One"), model("b/2", "Two")];
        let mut picker = ModelPicker::new(PickerIntent::Terminal, None);
        picker.select("b/2");
        assert_eq!(picker.chosen(&models).map(|m| m.id.as_str()), Some("b/2"));
        assert!(picker.chosen(&[]).is_none());
    }

    /// Build a repository of the tests.
    fn repo(name: &str) -> Repo {
        Repo {
            id: 1,
            name: name.to_string(),
            path: std::path::PathBuf::from("/tmp/demo"),
            upstream_url: String::new(),
            upstream_web: String::new(),
            main_branch: "main".to_string(),
            pending_commits: 0,
            last_fetched_at: None,
        }
    }

    #[test]
    fn a_repository_counts_its_missing_commits() {
        let mut view = RepoView::new(repo("demo"));
        assert_eq!(view.pending(), 0);
        assert_eq!(view.unassessed(), 0);

        view.set_commits(vec![UpstreamCommit {
            repo_id: 1,
            sha: "a".to_string(),
            short_sha: "a".to_string(),
            summary: "one".to_string(),
            body: String::new(),
            author: "ada".to_string(),
            author_email: String::new(),
            authored_at: 0,
            committed_at: 0,
            files: Vec::new(),
            insertions: 0,
            deletions: 0,
            url: String::new(),
        }]);
        assert_eq!(view.pending(), 1);
        assert_eq!(view.unassessed(), 1);
        assert!(view.assessment("a").is_none());
    }

    /// Build a commit of the tests.
    fn commit(sha: &str, author: &str, files: usize) -> UpstreamCommit {
        UpstreamCommit {
            repo_id: 1,
            sha: sha.to_string(),
            short_sha: sha.chars().take(8).collect(),
            summary: format!("the commit {sha}"),
            body: String::new(),
            author: author.to_string(),
            author_email: String::new(),
            authored_at: 0,
            committed_at: 0,
            files: (0..files).map(|index| format!("file-{index}.rs")).collect(),
            insertions: 12,
            deletions: 3,
            url: String::new(),
        }
    }

    /// Build an assessment of the tests.
    fn assessment(sha: &str) -> Assessment {
        Assessment {
            repo_id: 1,
            sha: sha.to_string(),
            criticality: Criticality::Medium,
            complexity: Complexity::Easy,
            criticality_confidence: 0.5,
            complexity_confidence: 0.5,
            needs_review: false,
            rationale: String::new(),
            model: "foq".to_string(),
            created_at: 0,
        }
    }

    #[test]
    fn the_commit_is_found_by_its_object_id() {
        let mut view = RepoView::new(repo("demo"));
        view.set_commits(vec![commit("aaa", "ada", 0), commit("bbb", "bob", 2)]);
        assert_eq!(
            view.commit("bbb").map(|commit| commit.author.as_str()),
            Some("bob")
        );
        assert!(view.commit("ccc").is_none());
    }

    #[test]
    fn the_row_of_a_commit_is_built_once() {
        let mut view = RepoView::new(repo("demo"));
        view.set_commits(vec![commit("aaa", "ada", 3)]);
        let row = view.row(0);
        assert_eq!(row.detail, "ada · aaa · +12 −3");
        assert_eq!(row.files.as_deref(), Some("3 файла"));

        // The row of a commit that touches no file has no number of files.
        view.set_commits(vec![commit("bbb", "bob", 0)]);
        assert!(view.row(0).files.is_none());
    }

    #[test]
    fn the_number_of_the_unrated_commits_follows_the_assessments() {
        let mut view = RepoView::new(repo("demo"));
        view.set_commits(vec![commit("aaa", "ada", 0), commit("bbb", "bob", 0)]);
        assert_eq!(view.unassessed(), 2);

        view.insert_assessment(assessment("aaa"));
        assert_eq!(view.unassessed(), 1);

        // The same assessment again does not change the number.
        view.insert_assessment(assessment("aaa"));
        assert_eq!(view.unassessed(), 1);

        // An assessment of a commit that is not pending changes nothing.
        view.insert_assessment(assessment("ccc"));
        assert_eq!(view.unassessed(), 1);

        // A new list of commits counts the ones that are not rated yet.
        view.set_commits(vec![commit("aaa", "ada", 0), commit("bbb", "bob", 0)]);
        assert_eq!(view.unassessed(), 1);
    }

    #[test]
    fn the_pipeline_state_follows_the_events() {
        assert_eq!(
            run_state_for(&RunEvent::Started {
                sha: "a".into(),
                branch: "b".into(),
                model: "m".into(),
            }),
            Some(RunState::Implementing)
        );
        assert_eq!(
            run_state_for(&RunEvent::PromptSent {
                role: AgentRole::Reviewer,
                prompt: "p".into(),
            }),
            Some(RunState::Reviewing)
        );
        assert_eq!(
            run_state_for(&RunEvent::PromptSent {
                role: AgentRole::Verifier,
                prompt: "p".into(),
            }),
            Some(RunState::Verifying)
        );
        assert_eq!(
            run_state_for(&RunEvent::Rejected {
                cycle: 1,
                feedback: "no".into(),
            }),
            Some(RunState::Rework)
        );
        assert_eq!(
            run_state_for(&RunEvent::Merged {
                branch: "b".into(),
                main_branch: "main".into(),
            }),
            Some(RunState::Merged)
        );
        assert_eq!(
            run_state_for(&RunEvent::Failed {
                message: "boom".into(),
            }),
            Some(RunState::Failed)
        );
        assert_eq!(
            run_state_for(&RunEvent::AgentThought {
                role: AgentRole::Implementer,
                text: "hmm".into(),
            }),
            None
        );
    }

    #[test]
    fn prompts_are_extracted_from_the_events() {
        let event = RunEvent::PromptSent {
            role: AgentRole::Implementer,
            prompt: "implement this".to_string(),
        };
        assert_eq!(prompt_of(&event), "implement this");
        assert_eq!(agent_name(&event), "implementer");
        assert!(!output_of(&event).is_empty());

        let rejected = RunEvent::Rejected {
            cycle: 2,
            feedback: "still wrong".to_string(),
        };
        assert_eq!(output_of(&rejected), "still wrong");
        assert_eq!(agent_name(&rejected), "pipeline");
    }

    #[test]
    fn timestamps_are_formatted() {
        assert_eq!(format_time(0), "1970-01-01 00:00:00");
        assert_eq!(format_time(1_700_000_000), "2023-11-14 22:13:20");
    }

    /// The relative luminance of a colour, following the sRGB definition of WCAG.
    fn relative_luminance(color: egui::Color32) -> f32 {
        let linear = |channel: u8| {
            let value = channel as f32 / 255.0;
            if value <= 0.03928 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        };
        let r = linear(color.r());
        let g = linear(color.g());
        let b = linear(color.b());
        0.2126 * r + 0.7152 * g + 0.0722 * b
    }

    /// The contrast ratio of a colour against white, following WCAG.
    fn contrast_against_white(color: egui::Color32) -> f32 {
        let luminance = relative_luminance(color);
        (1.0 + 0.05) / (luminance + 0.05)
    }

    /// The smallest contrast ratio a badge colour must keep against its white
    /// letter, following WCAG AA for normal-sized text.
    const BADGE_MIN_CONTRAST: f32 = 4.5;

    #[test]
    fn the_badges_have_distinct_colours() {
        assert_ne!(
            criticality_color(Criticality::Critical),
            criticality_color(Criticality::Trivial)
        );
        assert_ne!(
            complexity_color(Complexity::Expert),
            complexity_color(Complexity::Trivial)
        );
    }

    #[test]
    fn every_badge_colour_keeps_a_usable_contrast_with_its_letter() {
        let criticalities = [
            Criticality::Critical,
            Criticality::High,
            Criticality::Medium,
            Criticality::Low,
            Criticality::Trivial,
        ];
        let complexities = [
            Complexity::Expert,
            Complexity::Hard,
            Complexity::Medium,
            Complexity::Easy,
            Complexity::Trivial,
        ];
        for criticality in criticalities {
            let ratio = contrast_against_white(criticality_color(criticality));
            assert!(
                ratio >= BADGE_MIN_CONTRAST,
                "criticality {criticality:?} has contrast {ratio:.2}:1, below {BADGE_MIN_CONTRAST}:1"
            );
        }
        for complexity in complexities {
            let ratio = contrast_against_white(complexity_color(complexity));
            assert!(
                ratio >= BADGE_MIN_CONTRAST,
                "complexity {complexity:?} has contrast {ratio:.2}:1, below {BADGE_MIN_CONTRAST}:1"
            );
        }
    }
}
