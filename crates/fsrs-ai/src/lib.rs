//! Client of the local decision model used to rate upstream commits.
//!
//! `fs-rs` uses a purely local, open-source model — [Foq](https://github.com/yohanargentina-oss/Foq),
//! the free alternative to Jev — for exactly two tasks:
//!
//! 1. judging how **critical** an upstream commit is for the fork,
//! 2. judging how **complex** the port of that commit into the fork is.
//!
//! The model never writes code and never talks to the agents. It only
//! produces a typed decision with a calibrated confidence.
//!
//! # Architecture
//!
//! The runtime is a `llama-server` build shipped by the Foq project. It
//! exposes a `/completion` endpoint that returns, for the single generated
//! token, the log-probability of every candidate letter. This crate:
//!
//! * renders the prompt exactly like the reference Python implementation
//!   ([`schema::DecisionSchema::format_prompt`]),
//! * extracts the per-letter log-probabilities ([`client::extract_logprobs`]),
//! * calibrates them with temperature scaling ([`calibration`]),
//! * and maps the winning letter onto the domain types of `fsrs-core`.
//!
//! # Example
//!
//! ```no_run
//! use fsrs_ai::FoqClient;
//!
//! let client = FoqClient::new("http://127.0.0.1:8089");
//! assert!(client.is_ready());
//! ```

#![warn(missing_docs)]

pub mod calibration;
pub mod client;
pub mod gpu;
pub mod prompt;
pub mod schema;

use fsrs_core::{Assessment, Complexity, Criticality, UpstreamCommit};

pub use calibration::{CalibrationProfile, TemperatureScaler};
pub use client::{FoqClient, FoqDecision};
pub use gpu::{Accelerator, AcceleratorInfo, AcceleratorInventory, Backend};
pub use prompt::build_context;
pub use schema::{Decision, DecisionOption, DecisionSchema};

/// Confidence below which a decision is flagged for human review.
///
/// Mirrors `SEUIL_REVIEW_MESURE` in `foq/engine.py`.
pub const DEFAULT_MIN_CONFIDENCE: f64 = 0.8;

/// Map the letter chosen by the model onto a criticality value.
///
/// The schema orders the options from the most to the least severe.
pub fn criticality_from_key(key: &str) -> Option<Criticality> {
    match key {
        "A" => Some(Criticality::Critical),
        "B" => Some(Criticality::High),
        "C" => Some(Criticality::Medium),
        "D" => Some(Criticality::Low),
        "E" => Some(Criticality::Trivial),
        _ => None,
    }
}

/// Map the letter chosen by the model onto a complexity value.
///
/// The schema orders the options from the hardest to the easiest.
pub fn complexity_from_key(key: &str) -> Option<Complexity> {
    match key {
        "A" => Some(Complexity::Expert),
        "B" => Some(Complexity::Hard),
        "C" => Some(Complexity::Medium),
        "D" => Some(Complexity::Easy),
        "E" => Some(Complexity::Trivial),
        _ => None,
    }
}

/// Assess one upstream commit with the local decision model.
///
/// Two independent decisions are requested: the criticality of the commit and
/// the complexity of porting it. The returned assessment is flagged for review
/// when either decision stayed below the configured confidence threshold.
pub fn assess_commit(
    client: &FoqClient,
    commit: &UpstreamCommit,
    patch: Option<&str>,
) -> Result<Assessment, client::FoqError> {
    let context = build_context(commit, patch);

    let criticality = client.decide(&context, &DecisionSchema::criticality())?;
    let complexity = client.decide(&context, &DecisionSchema::complexity())?;

    let criticality_value = criticality_from_key(&criticality.decision.key).ok_or(
        client::FoqError::InvalidDecision {
            key: criticality.decision.key.clone(),
            domain: "criticality",
        },
    )?;
    let complexity_value =
        complexity_from_key(&complexity.decision.key).ok_or(client::FoqError::InvalidDecision {
            key: complexity.decision.key.clone(),
            domain: "complexity",
        })?;

    let needs_review = criticality.needs_review || complexity.needs_review;
    let rationale = format!(
        "criticalité={} ({:.0}%), complexité={} ({:.0}%)",
        criticality_value.label(),
        criticality.decision.confidence * 100.0,
        complexity_value.label(),
        complexity.decision.confidence * 100.0,
    );

    Ok(Assessment {
        repo_id: commit.repo_id,
        sha: commit.sha.clone(),
        criticality: criticality_value,
        complexity: complexity_value,
        criticality_confidence: criticality.decision.confidence,
        complexity_confidence: complexity.decision.confidence,
        needs_review,
        rationale,
        model: client.model().to_string(),
        created_at: fsrs_core::now_unix(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn criticality_keys_map_onto_the_domain() {
        assert_eq!(criticality_from_key("A"), Some(Criticality::Critical));
        assert_eq!(criticality_from_key("E"), Some(Criticality::Trivial));
        assert_eq!(criticality_from_key("Z"), None);
    }

    #[test]
    fn complexity_keys_map_onto_the_domain() {
        assert_eq!(complexity_from_key("A"), Some(Complexity::Expert));
        assert_eq!(complexity_from_key("E"), Some(Complexity::Trivial));
        assert_eq!(complexity_from_key("Z"), None);
    }

    #[test]
    fn the_default_threshold_matches_the_reference_implementation() {
        assert!((DEFAULT_MIN_CONFIDENCE - 0.8).abs() < 1e-12);
    }
}
