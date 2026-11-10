//! Interaction tests of the dashboard window.
//!
//! The tests drive the real window through [`egui_kittest`]: they build a
//! temporary upstream repository and a lagging fork, start the dashboard on
//! top of them and then click the widgets the user would click.
//!
//! Three things are checked:
//!
//! * the window shows the tracked repository, the missing commits and the
//!   button that starts an implementation,
//! * clicking a commit asks which model the terminal should use,
//! * clicking `РЕАЛИЗОВАТЬ` runs the whole three-agent pipeline against the
//!   mock agent and merges the result into the main branch.
//!
//! The dashboard talks to three programs: the local decision model, the
//! OpenCode agent and the git binary. The tests replace the first two with
//! local stand-ins: a tiny HTTP server answers like the decision model does,
//! and the mock agent of `fsrs-acp` speaks the agent protocol.

use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use fsrs_app::app::FsrsApp;
use fsrs_app::state::PickerIntent;
use fsrs_app::worker::Worker;
use fsrs_core::{Config, RepoConfig, RunState, Store};

// ----------------------------------------------------------------------
// The stand-ins
// ----------------------------------------------------------------------

/// The mock agent that speaks the agent protocol.
const MOCK_AGENT: &str = env!("CARGO_BIN_EXE_fsrs-mock-agent");

/// Run a git command inside `dir` and assert that it succeeded.
fn git(dir: &Path, arguments: &[&str]) {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(dir)
        .output()
        .expect("running git");
    assert!(
        output.status.success(),
        "git {arguments:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Run a git command inside `dir` and return its standard output.
fn git_output(dir: &Path, arguments: &[&str]) -> String {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(dir)
        .output()
        .expect("running git");
    assert!(
        output.status.success(),
        "git {arguments:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).to_string()
}

/// Create the upstream repository with two commits.
fn make_upstream(root: &Path) -> PathBuf {
    let path = root.join("upstream");
    std::fs::create_dir_all(&path).unwrap();
    git(&path, &["init", "-q", "-b", "main"]);
    git(&path, &["config", "user.email", "upstream@example.com"]);
    git(&path, &["config", "user.name", "Upstream"]);

    std::fs::write(path.join("a.txt"), "one\n").unwrap();
    git(&path, &["add", "."]);
    git(&path, &["commit", "-q", "-m", "First commit"]);

    std::fs::write(path.join("b.txt"), "two\n").unwrap();
    git(&path, &["add", "."]);
    git(&path, &["commit", "-q", "-m", "Add the second file"]);

    path
}

/// Create the fork that misses the last upstream commit.
fn make_fork(root: &Path, upstream: &Path) -> PathBuf {
    let path = root.join("fork");
    git(
        root,
        &[
            "clone",
            "-q",
            upstream.to_str().unwrap(),
            path.to_str().unwrap(),
        ],
    );
    git(&path, &["config", "user.email", "fork@example.com"]);
    git(&path, &["config", "user.name", "Fork"]);

    // The fork stays on the first commit, so the second one is missing.
    let first = git_output(&path, &["rev-parse", "HEAD~1"]);
    git(&path, &["reset", "-q", "--hard", first.trim()]);

    path
}

/// A stand-in for the local decision model.
///
/// The real server is a `llama-server` that answers on `/completion` with the
/// log-probabilities of the candidate letters. The stand-in answers the same
/// way and always prefers the letter `C`, which is the medium level of both
/// scales.
struct FakeFoq {
    port: u16,
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl FakeFoq {
    /// Start the server on a free port of the loopback interface.
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("binding the stand-in");
        let port = listener
            .local_addr()
            .expect("the address of the stand-in")
            .port();
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();

        let handle = std::thread::spawn(move || {
            for stream in listener.incoming() {
                if flag.load(Ordering::Relaxed) {
                    break;
                }
                let Ok(stream) = stream else { continue };
                let _ = answer_decision(stream);
            }
        });

        Self {
            port,
            stop,
            handle: Some(handle),
        }
    }

    /// The base URL of the stand-in.
    fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }
}

impl Drop for FakeFoq {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // Unblock the accept loop with one last connection.
        let _ = TcpStream::connect(("127.0.0.1", self.port));
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// Answer one request of the stand-in with the log-probabilities of `C`.
fn answer_decision(mut stream: TcpStream) -> std::io::Result<()> {
    use std::io::{BufRead, BufReader, Read, Write};

    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request_line = String::new();
    reader.read_line(&mut request_line)?;

    // The headers, and then the body of the request.
    let mut length = 0usize;
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header)? == 0 {
            break;
        }
        if header.trim().is_empty() {
            // The end of the headers.
            break;
        }
        let Some((name, value)) = header.split_once(':') else {
            continue;
        };
        if name.trim().eq_ignore_ascii_case("content-length") {
            length = value.trim().parse().unwrap_or(0);
        }
    }
    if length > 0 {
        let mut body = vec![0_u8; length];
        reader.read_exact(&mut body)?;
    }

    // The letters of the two scales, with `C` far in the front.
    let body = serde_json::json!({
        "completion_probabilities": [{
            "top_logprobs": [
                {"token": "C", "logprob": 0.0},
                {"token": "B", "logprob": -12.0},
                {"token": "D", "logprob": -13.0},
                {"token": "A", "logprob": -15.0},
                {"token": "E", "logprob": -16.0},
            ],
        }],
    })
    .to_string();

    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes())?;
    stream.flush()
}

