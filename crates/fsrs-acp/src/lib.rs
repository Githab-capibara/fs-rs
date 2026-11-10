//! Agent Client Protocol client and implementation pipeline for `fs-rs`.
//!
//! The dashboard never drives an agent through its own API: every
//! interaction goes through the [Agent Client Protocol](https://agentclientprotocol.com).
//!
//! * [`agent`] starts the agent process, opens one session per agent role,
//!   selects the model the user picked and sends the prompts,
//! * [`prompts`] builds the visible prompts of the three agents,
//! * [`pipeline`] runs the implement → review/fix → verify cycle until the
//!   verifier approves the implementation,
//! * [`events`] reports every step so that the GUI can show it,
//! * [`models`] discovers the models the agent exposes,
//! * [`terminal`] serves the `terminal/*` requests with real pseudo terminals,
//! * [`mock`] is a test double that speaks the same protocol.
//!
//! # Example
//!
//! ```no_run
//! use fsrs_acp::{AgentConfig, AgentConnection};
//!
//! # async fn example() -> Result<(), fsrs_acp::AcpError> {
//! let config = AgentConfig::new("opencode acp", "/srv/fork");
//! AgentConnection::run(&config, None, async move |connection| {
//!     connection.initialize().await?;
//!     Ok(())
//! })
//! .await
//! # }
//! ```

#![warn(missing_docs)]

pub mod agent;
pub mod events;
pub mod mock;
pub mod models;
pub mod pipeline;
pub mod prompts;
pub mod terminal;

pub use agent::{
    AcpError, AgentConfig, AgentConnection, AgentSession, DEFAULT_AGENT_COMMAND, PromptOutcome,
    discover_models,
};
pub use events::{AgentRole, RunEvent};
pub use models::{
    FORBIDDEN_MODELS, MODEL_CONFIG_ID, ModelChoice, is_forbidden, model_config_id,
    models_from_options,
};
pub use pipeline::{
    BRANCH_PREFIX, DEFAULT_MAX_CYCLES, PipelineError, PipelineOutcome, PipelineRequest,
    branch_name, run_pipeline,
};
pub use prompts::{
    MAX_PROMPT_DIFF_CHARS, VERDICT_MARKER, Verdict, implementer_prompt, parse_verdict,
    reviewer_prompt, rework_prompt, truncate_diff, verifier_prompt,
};
pub use terminal::{TerminalError, TerminalOutput, TerminalRegistry};
