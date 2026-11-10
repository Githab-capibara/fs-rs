//! Core domain model for fs-rs.
//!
//! Holds the vocabulary shared by every layer: repositories tracked as forks of
//! upstream projects, the upstream commits that are not yet in the fork's `main`,
//! the AI assessments attached to those commits and the persisted state of
//! implementation runs.

use std::fmt;
use std::path::PathBuf;

pub mod catalog;
pub mod git;
mod store;

pub use store::{Store, now_unix};

/// Criticality of an upstream commit, as judged by the local decision model.
///
/// Ordered from most to least severe; `Ord` follows declaration order.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum Criticality {
    /// Breaks the project, data loss, security hole.
    Critical,
    /// Major user-visible breakage.
    High,
    /// Bug fix or notable behaviour change.
    Medium,
    /// Minor improvement.
    Low,
    /// Cosmetic or no-op change.
    Trivial,
}

impl Criticality {
    /// Stable ordering key used by the UI and the database.
    pub const ALL: [Criticality; 5] = [
        Criticality::Critical,
        Criticality::High,
        Criticality::Medium,
        Criticality::Low,
        Criticality::Trivial,
    ];

    /// Stable identifier persisted in the database.
    pub fn as_str(self) -> &'static str {
        match self {
            Criticality::Critical => "critical",
            Criticality::High => "high",
            Criticality::Medium => "medium",
            Criticality::Low => "low",
            Criticality::Trivial => "trivial",
        }
    }

    /// Human readable label.
    pub fn label(self) -> &'static str {
        match self {
            Criticality::Critical => "Critical",
            Criticality::High => "High",
            Criticality::Medium => "Medium",
            Criticality::Low => "Low",
            Criticality::Trivial => "Trivial",
        }
    }

    /// Parse a label or snake_case identifier.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "critical" => Some(Criticality::Critical),
            "high" => Some(Criticality::High),
            "medium" => Some(Criticality::Medium),
            "low" => Some(Criticality::Low),
            "trivial" => Some(Criticality::Trivial),
            _ => None,
        }
    }

    /// Short single-character badge used in dense commit tables.
    pub fn badge(self) -> &'static str {
        match self {
            Criticality::Critical => "C",
            Criticality::High => "H",
            Criticality::Medium => "M",
            Criticality::Low => "L",
            Criticality::Trivial => "T",
        }
    }
}

impl fmt::Display for Criticality {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// Implementation effort needed to port an upstream commit into the fork.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum Complexity {
    /// Single trivial edit.
    Trivial,
    /// Localised change, no design decisions.
    Easy,
    /// Multi-file change, needs some design work.
    Medium,
    /// Cross-cutting change, risky.
    Hard,
    /// Architectural change, needs deep context.
    Expert,
}

impl Complexity {
    /// Stable ordering key used by the UI and the database.
    pub const ALL: [Complexity; 5] = [
        Complexity::Trivial,
        Complexity::Easy,
        Complexity::Medium,
        Complexity::Hard,
        Complexity::Expert,
    ];

    /// Stable identifier persisted in the database.
    pub fn as_str(self) -> &'static str {
        match self {
            Complexity::Trivial => "trivial",
            Complexity::Easy => "easy",
            Complexity::Medium => "medium",
            Complexity::Hard => "hard",
            Complexity::Expert => "expert",
        }
    }

    /// Human readable label.
    pub fn label(self) -> &'static str {
        match self {
            Complexity::Trivial => "Trivial",
            Complexity::Easy => "Easy",
            Complexity::Medium => "Medium",
            Complexity::Hard => "Hard",
            Complexity::Expert => "Expert",
        }
    }

    /// Parse a label or snake_case identifier.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "trivial" => Some(Complexity::Trivial),
            "easy" => Some(Complexity::Easy),
            "medium" => Some(Complexity::Medium),
            "hard" => Some(Complexity::Hard),
            "expert" => Some(Complexity::Expert),
            _ => None,
        }
    }

    /// Short single-character badge used in dense commit tables.
    pub fn badge(self) -> &'static str {
        match self {
            Complexity::Trivial => "T",
            Complexity::Easy => "E",
            Complexity::Medium => "M",
            Complexity::Hard => "H",
            Complexity::Expert => "X",
        }
    }
}