// ----------------------------------------------------------------------
// The fixture of the tests
// ----------------------------------------------------------------------

/// Everything one test needs.
struct Fixture {
    _temp: tempfile::TempDir,
    upstream: PathBuf,
    fork: PathBuf,
    foq: FakeFoq,
    config_path: PathBuf,
}

/// The configuration of the dashboard under test.
fn fixture_config(fixture: &Fixture) -> Config {
    Config {
        repos: vec![RepoConfig {
            name: "fork".to_string(),
            path: fixture.fork.clone(),
            // The dashboard fetches the upstream branch through this remote.
            upstream_url: fixture.upstream.to_string_lossy().to_string(),
            upstream_web: "https://example.com/upstream".to_string(),
            main_branch: "main".to_string(),
        }],
        foq_url: fixture.foq.url(),
        acp_command: MOCK_AGENT.to_string(),
        terminal_command: "true".to_string(),
        ..Config::default()
    }
}

/// Build the fixture: an upstream repository, a lagging fork and a stand-in
/// for the local decision model.
fn fixture() -> Fixture {
    let temp = tempfile::tempdir().expect("the temporary directory");
    let upstream = make_upstream(temp.path());
    let fork = make_fork(temp.path(), &upstream);
    let foq = FakeFoq::start();
    let config_path = temp.path().join("config.toml");

    Fixture {
        _temp: temp,
        upstream,
        fork,
        foq,
        config_path,
    }
}

/// Build the dashboard on top of the fixture.
///
/// The model discovery is skipped at startup: the dashboard asks the agent
/// for its models when the picker opens, and the tests wait for that.
fn dashboard(fixture: &Fixture) -> FsrsApp {
    let config = fixture_config(fixture);
    let store = Store::open_in_memory().expect("the database");
    let worker = Worker::new().expect("the background executor");
    FsrsApp::without_discovery(config, fixture.config_path.clone(), store, worker)
}

/// Pump the window until `ready` is satisfied or the deadline passes.
///
/// The background of the dashboard works on its own threads, so the test has
/// to give it time and to draw new frames in the meantime.
fn pump_until(
    harness: &mut Harness<'_, FsrsApp>,
    timeout: Duration,
    mut ready: impl FnMut(&FsrsApp) -> bool,
) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        harness.step();
        if ready(harness.state()) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Click a widget and wait for its effect.
///
/// The window may move a widget between the moment the test reads its place
/// and the moment the click is handled, so the click is repeated until the
/// expected effect shows up.
fn click_until(
    harness: &mut Harness<'_, FsrsApp>,
    label: &'static str,
    timeout: Duration,
    mut done: impl FnMut(&FsrsApp) -> bool,
) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        let Some(node) = harness.query_by_label(label) else {
            return false;
        };
        node.click();
        harness.step();
        if done(harness.state()) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        // Let the window settle, so that the next click uses fresh places.
        harness.step();
    }
}

