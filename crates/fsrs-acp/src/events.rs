//! Events emitted while the implementation pipeline runs.
//!
//! The pipeline is driven from a GUI, so every observable step is reported as
//! an event that can be rendered, persisted and replayed.

use serde::{Deserialize, Serialize};

/// Role of an agent inside the pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentRole {
    /// Writes the implementation of the upstream change.
    Implementer,
    /// Runs the linters and fixes everything they report.
    Reviewer,
    /// Judges the result and approves or rejects it.
    Verifier,
}

impl AgentRole {
    /// Stable identifier used in the database and in the UI.
    pub fn as_str(self) -> &'static str {
        match self {
            AgentRole::Implementer => "implementer",
            AgentRole::Reviewer => "reviewer",
            AgentRole::Verifier => "verifier",
        }
    }

    /// Human readable label.
    pub fn label(self) -> &'static str {
        match self {
            AgentRole::Implementer => "Implementer",
            AgentRole::Reviewer => "Reviewer",
            AgentRole::Verifier => "Verifier",
        }
    }
}

/// One observable step of the pipeline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RunEvent {
    /// The pipeline started.
    Started {
        /// Commit being implemented.
        sha: String,
        /// Branch created for the implementation.
        branch: String,
        /// Model used by every agent.
        model: String,
    },
    /// A visible prompt was sent to an agent.
    PromptSent {
        /// The agent that received the prompt.
        role: AgentRole,
        /// The full prompt text.
        prompt: String,
    },
    /// The agent produced a message.
    AgentMessage {
        /// The agent that produced the message.
        role: AgentRole,
        /// The message text.
        text: String,
    },
    /// The agent is reasoning.
    AgentThought {
        /// The agent that is reasoning.
        role: AgentRole,
        /// The reasoning text.
        text: String,
    },
    /// The agent called a tool.
    ToolCall {
        /// The agent that called the tool.
        role: AgentRole,
        /// Human readable title of the call.
        title: String,
        /// Current status of the call.
        status: String,
    },
    /// The agent asked for a permission.
    Permission {
        /// The agent that asked.
        role: AgentRole,
        /// What the agent wanted to do.
        title: String,
        /// How the request was answered.
        outcome: String,
    },
    /// The agent answered a prompt.
    AgentFinished {
        /// The agent that finished.
        role: AgentRole,
        /// Why the turn ended.
        stop_reason: String,
        /// Everything the agent said during the turn.
        text: String,
    },
    /// The verifier approved the implementation.
    Approved {
        /// Number of rework cycles that were needed.
        cycles: u32,
    },
    /// The verifier rejected the implementation.
    Rejected {
        /// Number of the cycle that was rejected.
        cycle: u32,
        /// The feedback sent back to the reviewer.
        feedback: String,
    },
    /// The branch was merged into the main branch.
    Merged {
        /// The branch that was merged.
        branch: String,
        /// The main branch it was merged into.
        main_branch: String,
    },
    /// The pipeline failed.
    Failed {
        /// Description of the failure.
        message: String,
    },
}

impl RunEvent {
    /// The role the event belongs to, when it has one.
    pub fn role(&self) -> Option<AgentRole> {
        match self {
            RunEvent::PromptSent { role, .. }
            | RunEvent::AgentMessage { role, .. }
            | RunEvent::AgentThought { role, .. }
            | RunEvent::ToolCall { role, .. }
            | RunEvent::Permission { role, .. }
            | RunEvent::AgentFinished { role, .. } => Some(*role),
            _ => None,
        }
    }

    /// Short one-line summary, used in compact lists.
    pub fn summary(&self) -> String {
        match self {
            RunEvent::Started { branch, model, .. } => {
                format!("Started on {branch} with {model}")
            }
            RunEvent::PromptSent { role, .. } => format!("Prompt → {}", role.label()),
            RunEvent::AgentMessage { role, text } => {
                format!("{}: {}", role.label(), first_line(text))
            }
            RunEvent::AgentThought { role, .. } => format!("{} thinking", role.label()),
            RunEvent::ToolCall { role, title, .. } => format!("{} tool: {title}", role.label()),
            RunEvent::Permission {
                role,
                title,
                outcome,
            } => {
                format!("{} permission {outcome}: {title}", role.label())
            }
            RunEvent::AgentFinished {
                role, stop_reason, ..
            } => {
                format!("{} finished ({stop_reason})", role.label())
            }
            RunEvent::Approved { cycles } => format!("Approved after {cycles} cycle(s)"),
            RunEvent::Rejected { cycle, .. } => format!("Rejected at cycle {cycle}"),
            RunEvent::Merged {
                branch,
                main_branch,
            } => {
                format!("Merged {branch} into {main_branch}")
            }
            RunEvent::Failed { message } => format!("Failed: {message}"),
        }
    }
}

/// The first line of a text, shortened for compact display.
fn first_line(text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");
    if line.chars().count() > 120 {
        let shortened: String = line.chars().take(117).collect();
        format!("{shortened}…")
    } else {
        line.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles_have_stable_identifiers() {
        assert_eq!(AgentRole::Implementer.as_str(), "implementer");
        assert_eq!(AgentRole::Reviewer.as_str(), "reviewer");
        assert_eq!(AgentRole::Verifier.as_str(), "verifier");
    }

    #[test]
    fn events_report_their_role() {
        let event = RunEvent::AgentMessage {
            role: AgentRole::Verifier,
            text: "looks good".to_string(),
        };
        assert_eq!(event.role(), Some(AgentRole::Verifier));
        assert!(event.summary().contains("Verifier"));
    }

    #[test]
    fn global_events_have_no_role() {
        assert_eq!(RunEvent::Approved { cycles: 0 }.role(), None);
    }

    #[test]
    fn summaries_use_the_first_non_empty_line() {
        let event = RunEvent::AgentMessage {
            role: AgentRole::Implementer,
            text: "\n\n  first line\nsecond line".to_string(),
        };
        assert_eq!(event.summary(), "Implementer: first line");
    }

    #[test]
    fn long_summaries_are_shortened() {
        let event = RunEvent::AgentMessage {
            role: AgentRole::Implementer,
            text: "x".repeat(500),
        };
        assert!(event.summary().ends_with('…'));
        assert!(event.summary().len() < 140);
    }

    #[test]
    fn events_roundtrip_through_json() {
        let event = RunEvent::ToolCall {
            role: AgentRole::Reviewer,
            title: "cargo clippy".to_string(),
            status: "completed".to_string(),
        };
        let json = serde_json::to_string(&event).unwrap();
        let parsed: RunEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(event, parsed);
    }
}
