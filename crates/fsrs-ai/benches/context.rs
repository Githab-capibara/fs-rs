//! Benchmark of the fitting of a prompt into the window of the runtime.
//!
//! The fitting asks the runtime to count the tokens of a prompt, and the
//! number of those calls is what costs time: the runtime answers in
//! milliseconds, but every call is a round trip over the loopback.
//!
//! Two algorithms are measured on the same fixture:
//!
//! * the current one, which counts the whole context once and corrects the
//!   estimate of the cut,
//! * the previous one, which searched the length of the cut with a binary
//!   search and asked the runtime once per step.
//!
//! The fixture is a context of the size the dashboard sends: the metadata of
//! a commit and a diff of six thousand characters.
//!
//! ```bash
//! cargo bench -p fsrs-ai --bench context
//! ```

use std::cell::Cell;

use criterion::{Criterion, criterion_group, criterion_main};
use fsrs_ai::client::fit_context;
use fsrs_ai::schema::DecisionSchema;

/// The size of the diff of the fixture.
const DIFF_CHARS: usize = 6000;

/// The size of the window of the runtime, in tokens.
///
/// The window is smaller than the one of the local model on purpose: a
/// window of 4096 tokens holds every context the dashboard sends, so the
/// fitting would never cut anything and the two algorithms would look the
/// same. A window of 1024 tokens is the interesting case — the context has
/// to be cut, and the algorithms differ.
const WINDOW: usize = 1024;

/// The marker appended to a context that had to be shortened.
const TRUNCATION_MARKER: &str = "… (contexte tronqué)";

/// A tokenizer that counts one token per four characters.
///
/// The tokenizer of the fixture is deterministic, so the measurement is of
/// the algorithm and not of the server.
fn tokens_of(text: &str) -> usize {
    text.chars().count().div_ceil(4)
}

/// The context of the fixture: the metadata of a commit and its diff.
fn context() -> String {
    let mut context = String::from(
        "Commit: 0123456789abcdef0123456789abcdef01234567\n\
         Title: Fix the parser of the configuration file\n\
         Author: dev <dev@example.com>\n\
         Date: 2026-11-17\n\
         Files: src/config.rs, src/parser.rs\n\
         +42 -17\n\n\
         Diff:\n",
    );
    while context.len() < DIFF_CHARS {
        context.push_str("+ the parser ignored the last section of the file\n");
    }
    context
}

/// The algorithm the client used before: a binary search over the length of
/// the context, asking the runtime to count the tokens every time.
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
            None => prompt.len() <= limit * 2,
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
    let boundary = |index: usize| {
        let mut index = index.min(context.len());
        while index > 0 && !context.is_char_boundary(index) {
            index -= 1;
        }
        index
    };
    let mut low = 0_usize;
    let mut high = context.len();
    while low < high {
        let cut = boundary((low + high).div_ceil(2));
        if cut <= low {
            break;
        }
        if fits(&marked(&context[..cut]), &mut count) {
            low = cut;
        } else {
            high = cut - 1;
        }
    }
    marked(&context[..boundary(low)])
}

/// The time of the fitting, and the number of calls it made.
fn measure(
    fit: impl Fn(&str, &DecisionSchema, usize, &mut dyn FnMut(&str) -> Option<usize>) -> String,
    context: &str,
    schema: &DecisionSchema,
    limit: usize,
) -> (std::time::Duration, usize) {
    let calls = Cell::new(0_usize);
    let started = std::time::Instant::now();
    let mut counter = |text: &str| {
        calls.set(calls.get() + 1);
        Some(tokens_of(text))
    };
    let _ = fit(context, schema, limit, &mut counter);
    (started.elapsed(), calls.get())
}

fn context_fitting(criterion: &mut Criterion) {
    let context = context();
    let schema = DecisionSchema::criticality();
    let limit = WINDOW - 16;

    // The number of calls to the runtime is the number that matters: every
    // call is a round trip over the loopback, and the local tokenizer of the
    // benchmark is too fast to show the difference in time.
    let (_, current_calls) = measure(
        |context, schema, limit, count| fit_context(context, schema, limit, count),
        &context,
        &schema,
        limit,
    );
    let (_, previous_calls) = measure(
        |context, schema, limit, count| fit_context_by_binary_search(context, schema, limit, count),
        &context,
        &schema,
        limit,
    );
    eprintln!(
        "calls to the runtime: the estimate and the correction {current_calls}, \
         the binary search {previous_calls}"
    );

    let mut group = criterion.benchmark_group("context_fitting");
    group.bench_function("estimate_and_correct", |bencher| {
        bencher.iter(|| {
            let (elapsed, calls) = measure(
                |context, schema, limit, count| fit_context(context, schema, limit, count),
                &context,
                &schema,
                limit,
            );
            (elapsed, calls)
        })
    });
    group.bench_function("binary_search", |bencher| {
        bencher.iter(|| {
            let (elapsed, calls) = measure(
                |context, schema, limit, count| {
                    fit_context_by_binary_search(context, schema, limit, count)
                },
                &context,
                &schema,
                limit,
            );
            (elapsed, calls)
        })
    });
    group.finish();
}

criterion_group!(benches, context_fitting);
criterion_main!(benches);
