//! Measure the cost of the graph walk on a real repository.
//!
//! The probe opens a repository, lists the commits that are missing from one
//! branch and reports how long the listing took. It is run twice: with the
//! caching and the hash verification of git2 turned on, and with them turned
//! off. The difference is the gain of the options that the checklist of the
//! performance work mentions in its eleventh point.
//!
//! ```bash
//! cargo run -p fsrs-core --example revwalk-probe -- /path/to/repo main
//! ```

use std::time::Instant;

use fsrs_core::git::GitRepo;

fn main() {
    let mut arguments = std::env::args().skip(1);
    let path = match arguments.next() {
        Some(path) => path,
        None => {
            eprintln!("usage: revwalk-probe <path> [branch] [--no-cache] [--no-verify]");
            std::process::exit(2);
        }
    };
    let branch = arguments.next().unwrap_or_else(|| "main".to_string());
    let no_cache = arguments.any(|argument| argument == "--no-cache");
    let no_verify = arguments.any(|argument| argument == "--no-verify");

    if no_cache {
        git2::opts::enable_caching(false);
    }
    if no_verify {
        git2::opts::strict_hash_verification(false);
    }

    let repo = GitRepo::open(std::path::Path::new(&path)).expect("the repository should open");
    // The listing of the metadata costs one object read per commit; the
    // listing of the commits also computes the statistics, which compares two
    // trees per commit. The difference tells how much of the time is the
    // graph walk and how much is the diff.
    let started = Instant::now();
    let metadata = repo
        .pending_commit_metadata(&branch)
        .expect("the listing should work");
    let metadata_elapsed = started.elapsed();

    let started = Instant::now();
    let pending = repo
        .pending_commits(&branch)
        .expect("the listing should work");
    let elapsed = started.elapsed();

    println!(
        "commits={} metadata_ms={} full_ms={} diff_ms={} caching={} verification={}",
        pending.len(),
        metadata_elapsed.as_millis(),
        elapsed.as_millis(),
        elapsed.saturating_sub(metadata_elapsed).as_millis(),
        if no_cache { "off" } else { "on" },
        if no_verify { "off" } else { "on" },
    );
    let _ = metadata;
}
