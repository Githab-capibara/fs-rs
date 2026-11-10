//! A mock ACP agent used by the integration tests of the pipeline.
//!
//! The agent speaks the Agent Client Protocol over standard input and output
//! and plays the three roles of the pipeline. The behaviour lives in
//! [`fsrs_acp::mock`], so that the tests of the dashboard can start the same
//! agent through the `fsrs-mock-agent` binary.
//!
//! The behaviour is tuned with the command line arguments `--rejections <n>`
//! (how often the verifier rejects), `--no-commit` (the implementer does not
//! commit its work) and `--no-verdict` (the verifier never prints a verdict).

fn main() {
    let options = fsrs_acp::mock::MockOptions::from_args(std::env::args().skip(1));
    fsrs_acp::mock::serve(options);
}
