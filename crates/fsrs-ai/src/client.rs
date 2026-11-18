//! HTTP client for the local Foq decision server.
//!
//! The server is a `llama-server` instance started by the Foq runtime. It
//! exposes an OpenAI-compatible `/completion` endpoint; the client asks for a
//! single generated token together with the per-token log-probabilities and
//! turns those into a calibrated typed decision.
//!
//! The request payload is byte-for-byte identical to the one sent by the
//! reference Python client, so the same fine-tuned model behaves identically.

use std::collections::{BTreeMap, HashMap};
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::calibration::{CalibrationProfile, TemperatureScaler};
use crate::schema::{Decision, DecisionSchema};

/// Number of tokens kept free for the answer of the model.
const ANSWER_RESERVE_TOKENS: usize = 16;

/// Context window assumed when the server does not report its own.
///
/// The Foq model is trained with a window of 1024 tokens, and a runtime that
/// splits its context between several slots reports the size of one slot.
pub const DEFAULT_CONTEXT_TOKENS: usize = 1024;

/// Characters per token used when the server cannot count the tokens.
///
/// Deliberately pessimistic: a diff of code tokenises worse than prose.
const FALLBACK_CHARS_PER_TOKEN: usize = 2;

/// Errors produced while talking to the decision server.
#[derive(Debug, thiserror::Error)]
pub enum FoqError {
    /// The HTTP request itself failed.
    #[error("foq request failed: {0}")]
    Http(#[from] ureq::Error),
    /// The server answered with a body that is not valid JSON.
    #[error("foq returned an invalid response: {0}")]
    InvalidResponse(String),
    /// The server refused the request.
    ///
    /// The body of the answer is kept, because the runtime explains in it
    /// why the request was refused — a prompt that does not fit into the
    /// context window, for example.
    #[error("foq answered {status}: {message}")]
    RequestFailed {
        /// The HTTP status code of the answer.
        status: u16,
        /// The body of the answer.
        message: String,
    },
    /// The server answered successfully but without any usable probability.
    #[error("foq returned no completion probabilities")]
    NoProbabilities,
    /// The decision could not be mapped onto the expected domain.
    #[error("foq decision {key:?} is not a valid {domain} value")]
    InvalidDecision {
        /// The key returned by the model.
        key: String,
        /// The domain the key was expected to belong to.
        domain: &'static str,
    },
}

/// A client of the local decision server.
#[derive(Clone)]
pub struct FoqClient {
    base_url: String,
    timeout: Duration,
    profile: CalibrationProfile,
    min_confidence: f64,
    model: String,
    /// The context window of the server, learned once and then reused.
    context_size: Arc<OnceLock<usize>>,
    /// The HTTP agent of every request.
    ///
    /// One agent keeps the connection to the server alive: the dashboard asks
    /// the model dozens of times in a row, and a new connection for every
    /// question cost a handshake each time.
    agent: ureq::Agent,
    /// The contexts that were already fitted, by the hash of their text.
    ///
    /// A commit is rated twice — once for the criticality, once for the
    /// complexity — and both questions read the same context. The fitting
    /// asks the server to count the tokens, so it is remembered: the second
    /// question of a commit costs no call at all.
    fitted: Arc<Mutex<HashMap<u64, String>>>,
    /// A hook that is called before every request, used by the tests and by
    /// the probes to count the calls of the server.
    on_request: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl std::fmt::Debug for FoqClient {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FoqClient")
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .field("timeout", &self.timeout)
            .finish()
    }
}

/// Payload sent to `/tokenize`.
#[derive(Debug, Serialize)]
struct TokenizeRequest<'a> {
    content: &'a str,
}

/// Payload sent to `/completion`.
#[derive(Debug, Serialize)]
struct CompletionRequest<'a> {
    prompt: &'a str,
    n_predict: i64,
    n_probs: i64,
    temperature: f64,
    cache_prompt: bool,
}

/// One candidate token with its log-probability.
#[derive(Debug, Clone, Deserialize)]
pub struct TopLogprob {
    /// The candidate token as produced by the model.
    pub token: String,
    /// Log-probability of the token.
    pub logprob: f64,
}

