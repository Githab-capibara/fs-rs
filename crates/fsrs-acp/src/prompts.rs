//! The visible prompts sent to the three agents of the pipeline.
//!
//! Every prompt is built by a pure function so that it can be asserted in
//! unit tests and shown to the user before it is sent.

use fsrs_core::UpstreamCommit;

/// Maximum number of characters of the patch embedded in a prompt.
pub const MAX_PROMPT_DIFF_CHARS: usize = 20_000;

/// Marker the verifier must print to approve or reject an implementation.
pub const VERDICT_MARKER: &str = "VERDICT:";

/// Outcome of the verification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// The implementation is accepted.
    Approved,
    /// The implementation must be fixed.
    Rejected(String),
}

/// The prompt that drives the implementer.
pub fn implementer_prompt(commit: &UpstreamCommit, patch: &str) -> String {
    let mut prompt = String::new();
    prompt.push_str("You are porting one upstream commit into this fork.\n\n");
    prompt.push_str(&format!("Commit: {}\n", commit.sha));
    if !commit.url.is_empty() {
        prompt.push_str(&format!("Link: {}\n", commit.url));
    }
    prompt.push_str(&format!("Title: {}\n", commit.summary));
    prompt.push_str(&format!(
        "Author: {} <{}>\n",
        commit.author, commit.author_email
    ));
    if !commit.body.trim().is_empty() {
        prompt.push_str("Description:\n");
        prompt.push_str(commit.body.trim());
        prompt.push('\n');
    }
    prompt.push_str(&format!(
        "Files touched ({}): {}\n",
        commit.files.len(),
        commit.files.join(", ")
    ));
    prompt.push_str(&format!(
        "Lines: +{} -{}\n\n",
        commit.insertions, commit.deletions
    ));

    let trimmed = truncate_diff(patch, MAX_PROMPT_DIFF_CHARS);
    if !trimmed.is_empty() {
        prompt.push_str("Diff of the upstream commit:\n");
        prompt.push_str("```diff\n");
        prompt.push_str(&trimmed);
        prompt.push_str("\n```\n\n");
    }

    prompt.push_str(
        "Tasks:\n\
         - Port the upstream change into this repository and adapt it to the local conventions.\n\
         - Keep the change minimal and focused; do not refactor unrelated code.\n\
         - Add or update the tests that cover the new behaviour.\n\
         - Update the documentation when the public API changes.\n\
         - Commit the result with a message describing the port of the upstream commit.\n\
         - Report briefly what you changed and which files you touched.\n",
    );
    prompt
}

/// The prompt that drives the reviewer on the first cycle.
pub fn reviewer_prompt(commit: &UpstreamCommit) -> String {
    let mut prompt = String::new();
    prompt.push_str(
        "The implementation of the upstream commit is in place on the current branch.\n\n",
    );
    prompt.push_str(&format!("Commit: {}\n", commit.sha));
    if !commit.url.is_empty() {
        prompt.push_str(&format!("Link: {}\n", commit.url));
    }
    prompt.push_str(&format!("Title: {}\n\n", commit.summary));

    prompt.push_str(
        "Run the fast, modern linters of this repository and fix everything they report:\n\
         - `cargo fmt --check`\n\
         - `cargo clippy -- -D warnings`\n\
         - `cargo test`\n\
         - `shellcheck` on every shell script\n\
         - `oxlint` (or `biome`) on the JavaScript and TypeScript sources\n\
         - `typos` over the whole tree\n\
         - `cargo audit` or `cargo deny` when a lock file is present\n\n",
    );
    prompt.push_str(
        "Rules:\n\
         - Install a missing tool only when the installation is quick; otherwise report that it could not be run.\n\
         - Never change the public behaviour of the implementation.\n\
         - Commit every fix you make.\n\
         - Report the exact list of commands you ran and their outcome.\n",
    );
    prompt
}

/// The prompt that sends the verifier's feedback back to the reviewer.
///
/// It is sent to the **same** reviewer session, so the reviewer keeps the
/// context of its previous work.
pub fn rework_prompt(feedback: &str) -> String {
    let mut prompt = String::new();
    prompt.push_str("The verifier rejected the implementation.\n\n");
    prompt.push_str("Feedback:\n");
    prompt.push_str(feedback.trim());
    prompt.push_str(
        "\n\nTasks:\n\
         - Fix every problem listed in the feedback above.\n\
         - Re-run the linters and the test suite.\n\
         - Commit the fixes.\n\
         - Report what you changed.\n",
    );
    prompt
}

/// The prompt that drives the verifier.
pub fn verifier_prompt(commit: &UpstreamCommit) -> String {
    let mut prompt = String::new();
    prompt
        .push_str("You are the verifier of an automated implementation of an upstream commit.\n\n");
    prompt.push_str(&format!("Commit: {}\n", commit.sha));
    if !commit.url.is_empty() {
        prompt.push_str(&format!("Link: {}\n", commit.url));
    }
    prompt.push_str(&format!("Title: {}\n\n", commit.summary));

    prompt.push_str(
        "Inspect the current state of the repository and judge whether:\n\
         1. the upstream change is correctly ported;\n\
         2. the linters pass;\n\
         3. the tests cover the new behaviour;\n\
         4. no unrelated change slipped in.\n\n",
    );
    prompt.push_str(
        "Run the checks you need. Then finish your answer with exactly one verdict line:\n\n\
         VERDICT: APPROVED\n\
         or\n\
         VERDICT: REJECTED\n\n",
    );
    prompt.push_str(
        "When you reject, list the concrete problems that must be fixed, one per line, right after the verdict line.\n",
    );
    prompt
}

