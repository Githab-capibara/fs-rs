//! Integration tests against a running `opencode acp` server.
//!
//! The tests are skipped automatically when OpenCode is not installed, so the
//! suite stays green on machines without the agent.
//!
//! Run them with:
//!
//! ```text
//! cargo test -p fsrs-acp --test live_opencode -- --nocapture
//! ```

use std::path::PathBuf;
use std::process::Stdio;

use fsrs_acp::{AgentConfig, AgentConnection, AgentRole};

/// Command line of the agent used by the tests.
const COMMAND: &str = "opencode acp";

/// The model the tests use.
///
/// The default model of OpenCode is a free one with a very small quota, so
/// the tests use an explicitly configured model instead. Override it with
/// `FSRS_TEST_MODEL`.
fn test_model() -> String {
    std::env::var("FSRS_TEST_MODEL").unwrap_or_else(|_| "hnc/MiniMax-M3.1-Flash".to_string())
}

/// Whether the agent is installed on this machine.
fn agent_available() -> bool {
    std::process::Command::new("sh")
        .arg("-c")
        .arg("command -v opencode")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// The directory the agents work in.
fn workspace() -> PathBuf {
    std::env::temp_dir().join("fsrs-acp-live")
}

#[tokio::test]
async fn the_agent_initialises_and_offers_its_models() {
    if !agent_available() {
        eprintln!("skipping: opencode is not installed");
        return;
    }
    let _ = std::fs::create_dir_all(workspace());

    let config = AgentConfig::new(COMMAND, workspace());
    let models = AgentConnection::run(&config, None, move |connection| async move {
        let response = connection.initialize().await?;
        eprintln!("protocol version: {:?}", response.protocol_version);

        let session = connection
            .new_session(&workspace(), AgentRole::Verifier)
            .await?;
        eprintln!("session: {}", session.session_id);
        eprintln!("models: {:?}", session.models);

        Ok::<_, fsrs_acp::AcpError>(session.models.len())
    })
    .await
    .expect("the agent should answer");

    assert!(models > 0, "the agent must offer at least one model");
}

#[tokio::test]
async fn the_agent_answers_a_prompt() {
    if !agent_available() {
        eprintln!("skipping: opencode is not installed");
        return;
    }
    let _ = std::fs::create_dir_all(workspace());

    let config = AgentConfig::new(COMMAND, workspace());
    let answer = AgentConnection::run(&config, None, move |connection| async move {
        connection.initialize().await?;
        let session = connection
            .new_session(&workspace(), AgentRole::Verifier)
            .await?;
        connection
            .select_model(&session.session_id, &test_model())
            .await?;
        let outcome = connection
            .prompt(
                &session.session_id,
                AgentRole::Verifier,
                "Answer with exactly the word READY and nothing else.",
            )
            .await?;
        Ok::<_, fsrs_acp::AcpError>(outcome.text)
    })
    .await
    .expect("the agent should answer");

    eprintln!("answer: {answer}");
    assert!(
        answer.to_uppercase().contains("READY"),
        "unexpected answer: {answer}"
    );
}

#[tokio::test]
async fn the_agent_can_use_a_selected_model() {
    if !agent_available() {
        eprintln!("skipping: opencode is not installed");
        return;
    }
    let _ = std::fs::create_dir_all(workspace());

    let config = AgentConfig::new(COMMAND, workspace());
    let (models, selected) = AgentConnection::run(&config, None, move |connection| async move {
        connection.initialize().await?;
        let session = connection
            .new_session(&workspace(), AgentRole::Verifier)
            .await?;
        let models = session.models.clone();
        let first = models
            .first()
            .map(|model| model.id.clone())
            .ok_or(fsrs_acp::AcpError::UnknownModel("no model".to_string()))?;
        connection.select_model(&session.session_id, &first).await?;
        Ok::<_, fsrs_acp::AcpError>((models, first))
    })
    .await
    .expect("the agent should accept the model");

    eprintln!("selected model: {selected}");
    assert!(!selected.is_empty());
    assert!(models.iter().any(|model| model.id == selected));
}
