//! A mock agent that speaks the Agent Client Protocol.
//!
//! The mock is a test double: it answers the requests of the dashboard the
//! way a real agent would, but it does not think. It plays the three roles of
//! the pipeline:
//!
//! * the **implementer** writes a file and commits it,
//! * the **reviewer** reports that the linters pass,
//! * the **verifier** rejects the first attempt and approves the second one.
//!
//! It lives in the library instead of in a binary, so that every crate of the
//! workspace can start it: the integration tests of the pipeline use it
//! directly, and the tests of the dashboard start the `fsrs-mock-agent`
//! binary, which is a thin wrapper around [`serve`].

use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::v1::schema::{
    ContentBlock, ContentChunk, InitializeResponse, NewSessionResponse, PromptResponse,
    SessionConfigOption, SessionConfigSelectOption, SessionNotification, SessionUpdate,
    SetSessionConfigOptionResponse, StopReason, TextContent,
};
use serde_json::{Value, json};

/// The file the implementer writes.
const PORTED_FILE: &str = "ported.txt";

/// The behaviour of the mock agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MockOptions {
    /// How often the verifier rejects the implementation.
    pub rejections: u32,
    /// Whether the implementer commits its work.
    pub commit: bool,
    /// Whether the verifier prints a verdict at all.
    pub verdict: bool,
}

impl Default for MockOptions {
    fn default() -> Self {
        Self {
            rejections: 1,
            commit: true,
            verdict: true,
        }
    }
}

impl MockOptions {
    /// Read the behaviour from the command line arguments.
    ///
    /// The understood arguments are `--rejections <n>` (how often the
    /// verifier rejects), `--no-commit` (the implementer does not commit its
    /// work) and `--no-verdict` (the verifier never prints a verdict).
    pub fn from_args(arguments: impl IntoIterator<Item = String>) -> Self {
        let mut options = Self::default();
        let arguments: Vec<String> = arguments.into_iter().collect();
        let mut index = 0;
        while index < arguments.len() {
            match arguments[index].as_str() {
                "--rejections" => {
                    index += 1;
                    options.rejections = arguments
                        .get(index)
                        .and_then(|value| value.parse().ok())
                        .unwrap_or(options.rejections);
                }
                "--no-commit" => options.commit = false,
                "--no-verdict" => options.verdict = false,
                _ => {}
            }
            index += 1;
        }
        options
    }
}

/// Serve the protocol on standard input and output until the input ends.
pub fn serve(options: MockOptions) {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    let mut sessions: HashMap<String, PathBuf> = HashMap::new();
    let mut counter = 0_u64;
    let mut verifier_calls = 0_u32;

    for line in stdin.lock().lines() {
        let Ok(line) = line else {
            break;
        };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(message): Result<Value, _> = serde_json::from_str(&line) else {
            continue;
        };

        let method = message
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let id = message.get("id").cloned();
        let params = message.get("params").cloned().unwrap_or(Value::Null);

        match method.as_str() {
            "initialize" => {
                let response = InitializeResponse::new(ProtocolVersion::V1);
                write_result(&mut stdout, id, &response);
            }
            "session/new" => {
                counter += 1;
                let session_id = format!("mock-session-{counter}");
                let cwd = params
                    .get("cwd")
                    .and_then(Value::as_str)
                    .map(PathBuf::from)
                    .unwrap_or_else(|| PathBuf::from("."));
                sessions.insert(session_id.clone(), cwd);

                let mut response = NewSessionResponse::new(session_id);
                response.config_options = Some(vec![SessionConfigOption::select(
                    "model",
                    "Model",
                    "mock/model",
                    vec![SessionConfigSelectOption::new("mock/model", "Mock model")],
                )]);
                write_result(&mut stdout, id, &response);
            }
            "session/set_config_option" => {
                let response = SetSessionConfigOptionResponse::new(vec![]);
                write_result(&mut stdout, id, &response);
            }
            "session/prompt" => {
                let session_id = params
                    .get("sessionId")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let cwd = sessions.get(&session_id).cloned();
                let text = prompt_text(&params);
                let answer = answer(&text, cwd.as_deref(), &mut verifier_calls, options);

                let notification = SessionNotification::new(
                    session_id,
                    SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(
                        TextContent::new(answer),
                    ))),
                );
                write_notification(&mut stdout, &notification);
                write_result(&mut stdout, id, &PromptResponse::new(StopReason::EndTurn));
            }
            _ => {
                if id.is_some() {
                    write_result(&mut stdout, id, &Value::Null);
                }
            }
        }
    }
}