/// Wait until the dashboard knows about the missing upstream commit.
fn wait_for_commits(harness: &mut Harness<'_, FsrsApp>) {
    let synced = pump_until(harness, Duration::from_secs(30), |app| {
        app.repos()
            .first()
            .is_some_and(|view| view.commits.len() == 1)
    });
    assert!(
        synced,
        "the upstream commits should arrive, the error was {:?}",
        harness.state().error()
    );
}

/// Build a window on top of a fresh fixture.
fn window(fixture: &Fixture) -> Harness<'_, FsrsApp> {
    Harness::builder()
        .with_size(egui::Vec2::new(1280.0, 800.0))
        .build_eframe(|_cc| dashboard(fixture))
}

// ----------------------------------------------------------------------
// The tests
// ----------------------------------------------------------------------

#[test]
fn the_window_shows_the_repository_and_its_missing_commits() {
    let fixture = fixture();
    let mut harness = window(&fixture);
    wait_for_commits(&mut harness);

    // The repository is listed with the number of the missing updates.
    assert!(
        harness.query_by_label("fork").is_some(),
        "the repository should be listed"
    );
    assert!(
        harness.query_by_label("1 обновление").is_some(),
        "the number of the updates should be shown"
    );

    // The commit is listed with its description.
    assert!(
        harness.query_by_label("Add the second file").is_some(),
        "the commit should be listed"
    );

    // The button that starts the implementation is there.
    assert!(
        harness.query_by_label("РЕАЛИЗОВАТЬ").is_some(),
        "the button should be shown"
    );
}

#[test]
fn clicking_a_commit_asks_for_the_model_of_the_terminal() {
    let fixture = fixture();
    let mut harness = window(&fixture);
    wait_for_commits(&mut harness);

    // Clicking the description of the commit opens the picker.
    harness.get_by_label("Add the second file").click();
    harness.step();

    let picker = harness
        .state()
        .picker()
        .expect("the picker should be open")
        .clone();
    assert_eq!(picker.intent, PickerIntent::Terminal);
    assert_eq!(picker.intent.title(), "Модель для терминала");

    // The dialog itself is on the screen.
    assert!(
        harness.query_by_label("Модель для терминала").is_some(),
        "the picker should be shown"
    );
}

#[test]
fn the_pipeline_ports_the_commit_and_merges_it_into_the_main_branch() {
    let fixture = fixture();
    let mut harness = window(&fixture);
    wait_for_commits(&mut harness);

    // The button starts the pipeline. The model is not known yet, so the
    // dashboard asks for it first.
    harness.get_by_label("РЕАЛИЗОВАТЬ").click();
    harness.step();

    let picker = harness
        .state()
        .picker()
        .expect("the picker should be open")
        .clone();
    assert_eq!(picker.intent, PickerIntent::Pipeline);
    assert_eq!(picker.intent.title(), "Модель для реализации");

    // The models of the agent arrive in the background.
    let loaded = pump_until(&mut harness, Duration::from_secs(30), |app| {
        !app.models().is_empty()
    });
    assert!(
        loaded,
        "the models of the agent should arrive, the error was {:?}",
        harness.state().error()
    );

    // Pick the model of the mock agent.
    let picked = click_until(
        &mut harness,
        "Mock model  ·  mock/model",
        Duration::from_secs(30),
        |app| {
            app.picker()
                .is_some_and(|picker| picker.selected.as_deref() == Some("mock/model"))
        },
    );
    assert!(picked, "the model should be picked");

    // And start the run.
    let started = click_until(
        &mut harness,
        "Запустить реализацию",
        Duration::from_secs(30),
        |app| app.picker().is_none(),
    );
    assert!(started, "the pipeline should start");

    // The three agents work in the background.
    let merged = pump_until(&mut harness, Duration::from_secs(120), |app| {
        app.run_state() == Some(RunState::Merged)
    });
    assert!(
        merged,
        "the pipeline should merge the branch, the state was {:?} and the error was {:?}",
        harness.state().run_state(),
        harness.state().error()
    );

    // The port landed on the main branch of the fork.
    let git = fsrs_core::git::GitRepo::open(&fixture.fork).expect("the fork");
    assert_eq!(git.current_branch().unwrap(), "main");
    assert!(fixture.fork.join("ported.txt").exists());
    assert!(git.status().unwrap().is_empty());

    // The journal of the window tells the story of the run.
    assert!(!harness.state().log().is_empty());
}

