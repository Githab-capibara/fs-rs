//! The terminal panel of the dashboard.
//!
//! The panel owns a real pseudo terminal: the OpenCode CLI runs inside it and
//! the bytes are rendered by the Ghostty VT engine that `egui_tty` embeds. It
//! is a terminal, not a log view, so full-screen programs and interactive
//! prompts behave as expected.

use std::io::{Read, Write};
use std::path::Path;
use std::sync::{Arc, Mutex, mpsc};

use egui_tty::{Terminal, TerminalStyle, Tty, TtyStream};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};

/// Number of rows of the pseudo terminal.
const ROWS: u16 = 40;

/// Number of columns of the pseudo terminal.
const COLS: u16 = 120;

/// A pseudo terminal, as an [`Tty`].
struct Pty {
    writer: Mutex<Box<dyn Write + Send>>,
    master: Mutex<Box<dyn MasterPty + Send>>,
}

impl Tty for Pty {
    fn write(&self, bytes: &[u8]) -> egui_tty::Result<()> {
        let mut writer = self.writer.lock().map_err(egui_tty::Error::msg)?;
        writer.write_all(bytes).map_err(egui_tty::Error::msg)?;
        writer.flush().map_err(egui_tty::Error::msg)
    }

    fn resize(&self, cols: u16, rows: u16) -> egui_tty::Result<()> {
        self.master
            .lock()
            .map_err(egui_tty::Error::msg)?
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(egui_tty::Error::msg)
    }
}

/// A terminal running one command.
pub struct TerminalPanel {
    terminal: Terminal,
    child: Option<Box<dyn Child + Send + Sync>>,
    command: String,
}

impl TerminalPanel {
    /// Start `command` with `args` in `cwd` and render its output.
    pub fn spawn(
        command: &str,
        args: &[String],
        cwd: &Path,
        label: &str,
    ) -> Result<Self, TerminalError> {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: ROWS,
                cols: COLS,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(TerminalError::Pty)?;

        let mut builder = CommandBuilder::new(command);
        builder.args(args);
        builder.cwd(cwd);
        builder.env("TERM", "xterm-256color");

        let child = pair
            .slave
            .spawn_command(builder)
            .map_err(TerminalError::Spawn)?;
        // The slave handle has to be dropped, or the reader never sees the
        // command exit.
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader().map_err(TerminalError::Pty)?;
        let writer = pair.master.take_writer().map_err(TerminalError::Pty)?;
        let (sender, output) = mpsc::channel();

        std::thread::spawn(move || {
            let mut buffer = vec![0_u8; 8 * 1024];
            while let Ok(count) = reader.read(&mut buffer) {
                if count == 0 || sender.send(buffer[..count].to_vec()).is_err() {
                    break;
                }
            }
        });

        let stream = TtyStream {
            output,
            tty: Arc::new(Pty {
                writer: Mutex::new(writer),
                master: Mutex::new(pair.master),
            }),
        };
        let mut terminal = Terminal::new(stream).map_err(TerminalError::Render)?;
        terminal = terminal.with_label(label);
        terminal.request_focus();

        Ok(Self {
            terminal,
            child: Some(child),
            command: format!("{command} {}", args.join(" ")),
        })
    }

    /// The command the terminal runs.
    pub fn command(&self) -> &str {
        &self.command
    }

    /// The process identifier of the running command.
    pub fn process_id(&self) -> Option<u32> {
        self.child.as_ref().and_then(|child| child.process_id())
    }

    /// Whether the command already exited.
    pub fn has_exited(&self) -> bool {
        self.terminal.has_exited()
    }

    /// The title the running program published.
    pub fn title(&self) -> Option<String> {
        self.terminal.title()
    }

    /// Draw the terminal.
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        let style = TerminalStyle::from_visuals(ui.visuals());
        self.terminal.ui(ui, &style);
    }
}

impl std::fmt::Debug for TerminalPanel {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TerminalPanel")
            .field("command", &self.command)
            .field("process_id", &self.process_id())
            .finish()
    }
}

impl Drop for TerminalPanel {
    /// Terminate the child process when the panel is dropped.
    ///
    /// Without this the child becomes a zombie: its entry remains in the
    /// process table and the reader thread on the master side never sees the
    /// end-of-file that triggers its exit.
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Errors raised while starting a terminal.
#[derive(Debug, thiserror::Error)]
pub enum TerminalError {
    /// The pseudo terminal could not be created.
    #[error("the pseudo terminal could not be created: {0}")]
    Pty(anyhow::Error),
    /// The command could not be started.
    #[error("the command could not be started: {0}")]
    Spawn(anyhow::Error),
    /// The terminal could not be rendered.
    #[error("the terminal could not be rendered: {0}")]
    Render(egui_tty::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_runs_in_the_terminal() {
        let dir = std::env::temp_dir();
        let panel = TerminalPanel::spawn(
            "sh",
            &["-c".to_string(), "echo fsrs-terminal".to_string()],
            &dir,
            "test",
        )
        .expect("the terminal should start");
        assert!(panel.command().contains("fsrs-terminal"));
    }

    #[test]
    fn an_unknown_command_is_reported() {
        let dir = std::env::temp_dir();
        let error = TerminalPanel::spawn("fsrs-command-that-does-not-exist", &[], &dir, "test")
            .expect_err("the command should not start");
        assert!(matches!(error, TerminalError::Spawn(_)));
    }
}
