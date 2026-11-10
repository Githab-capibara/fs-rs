//! Render a screenshot of the dashboard for the documentation.
//!
//! The image is produced from the real window: the same egui code, the same
//! layout, the same widgets. Only the data is prepared, so that the picture
//! shows a repository with several commits and their ratings.
//!
//! ```text
//! cargo run -p fsrs-app --example screenshot -- docs/images/hero.png
//! ```

use std::path::PathBuf;
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use fsrs_app::app::{FsrsApp, ScreenshotRow};
use fsrs_app::worker::Worker;
use fsrs_core::{Complexity, Config, Criticality, RepoConfig, Store};

/// Width of the rendered window.
const WIDTH: f32 = 1400.0;

/// Height of the rendered window.
const HEIGHT: f32 = 860.0;

/// The commits of the picture, with the ratings that the model produced.
const COMMITS: &[(&str, &str, Criticality, Complexity)] = &[
    (
        "fix: keep the file descriptor open across the rename",
        "tmccombs",
        Criticality::Medium,
        Complexity::Easy,
    ),
    (
        "feat: add --hyperlink to the completion output",
        "sharkdp",
        Criticality::High,
        Complexity::Medium,
    ),
    (
        "chore: bump the action versions to 4.2.2",
        "dependabot",
        Criticality::Low,
        Complexity::Easy,
    ),
    (
        "fix: handle the empty input in the parser",
        "BTF-Kabir-2020",
        Criticality::Medium,
        Complexity::Easy,
    ),
    (
        "docs: update the Guix package link",
        "sharkdp",
        Criticality::Trivial,
        Complexity::Easy,
    ),
];

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let target = PathBuf::from(
        std::env::args()
            .nth(1)
            .unwrap_or_else(|| "docs/images/hero.png".to_string()),
    );

    let config = Config {
        repos: vec![RepoConfig {
            name: "fd".to_string(),
            path: PathBuf::from("/home/user/projects/fd"),
            upstream_url: "https://github.com/sharkdp/fd.git".to_string(),
            upstream_web: "https://github.com/sharkdp/fd".to_string(),
            main_branch: "master".to_string(),
        }],
        ..Config::default()
    };

    let store = Store::open_in_memory()?;
    let worker = Worker::new()?;
    let app = FsrsApp::without_discovery(
        config,
        PathBuf::from("/home/user/.config/fs-rs/config.toml"),
        store,
        worker,
    );

    let mut harness = Harness::builder()
        .with_size(egui::Vec2::new(WIDTH, HEIGHT))
        .build_eframe(|_cc| app);

    // Fill the window with the prepared data.
    let rows: Vec<ScreenshotRow> = COMMITS
        .iter()
        .map(|(summary, author, criticality, complexity)| ScreenshotRow {
            summary: (*summary).to_string(),
            author: (*author).to_string(),
            criticality: *criticality,
            complexity: *complexity,
        })
        .collect();
    harness.state_mut().prepare_screenshot(&rows);

    // Let the window settle: the background worker sends its first messages.
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        harness.step();
        std::thread::sleep(Duration::from_millis(20));
    }

    let image = harness.render()?;
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    image.save(&target)?;

    println!(
        "written {} ({}x{})",
        target.display(),
        image.width(),
        image.height()
    );
    Ok(())
}