impl fmt::Display for Complexity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// A commit that exists upstream but is not yet ported into the fork's `main`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct UpstreamCommit {
    /// Repository this commit belongs to.
    pub repo_id: i64,
    /// Full git object id.
    pub sha: String,
    /// Short git object id.
    pub short_sha: String,
    /// First line of the commit message.
    pub summary: String,
    /// Full commit message body.
    pub body: String,
    /// Author name.
    pub author: String,
    /// Author email.
    pub author_email: String,
    /// Author timestamp, unix seconds.
    pub authored_at: i64,
    /// Committer timestamp, unix seconds.
    pub committed_at: i64,
    /// Files touched by the commit.
    pub files: Vec<String>,
    /// Number of inserted lines.
    pub insertions: u32,
    /// Number of deleted lines.
    pub deletions: u32,
    /// Browser URL of the commit in the upstream repository.
    pub url: String,
}

/// A repository tracked by the dashboard: a fork that follows an upstream project.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Repo {
    /// Database identifier.
    pub id: i64,
    /// Display name.
    pub name: String,
    /// Absolute path of the local working copy.
    pub path: PathBuf,
    /// Upstream remote URL (the original project).
    pub upstream_url: String,
    /// Human readable upstream URL used to build commit links.
    pub upstream_web: String,
    /// Branch that tracks upstream updates.
    pub main_branch: String,
    /// Number of upstream commits not yet present in `main_branch`.
    pub pending_commits: i64,
    /// Timestamp of the last successful fetch.
    pub last_fetched_at: Option<i64>,
}

/// Result of assessing one upstream commit with the local decision model.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Assessment {
    /// Repository the assessed commit belongs to.
    pub repo_id: i64,
    /// Commit object id.
    pub sha: String,
    /// Judged criticality.
    pub criticality: Criticality,
    /// Judged implementation complexity.
    pub complexity: Complexity,
    /// Calibrated confidence of the criticality decision, `0.0..=1.0`.
    pub criticality_confidence: f64,
    /// Calibrated confidence of the complexity decision, `0.0..=1.0`.
    pub complexity_confidence: f64,
    /// Set when the model flagged the decision as uncertain.
    pub needs_review: bool,
    /// Free-form reasoning captured with the decision.
    pub rationale: String,
    /// Model identifier that produced the decision.
    pub model: String,
    /// Unix timestamp of the assessment.
    pub created_at: i64,
}

/// State of an implementation run driven by the three-agent pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    /// Branch created, nothing sent to the implementer yet.
    Prepared,
    /// First agent is implementing the upstream change.
    Implementing,
    /// Second agent is fixing lint and test failures.
    Reviewing,
    /// Third agent is judging the result.
    Verifying,
    /// Verifier rejected the result; feedback goes back to the reviewer.
    Rework,
    /// Verifier approved the result.
    Approved,
    /// The run was merged into the fork's main branch.
    Merged,
    /// The run failed and cannot continue.
    Failed,
}

impl RunState {
    /// Stable identifier persisted in the database.
    pub fn as_str(self) -> &'static str {
        match self {
            RunState::Prepared => "prepared",
            RunState::Implementing => "implementing",
            RunState::Reviewing => "reviewing",
            RunState::Verifying => "verifying",
            RunState::Rework => "rework",
            RunState::Approved => "approved",
            RunState::Merged => "merged",
            RunState::Failed => "failed",
        }
    }

    /// Parse a persisted identifier.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "prepared" => Some(RunState::Prepared),
            "implementing" => Some(RunState::Implementing),
            "reviewing" => Some(RunState::Reviewing),
            "verifying" => Some(RunState::Verifying),
            "rework" => Some(RunState::Rework),
            "approved" => Some(RunState::Approved),
            "merged" => Some(RunState::Merged),
            "failed" => Some(RunState::Failed),
            _ => None,
        }
    }

    /// True while the pipeline still has work to do.
    pub fn is_active(self) -> bool {
        matches!(
            self,
            RunState::Prepared
                | RunState::Implementing
                | RunState::Reviewing
                | RunState::Verifying
                | RunState::Rework
        )
    }
}

