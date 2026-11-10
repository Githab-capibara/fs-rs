//! PTY-backed terminals for the ACP `terminal/*` requests.
//!
//! An agent that runs shell commands through the client protocol expects the
//! client to provide a real pseudo terminal: the command must keep running
//! between requests, its output must be replayable, and its exit status must
//! be observable. This module implements that contract on top of
//! [`portable_pty`].

use portable_pty::{Child, ChildKiller, CommandBuilder, PtySize};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// Number of rows of the created pseudo terminals.
pub const DEFAULT_ROWS: u16 = 40;

/// Number of columns of the created pseudo terminals.
pub const DEFAULT_COLS: u16 = 120;

/// Default number of output bytes kept per terminal.
pub const DEFAULT_OUTPUT_LIMIT: usize = 1024 * 1024;

/// Exit status of a terminal command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExitStatus {
    /// Exit code of the process, if it exited normally.
    pub exit_code: Option<u32>,
    /// Signal that terminated the process, if any.
    pub signal: Option<String>,
}

/// A terminal created for the agent.
struct Terminal {
    child: Box<dyn Child + Send + Sync>,
    killer: Box<dyn ChildKiller + Send + Sync>,
    writer: Option<Box<dyn Write + Send>>,
    output: Arc<Mutex<Vec<u8>>>,
    exit: Arc<Mutex<Option<ExitStatus>>>,
    limit: usize,
}

/// Registry of the terminals owned by one ACP connection.
pub struct TerminalRegistry {
    terminals: Mutex<HashMap<String, Terminal>>,
    next_id: AtomicU64,
    default_cwd: PathBuf,
}

impl TerminalRegistry {
    /// Create a registry whose commands run in `default_cwd` by default.
    pub fn new(default_cwd: impl Into<PathBuf>) -> Self {
        Self {
            terminals: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            default_cwd: default_cwd.into(),
        }
    }

    /// Number of terminals that are still alive.
    pub fn len(&self) -> usize {
        self.terminals.lock().map(|map| map.len()).unwrap_or(0)
    }

