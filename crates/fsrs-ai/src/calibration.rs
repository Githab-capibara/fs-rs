//! Temperature scaling calibration for Foq decisions.
//!
//! The raw logits returned by the model are systematically overconfident. Foq
//! therefore ships a *calibration profile* fitted on a held-out evaluation
//! set; the profile stores a single scalar temperature `T` that is applied to
//! the log-probabilities before the softmax.
//!
//! This module is a faithful port of `foq/calibration.py` from the upstream
//! project, so that the Rust client and the reference Python implementation
//! produce numerically identical confidences.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Temperature used when the model runs without the decision LoRA adapter.
///
/// Fitted on `eval/data/calib_val.jsonl` and measured on the held-out
/// `exam_test.jsonl`; see `foq/calibration_profile_bare.json`.
pub const BARE_TEMPERATURE: f64 = 3.6381;

/// Temperature used when the model runs with the decision LoRA adapter.
///
/// Fitted on `eval/data/calib_val.jsonl` and measured on the held-out
/// `exam_test.jsonl`; see `foq/calibration_profile.json`.
pub const LORA_TEMPERATURE: f64 = 2.6353;

/// Log-probability assigned to an option the model did not mention.
///
/// Matches the sentinel used by the reference implementation: it is low enough
/// that a missing option never wins, but high enough to keep the exponential
/// in a safe range.
pub const MISSING_LOGPROB: f64 = -25.0;

/// A calibration profile as stored on disk by Foq.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CalibrationProfile {
    /// Alias of the model the profile was fitted for.
    #[serde(default)]
    pub model_alias: String,
    /// Fitted temperature.
    #[serde(default = "default_temperature")]
    pub temperature: f64,
    /// Expected calibration error measured on the held-out set.
    #[serde(default)]
    pub ece: f64,
    /// Brier score measured on the held-out set.
    #[serde(default)]
    pub brier_score: f64,
    /// Dataset the temperature was fitted on.
    #[serde(default)]
    pub fitted_on: String,
    /// Dataset the error metrics were measured on.
    #[serde(default)]
    pub measured_on: String,
    /// Provenance of the profile.
    #[serde(default)]
    pub source: String,
}

fn default_temperature() -> f64 {
    1.0
}

impl Default for CalibrationProfile {
    fn default() -> Self {
        Self {
            model_alias: "foq-8b".to_string(),
            temperature: BARE_TEMPERATURE,
            ece: 0.0,
            brier_score: 0.0,
            fitted_on: String::new(),
            measured_on: String::new(),
            source: String::new(),
        }
    }
}

impl CalibrationProfile {
    /// Profile for a model running with the decision LoRA adapter.
    // The metrics below are measured values, not mathematical constants.
    #[allow(clippy::approx_constant)]
    pub fn with_lora() -> Self {
        Self {
            model_alias: "foq-8b-lora".to_string(),
            temperature: LORA_TEMPERATURE,
            ece: 0.0477,
            brier_score: 0.3927,
            fitted_on: "eval/data/calib_val.jsonl (1250 cases)".to_string(),
            measured_on: "eval/data/exam_test.jsonl (1250 cases, held out)".to_string(),
            source: "eval/results/calibration_lora.json".to_string(),
        }
    }

    /// Profile for a model running without the decision LoRA adapter.
    pub fn bare() -> Self {
        Self {
            model_alias: "foq-8b".to_string(),
            temperature: BARE_TEMPERATURE,
            ece: 0.055,
            brier_score: 0.4518,
            fitted_on: "eval/data/calib_val.jsonl (1250 cases)".to_string(),
            measured_on: "eval/data/exam_test.jsonl (1250 cases, held out)".to_string(),
            source: "eval/results/calibration_bare.json".to_string(),
        }
    }

    /// Parse a profile from the JSON produced by Foq.
    pub fn from_json(raw: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(raw)
    }

    /// Serialise the profile back to Foq's JSON layout.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    /// Scaler configured with this profile's temperature.
    pub fn scaler(&self) -> TemperatureScaler {
        TemperatureScaler::new(self.temperature)
    }
}

/// Applies temperature scaling to a set of log-probabilities.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TemperatureScaler {
    temperature: f64,
}

impl TemperatureScaler {
    /// Create a scaler. Temperatures below `0.01` are clamped, mirroring the
    /// reference implementation which refuses to divide by zero.
    pub fn new(temperature: f64) -> Self {
        Self {
            temperature: temperature.max(0.01),
        }
    }

    /// Scaler that does not modify the distribution.
    pub fn identity() -> Self {
        Self::new(1.0)
    }

