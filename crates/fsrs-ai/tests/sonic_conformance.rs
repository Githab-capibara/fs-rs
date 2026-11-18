//! The two serializers must agree, byte for byte.
//!
//! The client writes and reads the body of a request with `sonic-rs`, which
//! is faster than the general purpose serializer but knows less about the
//! format. A difference between the two would be silent: the answer of the
//! server would be wrong, or refused, and nothing would say why.
//!
//! The test therefore runs the same values through both serializers and
//! compares the bytes and the values they read back.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// A request of the runtime, shaped like the one of the client.
#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct CompletionRequest {
    prompt: String,
    n_predict: i64,
    n_probs: i64,
    temperature: f64,
    cache_prompt: bool,
}

/// The answer of the runtime, shaped like the one of the client.
#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct CompletionResponse {
    content: String,
    tokens: Vec<i64>,
    stop: bool,
    model: String,
    tokens_predicted: i64,
    /// The probabilities of the tokens, when the server sends them.
    #[serde(default)]
    top_logprobs: Vec<TopLogprob>,
}

/// One candidate token with its log-probability.
#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct TopLogprob {
    token: String,
    logprob: f64,
}

/// The values that every round trip must survive.
fn samples() -> Vec<CompletionRequest> {
    vec![
        CompletionRequest {
            prompt: String::new(),
            n_predict: 1,
            n_probs: 25,
            temperature: 0.0,
            cache_prompt: true,
        },
        CompletionRequest {
            prompt: "Commit: abc\nDiff:\n+line\n".to_string(),
            n_predict: 1,
            n_probs: 25,
            temperature: 0.0,
            cache_prompt: true,
        },
        CompletionRequest {
            // The characters that a naive serializer escapes differently.
            prompt: "quotes \" backslash \\ slash / newline \n tab \t".to_string(),
            n_predict: 128,
            n_probs: 1,
            temperature: 0.7,
            cache_prompt: false,
        },
        CompletionRequest {
            // The characters outside the basic plane.
            prompt: "emoji 🦀 and accents éàü and a control \u{0007}".to_string(),
            n_predict: 1,
            n_probs: 25,
            temperature: -0.5,
            cache_prompt: true,
        },
        CompletionRequest {
            // A diff of the size the dashboard sends.
            prompt: "+ line of the diff\n".repeat(500),
            n_predict: 1,
            n_probs: 25,
            temperature: 0.0,
            cache_prompt: true,
        },
    ]
}

#[test]
fn the_two_serializers_write_the_same_bytes() {
    for sample in samples() {
        let reference =
            serde_json::to_string(&sample).expect("serde_json should write the request");
        let fast = sonic_rs::to_string(&sample).expect("sonic-rs should write the request");
        assert_eq!(
            reference, fast,
            "the two serializers disagree about the request"
        );
    }
}

#[test]
fn the_two_serializers_read_the_same_values() {
    let responses = vec![
        CompletionResponse {
            content: "C".to_string(),
            tokens: Vec::new(),
            stop: true,
            model: "foq".to_string(),
            tokens_predicted: 1,
            top_logprobs: Vec::new(),
        },
        CompletionResponse {
            content: "".to_string(),
            tokens: vec![1, 2, 3],
            stop: false,
            model: "foq".to_string(),
            tokens_predicted: 3,
            top_logprobs: vec![TopLogprob {
                token: "A".to_string(),
                logprob: -0.5,
            }],
        },
    ];
    for response in responses {
        let bytes = serde_json::to_string(&response).expect("serde_json should write the answer");
        let from_serde: CompletionResponse =
            serde_json::from_str(&bytes).expect("serde_json should read the answer");
        let from_sonic: CompletionResponse =
            sonic_rs::from_str(&bytes).expect("sonic-rs should read the answer");
        assert_eq!(from_serde, from_sonic);
        // And what one writes, the other reads.
        let fast = sonic_rs::to_string(&response).expect("sonic-rs should write the answer");
        let from_fast: CompletionResponse =
            serde_json::from_str(&fast).expect("serde_json should read the answer of sonic-rs");
        assert_eq!(from_serde, from_fast);
    }
}

#[test]
fn the_two_serializers_keep_the_escapes_the_same() {
    // The strings that a hand-written serializer tends to escape differently.
    let tricky = [
        "plain",
        "quote\"inside",
        "backslash\\inside",
        "newline\ninside",
        "tab\tinside",
        "carriage\rreturn",
        "control\u{0001}character",
        "solidus/inside",
        "unicode🦀",
    ];
    for text in tricky {
        let mut map = BTreeMap::new();
        map.insert("text", text);
        let reference = serde_json::to_string(&map).expect("serde_json should write the map");
        let fast = sonic_rs::to_string(&map).expect("sonic-rs should write the map");
        assert_eq!(
            reference, fast,
            "the two serializers disagree about {text:?}"
        );
    }
}

#[test]
fn the_two_serializers_reject_the_same_broken_bodies() {
    let broken = [
        "",
        "{",
        "{\"content\":}",
        "{\"content\":\"unclosed}",
        "not json at all",
    ];
    for body in broken {
        let from_serde = serde_json::from_str::<CompletionResponse>(body).is_ok();
        let from_sonic = sonic_rs::from_str::<CompletionResponse>(body).is_ok();
        assert_eq!(
            from_serde, from_sonic,
            "the two serializers disagree about {body:?}"
        );
    }
}
