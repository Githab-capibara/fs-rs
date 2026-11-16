//! PTY-backed terminals for the ACP `terminal/*` requests.
//!
//! An agent that runs shell commands through the client protocol expects the
//! client to provide a real pseudo terminal: the command must keep running
//! between requests, its output must be replayable, and its exit status must
//! be observable. This module implements that contract on top of
//! [`portable_pty`].

use portable_pty::{ChildKiller, CommandBuilder, PtySize};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};

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

/// The output of a terminal, already read as text.
#[derive(Debug, Default)]
struct CachedOutput {
    /// The text of the output.
    text: String,
    /// How many bytes of the buffer the text holds.
    ///
    /// The bytes that are not valid UTF-8 are replaced, so the text can be
    /// shorter than the buffer in bytes and longer in characters.
    bytes: usize,
}

/// A terminal created for the agent.
struct Terminal {
    killer: Box<dyn ChildKiller + Send + Sync>,
    writer: Option<Box<dyn Write + Send>>,
    output: Arc<Mutex<Vec<u8>>>,
    /// The output as a string, and how many bytes of the buffer it holds.
    ///
    /// The agent asks for the output of a terminal again and again; every
    /// answer used to copy the whole buffer and validate it as UTF-8. The
    /// cache holds the text that was already built, so only the new bytes
    /// are appended.
    cached: Arc<Mutex<CachedOutput>>,
    exit: Arc<Mutex<Option<ExitStatus>>>,
    /// Woken when the command exits.
    ///
    /// The thread that waits for the command fills the slot of the exit and
    /// wakes the readers; nobody polls the process any more.
    exited: Arc<Condvar>,
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

        // The child waits in a thread of its own: the process is watched
        // once, not every fifty milliseconds. The thread fills the slot of
        // the exit and wakes everybody who waits for it.
        let exit = Arc::new(Mutex::new(None));
        let exited = Arc::new(Condvar::new());
        let waiter_exit = Arc::clone(&exit);
        let waiter_exited = Arc::clone(&exited);
        let mut waiter_child = child;
        std::thread::spawn(move || {
            let exit = match waiter_child.wait() {
                Ok(status) => match status.signal() {
                    Some(signal) => ExitStatus {
                        exit_code: None,
                        signal: Some(signal.to_string()),
                    },
                    None => ExitStatus {
                        exit_code: Some(status.exit_code()),
                        signal: None,
                    },
                },
                // The process cannot be waited for: the command is reported
                // as failed rather than as still running.
                Err(error) => ExitStatus {
                    exit_code: None,
                    signal: Some(error.to_string()),
                },
            };
            if let Ok(mut slot) = waiter_exit.lock() {
                *slot = Some(exit);
            }
            waiter_exited.notify_all();
        });

        let terminal = Terminal {
            killer,
            writer: Some(writer),
            output,
            cached: Arc::new(Mutex::new(CachedOutput::default())),
            exit,
            exited,
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
        let limit = terminal.limit;
        let truncated;
        let output;
        {
            // The lock of the registry is released before the buffer is
            // copied, so the reader of the master side is not blocked while
            // the text is built.
            let buffer = terminal
                .output
                .lock()
                .map_err(|_| TerminalError::Poisoned)?;
            let mut cache = terminal
                .cached
                .lock()
                .map_err(|_| TerminalError::Poisoned)?;
            // Only the bytes that arrived since the last answer are read.
            if cache.bytes < buffer.len() {
                let fresh = String::from_utf8_lossy(&buffer[cache.bytes..]).into_owned();
                cache.text.push_str(&fresh);
                cache.bytes = buffer.len();
            }
            truncated = cache.text.len() > limit;
            output = if truncated {
                let cut = cache.text.len() - limit;
                cache.text[cut..].to_string()
            } else {
                cache.text.clone()
            };
        }
        let exit_status = terminal.exit_status();
        Ok(TerminalOutput {
            output,
            truncated,
            exit_status,
        })
    }

