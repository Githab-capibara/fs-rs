//! Measure what parallel assessments do to the decision server.
//!
//! The server of the decision model has a number of slots: every slot holds
//! one request at a time. The assessment of a commit asks the server twice —
//! once for the criticality and once for the complexity — and the second ask
//! reuses the fitted prompt, so the cost of one assessment is one completion
//! and a few tokenizations.
//!
//! The probe sends the same number of completions in a row and then the same
//! number in parallel, and reports how long each run took. The difference is
//! the gain of the parallelism — or its absence, when the server has one slot
//! and the requests queue up behind each other anyway.
//!
//! ```bash
//! cargo run -p fsrs-ai --example slot-probe -- http://127.0.0.1:8089 4
//! ```

use std::time::Instant;

use fsrs_ai::{DecisionSchema, FoqClient};

/// The context of the probe: a small commit description.
const CONTEXT: &str = "Commit: 0123456789abcdef\nTitle: fix the crash in the parser\n\nThe parser used to read past the end of the buffer.";

fn main() {
    let mut arguments = std::env::args().skip(1);
    let url = arguments
        .next()
        .unwrap_or_else(|| "http://127.0.0.1:8089".to_string());
    let count: usize = arguments
        .next()
        .unwrap_or_else(|| "4".to_string())
        .parse()
        .unwrap_or(4);

    let client = FoqClient::new(&url);
    if !client.is_ready() {
        eprintln!("the server at {url} does not answer");
        std::process::exit(1);
    }

    let schema = DecisionSchema::criticality();
    // The prompt is fitted once, so the measurement is of the completions and
    // not of the fitting.
    let _ = client.fit_prompt(CONTEXT, &schema);

    // One after another: the baseline.
    let started = Instant::now();
    for index in 0..count {
        let decision = client
            .decide(CONTEXT, &schema)
            .unwrap_or_else(|error| panic!("the request {index} failed: {error}"));
        std::hint::black_box(decision.decision);
    }
    let serial = started.elapsed();

    // All of them at once: the parallel run.
    let started = Instant::now();
    let handles: Vec<_> = (0..count)
        .map(|index| {
            let client = client.clone();
            let schema = schema.clone();
            std::thread::spawn(move || {
                let decision = client
                    .decide(CONTEXT, &schema)
                    .unwrap_or_else(|error| panic!("the request {index} failed: {error}"));
                std::hint::black_box(decision.decision);
            })
        })
        .collect();
    for handle in handles {
        handle.join().expect("the thread should not panic");
    }
    let parallel = started.elapsed();

    println!("requests={count}");
    println!("slots={}", client.slots());
    println!("serial_ms={}", serial.as_millis());
    println!("parallel_ms={}", parallel.as_millis());
    println!(
        "per_request_serial_ms={}",
        serial.as_millis() / count as u128
    );
    println!(
        "per_request_parallel_ms={}",
        parallel.as_millis() / count as u128
    );
}
