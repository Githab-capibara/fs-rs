//! Integration tests against a running Foq decision server.
//!
//! These tests are skipped automatically when no server is listening, so the
//! suite stays green on machines without the local model.

use fsrs_ai::{DecisionSchema, FoqClient};
use fsrs_core::{Complexity, Criticality, UpstreamCommit};

/// Default address of the local decision server.
const DEFAULT_URL: &str = "http://127.0.0.1:8089";

/// Build a client pointing at the server, if one is listening.
fn client() -> Option<FoqClient> {
    let client = FoqClient::new(DEFAULT_URL);
    client.is_ready().then_some(client)
}

fn commit() -> UpstreamCommit {
    UpstreamCommit {
        repo_id: 1,
        sha: "a".repeat(40),
        short_sha: "aaaaaaaa".to_string(),
        summary: "Fix buffer overflow in the TLS parser".to_string(),
        body: String::new(),
        author: "Upstream Author".to_string(),
        author_email: "author@example.com".to_string(),
        authored_at: 1_700_000_000,
        committed_at: 1_700_000_000,
        files: vec!["src/tls/parser.c".to_string()],
        insertions: 42,
        deletions: 17,
        url: "https://example.com/commit".to_string(),
    }
}

#[test]
fn the_server_answers_a_real_criticality_question() {
    let Some(client) = client() else {
        eprintln!("skipping: no Foq server on {DEFAULT_URL}");
        return;
    };
    let context = fsrs_ai::build_context(&commit(), None);
    let result = client
        .decide(&context, &DecisionSchema::criticality())
        .expect("the decision server should answer");

    assert!(["A", "B", "C", "D", "E"].contains(&result.decision.key.as_str()));
    assert!(result.decision.confidence > 0.0);
    assert!(result.decision.confidence <= 1.0);

    let sum: f64 = result.decision.probabilities.values().sum();
    assert!(
        (sum - 1.0).abs() < 1e-6,
        "probabilities must sum to one: {sum}"
    );
    assert_eq!(result.decision.probabilities.len(), 5);
    assert!(result.latency_ms > 0.0);
}

#[test]
fn a_security_fix_is_judged_critical() {
    let Some(client) = client() else {
        eprintln!("skipping: no Foq server on {DEFAULT_URL}");
        return;
    };
    let context = fsrs_ai::build_context(&commit(), None);
    let result = client
        .decide(&context, &DecisionSchema::criticality())
        .expect("the decision server should answer");
    let criticality = fsrs_ai::criticality_from_key(&result.decision.key).unwrap();
    assert_eq!(criticality, Criticality::Critical);
}

#[test]
fn the_server_answers_a_real_complexity_question() {
    let Some(client) = client() else {
        eprintln!("skipping: no Foq server on {DEFAULT_URL}");
        return;
    };
    let context = fsrs_ai::build_context(&commit(), None);
    let result = client
        .decide(&context, &DecisionSchema::complexity())
        .expect("the decision server should answer");
    let complexity = fsrs_ai::complexity_from_key(&result.decision.key).unwrap();
    assert!(matches!(
        complexity,
        Complexity::Expert
            | Complexity::Hard
            | Complexity::Medium
            | Complexity::Easy
            | Complexity::Trivial
    ));
}

#[test]
fn a_full_assessment_is_produced_for_a_commit() {
    let Some(client) = client() else {
        eprintln!("skipping: no Foq server on {DEFAULT_URL}");
        return;
    };
    let assessment = fsrs_ai::assess_commit(&client, &commit(), None).expect("assessment");
    assert_eq!(assessment.repo_id, 1);
    assert_eq!(assessment.sha, "a".repeat(40));
    assert!(!assessment.rationale.is_empty());
    assert_eq!(assessment.model, "foq");
    assert!(assessment.created_at > 0);
}

#[test]
fn a_patch_that_does_not_fit_into_the_window_is_shortened() {
    let Some(client) = client() else {
        eprintln!("skipping: no Foq server on {DEFAULT_URL}");
        return;
    };
    // A patch far larger than any context window of the model.
    let patch = "+ a line of the diff that is much too long to fit\n".repeat(2_000);
    let context = fsrs_ai::build_context(&commit(), Some(&patch));
    let result = client
        .decide(&context, &DecisionSchema::criticality())
        .expect("the decision server should answer even for a huge diff");
    assert!(["A", "B", "C", "D", "E"].contains(&result.decision.key.as_str()));
}

#[test]
fn the_window_of_the_server_is_discovered() {
    let Some(client) = client() else {
        eprintln!("skipping: no Foq server on {DEFAULT_URL}");
        return;
    };
    let size = client.context_size();
    assert!(size >= 512, "the window should be usable: {size}");
    // The size is remembered, so the second call costs nothing.
    assert_eq!(size, client.context_size());
}

#[test]
fn the_server_counts_the_tokens_of_a_text() {
    let Some(client) = client() else {
        eprintln!("skipping: no Foq server on {DEFAULT_URL}");
        return;
    };
    let short = client.count_tokens("hello");
    let long = client.count_tokens(&"hello ".repeat(100));
    assert!(short.is_some());
    assert!(long.unwrap() > short.unwrap());
}