/// Parse the verdict out of the text produced by the verifier.
///
/// The last verdict marker wins, so an agent that mentions both outcomes in
/// its reasoning still ends up with an unambiguous result.
pub fn parse_verdict(text: &str) -> Option<Verdict> {
    let mut verdict = None;
    for line in text.lines() {
        let upper = line.to_uppercase();
        let Some(position) = upper.find(VERDICT_MARKER) else {
            continue;
        };
        let rest = &upper[position + VERDICT_MARKER.len()..];
        let rest = rest.trim_start_matches([' ', ':', '=', '-']);
        let rest = rest.trim_start();
        if rest.starts_with("APPROVED") {
            verdict = Some(Verdict::Approved);
        } else if rest.starts_with("REJECTED") {
            verdict = Some(Verdict::Rejected(String::new()));
        }
    }
    // Attach the reasons that follow the final verdict line.
    if let Some(Verdict::Rejected(reasons)) = &mut verdict {
        let mut collected = Vec::new();
        let mut after_verdict = false;
        for line in text.lines() {
            if line.to_uppercase().contains(VERDICT_MARKER) {
                after_verdict = true;
                continue;
            }
            if after_verdict && !line.trim().is_empty() {
                collected.push(line.trim().to_string());
            }
        }
        *reasons = collected.join("\n");
    }
    verdict
}

/// Truncate a diff while keeping the beginning and the end.
///
/// The beginning carries the file headers, the end carries the most relevant
/// hunks; the middle is the part that is least useful for a first pass.
pub fn truncate_diff(diff: &str, limit: usize) -> String {
    if diff.len() <= limit {
        return diff.to_string();
    }
    let head = limit / 2;
    let tail = limit - head;
    let mut head_end = head;
    while head_end > 0 && !diff.is_char_boundary(head_end) {
        head_end -= 1;
    }
    let tail_start = diff.len() - tail;
    let mut tail_start = tail_start;
    while tail_start < diff.len() && !diff.is_char_boundary(tail_start) {
        tail_start += 1;
    }
    format!(
        "{}\n… ({ } characters omitted) …\n{}",
        &diff[..head_end],
        diff.len() - head_end - (diff.len() - tail_start),
        &diff[tail_start..]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commit() -> UpstreamCommit {
        UpstreamCommit {
            repo_id: 1,
            sha: "abc1234".to_string(),
            short_sha: "abc1234".to_string(),
            summary: "Fix the parser".to_string(),
            body: "It used to panic.".to_string(),
            author: "Ada".to_string(),
            author_email: "ada@example.com".to_string(),
            authored_at: 0,
            committed_at: 0,
            files: vec!["src/lib.rs".to_string()],
            insertions: 3,
            deletions: 1,
            url: "https://example.com/commit/abc1234".to_string(),
        }
    }

    #[test]
    fn the_implementer_prompt_carries_link_description_and_diff() {
        let prompt = implementer_prompt(&commit(), "--- a/x\n+++ b/x\n@@\n+line\n");
        assert!(prompt.contains("https://example.com/commit/abc1234"));
        assert!(prompt.contains("It used to panic."));
        assert!(prompt.contains("+line"));
        assert!(prompt.contains("Commit: abc1234"));
    }

    #[test]
    fn the_reviewer_prompt_lists_every_linter() {
        let prompt = reviewer_prompt(&commit());
        for tool in [
            "cargo fmt --check",
            "cargo clippy -- -D warnings",
            "cargo test",
            "shellcheck",
            "oxlint",
            "typos",
        ] {
            assert!(prompt.contains(tool), "missing {tool}");
        }
    }

    #[test]
    fn the_rework_prompt_quotes_the_feedback() {
        let prompt = rework_prompt("clippy warns about a needless borrow");
        assert!(prompt.contains("needless borrow"));
        assert!(!prompt.contains("same session"));
    }

    #[test]
    fn the_verifier_prompt_asks_for_a_machine_readable_verdict() {
        let prompt = verifier_prompt(&commit());
        assert!(prompt.contains("VERDICT: APPROVED"));
        assert!(prompt.contains("VERDICT: REJECTED"));
    }

    #[test]
    fn an_approved_verdict_is_parsed() {
        let text = "Everything is fine.\n\nVERDICT: APPROVED\n";
        assert_eq!(parse_verdict(text), Some(Verdict::Approved));
    }

    #[test]
    fn a_rejected_verdict_keeps_the_reasons() {
        let text = "VERDICT: REJECTED\n- clippy fails on src/lib.rs\n- no test was added\n";
        let verdict = parse_verdict(text).unwrap();
        match verdict {
            Verdict::Rejected(reasons) => {
                assert!(reasons.contains("clippy fails"));
                assert!(reasons.contains("no test was added"));
            }
            other => panic!("unexpected verdict: {other:?}"),
        }
    }

    #[test]
    fn the_last_verdict_wins() {
        let text = "VERDICT: REJECTED\nnot fixed\nVERDICT: APPROVED\n";
        assert_eq!(parse_verdict(text), Some(Verdict::Approved));
    }

    #[test]
    fn text_without_a_verdict_yields_nothing() {
        assert_eq!(parse_verdict("I could not decide"), None);
    }

    #[test]
    fn the_verdict_marker_is_case_insensitive() {
        assert_eq!(parse_verdict("verdict: approved"), Some(Verdict::Approved));
    }

    #[test]
    fn long_diffs_keep_their_head_and_tail() {
        let diff = "HEAD\n".to_string() + &"x".repeat(1000) + "\nTAIL";
        let truncated = truncate_diff(&diff, 100);
        assert!(truncated.starts_with("HEAD"));
        assert!(truncated.ends_with("TAIL"));
        assert!(truncated.contains("characters omitted"));
    }

    #[test]
    fn short_diffs_are_untouched() {
        assert_eq!(truncate_diff("small diff", 100), "small diff");
    }
}
