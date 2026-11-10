//! A mock ACP agent for the tests of the dashboard.
//!
//! The binary is a thin wrapper around [`fsrs_acp::mock`]: it speaks the
//! Agent Client Protocol over standard input and output, but it does not
//! think. The tests of the dashboard use it instead of a real agent, so that
//! they do not need a network connection and a provider.

fn main() {
    let options = fsrs_acp::mock::MockOptions::from_args(std::env::args().skip(1));
    fsrs_acp::mock::serve(options);
}