/// The text of the prompt of a request.
fn prompt_text(params: &Value) -> String {
    let Some(blocks) = params.get("prompt").and_then(Value::as_array) else {
        return String::new();
    };
    let mut text = String::new();
    for block in blocks {
        if let Some(part) = block.get("text").and_then(Value::as_str) {
            text.push_str(part);
        }
    }
    text
}

/// The answer of the mock agent for one prompt.
fn answer(
    text: &str,
    cwd: Option<&Path>,
    verifier_calls: &mut u32,
    options: MockOptions,
) -> String {
    if text.contains("You are porting one upstream commit") {
        if options.commit
            && let Some(cwd) = cwd
        {
            commit_the_port(cwd);
        }
        return "The upstream change is ported and committed.".to_string();
    }

    if text.contains("You are the verifier") {
        if !options.verdict {
            return "The implementation looks fine to me.".to_string();
        }
        *verifier_calls += 1;
        if *verifier_calls <= options.rejections {
            return "VERDICT: REJECTED\n- the test for the new behaviour is missing".to_string();
        }
        return "VERDICT: APPROVED".to_string();
    }

    "The linters pass, nothing had to be fixed.".to_string()
}

/// Write the ported file and commit it.
fn commit_the_port(cwd: &Path) {
    let _ = std::fs::write(cwd.join(PORTED_FILE), "ported\n");
    let _ = git(cwd, &["add", PORTED_FILE]);
    let _ = git(
        cwd,
        &["commit", "-q", "-m", "feat: port the upstream commit"],
    );
}

/// Run a git command inside `cwd`.
fn git(cwd: &Path, arguments: &[&str]) -> std::process::Output {
    Command::new("git")
        .args(arguments)
        .current_dir(cwd)
        .output()
        .expect("running git")
}

/// Write a JSON-RPC result.
fn write_result<T: serde::Serialize>(stdout: &mut impl Write, id: Option<Value>, result: &T) {
    let id = id.unwrap_or(Value::Null);
    let message = json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": serde_json::to_value(result).unwrap_or(Value::Null),
    });
    let _ = writeln!(stdout, "{message}");
    let _ = stdout.flush();
}

/// Write a JSON-RPC notification.
fn write_notification<T: serde::Serialize>(stdout: &mut impl Write, params: &T) {
    let message = json!({
        "jsonrpc": "2.0",
        "method": "session/update",
        "params": serde_json::to_value(params).unwrap_or(Value::Null),
    });
    let _ = writeln!(stdout, "{message}");
    let _ = stdout.flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_behaviour_rejects_once_and_commits() {
        let options = MockOptions::default();
        assert_eq!(options.rejections, 1);
        assert!(options.commit);
        assert!(options.verdict);
    }

    #[test]
    fn the_arguments_are_read() {
        let options = MockOptions::from_args(
            ["--rejections", "3", "--no-commit", "--no-verdict"]
                .into_iter()
                .map(str::to_string),
        );
        assert_eq!(options.rejections, 3);
        assert!(!options.commit);
        assert!(!options.verdict);
    }

    #[test]
    fn an_unknown_argument_is_ignored() {
        let options = MockOptions::from_args(["--nope"].into_iter().map(str::to_string));
        assert_eq!(options, MockOptions::default());
    }

    #[test]
    fn the_verifier_rejects_before_it_approves() {
        let mut calls = 0;
        let options = MockOptions::default();
        let first = answer(
            "You are the verifier of an automated implementation",
            None,
            &mut calls,
            options,
        );
        assert!(first.contains("VERDICT: REJECTED"));
        let second = answer(
            "You are the verifier of an automated implementation",
            None,
            &mut calls,
            options,
        );
        assert!(second.contains("VERDICT: APPROVED"));
    }

    #[test]
    fn the_verifier_can_stay_silent() {
        let mut calls = 0;
        let options = MockOptions {
            verdict: false,
            ..MockOptions::default()
        };
        let answer = answer(
            "You are the verifier of an automated implementation",
            None,
            &mut calls,
            options,
        );
        assert!(!answer.contains("VERDICT:"));
    }
}
