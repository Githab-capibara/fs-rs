//! The state of the dashboard and the logic that does not need a window.
//!
//! Everything in this module is plain data or a pure function, so it can be
//! tested without starting a graphical interface.

use std::collections::HashMap;

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

/// One repository of the central list.
#[derive(Debug, Clone)]
pub struct RepoView {
    /// The stored repository.
    pub repo: Repo,
    /// The upstream commits that are missing from the fork.
    pub commits: Vec<UpstreamCommit>,
    /// The assessments of those commits, by commit id.
    pub assessments: HashMap<String, Assessment>,
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
            syncing: false,
            error: None,
        }
    }

    /// The assessment of one commit, when it is already known.
    pub fn assessment(&self, sha: &str) -> Option<&Assessment> {
        self.assessments.get(sha)
    }

    /// The number of upstream commits that are still missing.
    pub fn pending(&self) -> usize {
        self.commits.len()
    }

    /// The number of commits that still wait for an assessment.
    pub fn unassessed(&self) -> usize {
        self.commits
            .iter()
            .filter(|commit| !self.assessments.contains_key(&commit.sha))
            .count()
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
        }
    }

    /// The models that match the search text of the user.
    pub fn filtered<'a>(&self, models: &'a [ModelChoice]) -> Vec<&'a ModelChoice> {
        let needle = self.filter.trim().to_lowercase();
        models
            .iter()
            .filter(|model| {
                if needle.is_empty() {
                    return true;
                }
                model.id.to_lowercase().contains(&needle)
                    || model.name.to_lowercase().contains(&needle)
            })
            .collect()
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
#[derive(Debug, Clone)]
pub struct PipelineLog {
    /// Unix timestamp of the event.
    pub at: i64,
    /// The event itself.
    pub event: RunEvent,
}

impl PipelineLog {
    /// Build a log entry.
    pub fn new(at: i64, event: RunEvent) -> Self {
        Self { at, event }
    }

    /// The agent the event belongs to, when it has one.
    pub fn role(&self) -> Option<AgentRole> {
        self.event.role()
    }

    /// The one-line summary shown in the log.
    pub fn summary(&self) -> String {
        self.event.summary()
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
pub fn criticality_color(criticality: fsrs_core::Criticality) -> egui::Color32 {
    use fsrs_core::Criticality;
    match criticality {
        Criticality::Critical => egui::Color32::from_rgb(180, 30, 30),
        Criticality::High => egui::Color32::from_rgb(200, 90, 20),
        Criticality::Medium => egui::Color32::from_rgb(190, 150, 20),
        Criticality::Low => egui::Color32::from_rgb(70, 140, 70),
        Criticality::Trivial => egui::Color32::from_rgb(90, 90, 100),
    }
}

/// The colour of a complexity badge.
pub fn complexity_color(complexity: fsrs_core::Complexity) -> egui::Color32 {
    use fsrs_core::Complexity;
    match complexity {
        Complexity::Expert => egui::Color32::from_rgb(150, 40, 190),
        Complexity::Hard => egui::Color32::from_rgb(60, 90, 190),
        Complexity::Medium => egui::Color32::from_rgb(40, 140, 170),
        Complexity::Easy => egui::Color32::from_rgb(70, 150, 90),
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
        assert_eq!(filtered[0].id, "hnc/MiniMax-M3.1-Flash");

        picker.filter = "codex".to_string();
        assert_eq!(picker.filtered(&models).len(), 1);

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
    fn the_chosen_model_is_found_again() {
        let models = vec![model("a/1", "One"), model("b/2", "Two")];
        let mut picker = ModelPicker::new(PickerIntent::Terminal, None);
        picker.select("b/2");
        assert_eq!(picker.chosen(&models).map(|m| m.id.as_str()), Some("b/2"));
        assert!(picker.chosen(&[]).is_none());
    }

    #[test]
    fn a_repository_counts_its_missing_commits() {
        let repo = Repo {
            id: 1,
            name: "demo".to_string(),
            path: std::path::PathBuf::from("/tmp/demo"),
            upstream_url: String::new(),
            upstream_web: String::new(),
            main_branch: "main".to_string(),
            pending_commits: 0,
            last_fetched_at: None,
        };
        let mut view = RepoView::new(repo);
        assert_eq!(view.pending(), 0);
        assert_eq!(view.unassessed(), 0);

        view.commits.push(UpstreamCommit {
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
        });
        assert_eq!(view.pending(), 1);
        assert_eq!(view.unassessed(), 1);
        assert!(view.assessment("a").is_none());
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
}