    /// Whether the registry holds no terminal.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Start a command in a new pseudo terminal.
    pub fn create(
        &self,
        command: &str,
        args: &[String],
        env: &[(String, String)],
        cwd: Option<&Path>,
        output_limit: Option<usize>,
    ) -> Result<String, TerminalError> {
        let pty_system = portable_pty::native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows: DEFAULT_ROWS,
                cols: DEFAULT_COLS,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(TerminalError::Spawn)?;

        let mut builder = CommandBuilder::new(command);
        builder.args(args);
        builder.cwd(cwd.unwrap_or(&self.default_cwd));
        for (key, value) in env {
            builder.env(key, value);
        }

        let child = pair
            .slave
            .spawn_command(builder)
            .map_err(TerminalError::Spawn)?;
        let killer = child.clone_killer();
        let writer = pair.master.take_writer().map_err(TerminalError::Spawn)?;
        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(TerminalError::Spawn)?;

        let output = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&output);
        std::thread::spawn(move || {
            let mut buffer = [0_u8; 8192];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(count) => {
                        if let Ok(mut sink) = sink.lock() {
                            sink.extend_from_slice(&buffer[..count]);
                        }
                    }
                }
            }
        });

        let id = format!("term-{}", self.next_id.fetch_add(1, Ordering::Relaxed));
        let terminal = Terminal {
            child,
            killer,
            writer: Some(writer),
            output,
            exit: Arc::new(Mutex::new(None)),
            limit: output_limit.unwrap_or(DEFAULT_OUTPUT_LIMIT),
        };
        let mut terminals = self.terminals.lock().map_err(|_| TerminalError::Poisoned)?;
        terminals.insert(id.clone(), terminal);
        Ok(id)
    }

    /// Read the output produced so far.
    ///
    /// Returns the output, whether it had to be truncated, and the exit status
    /// when the command already finished.
    pub fn output(&self, id: &str) -> Result<TerminalOutput, TerminalError> {
        let mut terminals = self.terminals.lock().map_err(|_| TerminalError::Poisoned)?;
        let terminal = terminals
            .get_mut(id)
            .ok_or_else(|| TerminalError::Unknown(id.to_string()))?;
        terminal.refresh_exit_status();
        let buffer = terminal
            .output
            .lock()
            .map_err(|_| TerminalError::Poisoned)?;
        let limit = terminal.limit;
        let truncated = buffer.len() > limit;
        let slice = if truncated {
            &buffer[buffer.len() - limit..]
        } else {
            &buffer[..]
        };
        let output = String::from_utf8_lossy(slice).to_string();
        let exit_status = terminal.exit_status();
        Ok(TerminalOutput {
            output,
            truncated,
            exit_status,
        })
    }

    /// Wait until the command exits and return its status.
    pub fn wait_for_exit(&self, id: &str) -> Result<ExitStatus, TerminalError> {
        loop {
            {
                let mut terminals = self.terminals.lock().map_err(|_| TerminalError::Poisoned)?;
                let terminal = terminals
                    .get_mut(id)
                    .ok_or_else(|| TerminalError::Unknown(id.to_string()))?;
                terminal.refresh_exit_status();
                if let Some(status) = terminal.exit_status() {
                    return Ok(status);
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }

    /// Kill the command and forget the terminal.
    pub fn kill(&self, id: &str) -> Result<(), TerminalError> {
        let mut terminals = self.terminals.lock().map_err(|_| TerminalError::Poisoned)?;
        let terminal = terminals
            .get_mut(id)
            .ok_or_else(|| TerminalError::Unknown(id.to_string()))?;
        let _ = terminal.killer.kill();
        Ok(())
    }

    /// Release the terminal: close its input and forget it.
    pub fn release(&self, id: &str) -> Result<(), TerminalError> {
        let mut terminals = self.terminals.lock().map_err(|_| TerminalError::Poisoned)?;
        if let Some(mut terminal) = terminals.remove(id) {
            terminal.writer = None;
        }
        Ok(())
    }
}

impl Terminal {
    /// Poll the child process and remember its exit status.
    fn refresh_exit_status(&mut self) {
        if self.exit_status().is_some() {
            return;
        }
        if let Ok(Some(status)) = self.child.try_wait() {
            let exit = match status.signal() {
                Some(signal) => ExitStatus {
                    exit_code: None,
                    signal: Some(signal.to_string()),
                },
                None => ExitStatus {
                    exit_code: Some(status.exit_code()),
                    signal: None,
                },
            };
            if let Ok(mut slot) = self.exit.lock() {
                *slot = Some(exit);
            }
        }
    }

    /// The exit status of the command, when it already exited.
    fn exit_status(&self) -> Option<ExitStatus> {
        self.exit.lock().ok().and_then(|slot| slot.clone())
    }
}

/// Output of a terminal.
#[derive(Debug, Clone)]
pub struct TerminalOutput {
    /// Everything the command printed.
    pub output: String,
    /// Whether the beginning of the output had to be dropped.
    pub truncated: bool,
    /// The exit status, when the command already finished.
    pub exit_status: Option<ExitStatus>,
}

/// Errors raised by the terminal registry.
#[derive(Debug, thiserror::Error)]
pub enum TerminalError {
    /// The pseudo terminal could not be created.
    #[error("failed to create the terminal: {0}")]
    Spawn(anyhow::Error),
    /// The terminal does not exist (any more).
    #[error("unknown terminal {0}")]
    Unknown(String),
    /// The registry was poisoned by a panic in another thread.
    #[error("the terminal registry is poisoned")]
    Poisoned,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_runs_and_produces_output() {
        let registry = TerminalRegistry::new(std::env::temp_dir());
        let id = registry
            .create(
                "sh",
                &["-c".to_string(), "echo hello-from-fsrs".to_string()],
                &[],
                None,
                None,
            )
            .expect("the terminal should start");
        let status = registry
            .wait_for_exit(&id)
            .expect("the terminal should exist");
        assert_eq!(status.exit_code, Some(0));

        let output = registry.output(&id).expect("the terminal should exist");
        assert!(output.output.contains("hello-from-fsrs"));
    }

    #[test]
    fn the_exit_code_is_reported() {
        let registry = TerminalRegistry::new(std::env::temp_dir());
        let id = registry
            .create(
                "sh",
                &["-c".to_string(), "exit 3".to_string()],
                &[],
                None,
                None,
            )
            .expect("the terminal should start");
        let status = registry
            .wait_for_exit(&id)
            .expect("the terminal should exist");
        assert_eq!(status.exit_code, Some(3));
    }

    #[test]
    fn a_released_terminal_disappears() {
        let registry = TerminalRegistry::new(std::env::temp_dir());
        let id = registry
            .create(
                "sh",
                &["-c".to_string(), "sleep 30".to_string()],
                &[],
                None,
                None,
            )
            .expect("the terminal should start");
        assert_eq!(registry.len(), 1);
        registry.release(&id).expect("the terminal should exist");
        assert!(registry.is_empty());
        assert!(registry.output(&id).is_err());
    }

    #[test]
    fn an_unknown_terminal_is_reported() {
        let registry = TerminalRegistry::new(std::env::temp_dir());
        assert!(matches!(
            registry.output("nope"),
            Err(TerminalError::Unknown(_))
        ));
    }

    #[test]
    fn the_output_is_truncated_from_the_beginning() {
        let registry = TerminalRegistry::new(std::env::temp_dir());
        let id = registry
            .create(
                "sh",
                &["-c".to_string(), "printf '0123456789'".to_string()],
                &[],
                None,
                Some(4),
            )
            .expect("the terminal should start");
        registry
            .wait_for_exit(&id)
            .expect("the terminal should exist");
        let output = registry.output(&id).expect("the terminal should exist");
        assert!(output.truncated);
        assert!(output.output.ends_with("6789"));
        assert!(!output.output.contains('0'));
    }
}