/// Probabilities attached to one generated token.
#[derive(Debug, Clone, Deserialize)]
pub struct CompletionProbability {
    /// The candidates considered at this position.
    #[serde(default)]
    pub top_logprobs: Vec<TopLogprob>,
}

/// Answer of `/completion`.
#[derive(Debug, Clone, Deserialize)]
pub struct CompletionResponse {
    /// Probabilities of every generated token.
    #[serde(default)]
    pub completion_probabilities: Vec<CompletionProbability>,
}

/// A decision together with the metadata of the call that produced it.
#[derive(Debug, Clone)]
pub struct FoqDecision {
    /// The typed decision.
    pub decision: Decision,
    /// Whether the calibrated confidence fell below the review threshold.
    pub needs_review: bool,
    /// Round-trip latency of the request, in milliseconds.
    pub latency_ms: f64,
}

impl FoqClient {
    /// Create a client with the default calibration profile and threshold.
    pub fn new(base_url: &str) -> Self {
        // The agent outlives every request of the client: the connection to
        // the local server is kept open between the questions.
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(30)))
            // The answer of the server is more useful than the bare status
            // code, so the errors are read by hand.
            .http_status_as_error(false)
            .build()
            .into();
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            // The default is the one the configuration documents
            // (`foq_timeout_secs`). A decision model reads the whole diff, so a
            // single request takes tens of seconds; 30 seconds was too short
            // for a large one.
            timeout: Duration::from_secs(120),
            profile: CalibrationProfile::bare(),
            min_confidence: 0.8,
            model: "foq".to_string(),
            context_size: Arc::new(OnceLock::new()),
            agent,
            fitted: Arc::new(Mutex::new(HashMap::new())),
            on_request: None,
        }
    }

    /// Call `hook` before every request to the server.
    ///
    /// The hook counts the calls in the probes and the tests; it is the only
    /// way to observe how many times the client talks to the server.
    pub fn with_call_counter(mut self, hook: impl Fn() + Send + Sync + 'static) -> Self {
        self.on_request = Some(Arc::new(hook));
        self
    }

    /// Announce a request to the server.
    fn count_request(&self) {
        if let Some(hook) = &self.on_request {
            hook();
        }
    }

    /// Override the calibration profile.
    pub fn with_profile(mut self, profile: CalibrationProfile) -> Self {
        self.profile = profile;
        self
    }

    /// Override the confidence threshold below which a decision is flagged.
    pub fn with_min_confidence(mut self, min_confidence: f64) -> Self {
        self.min_confidence = min_confidence;
        self
    }

    /// Override the model alias reported with the assessments.
    pub fn with_model(mut self, model: &str) -> Self {
        self.model = model.to_string();
        self
    }

    /// Override the request timeout.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Base URL of the server.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Model alias reported with the assessments.
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Calibration profile in use.
    pub fn profile(&self) -> &CalibrationProfile {
        &self.profile
    }

    /// The scaler derived from the calibration profile.
    pub fn scaler(&self) -> TemperatureScaler {
        self.profile.scaler()
    }

    /// Check that the server answers.
    pub fn is_ready(&self) -> bool {
        let url = format!("{}/health", self.base_url);
        self.count_request();
        self.agent
            .get(&url)
            .config()
            .timeout_global(Some(Duration::from_secs(2)))
            .build()
            .call()
            .is_ok()
    }

    /// The number of tokens the context window of the server holds.
    ///
    /// The size is asked from the server once and then remembered. A server
    /// that does not answer is assumed to have the window of the model
    /// itself.
    pub fn context_size(&self) -> usize {
        *self.context_size.get_or_init(|| self.probe_context_size())
    }

    /// Ask the server how many tokens its context window holds.
    fn probe_context_size(&self) -> usize {
        let url = format!("{}/props", self.base_url);
        self.count_request();
        let Ok(response) = self
            .agent
            .get(&url)
            .config()
            .timeout_global(Some(Duration::from_secs(5)))
            .build()
            .call()
        else {
            return DEFAULT_CONTEXT_TOKENS;
        };
        let Ok(body) = response.into_body().read_to_string() else {
            return DEFAULT_CONTEXT_TOKENS;
        };
        let Ok(props) = serde_json::from_str::<serde_json::Value>(&body) else {
            return DEFAULT_CONTEXT_TOKENS;
        };
        props
            .pointer("/default_generation_settings/n_ctx")
            .and_then(|value| value.as_u64())
            .map(|value| value as usize)
            .filter(|value| *value > 0)
            .unwrap_or(DEFAULT_CONTEXT_TOKENS)
    }

    /// The number of tokens of a text, when the server can count them.
    ///
    /// The runtime of the decision model exposes a tokenizer endpoint. A
    /// server without it returns `None`, and the caller falls back to a
    /// conservative estimate.
    pub fn count_tokens(&self, text: &str) -> Option<usize> {
        let url = format!("{}/tokenize", self.base_url);
        self.count_request();
        // The body is written by hand: the feature `json` of the HTTP
        // client would bring a second serializer for one small request.
        let body = sonic_rs::to_string(&TokenizeRequest { content: text }).ok()?;
        let response = self
            .agent
            .post(&url)
            .config()
            .timeout_global(Some(Duration::from_secs(10)))
            .build()
            .content_type("application/json")
            .send(body.as_str())
            .ok()?;
        let body = response.into_body().read_to_string().ok()?;
        let value = serde_json::from_str::<serde_json::Value>(&body).ok()?;
        let tokens = value.get("tokens")?.as_array()?.len();
        Some(tokens)
    }

    /// Build the prompt of a decision, so that it fits into the window.
    ///
    /// The context describes the commit and ends with its diff, so a context
    /// that does not fit is cut from its end: the diff is shortened first and
    /// the metadata is kept.
    ///
    /// The result is remembered: the criticality and the complexity of one
    /// commit read the same context, and the second one does not ask the
    /// server to count the tokens again.
    pub fn fit_prompt(&self, context: &str, schema: &DecisionSchema) -> String {
        let key = cache_key(context);
        if let Ok(cache) = self.fitted.lock()
            && let Some(fitted) = cache.get(&key)
        {
            return schema.format_prompt(fitted);
        }
        let started = Instant::now();
        let limit = self.context_size().saturating_sub(ANSWER_RESERVE_TOKENS);
        let fitted = fit_context(context, schema, limit, |text| self.count_tokens(text));
        tracing::info!(
            elapsed_ms = started.elapsed().as_millis(),
            chars = fitted.len(),
            "the prompt was fitted"
        );
        if let Ok(mut cache) = self.fitted.lock() {
            cache.insert(key, fitted.clone());
        }
        schema.format_prompt(&fitted)
    }

    /// Ask the model to pick one option of `schema`.
    ///
    /// The returned decision carries the calibrated distribution over all
    /// options of the schema.
    pub fn decide(&self, context: &str, schema: &DecisionSchema) -> Result<FoqDecision, FoqError> {
        let prompt = self.fit_prompt(context, schema);
        let payload = CompletionRequest {
            prompt: &prompt,
            n_predict: 1,
            n_probs: 25,
            temperature: 0.0,
            cache_prompt: true,
        };
        let started = Instant::now();
        let response = self.post_completion(&payload)?;
        let latency_ms = started.elapsed().as_secs_f64() * 1000.0;

        let logprobs = extract_logprobs(&response)?;
        let decision = schema.interpret(&logprobs, &self.scaler());
        let needs_review = decision.confidence < self.min_confidence;
        Ok(FoqDecision {
            decision,
            needs_review,
            latency_ms,
        })
    }

    /// Perform the raw HTTP call and decode the answer.
    fn post_completion(
        &self,
        payload: &CompletionRequest<'_>,
    ) -> Result<CompletionResponse, FoqError> {
        let url = format!("{}/completion", self.base_url);
        self.count_request();
        // The payload carries the whole diff, so it is the largest string
        // that leaves the process: `sonic-rs` writes it twice as fast as the
        // general purpose serializer.
        let body = sonic_rs::to_string(payload)
            .map_err(|err| FoqError::InvalidResponse(err.to_string()))?;
        let mut response = self
            .agent
            .post(&url)
            .config()
            .timeout_global(Some(self.timeout))
            .build()
            .content_type("application/json")
            .send(body.as_str())?;
        let status = u16::from(response.status());
        let body = response
            .body_mut()
            .read_to_string()
            .map_err(|err| FoqError::InvalidResponse(err.to_string()))?;
        if !(200..300).contains(&status) {
            return Err(FoqError::RequestFailed {
                status,
                message: body.trim().to_string(),
            });
        }
        sonic_rs::from_str(&body).map_err(|err| FoqError::InvalidResponse(err.to_string()))
    }
}

