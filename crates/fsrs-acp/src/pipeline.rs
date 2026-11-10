//! The three-agent implementation pipeline.
//!
//! The pipeline ports one upstream commit into a fork:
//!
//! 1. the **implementer** writes the port of the upstream change,
//! 2. the **reviewer** runs the fast, modern linters and fixes everything
//!    they report,
//! 3. the **verifier** judges the result and approves or rejects it.
//!
//! When the verifier rejects, the feedback goes back to the **same** reviewer
//! session — it keeps the context of its previous work — and the cycle starts
//! again. Only an approval lets the branch be merged into the main branch.

use std::path::PathBuf;

use fsrs_core::UpstreamCommit;
use fsrs_core::git::GitRepo;
use tokio::sync::mpsc::UnboundedSender;

use crate::agent::{AcpError, AgentConfig, AgentConnection};
use crate::events::{AgentRole, RunEvent};
use crate::prompts::{self, Verdict};

/// Prefix of the branches created by the pipeline.
pub const BRANCH_PREFIX: &str = "fsrs/implement";

/// Default number of rework cycles before the pipeline gives up.
pub const DEFAULT_MAX_CYCLES: u32 = 5;

/// Description of one implementation run.
#[derive(Debug, Clone)]
pub struct PipelineRequest {
    /// Path of the working copy of the fork.
    pub repo_path: PathBuf,
    /// The upstream commit that has to be ported.
    pub commit: UpstreamCommit,
    /// The diff of the upstream commit.
    pub patch: String,
    /// The branch the implementation is created on.
    pub branch: String,
    /// The branch the implementation is merged into.
    pub main_branch: String,
    /// The model every agent of the run uses.
    pub model: String,
    /// Command line that starts the agent.
    pub agent_command: String,
    /// How many rework cycles are allowed.
    pub max_cycles: u32,
}

impl PipelineRequest {
    /// Build a request with the default branch name and cycle limit.
    pub fn new(
        repo_path: impl Into<PathBuf>,
        commit: UpstreamCommit,
        patch: String,
        main_branch: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        let branch = branch_name(&commit.short_sha);
        Self {
            repo_path: repo_path.into(),
            commit,
            patch,
            branch,
            main_branch: main_branch.into(),
            model: model.into(),
            agent_command: crate::agent::DEFAULT_AGENT_COMMAND.to_string(),
            max_cycles: DEFAULT_MAX_CYCLES,
        }
    }

    /// Override the command line that starts the agent.
    pub fn with_agent_command(mut self, command: impl Into<String>) -> Self {
        self.agent_command = command.into();
        self
    }

    /// Override the number of allowed rework cycles.
    pub fn with_max_cycles(mut self, cycles: u32) -> Self {
        self.max_cycles = cycles;
        self
    }
}

/// Result of a successful pipeline run.
#[derive(Debug, Clone)]
pub struct PipelineOutcome {
    /// The branch the implementation was created on.
    pub branch: String,
    /// The branch the implementation was merged into.
    pub main_branch: String,
    /// How many rework cycles were needed.
    pub cycles: u32,
    /// The commit the main branch points at after the merge.
    pub head: String,
}

