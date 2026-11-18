//! Measure how long it takes to open a repository.
//!
//! The dashboard reads the patch of every commit it rates, and the patch is
//! read from the working copy of the fork. The working copy is opened for
//! every commit, so the time of the opening is paid for once per commit of
//! the backlog.
//!
//! ```bash
//! cargo run -p fsrs-core --example repo-open-probe -- /tmp/opencode/fd-fork 50
//! ```

use std::time::Instant;

fn main() {
    let mut arguments = std::env::args().skip(1);
    let path = arguments
        .next()
        .unwrap_or_else(|| "/tmp/opencode/fd-fork".to_string());
    let runs: usize = arguments
        .next()
        .and_then(|value| value.parse().ok())
        .unwrap_or(50);

    let started = Instant::now();
    for _ in 0..runs {
        let git = fsrs_core::git::GitRepo::open(std::path::Path::new(&path))
            .expect("the repository should open");
        std::hint::black_box(&git);
    }
    let elapsed = started.elapsed();
    println!(
        "{runs} openings of {path}: {:?} in total, {:?} each",
        elapsed,
        elapsed / runs as u32
    );
}