    /// The configured temperature.
    pub fn temperature(&self) -> f64 {
        self.temperature
    }

    /// Divide every log-probability by the temperature.
    pub fn scale_logits(&self, logprobs: &BTreeMap<String, f64>) -> BTreeMap<String, f64> {
        logprobs
            .iter()
            .map(|(key, value)| (key.clone(), value / self.temperature))
            .collect()
    }

    /// Convert log-probabilities into a calibrated probability distribution.
    ///
    /// The computation is numerically stable: the log-probabilities are shifted
    /// by their maximum before the exponential is applied.
    pub fn scale_probabilities(&self, logprobs: &BTreeMap<String, f64>) -> BTreeMap<String, f64> {
        if logprobs.is_empty() {
            return BTreeMap::new();
        }
        let scaled = self.scale_logits(logprobs);
        let max = scaled.values().copied().fold(f64::NEG_INFINITY, f64::max);
        let mut probabilities = BTreeMap::new();
        let mut sum = 0.0;
        for (key, value) in &scaled {
            let weight = (value - max).exp();
            probabilities.insert(key.clone(), weight);
            sum += weight;
        }
        if sum > 0.0 {
            for value in probabilities.values_mut() {
                *value /= sum;
            }
        }
        probabilities
    }
}

/// Turn raw log-probabilities into an uncalibrated probability distribution.
///
/// Used to report the model's raw opinion next to the calibrated confidence.
pub fn softmax(logprobs: &BTreeMap<String, f64>) -> BTreeMap<String, f64> {
    TemperatureScaler::identity().scale_probabilities(logprobs)
}

impl From<fsrs_core::FoqProfile> for CalibrationProfile {
    fn from(profile: fsrs_core::FoqProfile) -> Self {
        match profile {
            fsrs_core::FoqProfile::Lora => CalibrationProfile::with_lora(),
            fsrs_core::FoqProfile::Bare => CalibrationProfile::bare(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn logprobs() -> BTreeMap<String, f64> {
        BTreeMap::from([
            ("A".to_string(), -0.1),
            ("B".to_string(), -2.0),
            ("C".to_string(), -3.0),
        ])
    }

    #[test]
    fn probabilities_sum_to_one() {
        let scaled = TemperatureScaler::new(2.0).scale_probabilities(&logprobs());
        let sum: f64 = scaled.values().sum();
        assert!((sum - 1.0).abs() < 1e-12, "sum was {sum}");
    }

    #[test]
    fn scaling_softens_the_distribution() {
        let raw = softmax(&logprobs());
        let calibrated = TemperatureScaler::new(3.6381).scale_probabilities(&logprobs());
        assert!(
            calibrated["A"] < raw["A"],
            "calibration must reduce overconfidence: {} vs {}",
            calibrated["A"],
            raw["A"]
        );
    }

    #[test]
    fn identity_scaler_keeps_the_distribution() {
        let raw = softmax(&logprobs());
        let scaled = TemperatureScaler::identity().scale_probabilities(&logprobs());
        for (key, value) in &raw {
            assert!((scaled[key] - value).abs() < 1e-12);
        }
    }

    #[test]
    fn temperature_is_clamped_to_a_positive_value() {
        assert_eq!(TemperatureScaler::new(0.0).temperature(), 0.01);
        assert_eq!(TemperatureScaler::new(-5.0).temperature(), 0.01);
    }

    #[test]
    fn empty_input_yields_empty_output() {
        assert!(
            TemperatureScaler::new(2.0)
                .scale_probabilities(&BTreeMap::new())
                .is_empty()
        );
    }

    #[test]
    fn missing_option_never_wins() {
        let mut probs = logprobs();
        probs.insert("D".to_string(), MISSING_LOGPROB);
        let scaled = TemperatureScaler::new(LORA_TEMPERATURE).scale_probabilities(&probs);
        let best = scaled
            .iter()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .unwrap();
        assert_eq!(best.0, "A");
    }

    #[test]
    fn profiles_roundtrip_through_json() {
        let profile = CalibrationProfile::with_lora();
        let json = profile.to_json().unwrap();
        let parsed = CalibrationProfile::from_json(&json).unwrap();
        assert_eq!(profile, parsed);
    }

    #[test]
    fn profile_temperatures_match_the_upstream_fixtures() {
        assert!((CalibrationProfile::with_lora().temperature - 2.6353).abs() < 1e-9);
        assert!((CalibrationProfile::bare().temperature - 3.6381).abs() < 1e-9);
    }
}