/// The models of the agent are offered in the picker of the window.
#[test]
fn the_picker_offers_the_models_of_the_agent() {
    let fixture = fixture();
    let mut harness = window(&fixture);
    wait_for_commits(&mut harness);

    harness.get_by_label("РЕАЛИЗОВАТЬ").click();
    harness.step();

    // The mock agent is asked for its models in the background.
    let loaded = pump_until(&mut harness, Duration::from_secs(30), |app| {
        !app.models().is_empty()
    });
    assert!(loaded, "the models of the agent should arrive");

    assert!(
        harness
            .query_by_label("Mock model  ·  mock/model")
            .is_some(),
        "the model should be offered"
    );
}

/// The local decision model rates the commits of the fork.
#[test]
fn the_commits_are_rated_by_the_local_model() {
    let fixture = fixture();
    let mut harness = window(&fixture);

    let rated = pump_until(&mut harness, Duration::from_secs(60), |app| {
        app.repos()
            .first()
            .is_some_and(|view| !view.assessments.is_empty())
    });
    assert!(
        rated,
        "the local model should rate the commit, the error was {:?}",
        harness.state().error()
    );

    // The medium level of both scales is shown in the table.
    let assessment = harness
        .state()
        .repos()
        .first()
        .and_then(|view| view.assessments.values().next())
        .expect("an assessment");
    assert_eq!(assessment.criticality.badge(), "M");
    assert_eq!(assessment.complexity.badge(), "M");
}

/// The prepared data of the screenshot fills the window and survives the
/// background worker.
#[test]
fn the_prepared_data_survives_the_background() {
    let fixture = fixture();
    let mut harness = window(&fixture);
    wait_for_commits(&mut harness);

    harness.state_mut().prepare_screenshot(&[
        fsrs_app::app::ScreenshotRow {
            summary: "feat: add the option".to_string(),
            author: "upstream".to_string(),
            criticality: fsrs_core::Criticality::High,
            complexity: fsrs_core::Complexity::Medium,
        },
        fsrs_app::app::ScreenshotRow {
            summary: "fix: correct the value".to_string(),
            author: "upstream".to_string(),
            criticality: fsrs_core::Criticality::Low,
            complexity: fsrs_core::Complexity::Easy,
        },
    ]);
    harness.step();

    let app = harness.state();
    let view = &app.repos()[0];
    assert_eq!(view.commits.len(), 2);
    assert_eq!(view.assessments.len(), 2);
    assert_eq!(view.unassessed(), 0);
    // The number of the updates is shown in the right form of the plural.
    assert!(
        harness.query_by_label("2 обновления").is_some(),
        "the number of the updates should be shown"
    );
    assert!(app.error().is_none(), "the window should show no error");

    // A message of the background worker must not empty the window.
    harness.step();
    assert_eq!(harness.state().repos()[0].commits.len(), 2);
}

/// The buttons of the window do not need a real agent to be usable.
#[test]
fn the_window_survives_a_missing_agent() {
    let fixture = fixture();
    let mut config = fixture_config(&fixture);
    // The agent cannot be started, so the model list stays empty.
    config.acp_command = "/nonexistent/fsrs-agent".to_string();

    let store = Store::open_in_memory().expect("the database");
    let worker = Worker::new().expect("the background executor");
    let mut harness = Harness::builder()
        .with_size(egui::Vec2::new(1280.0, 800.0))
        .build_eframe(|_cc| {
            FsrsApp::without_discovery(config, fixture.config_path.clone(), store, worker)
        });
    wait_for_commits(&mut harness);

    harness.get_by_label("РЕАЛИЗОВАТЬ").click();
    harness.step();

    // The picker opens and reports that the agent is not there.
    let failed = pump_until(&mut harness, Duration::from_secs(30), |app| {
        app.picker().is_some_and(|picker| picker.error.is_some())
    });
    assert!(
        failed,
        "the picker should report the broken agent, the error was {:?}",
        harness.state().error()
    );
}
