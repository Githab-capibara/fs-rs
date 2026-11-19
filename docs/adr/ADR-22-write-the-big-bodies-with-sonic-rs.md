# 22. Write the big bodies with sonic-rs

- **Status:** Accepted
- **Date:** 2026-11-19
- **Deciders:** @Githab-capibara
- **Related:** [ADR-10](ADR-10-the-decision-server-is-asked-four-times.md),
  [ADR-11](ADR-11-build-for-the-machine.md),
  [`crates/fsrs-ai/benches/json.rs`](../../crates/fsrs-ai/benches/json.rs),
  [`crates/fsrs-ai/tests/sonic_conformance.rs`](../../crates/fsrs-ai/tests/sonic_conformance.rs)

## Context

Every rating of a commit writes the body of a request to the local runtime.
The body carries the prompt, and the prompt carries the diff of the commit, so
the body is tens of kilobytes of text. The same body is written twice for every
commit — once for the criticality and once for the complexity.

The general purpose serializer was the only one in the tree. It is correct and
it is slow on long strings: it walks the string byte by byte and checks every
boundary. A serializer that uses the instructions of the processor was already
in the dependency tree of the runtime, and the build was tuned for the machine
it runs on ([ADR-11](ADR-11-build-for-the-machine.md)), so the instructions
were there but unused.

Two things hold the general purpose serializer in place, and both are outside
of our reach:

* the answers of `/props` and `/tokenize` are read into an untyped value, and
  the typed serializer does not build one,
* the protocol of the agents is implemented by a crate that uses the general
  purpose serializer inside itself.

The two serializers have to agree, byte for byte. A difference between them
would be silent: the runtime would answer something that the client refuses to
read, and nothing would say why.

## Decision

The workspace keeps both serializers and uses each of them where it fits:

* the typed bodies — the request of a completion, the answer of a completion,
  the column `files` of the commits — are written and read with `sonic-rs`,
* the small untyped answers of `/props` and `/tokenize`, and everything the
  protocol of the agents does, stay with `serde_json`,
* `fsrs-core` depends on `sonic-rs` only: it has no use for the untyped
  serializer,
* the HTTP client is built without the `json` feature, so the tree holds one
  serializer less than before,
* a differential test runs the same values through both serializers and
  compares the bytes and the values they read back.

## Consequences

- **Easier:** the body of a request is written in 11.5 µs instead of 68.3 µs —
  five times and a half faster on the fixture of sixty thousand characters of
  diff. The numbers are the medians of Criterion over
  [`crates/fsrs-ai/benches/json.rs`](../../crates/fsrs-ai/benches/json.rs). The
  crate of the core is lighter by one dependency, and the tree holds one
  serializer less.
- **Harder:** the tree holds two serializers, and a value that crosses from
  one to the other has to be converted. The differential test is the guard: it
  fails the build when the two disagree.
- **Given up:** the fast serializer is not faster when the body is read. The
  answer of the runtime is read in 10.4 µs by `sonic-rs` and in 10.0 µs by
  `serde_json` — the difference is inside the noise, so the small answers stay
  with the general purpose serializer, which knows the format better.
- **Migration:** none. The bodies of the protocol of the agents are untouched:
  they belong to the crate that implements it.

## Alternatives considered

- **Option A:** keep one serializer for the whole tree. Rejected because the
  untyped answers and the protocol of the agents need the general purpose one
  anyway, and replacing it would mean a rewrite of the protocol.
- **Option B:** rename `serde_json` to `sonic-rs` in the manifests, so the
  whole tree uses the fast serializer. Rejected because the trick does not
  compile: the two crates do not expose the same API, and the parts that need
  the untyped values would break.
- **Option C:** use the fast serializer for the answers of the runtime as
  well. Measured and rejected: the answers are small, and the general purpose
  serializer reads them as fast.