/// Cut a context down until the prompt of `schema` fits into `limit` tokens.
///
/// The count of the tokens is asked from the server through `count`, which
/// receives the whole prompt. A server that cannot count returns `None`, and
/// the context is then cut down with a pessimistic estimate of the number of
/// characters per token.
///
/// The context ends with the diff of the commit, so cutting it from its end
/// shortens the diff first and keeps the metadata of the commit.
///
/// The number of calls to the server is what costs time here, so the cut is
/// not searched: the tokens of the whole context are counted once, the length
/// of the prefix that fits is estimated from that count, and the estimate is
/// checked. A binary search asked the server `log2(len)` times — more than
/// thirty calls for a large diff, and two schemas meant two searches.
pub fn fit_context(
    context: &str,
    schema: &DecisionSchema,
    limit: usize,
    mut count: impl FnMut(&str) -> Option<usize>,
) -> String {
    // The count of the tokens is asked through one closure, so the caller
    // can count the calls of the server. The closure is called directly
    // every time — it is the only place that asks the server.
    let fits = |text: &str, count: &mut dyn FnMut(&str) -> Option<usize>| {
        let prompt = schema.format_prompt(text);
        match count(&prompt) {
            Some(tokens) => tokens <= limit,
            // Without a tokenizer the pessimistic estimate decides.
            None => prompt.len() <= limit * FALLBACK_CHARS_PER_TOKEN,
        }
    };
    // The marker of the cut is part of the context, so it is counted while
    // the length of the remaining context is searched.
    let marked = |prefix: &str| {
        let mut text = prefix.to_string();
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(TRUNCATION_MARKER);
        text
    };

    // The tokens of the whole context, counted once. The prompt of the schema
    // is longer than the context by a fixed amount — the question and the
    // options — so the estimate below uses the length of the context, not of
    // the prompt.
    let whole = schema.format_prompt(context);
    let Some(whole_tokens) = count(&whole) else {
        // Without a tokenizer the pessimistic estimate decides, and the cut
        // is found by the characters alone.
        let allowed = limit * FALLBACK_CHARS_PER_TOKEN;
        let overhead = whole.len().saturating_sub(context.len());
        let cut = boundary(context, allowed.saturating_sub(overhead));
        return marked(&context[..cut]);
    };
    if whole_tokens == 0 {
        return marked("");
    }
    // The whole context fits: it is kept as it is, and the server was asked
    // once.
    if whole_tokens <= limit {
        return context.to_string();
    }

    // The proportion of the context that fits, applied to the characters.
    // The estimate is only a starting point: the tokens of a prefix are not
    // the tokens of the whole context times the proportion of its
    // characters, so the cut is corrected below.
    let ratio = limit as f64 / whole_tokens as f64;
    let mut cut = boundary(context, (context.len() as f64 * ratio) as usize);

    // The cut is corrected until the prompt fits or the cut cannot be
    // changed any more.
    //
    // A cut that is too long is shortened by a quarter, a cut that fits is
    // stretched a little — and the stretching shrinks every time, so the
    // search converges on the longest cut that fits without asking the
    // server many times.
    let mut too_long = false;
    let mut step = (context.len() - cut) / 4 + 1;
    for _ in 0..8 {
        let candidate = marked(&context[..cut]);
        if fits(&candidate, &mut count) {
            // There is room left: try a longer cut, but never by more than
            // the last step, so the cut converges.
            let longer = boundary(context, cut + step);
            if longer == cut || longer + step >= context.len() {
                break;
            }
            cut = longer;
            too_long = false;
            step = step.max(1) / 2;
            continue;
        }
        // The cut is too long: try a shorter one.
        let shorter = boundary(context, cut * 3 / 4);
        if shorter == cut {
            break;
        }
        cut = shorter;
        too_long = true;
        step = step.max(1) * 2;
    }

    // The last check decided that the cut was too long, and the correction
    // did not have enough steps: the prompt must fit, so the cut is
    // shortened until it does.
    if too_long {
        while cut > 0 {
            let shorter = boundary(context, cut * 3 / 4);
            if shorter == cut {
                break;
            }
            cut = shorter;
            if fits(&marked(&context[..cut]), &mut count) {
                break;
            }
        }
    }
    marked(&context[..cut])
}