/// One recorded prompt sent to one agent of the pipeline.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RunEvent {
    /// Run this event belongs to.
    pub run_id: i64,
    /// Which agent produced the event (`implementer`, `reviewer`, `verifier`).
    pub agent: String,
    /// Prompt text sent to the agent, empty for pure status events.
    pub prompt: String,
    /// Agent answer or status text.
    pub output: String,
    /// Unix timestamp of the event.
    pub created_at: i64,
}

/// A tracked implementation run of one upstream commit.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Run {
    /// Database identifier.
    pub id: i64,
    /// Repository the run belongs to.
    pub repo_id: i64,
    /// Commit being implemented.
    pub sha: String,
    /// Branch created for the implementation.
    pub branch: String,
    /// Current pipeline state.
    pub state: RunState,
    /// Number of verify/rework cycles completed.
    pub cycles: i64,
    /// Model used for every agent of this run.
    pub model: String,
    /// Unix timestamp of creation.
    pub created_at: i64,
    /// Unix timestamp of the last state change.
    pub updated_at: i64,
}

/// User configuration, loaded from `~/.config/fs-rs/config.toml`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Config {
    /// Tracked repositories.
    pub repos: Vec<RepoConfig>,
    /// Base URL of the Foq decision server.
    pub foq_url: String,
    /// Model alias reported by the Foq server.
    pub foq_model: String,
    /// Calibration profile of the local decision model.
    ///
    /// The `lora` profile is fitted for the model running with the decision
    /// adapter (`foq_decision_8b.gguf`); `bare` is fitted for the model alone.
    pub foq_profile: FoqProfile,
    /// Minimum calibrated confidence below which a decision is flagged for review.
    pub min_confidence: f64,
    /// Timeout of a single request to the decision server, in seconds.
    ///
    /// The local model needs a long time for a big diff on a slow accelerator,
    /// therefore the timeout is generous by default.
    pub foq_timeout_secs: u64,
    /// Command line that starts the OpenCode ACP agent.
    ///
    /// The agent is only ever driven through the Agent Client Protocol, so
    /// this has to be a command that speaks ACP, for example `opencode acp`.
    pub acp_command: String,
    /// Command line that runs the OpenCode CLI in the manual terminal.
    ///
    /// The full `opencode` user interface does not accept a `--model` flag,
    /// therefore the minimal interactive interface is used: it honours the
    /// model the user picked.
    pub terminal_command: String,
    /// GPU preference order for the local decision model.
    pub gpu_preference: Vec<GpuPreference>,
    /// Whether permission requests from agents are auto-approved.
    pub auto_approve: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            repos: Vec::new(),
            foq_url: "http://127.0.0.1:8089".to_string(),
            foq_model: "foq".to_string(),
            foq_profile: FoqProfile::Lora,
            min_confidence: 0.8,
            foq_timeout_secs: 120,
            acp_command: "opencode acp".to_string(),
            terminal_command: "opencode mini".to_string(),
            gpu_preference: vec![
                GpuPreference::Nvidia,
                GpuPreference::Intel,
                GpuPreference::Cpu,
            ],
            auto_approve: true,
        }
    }
}

/// Calibration profile applied to the decisions of the local model.
///
/// The two profiles come from the upstream Foq project and correspond to the
/// two ways the model can be served.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FoqProfile {
    /// Model served with the decision LoRA adapter; temperature `2.6353`.
    Lora,
    /// Model served alone; temperature `3.6381`.
    Bare,
}

impl FoqProfile {
    /// Human readable label.
    pub fn label(self) -> &'static str {
        match self {
            FoqProfile::Lora => "Foq 8B + decision adapter",
            FoqProfile::Bare => "Foq 8B",
        }
    }
}

/// How a local accelerator should be selected for the decision model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GpuPreference {
    /// NVIDIA discrete GPU.
    Nvidia,
    /// Intel integrated GPU through Vulkan.
    Intel,
    /// CPU fallback.
    Cpu,
}

impl GpuPreference {
    /// Human readable label.
    pub fn label(self) -> &'static str {
        match self {
            GpuPreference::Nvidia => "NVIDIA GPU",
            GpuPreference::Intel => "Intel GPU",
            GpuPreference::Cpu => "CPU",
        }
    }
}

/// Configuration of one tracked repository.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct RepoConfig {
    /// Display name.
    pub name: String,
    /// Absolute path of the local working copy.
    pub path: PathBuf,
    /// Upstream remote URL.
    pub upstream_url: String,
    /// Human readable upstream URL used to build commit links.
    pub upstream_web: String,
    /// Branch that tracks upstream updates.
    pub main_branch: String,
}

