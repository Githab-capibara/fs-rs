//! The `fsrs` binary: the desktop dashboard.
//!
//! The dashboard reads its configuration from `~/.config/fs-rs/config.toml`,
//! keeps its state in `~/.local/share/fs-rs/fsrs.db` and shows the upstream
//! commits that are still missing from the tracked forks.

use std::path::PathBuf;

use anyhow::{Context, Result};
use fsrs_app::app::FsrsApp;
use fsrs_app::worker::Worker;
use fsrs_core::{Store, config};

/// The configuration file that is written when the dashboard starts for the
/// first time.
const DEFAULT_CONFIG: &str = r#"# fs-rs — the dashboard that ports upstream commits into forks.
#
# Every repository is a fork that follows an upstream project. The path is the
# local working copy, `upstream_url` is the remote of the original project and
# `upstream_web` is its address in the browser; the last one is used to build
# the link to every commit.

# [[repos]]
# name = "my-project"
# path = "/home/user/projects/my-project"
# upstream_url = "https://github.com/upstream/my-project.git"
# upstream_web = "https://github.com/upstream/my-project"
# main_branch = "main"

# The local decision model that rates the criticality and the complexity.
foq_url = "http://127.0.0.1:8089"
foq_model = "foq"
foq_profile = "lora"
min_confidence = 0.8
# A big diff needs a long time on a slow accelerator.
foq_timeout_secs = 120

# The agent that implements the commits (Agent Client Protocol) and the
# command that runs the OpenCode CLI in the manual terminal.
acp_command = "opencode acp"
terminal_command = "opencode mini"

# Whether the requests of the agents for a permission are approved.
auto_approve = true
"#;

/// The options of the command line.
#[derive(Debug, Default, PartialEq, Eq)]
struct Options {
    /// An alternative configuration file.
    config: Option<PathBuf>,
    /// An alternative directory for the database.
    data: Option<PathBuf>,
    /// Whether the help was requested.
    help: bool,
}

impl Options {
    /// Parse the command line.
    fn parse(arguments: impl IntoIterator<Item = String>) -> Result<Self> {
        let mut options = Options::default();
        let mut arguments = arguments.into_iter();
        while let Some(argument) = arguments.next() {
            match argument.as_str() {
                "--config" => {
                    let value = arguments
                        .next()
                        .context("--config needs the path of the configuration file")?;
                    options.config = Some(PathBuf::from(value));
                }
                "--data" => {
                    let value = arguments
                        .next()
                        .context("--data needs the path of the data directory")?;
                    options.data = Some(PathBuf::from(value));
                }
                "--help" | "-h" => options.help = true,
                other => anyhow::bail!("unknown argument {other}; try --help"),
            }
        }
        Ok(options)
    }

    /// The text of the help.
    fn help() -> &'static str {
        "fs-rs — the dashboard that ports upstream commits into forks\n\n\
         Usage: fsrs [OPTIONS]\n\n\
         Options:\n\
         \x20 --config <PATH>  use another configuration file\n\
         \x20 --data <PATH>    use another directory for the database\n\
         \x20 -h, --help       show this help\n"
    }
}

/// Write the default configuration file.
fn write_default_config(path: &std::path::Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(path, DEFAULT_CONFIG).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let options = Options::parse(std::env::args().skip(1))?;
    if options.help {
        println!("{}", Options::help());
        return Ok(());
    }

    let config_path = options.config.unwrap_or_else(config::default_path);
    let data_dir = options.data.unwrap_or_else(config::default_data_dir);

    if !config_path.exists() {
        write_default_config(&config_path)?;
        tracing::info!(path = %config_path.display(), "the configuration was created");
    }

    let settings = config::load(&config_path)?;
    let store = Store::open(&data_dir.join("fsrs.db"))?;
    let worker = Worker::new()?;

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("fs-rs")
            .with_inner_size([1600.0, 1000.0])
            .with_min_inner_size([900.0, 600.0]),
        ..Default::default()
    };

    eframe::run_native(
        "fs-rs",
        native_options,
        Box::new(move |_creation| Ok(Box::new(FsrsApp::new(settings, config_path, store, worker)))),
    )
    .context("the dashboard could not be started")?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(arguments: &[&str]) -> Result<Options> {
        Options::parse(arguments.iter().map(|argument| argument.to_string()))
    }

    #[test]
    fn the_options_are_parsed() {
        let options = parse(&["--config", "/tmp/a.toml", "--data", "/tmp/data"]).unwrap();
        assert_eq!(options.config, Some(PathBuf::from("/tmp/a.toml")));
        assert_eq!(options.data, Some(PathBuf::from("/tmp/data")));
        assert!(!options.help);
    }

    #[test]
    fn the_help_is_recognised() {
        assert!(parse(&["--help"]).unwrap().help);
        assert!(parse(&["-h"]).unwrap().help);
    }

    #[test]
    fn an_unknown_argument_is_rejected() {
        assert!(parse(&["--nope"]).is_err());
    }

    #[test]
    fn an_option_without_a_value_is_rejected() {
        assert!(parse(&["--config"]).is_err());
        assert!(parse(&["--data"]).is_err());
    }

    #[test]
    fn the_default_configuration_is_valid_toml() {
        let settings: fsrs_core::Config = toml::from_str(DEFAULT_CONFIG).expect("valid toml");
        assert!(settings.repos.is_empty());
        assert_eq!(settings.foq_url, "http://127.0.0.1:8089");
        assert_eq!(settings.acp_command, "opencode acp");
        assert_eq!(settings.terminal_command, "opencode mini");
    }
}
