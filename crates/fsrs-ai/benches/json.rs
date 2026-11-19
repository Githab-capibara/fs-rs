//! Benchmark of the two serializers of the workspace.
//!
//! The body of a completion carries the prompt of the commit, and the prompt
//! carries the diff. The payload is therefore tens of kilobytes of text, and
//! the serializer that writes it is on the path of every rating of every
//! commit.
//!
//! Two serializers are measured on the same fixture:
//!
//! * `sonic-rs`, which the client uses for the typed bodies,
//! * `serde_json`, which the client keeps for the small untyped answers of
//!   the runtime.
//!
//! The fixture is a request of the runtime with a diff of the size the
//! dashboard sends.
//!
//! ```bash
//! cargo bench -p fsrs-ai --bench json
//! ```

use criterion::{Criterion, criterion_group, criterion_main};
use serde::{Deserialize, Serialize};

/// The size of the diff of the fixture.
const DIFF_CHARS: usize = 60_000;

/// A request of the runtime, shaped like the one of the client.
#[derive(Debug, Serialize, Deserialize)]
struct CompletionRequest {
    prompt: String,
    n_predict: i64,
    n_probs: i64,
    temperature: f64,
    cache_prompt: bool,
}

/// The answer of the runtime, shaped like the one of the client.
#[derive(Debug, Serialize, Deserialize)]
struct CompletionResponse {
    content: String,
    tokens: Vec<i64>,
    stop: bool,
    model: String,
    tokens_predicted: i64,
}

/// The context of the fixture: the metadata of a commit and its diff.
fn prompt() -> String {
    let mut prompt = String::from(
        "Commit: 0123456789abcdef0123456789abcdef01234567\n\
         Title: Fix the parser of the configuration file\n\
         Author: dev <dev@example.com>\n\
         Date: 2026-11-19\n\
         Files: src/config.rs, src/parser.rs\n\
         +42 -17\n\n\
         Diff:\n",
    );
    while prompt.len() < DIFF_CHARS {
        prompt.push_str("+ the parser ignored the last section of the file\n");
    }
    prompt
}

/// The request of the fixture.
fn request() -> CompletionRequest {
    CompletionRequest {
        prompt: prompt(),
        n_predict: 1,
        n_probs: 25,
        temperature: 0.0,
        cache_prompt: true,
    }
}

/// The answer of the fixture: the text of a decision and its tokens.
fn response() -> CompletionResponse {
    CompletionResponse {
        content: "The change touches the parser of the configuration, so it is critical.".repeat(4),
        tokens: (0..512).collect(),
        stop: true,
        model: "qwen3-30b".to_string(),
        tokens_predicted: 512,
    }
}

fn serializers(criterion: &mut Criterion) {
    let request = request();
    let response = response();

    let mut group = criterion.benchmark_group("json");
    group.bench_function("write_the_request_with_sonic", |bencher| {
        bencher.iter(|| sonic_rs::to_string(&request).expect("sonic-rs should write the request"))
    });
    group.bench_function("write_the_request_with_serde_json", |bencher| {
        bencher
            .iter(|| serde_json::to_string(&request).expect("serde_json should write the request"))
    });
    group.bench_function("read_the_answer_with_sonic", |bencher| {
        let body = serde_json::to_string(&response).expect("serde_json should write the answer");
        bencher.iter(|| {
            sonic_rs::from_str::<CompletionResponse>(&body)
                .expect("sonic-rs should read the answer")
        })
    });
    group.bench_function("read_the_answer_with_serde_json", |bencher| {
        let body = serde_json::to_string(&response).expect("serde_json should write the answer");
        bencher.iter(|| {
            serde_json::from_str::<CompletionResponse>(&body)
                .expect("serde_json should read the answer")
        })
    });
    group.finish();
}

criterion_group!(benches, serializers);
criterion_main!(benches);