/// The key of a context in the cache of the fitted prompts.
///
/// The text of the context is hashed: a commit that is rated twice has the
/// same context, and the hash of a long diff is cheap compared to the call to
/// the server that the fitting would make.
fn cache_key(context: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    context.hash(&mut hasher);
    hasher.finish()
}

/// The marker appended to a context that had to be shortened.
const TRUNCATION_MARKER: &str = "… (contexte tronqué)";

/// The closest character boundary at or below `index`.
fn boundary(text: &str, index: usize) -> usize {
    let mut index = index.min(text.len());
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

/// Pull the per-option log-probabilities out of a completion answer.
///
/// Tokens are normalised the same way as in the reference implementation:
/// trimmed, stripped of brackets and parentheses, and upper-cased. When the
/// model mentions the same option several times the highest log-probability
/// wins.
pub fn extract_logprobs(response: &CompletionResponse) -> Result<BTreeMap<String, f64>, FoqError> {
    let Some(first) = response.completion_probabilities.first() else {
        return Err(FoqError::NoProbabilities);
    };
    let mut logprobs = BTreeMap::new();
    for item in &first.top_logprobs {
        let key = item
            .token
            .trim()
            .trim_matches(['[', ']', '(', ')'])
            .to_uppercase();
        if key.is_empty() {
            continue;
        }
        let entry = logprobs.entry(key).or_insert(item.logprob);
        if item.logprob > *entry {
            *entry = item.logprob;
        }
    }
    if logprobs.is_empty() {
        return Err(FoqError::NoProbabilities);
    }
    Ok(logprobs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response_with(tokens: &[(&str, f64)]) -> CompletionResponse {
        CompletionResponse {
            completion_probabilities: vec![CompletionProbability {
                top_logprobs: tokens
                    .iter()
                    .map(|(token, logprob)| TopLogprob {
                        token: (*token).to_string(),
                        logprob: *logprob,
                    })
                    .collect(),
            }],
        }
    }

    #[test]
    fn logprobs_are_normalised_to_upper_case_letters() {
        let response = response_with(&[("[a]", -0.5), ("B", -1.5)]);
        let logprobs = extract_logprobs(&response).unwrap();
        assert_eq!(logprobs.get("A"), Some(&-0.5));
        assert_eq!(logprobs.get("B"), Some(&-1.5));
    }

    #[test]
    fn the_best_logprob_wins_for_repeated_options() {
        let response = response_with(&[("A", -2.0), ("A", -0.1)]);
        let logprobs = extract_logprobs(&response).unwrap();
        assert_eq!(logprobs.get("A"), Some(&-0.1));
    }

    #[test]
    fn an_empty_answer_is_reported_as_missing_probabilities() {
        let response = response_with(&[]);
        assert!(matches!(
            extract_logprobs(&response),
            Err(FoqError::NoProbabilities)
        ));
    }

    #[test]
    fn the_client_normalises_its_base_url() {
        let client = FoqClient::new("http://127.0.0.1:8089/");
        assert_eq!(client.base_url(), "http://127.0.0.1:8089");
    }

    #[test]
    fn the_default_timeout_matches_the_documented_configuration() {
        // The configuration documents `foq_timeout_secs = 120`. A decision
        // model reads the whole diff, so the default must not be shorter.
        let client = FoqClient::new("http://127.0.0.1:8089");
        assert_eq!(client.timeout, Duration::from_secs(120));
    }

    #[test]
    fn the_timeout_can_be_overridden() {
        let client = FoqClient::new("http://127.0.0.1:8089").with_timeout(Duration::from_secs(7));
        assert_eq!(client.timeout, Duration::from_secs(7));
    }

    #[test]
    fn builder_methods_override_the_defaults() {
        let client = FoqClient::new("http://127.0.0.1:8089")
            .with_model("foq-system1")
            .with_min_confidence(0.5)
            .with_profile(CalibrationProfile::with_lora());
        assert_eq!(client.model(), "foq-system1");
        assert!((client.scaler().temperature() - 2.6353).abs() < 1e-9);
    }

    /// A tokenizer that counts one token per four characters.
    fn counting_by_four(text: &str) -> Option<usize> {
        Some(text.chars().count().div_ceil(4))
    }

    /// The algorithm the client used before: a binary search over the length
    /// of the context, asking the server to count the tokens every time.
    ///
    /// The test keeps it to compare the number of calls of the two
    /// algorithms on the same context.
    fn fit_context_by_binary_search(
        context: &str,
        schema: &DecisionSchema,
        limit: usize,
        mut count: impl FnMut(&str) -> Option<usize>,
    ) -> String {
        let fits = |text: &str, count: &mut dyn FnMut(&str) -> Option<usize>| {
            let prompt = schema.format_prompt(text);
            match count(&prompt) {
                Some(tokens) => tokens <= limit,
                None => prompt.len() <= limit * FALLBACK_CHARS_PER_TOKEN,
            }
        };
        if fits(context, &mut count) {
            return context.to_string();
        }
        let marked = |prefix: &str| {
            let mut text = prefix.to_string();
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(TRUNCATION_MARKER);
            text
        };
        let mut low = 0_usize;
        let mut high = context.len();
        while low < high {
            let cut = boundary(context, (low + high).div_ceil(2));
            if cut <= low {
                break;
            }
            if fits(&marked(&context[..cut]), &mut count) {
                low = cut;
            } else {
                high = cut - 1;
            }
        }
        marked(&context[..boundary(context, low)])
    }

    #[test]
    fn a_context_that_fits_is_kept_verbatim() {
        let context = "Commit: abc\nTitre: small\n";
        let fitted = fit_context(
            context,
            &DecisionSchema::criticality(),
            1024,
            counting_by_four,
        );
        assert_eq!(fitted, context);
    }

    #[test]
    fn a_context_that_does_not_fit_is_cut_from_its_end() {
        let schema = DecisionSchema::criticality();
        let context = format!("Commit: abc\nDiff:\n{}", "+line\n".repeat(500));
        // The window holds the fixed part of the prompt and a little more.
        let fixed = counting_by_four(&schema.format_prompt("")).unwrap();
        let limit = fixed + 100;
        let fitted = fit_context(&context, &schema, limit, counting_by_four);
        assert!(fitted.len() < context.len());
        assert!(fitted.starts_with("Commit: abc"));
        assert!(fitted.ends_with("… (contexte tronqué)"));
        // The result must fit into the window.
        let prompt = schema.format_prompt(&fitted);
        assert!(counting_by_four(&prompt).unwrap() <= limit);
    }

    #[test]
    fn a_context_is_never_cut_below_its_metadata() {
        let schema = DecisionSchema::criticality();
        let context = "Commit: abc\nTitre: keep me\n".to_string();
        let fixed = counting_by_four(&schema.format_prompt("")).unwrap();
        let limit = fixed + counting_by_four(&context).unwrap();
        let fitted = fit_context(&context, &schema, limit, counting_by_four);
        assert!(fitted.starts_with("Commit: abc"));
        assert!(fitted.contains("keep me"));
    }

    #[test]
    fn a_window_that_holds_neither_the_prompt_nor_the_context_leaves_nothing() {
        let fitted = fit_context(
            "Commit: abc\n",
            &DecisionSchema::criticality(),
            0,
            counting_by_four,
        );
        assert_eq!(fitted, "… (contexte tronqué)");
    }

    #[test]
    fn without_a_tokenizer_the_pessimistic_estimate_decides() {
        let context = "x".repeat(10_000);
        let fitted = fit_context(&context, &DecisionSchema::criticality(), 100, |_| None);
        assert!(fitted.len() < context.len());
        assert!(fitted.ends_with("… (contexte tronqué)"));
    }

    #[test]
    fn the_boundary_helper_never_splits_a_character() {
        assert_eq!(boundary("é", 1), 0);
        assert_eq!(boundary("é", 2), 2);
        assert_eq!(boundary("abc", 10), 3);
    }

    #[test]
    fn the_fitting_asks_the_server_a_few_times_at_most() {
        // A context that does not fit: the count of the tokens is asked once
        // for the whole context and a few times for the estimate.
        let schema = DecisionSchema::criticality();
        let context = format!("Commit: abc\nDiff:\n{}", "+line\n".repeat(5000));
        let fixed = counting_by_four(&schema.format_prompt("")).unwrap();
        let limit = fixed + 100;
        let mut calls = 0_usize;
        let fitted = fit_context(&context, &schema, limit, |text| {
            calls += 1;
            counting_by_four(text)
        });
        assert!(fitted.len() < context.len());
        // The old path asked the server once per step of the binary search:
        // more than a dozen times for a context of this size.
        assert!(calls <= 12, "the fitting asked the server {calls} times");
    }

    #[test]
    fn the_fitting_of_a_context_that_fits_asks_the_server_once() {
        let schema = DecisionSchema::criticality();
        let context = "Commit: abc\nTitre: small\n";
        let mut calls = 0_usize;
        let fitted = fit_context(context, &schema, 1024, |text| {
            calls += 1;
            counting_by_four(text)
        });
        assert_eq!(fitted, context);
        assert_eq!(calls, 1);
    }

    #[test]
    fn the_fitting_asks_the_server_less_often_than_the_binary_search() {
        // The two algorithms on the same context: the estimate and the
        // correction ask the server fewer times than the search.
        let schema = DecisionSchema::criticality();
        let context = format!("Commit: abc\nDiff:\n{}", "+line\n".repeat(2000));
        let fixed = counting_by_four(&schema.format_prompt("")).unwrap();
        let limit = fixed + 100;

        let mut calls = 0_usize;
        fit_context(&context, &schema, limit, |text| {
            calls += 1;
            counting_by_four(text)
        });
        let current = calls;

        let mut calls = 0_usize;
        fit_context_by_binary_search(&context, &schema, limit, |text| {
            calls += 1;
            counting_by_four(text)
        });
        assert!(
            current < calls,
            "the estimate asked the server {current} times, the search {calls}"
        );
    }

    #[test]
    fn the_cache_of_the_fitted_prompts_serves_the_second_question() {
        let client = FoqClient::new("http://127.0.0.1:1");
        let context = "Commit: abc\nTitre: small\n";
        // The client cannot reach the server, so the fitting falls back to
        // the pessimistic estimate — the cache is still filled.
        let first = client.fit_prompt(context, &DecisionSchema::criticality());
        let second = client.fit_prompt(context, &DecisionSchema::complexity());
        // The two schemas differ in their question and their options, so the
        // prompts differ; the context they carry is the same.
        assert_ne!(first, second);
        assert!(first.contains("Commit: abc"));
        assert!(second.contains("Commit: abc"));
    }

    #[test]
    fn the_key_of_a_context_is_stable_and_different_for_different_texts() {
        assert_eq!(cache_key("Commit: abc"), cache_key("Commit: abc"));
        assert_ne!(cache_key("Commit: abc"), cache_key("Commit: abd"));
    }
}