/// Errors raised by the pipeline.
#[derive(Debug, thiserror::Error)]
pub enum PipelineError {
    /// A git operation failed.
    #[error("git failed: {0}")]
    Git(#[from] anyhow::Error),
    /// The agent protocol failed.
    #[error("the agent failed: {0}")]
    Agent(#[from] AcpError),
    /// The verifier did not produce a verdict.
    #[error("the verifier did not return a verdict")]
    MissingVerdict,
    /// The pipeline ran out of rework cycles.
    #[error("the implementation was not approved after {cycles} cycle(s)")]
    TooManyCycles {
        /// How many cycles were performed.
        cycles: u32,
    },
}

impl From<PipelineError> for agent_client_protocol::Error {
    fn from(error: PipelineError) -> Self {
        agent_client_protocol::Error::new(
            agent_client_protocol::ErrorCode::INTERNAL_ERROR,
            error.to_string(),
        )
    }
}

/// The name of the branch that carries one implementation.
pub fn branch_name(short_sha: &str) -> String {
    let clean: String = short_sha
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .take(12)
        .collect();
    if clean.is_empty() {
        format!("{BRANCH_PREFIX}/commit")
    } else {
        format!("{BRANCH_PREFIX}/{clean}")
    }
}

/// Run the whole pipeline.
pub async fn run_pipeline(
    request: PipelineRequest,
    events: UnboundedSender<RunEvent>,
) -> Result<PipelineOutcome, PipelineError> {
    let repo = GitRepo::open(&request.repo_path)?;

    // The implementation is isolated on its own branch.
    repo.create_branch(&request.branch, &request.main_branch)?;

    let started = RunEvent::Started {
        sha: request.commit.sha.clone(),
        branch: request.branch.clone(),
        model: request.model.clone(),
    };
    let _ = events.send(started);

    let result = drive_agents(&request, events.clone()).await;

    let outcome = match result {
        Ok(outcome) => outcome,
        Err(error) => {
            let _ = events.send(RunEvent::Failed {
                message: error.to_string(),
            });
            // Leave the working copy on the main branch again.
            let _ = repo.checkout(&request.main_branch);
            return Err(error);
        }
    };

    // The implementation was approved: merge it into the main branch.
    repo.checkout(&request.main_branch)?;
    // The agents are told to commit everything; anything left over is
    // committed as well so that the fast-forward merge stays possible.
    if repo.has_local_changes()? {
        repo.commit_all(&format!(
            "chore: leftover changes of the port of {}",
            request.commit.short_sha
        ))?;
    }
    repo.merge_fast_forward(&request.branch)?;
    let head = repo.current_branch()?;

    let _ = events.send(RunEvent::Merged {
        branch: request.branch.clone(),
        main_branch: request.main_branch.clone(),
    });
    let _ = events.send(RunEvent::Approved {
        cycles: outcome.cycles,
    });

    Ok(PipelineOutcome {
        branch: request.branch,
        main_branch: request.main_branch,
        cycles: outcome.cycles,
        head,
    })
}

/// Run the three agents until the verifier approves the implementation.
async fn drive_agents(
    request: &PipelineRequest,
    events: UnboundedSender<RunEvent>,
) -> Result<PipelineOutcome, PipelineError> {
    let config = AgentConfig::new(request.agent_command.clone(), request.repo_path.clone());

    AgentConnection::run(&config, Some(events.clone()), move |connection: AgentConnection| async move {
        connection.initialize().await?;

        let implementer = connection
            .new_session(&request.repo_path, AgentRole::Implementer)
            .await?;
        let reviewer = connection
            .new_session(&request.repo_path, AgentRole::Reviewer)
            .await?;
        let verifier = connection
            .new_session(&request.repo_path, AgentRole::Verifier)
            .await?;

        for session in [&implementer, &reviewer, &verifier] {
            if !request.model.is_empty() {
                connection
                    .select_model(&session.session_id, &request.model)
                    .await?;
            }
        }

        // 1. The implementer ports the upstream commit.
        let implement_prompt = prompts::implementer_prompt(&request.commit, &request.patch);
        let implemented = connection
            .prompt(
                &implementer.session_id,
                AgentRole::Implementer,
                &implement_prompt,
            )
            .await?;
        let _ = events.send(RunEvent::AgentFinished {
            role: AgentRole::Implementer,
            stop_reason: format!("{:?}", implemented.stop_reason),
            text: implemented.text,
        });

        // 2. and 3. The reviewer fixes, the verifier judges. Both sessions
        //    continue across the cycles, so the reviewer keeps the context of
        //    its previous work when the verifier rejects.
        let mut cycles = 0_u32;
        let mut feedback: Option<String> = None;

        loop {
            let review_prompt = match feedback.take() {
                Some(reason) => prompts::rework_prompt(&reason),
                None => prompts::reviewer_prompt(&request.commit),
            };
            let reviewed = connection
                .prompt(&reviewer.session_id, AgentRole::Reviewer, &review_prompt)
                .await?;
            let _ = events.send(RunEvent::AgentFinished {
                role: AgentRole::Reviewer,
                stop_reason: format!("{:?}", reviewed.stop_reason),
                text: reviewed.text,
            });

            let verify_prompt = prompts::verifier_prompt(&request.commit);
            let verified = connection
                .prompt(&verifier.session_id, AgentRole::Verifier, &verify_prompt)
                .await?;
            let _ = events.send(RunEvent::AgentFinished {
                role: AgentRole::Verifier,
                stop_reason: format!("{:?}", verified.stop_reason),
                text: verified.text.clone(),
            });

            match prompts::parse_verdict(&verified.text) {
                Some(Verdict::Approved) => {
                    break;
                }
                Some(Verdict::Rejected(reasons)) => {
                    cycles += 1;
                    if cycles > request.max_cycles {
                        return Err(PipelineError::TooManyCycles { cycles });
                    }
                    let reasons = if reasons.trim().is_empty() {
                        format!(
                            "The verifier rejected the implementation at cycle {cycles} without \
                             giving a reason. Re-check the port against the upstream commit."
                        )
                    } else {
                        reasons
                    };
                    let _ = events.send(RunEvent::Rejected {
                        cycle: cycles,
                        feedback: reasons.clone(),
                    });
                    feedback = Some(reasons);
                }
                None => return Err(PipelineError::MissingVerdict),
            }
        }

        Ok(PipelineOutcome {
            branch: request.branch.clone(),
            main_branch: request.main_branch.clone(),
            cycles,
            head: String::new(),
        })
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_branch_name_is_derived_from_the_commit() {
        assert_eq!(branch_name("abc1234def"), "fsrs/implement/abc1234def");
    }

    #[test]
    fn the_branch_name_is_shortened() {
        assert_eq!(
            branch_name("0123456789abcdef0123"),
            "fsrs/implement/0123456789ab"
        );
    }

    #[test]
    fn a_branch_name_without_usable_characters_still_works() {
        assert_eq!(branch_name("---"), "fsrs/implement/commit");
    }

    #[test]
    fn the_default_request_uses_the_default_branch_and_cycles() {
        let commit = UpstreamCommit {
            repo_id: 1,
            sha: "a".repeat(40),
            short_sha: "abc1234".to_string(),
            summary: "Fix".to_string(),
            body: String::new(),
            author: "Ada".to_string(),
            author_email: "ada@example.com".to_string(),
            authored_at: 0,
            committed_at: 0,
            files: Vec::new(),
            insertions: 0,
            deletions: 0,
            url: String::new(),
        };
        let request = PipelineRequest::new("/tmp", commit, String::new(), "main", "hnc/model");
        assert_eq!(request.branch, "fsrs/implement/abc1234");
        assert_eq!(request.main_branch, "main");
        assert_eq!(request.max_cycles, DEFAULT_MAX_CYCLES);
    }
}