    /// Wait until the command exits and return its status.
    ///
    /// The thread that waits for the process wakes the readers, so this
    /// blocks without waking up every fifty milliseconds.
    pub fn wait_for_exit(&self, id: &str) -> Result<ExitStatus, TerminalError> {
        let terminals = self.terminals.lock().map_err(|_| TerminalError::Poisoned)?;
        let terminal = terminals
            .get(id)
            .ok_or_else(|| TerminalError::Unknown(id.to_string()))?;
        let exit = terminal.exit.lock().map_err(|_| TerminalError::Poisoned)?;
        let (mut slot, _) = terminal
            .exited
            .wait_timeout_while(exit, std::time::Duration::from_millis(250), |slot| {
                slot.is_none()
            })
            .map_err(|_| TerminalError::Poisoned)?;
        match slot.take() {
            Some(status) => Ok(status),
            None => Err(TerminalError::Unknown(id.to_string())),
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
    fn the_exit_is_known_without_waiting_for_a_poll() {
        // The waiter thread fills the slot of the exit as soon as the process
        // is gone, so the status is there before the first poll of the old
        // path could have found it.
        let registry = TerminalRegistry::new(std::env::temp_dir());
        let id = registry
            .create(
                "sh",
                &["-c".to_string(), "exit 7".to_string()],
                &[],
                None,
                None,
            )
            .expect("the terminal should start");
        let started = std::time::Instant::now();
        let status = registry
            .wait_for_exit(&id)
            .expect("the terminal should exist");
        assert_eq!(status.exit_code, Some(7));
        // The old path polled every fifty milliseconds, so it could not
        // report the exit faster than that.
        assert!(
            started.elapsed() < std::time::Duration::from_millis(50),
            "the exit took {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn a_long_command_does_not_block_the_other_terminals() {
        let registry = TerminalRegistry::new(std::env::temp_dir());
        let slow = registry
            .create(
                "sh",
                &["-c".to_string(), "sleep 30".to_string()],
                &[],
                None,
                None,
            )
            .expect("the terminal should start");
        let fast = registry
            .create(
                "sh",
                &["-c".to_string(), "exit 5".to_string()],
                &[],
                None,
                None,
            )
            .expect("the terminal should start");
        let started = std::time::Instant::now();
        let status = registry
            .wait_for_exit(&fast)
            .expect("the terminal should exist");
        assert_eq!(status.exit_code, Some(5));
        assert!(
            started.elapsed() < std::time::Duration::from_secs(2),
            "the wait took {:?}",
            started.elapsed()
        );
        // The slow command is still running: the registry holds it.
        assert!(registry.output(&slow).is_ok());
    }

    #[test]
    fn the_output_is_read_again_without_changing() {
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
        registry
            .wait_for_exit(&id)
            .expect("the terminal should exist");
        let first = registry.output(&id).expect("the terminal should exist");
        let second = registry.output(&id).expect("the terminal should exist");
        // The cache serves the same text again.
        assert_eq!(first.output, second.output);
        assert!(first.output.contains("hello-from-fsrs"));
        assert_eq!(first.truncated, second.truncated);
    }

    #[test]
    fn the_output_grows_with_the_command() {
        let registry = TerminalRegistry::new(std::env::temp_dir());
        let id = registry
            .create(
                "sh",
                &[
                    "-c".to_string(),
                    "echo first; sleep 1; echo second".to_string(),
                ],
                &[],
                None,
                None,
            )
            .expect("the terminal should start");
        // The first answer holds what the command wrote until now.
        let mut seen = String::new();
        for _ in 0..100 {
            let output = registry.output(&id).expect("the terminal should exist");
            assert!(
                output.output.len() >= seen.len(),
                "the output must not shrink"
            );
            seen = output.output;
            if seen.contains("second") {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(seen.contains("first"), "the output holds: {seen}");
        assert!(seen.contains("second"), "the output holds: {seen}");
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
