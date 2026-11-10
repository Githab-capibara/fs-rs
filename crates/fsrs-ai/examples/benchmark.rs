//! Benchmark of the local decision model.
//!
//! The benchmark rates a set of real upstream commits with the local model
//! and reports how long the rating takes and what the model says. It is the
//! tool behind the numbers in the [benchmark report][report].
//!
//! ```text
//! cargo run -p fsrs-ai --example benchmark -- <repository> <revision> [limit]
//! ```
//!
//! * `<repository>` — path of a git working copy,
//! * `<revision>` — any revision understood by `git log`, for example `HEAD`
//!   or `main~100`,
//! * `limit` — how many commits to rate, newest first. The default is 40.
//!
//! The report is written to standard output as JSON, so that it can be
//! stored and compared over time:
//!
//! ```text
//! cargo run -p fsrs-ai --example benchmark -- /tmp/upstream HEAD 60 \
//!     > docs/benchmarks/decision-model.json
//! ```
//!
//! [report]: ../../docs/benchmarks/README.md

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Command;
use std::time::Instant;

use fsrs_ai::{CalibrationProfile, FoqClient, assess_commit};
use fsrs_core::catalog::to_upstream_commit;
use fsrs_core::git::GitRepo;

/// One rated commit.
///
/// A commit that the model refused to rate is kept in the report with the
/// reason in `error`, so that a single failure never hides the rest of the
/// run.
#[derive(serde::Serialize)]
struct Row {
    sha: String,
    summary: String,
    criticality: String,
    complexity: String,
    criticality_confidence: f64,
    complexity_confidence: f64,
    needs_review: bool,
    latency_ms: f64,
    /// The reason why the commit could not be rated, if it failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

/// The summary of a run.
struct Report {
    repository: String,
    revision: String,
    model: String,
    commits: usize,
    failed: usize,
    latency_p50: f64,
    latency_p95: f64,
    latency_mean: f64,
    latency_min: f64,
    latency_max: f64,
    criticality: BTreeMap<String, usize>,
    complexity: BTreeMap<String, usize>,
    needs_review: usize,
    rows: Vec<Row>,
}

fn main() -> anyhow::Result<()> {
    let mut arguments = std::env::args().skip(1);
    let repository = PathBuf::from(
        arguments
            .next()
            .ok_or_else(|| anyhow::anyhow!("usage: benchmark <repository> <revision> [limit]"))?,
    );
    let revision = arguments
        .next()
        .ok_or_else(|| anyhow::anyhow!("usage: benchmark <repository> <revision> [limit]"))?;
    let limit: usize = arguments
        .next()
        .map(|value| value.parse())
        .transpose()?
        .unwrap_or(40);

    let url = std::env::var("FSRS_FOQ_URL").unwrap_or_else(|_| "http://127.0.0.1:8089".to_string());
    // The decision server runs the model with its decision adapter, so the
    // calibration of the `lora` profile is the right one.
    let client = FoqClient::new(&url)
        .with_profile(CalibrationProfile::with_lora())
        .with_timeout(std::time::Duration::from_secs(180));
    if !client.is_ready() {
        anyhow::bail!("the decision model is not answering on {url}");
    }

    let git = GitRepo::open(&repository)?;
    let shas = list_commits(&repository, &revision, limit)?;

    let mut rows = Vec::new();
    let mut criticality: BTreeMap<String, usize> = BTreeMap::new();
    let mut complexity: BTreeMap<String, usize> = BTreeMap::new();
    let mut needs_review = 0;
    let mut failed = 0;

    for sha in &shas {
        let commit = git.resolve_commit(sha)?;
        let pending = git.describe_commit(&commit)?;
        let upstream = to_upstream_commit(0, pending, "");
        let patch = git.commit_patch(sha).ok();

        let started = Instant::now();
        let assessment = match assess_commit(&client, &upstream, patch.as_deref()) {
            Ok(assessment) => assessment,
            Err(error) => {
                // A single commit that the model refuses must not hide the
                // rest of the run.
                failed += 1;
                eprintln!("     FAIL  {}  {error}  {}", upstream.sha, upstream.summary);
                rows.push(Row {
                    sha: upstream.sha.clone(),
                    summary: upstream.summary.clone(),
                    criticality: String::new(),
                    complexity: String::new(),
                    criticality_confidence: 0.0,
                    complexity_confidence: 0.0,
                    needs_review: true,
                    latency_ms: round(started.elapsed().as_secs_f64() * 1000.0),
                    error: Some(error.to_string()),
                });
                continue;
            }
        };
        let latency_ms = started.elapsed().as_secs_f64() * 1000.0;

        *criticality
            .entry(assessment.criticality.label().to_string())
            .or_insert(0) += 1;
        *complexity
            .entry(assessment.complexity.label().to_string())
            .or_insert(0) += 1;
        if assessment.needs_review {
            needs_review += 1;
        }

        eprintln!(
            "{:>8.0} ms  {:<9} {:<9} {}",
            latency_ms,
            assessment.criticality.label(),
            assessment.complexity.label(),
            upstream.summary
        );

        rows.push(Row {
            sha: upstream.sha.clone(),
            summary: upstream.summary.clone(),
            criticality: assessment.criticality.label().to_string(),
            complexity: assessment.complexity.label().to_string(),
            criticality_confidence: round(assessment.criticality_confidence),
            complexity_confidence: round(assessment.complexity_confidence),
            needs_review: assessment.needs_review,
            latency_ms: round(latency_ms),
            error: None,
        });
    }

    // Only the commits that the model rated have a latency worth reporting.
    let mut latencies: Vec<f64> = rows
        .iter()
        .filter(|row| row.error.is_none())
        .map(|row| row.latency_ms)
        .collect();
    latencies.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let report = Report {
        repository: repository.display().to_string(),
        revision: revision.clone(),
        model: client.model().to_string(),
        commits: rows.len() - failed,
        failed,
        latency_p50: percentile(&latencies, 0.50),
        latency_p95: percentile(&latencies, 0.95),
        latency_mean: if latencies.is_empty() {
            0.0
        } else {
            latencies.iter().sum::<f64>() / latencies.len() as f64
        },
        latency_min: latencies.first().copied().unwrap_or(0.0),
        latency_max: latencies.last().copied().unwrap_or(0.0),
        criticality,
        complexity,
        needs_review,
        rows,
    };

    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

/// List the object ids of the commits to rate, newest first.
fn list_commits(
    repository: &std::path::Path,
    revision: &str,
    limit: usize,
) -> anyhow::Result<Vec<String>> {
    let output = Command::new("git")
        .args(["log", "--format=%H", "-n", &limit.to_string(), revision])
        .current_dir(repository)
        .output()?;
    if !output.status.success() {
        anyhow::bail!(
            "git log failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|line| line.trim().to_string())
        .filter(|line| !line.is_empty())
        .collect())
}

/// The `p`-th percentile of a sorted list of measurements.
fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let index = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[index]
}

/// The report of a run, as it is written to standard output.
impl serde::Serialize for Report {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;

        let mut state = serializer.serialize_struct("Report", 14)?;
        state.serialize_field("repository", &self.repository)?;
        state.serialize_field("revision", &self.revision)?;
        state.serialize_field("model", &self.model)?;
        state.serialize_field("commits", &self.commits)?;
        state.serialize_field("failed", &self.failed)?;
        state.serialize_field("latency_p50_ms", &round(self.latency_p50))?;
        state.serialize_field("latency_p95_ms", &round(self.latency_p95))?;
        state.serialize_field("latency_mean_ms", &round(self.latency_mean))?;
        state.serialize_field("latency_min_ms", &round(self.latency_min))?;
        state.serialize_field("latency_max_ms", &round(self.latency_max))?;
        state.serialize_field("criticality", &self.criticality)?;
        state.serialize_field("complexity", &self.complexity)?;
        state.serialize_field("needs_review", &self.needs_review)?;
        state.serialize_field("rows", &self.rows)?;
        state.end()
    }
}

/// Round a number to three decimals, so that the report stays readable.
fn round(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}
