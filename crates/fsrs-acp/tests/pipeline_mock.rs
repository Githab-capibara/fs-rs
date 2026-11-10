//! End-to-end test of the three-agent pipeline against a mock agent.
//!
//! The mock agent speaks the Agent Client Protocol over standard input and
//! output, so the whole path of the pipeline is exercised: the protocol, the
//! three sessions, the visible prompts, the rework loop and the merge into the
//! main branch.

use std::path::{Path, PathBuf};
use std::process::Command;

use fsrs_acp::{AgentRole, PipelineError, PipelineRequest, RunEvent, run_pipeline};
use fsrs_core::UpstreamCommit;
use tokio::sync::mpsc::unbounded_channel;

/// The mock agent the pipeline talks to.
const MOCK_AGENT: &str = env!("CARGO_BIN_EXE_mock_agent");

/// Run a git command inside `dir` and assert it succeeded.
fn git(dir: &Path, arguments: &[&str]) {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(dir)
        .output()
        .expect("running git");
    assert!(
        output.status.success(),
        "git {arguments:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Create a repository with one commit on `main`.
fn make_repository(root: &Path) -> PathBuf {
    let path = root.join("fork");
    std::fs::create_dir_all(&path).unwrap();
    git(&path, &["init", "-q", "-b", "main"]);
    git(&path, &["config", "user.email", "fork@example.com"]);
    git(&path, &["config", "user.name", "Fork"]);
    std::fs::write(path.join("a.txt"), "one\n").unwrap();
    git(&path, &["add", "."]);
    git(&path, &["commit", "-q", "-m", "First commit"]);
    path
}

/// The upstream commit the pipeline ports.
fn upstream_commit() -> UpstreamCommit {
    UpstreamCommit {
        repo_id: 1,
        sha: "a".repeat(40),
        short_sha: "abcdef123456".to_string(),
        summary: "Add the second file".to_string(),
        body: "The upstream added b.txt.".to_string(),
        author: "Ada".to_string(),
        author_email: "ada@example.com".to_string(),
        authored_at: 0,
        committed_at: 0,
        files: vec!["b.txt".to_string()],
        insertions: 1,
        deletions: 0,
        url: "https://example.com/upstream/commit/aaaa".to_string(),
    }
}

/// Collect every event of the pipeline.
fn collect(receiver: &mut tokio::sync::mpsc::UnboundedReceiver<RunEvent>) -> Vec<RunEvent> {
    let mut events = Vec::new();
    while let Ok(event) = receiver.try_recv() {
        events.push(event);
    }
    events
}

/// The prompts that were sent, in order.
fn prompts(events: &[RunEvent]) -> Vec<(AgentRole, String)> {
    events
        .iter()
        .filter_map(|event| match event {
            RunEvent::PromptSent { role, prompt } => Some((*role, prompt.clone())),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn the_pipeline_implements_reviews_and_merges_the_branch() {
    let temp = tempfile::tempdir().unwrap();
    let repository = make_repository(temp.path());
    let (events, mut receiver) = unbounded_channel();

    let request = PipelineRequest::new(
        &repository,
        upstream_commit(),
        "--- a/b.txt\n+++ b/b.txt\n@@\n+two\n".to_string(),
        "main",
        "mock/model",
    )
    .with_agent_command(MOCK_AGENT)
    .with_max_cycles(3);

    let outcome = run_pipeline(request, events)
        .await
        .expect("the pipeline should succeed");

    assert_eq!(outcome.cycles, 1, "the verifier rejected exactly once");
    assert_eq!(outcome.main_branch, "main");
    assert_eq!(outcome.branch, "fsrs/implement/abcdef123456");

    // The port landed on the main branch.
    let git = fsrs_core::git::GitRepo::open(&repository).unwrap();
    assert_eq!(git.current_branch().unwrap(), "main");
    assert!(repository.join("ported.txt").exists());
    assert!(git.status().unwrap().is_empty());

    let events = collect(&mut receiver);

    // The pipeline reported that it started and that it merged the branch.
    assert!(matches!(events.first(), Some(RunEvent::Started { .. })));
    assert!(
        events
            .iter()
            .any(|event| matches!(event, RunEvent::Merged { .. }))
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, RunEvent::Approved { cycles: 1 }))
    );

    // The verifier rejected the first attempt.
    let rejection = events
        .iter()
        .find_map(|event| match event {
            RunEvent::Rejected { cycle, feedback } => Some((*cycle, feedback.clone())),
            _ => None,
        })
        .expect("the verifier should have rejected once");
    assert_eq!(rejection.0, 1);
    assert!(rejection.1.contains("test"));

    // The three agents received their prompts, and the reviewer was asked
    // again after the rejection.
    let prompts = prompts(&events);
    assert_eq!(prompts[0].0, AgentRole::Implementer);
    assert!(
        prompts[0]
            .1
            .contains("https://example.com/upstream/commit/")
    );
    assert!(prompts[0].1.contains("Add the second file"));
    assert!(prompts[0].1.contains("+two"));

    let reviewer_prompts: Vec<&(AgentRole, String)> = prompts
        .iter()
        .filter(|(role, _)| *role == AgentRole::Reviewer)
        .collect();
    assert_eq!(reviewer_prompts.len(), 2, "the reviewer runs twice");
    assert!(
        reviewer_prompts[0]
            .1
            .contains("cargo clippy -- -D warnings")
    );
    assert!(
        reviewer_prompts[1].1.contains("The verifier rejected"),
        "the second reviewer prompt carries the feedback"
    );
    assert!(
        reviewer_prompts[1]
            .1
            .contains("the test for the new behaviour")
    );

    let verifier_prompts: Vec<&(AgentRole, String)> = prompts
        .iter()
        .filter(|(role, _)| *role == AgentRole::Verifier)
        .collect();
    assert_eq!(verifier_prompts.len(), 2, "the verifier runs twice");
}

#[tokio::test]
async fn the_pipeline_gives_up_when_the_verifier_never_approves() {
    let temp = tempfile::tempdir().unwrap();
    let repository = make_repository(temp.path());
    let (events, mut receiver) = unbounded_channel();

    let request = PipelineRequest::new(
        &repository,
        upstream_commit(),
        String::new(),
        "main",
        "mock/model",
    )
    .with_agent_command(format!("{MOCK_AGENT} --rejections 10"))
    .with_max_cycles(1);

    let error = run_pipeline(request, events)
        .await
        .expect_err("the pipeline should give up");
    assert!(
        matches!(error, PipelineError::TooManyCycles { cycles: 2 }),
        "the error was {error:?}"
    );

    // The working copy is back on the main branch.
    let git = fsrs_core::git::GitRepo::open(&repository).unwrap();
    assert_eq!(git.current_branch().unwrap(), "main");

    let events = collect(&mut receiver);
    assert!(
        events
            .iter()
            .any(|event| matches!(event, RunEvent::Failed { .. }))
    );
}

#[tokio::test]
async fn the_pipeline_reports_a_missing_verdict() {
    let temp = tempfile::tempdir().unwrap();
    let repository = make_repository(temp.path());
    let (events, mut receiver) = unbounded_channel();

    // The mock never approves and never rejects: without a verdict the
    // pipeline cannot continue.
    let request = PipelineRequest::new(
        &repository,
        upstream_commit(),
        String::new(),
        "main",
        "mock/model",
    )
    .with_agent_command(format!("{MOCK_AGENT} --no-verdict"))
    .with_max_cycles(1);

    let error = run_pipeline(request, events)
        .await
        .expect_err("the pipeline should fail");
    assert!(
        matches!(error, PipelineError::MissingVerdict),
        "the error was {error:?}"
    );

    let _ = collect(&mut receiver);
}