impl Default for RepoConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            path: PathBuf::new(),
            upstream_url: String::new(),
            upstream_web: String::new(),
            main_branch: "main".to_string(),
        }
    }
}

/// Configuration file locations and loading.
pub mod config {
    use std::path::{Path, PathBuf};

    use anyhow::{Context, Result};

    /// Default configuration file path.
    pub fn default_path() -> PathBuf {
        if let Ok(dir) = std::env::var("XDG_CONFIG_HOME") {
            PathBuf::from(dir).join("fs-rs").join("config.toml")
        } else {
            home_dir().join(".config").join("fs-rs").join("config.toml")
        }
    }

    /// Default state directory for the SQLite database.
    pub fn default_data_dir() -> PathBuf {
        if let Ok(dir) = std::env::var("XDG_DATA_HOME") {
            PathBuf::from(dir).join("fs-rs")
        } else {
            home_dir().join(".local").join("share").join("fs-rs")
        }
    }

    /// User home directory.
    pub fn home_dir() -> PathBuf {
        std::env::var("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("/"))
    }

    /// Load configuration from `path`, falling back to defaults when absent.
    pub fn load(path: &Path) -> Result<super::Config> {
        if !path.exists() {
            return Ok(super::Config::default());
        }
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("reading config {}", path.display()))?;
        let config: super::Config =
            toml::from_str(&raw).with_context(|| format!("parsing config {}", path.display()))?;
        Ok(config)
    }

    /// Persist configuration to `path`, creating parent directories.
    pub fn save(path: &Path, config: &super::Config) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let raw = toml::to_string_pretty(config).context("serializing config")?;
        std::fs::write(path, raw).with_context(|| format!("writing config {}", path.display()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn criticality_orders_by_severity() {
        assert!(Criticality::Critical < Criticality::High);
        assert!(Criticality::High < Criticality::Medium);
        assert!(Criticality::Trivial > Criticality::Critical);
    }

    #[test]
    fn criticality_roundtrips_through_parse() {
        for level in Criticality::ALL {
            assert_eq!(Criticality::parse(level.label()), Some(level));
            assert_eq!(
                Criticality::parse(&level.label().to_uppercase()),
                Some(level)
            );
        }
        assert_eq!(Criticality::parse("nope"), None);
    }

    #[test]
    fn complexity_roundtrips_through_parse() {
        for level in Complexity::ALL {
            assert_eq!(Complexity::parse(level.label()), Some(level));
        }
        assert_eq!(Complexity::parse("impossible"), None);
    }

    #[test]
    fn run_state_roundtrips_through_parse() {
        for state in [
            RunState::Prepared,
            RunState::Implementing,
            RunState::Reviewing,
            RunState::Verifying,
            RunState::Rework,
            RunState::Approved,
            RunState::Merged,
            RunState::Failed,
        ] {
            assert_eq!(RunState::parse(state.as_str()), Some(state));
        }
        assert_eq!(RunState::parse("unknown"), None);
    }

    #[test]
    fn run_state_activity_matches_pipeline_lifecycle() {
        assert!(RunState::Implementing.is_active());
        assert!(RunState::Rework.is_active());
        assert!(!RunState::Merged.is_active());
        assert!(!RunState::Failed.is_active());
    }

    #[test]
    fn the_default_configuration_targets_the_local_decision_model() {
        let config = super::Config::default();
        assert_eq!(config.foq_profile, super::FoqProfile::Lora);
        assert_eq!(config.foq_timeout_secs, 120);
        assert_eq!(config.acp_command, "opencode acp");
        assert_eq!(config.terminal_command, "opencode mini");
    }

    #[test]
    fn a_missing_timeout_falls_back_to_the_default() {
        let config: super::Config = toml::from_str("foq_url = \"http://127.0.0.1:8089\"").unwrap();
        assert_eq!(config.foq_timeout_secs, 120);
    }

    #[test]
    fn the_timeout_of_the_decision_model_can_be_set() {
        let config: super::Config = toml::from_str("foq_timeout_secs = 300").unwrap();
        assert_eq!(config.foq_timeout_secs, 300);
    }
}
