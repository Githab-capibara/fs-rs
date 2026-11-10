//! ACP client: connection, sessions, model selection, prompts and handlers.
//!
//! The dashboard never talks to the agent directly. Everything goes through the
//! [Agent Client Protocol](https://agentclientprotocol.com): the client
//! starts the agent process, initialises the protocol, opens one session per
//! agent role, selects the model the user picked and sends the prompts.
//!
//! The client also answers the requests the agent sends back:
//!
//! * `session/request_permission` is approved automatically, because the user
//!   explicitly asked for an autonomous pipeline,
//! * `fs/read_text_file` and `fs/write_text_file` are served, but only inside
//!   the directories the user opened,
//! * `terminal/*` is served by a real pseudo terminal.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::{Arc, Mutex};

use agent_client_protocol::v1::ConnectionTo;
use agent_client_protocol::v1::schema::{
    ContentBlock, CreateTerminalRequest, CreateTerminalResponse, InitializeRequest,
    InitializeResponse, KillTerminalRequest, KillTerminalResponse, NewSessionRequest,
    NewSessionResponse, PermissionOptionKind, PromptRequest, PromptResponse, ReadTextFileRequest,
    ReadTextFileResponse, ReleaseTerminalRequest, ReleaseTerminalResponse,
    RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
    SelectedPermissionOutcome, SessionConfigOptionValue, SessionId, SessionNotification,
    SessionUpdate, SetSessionConfigOptionRequest, StopReason, TerminalExitStatus,
    TerminalOutputRequest, TerminalOutputResponse, TextContent, WaitForTerminalExitRequest,
    WaitForTerminalExitResponse, WriteTextFileRequest, WriteTextFileResponse,
};
use agent_client_protocol::{AcpAgent, Agent, Client};
use tokio::sync::mpsc::UnboundedSender;

use crate::events::{AgentRole, RunEvent};
use crate::models::{ModelChoice, models_from_options};
use crate::terminal::TerminalRegistry;

/// Default command line that starts the agent.
pub const DEFAULT_AGENT_COMMAND: &str = "opencode acp";

/// Configuration of the connection to the agent.
#[derive(Debug, Clone)]
pub struct AgentConfig {
    /// Command line that starts the agent, for example `opencode acp`.
    pub command: String,
    /// Directories the agent is allowed to read and write.
    pub roots: Vec<PathBuf>,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            command: DEFAULT_AGENT_COMMAND.to_string(),
            roots: Vec::new(),
        }
    }
}

impl AgentConfig {
    /// Build a configuration for one command line and one working directory.
    pub fn new(command: impl Into<String>, root: impl Into<PathBuf>) -> Self {
        Self {
            command: command.into(),
            roots: vec![root.into()],
        }
    }

    /// Allow one more directory.
    pub fn with_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.roots.push(root.into());
        self
    }
}

/// A session opened with the agent.
#[derive(Debug, Clone)]
pub struct AgentSession {
    /// Identifier of the session.
    pub session_id: SessionId,
    /// The models the agent offers for this session.
    pub models: Vec<ModelChoice>,
}

/// Outcome of one prompt turn.
#[derive(Debug, Clone)]
pub struct PromptOutcome {
    /// Why the turn ended.
    pub stop_reason: StopReason,
    /// Everything the agent said during the turn.
    pub text: String,
}

/// State shared between the connection and the protocol handlers.
#[derive(Default)]
struct SharedState {
    roles: Mutex<HashMap<String, AgentRole>>,
    texts: Mutex<HashMap<String, String>>,
}

impl SharedState {
    /// Remember which agent role owns a session.
    fn set_role(&self, session_id: &SessionId, role: AgentRole) {
        if let Ok(mut roles) = self.roles.lock() {
            roles.insert(session_id.to_string(), role);
        }
    }

    /// The role of a session, when it is known.
    fn role(&self, session_id: &SessionId) -> Option<AgentRole> {
        self.roles
            .lock()
            .ok()
            .and_then(|roles| roles.get(&session_id.to_string()).copied())
    }

    /// Forget everything the agent said in a session.
    fn clear_text(&self, session_id: &SessionId) {
        if let Ok(mut texts) = self.texts.lock() {
            texts.insert(session_id.to_string(), String::new());
        }
    }

    /// Append text the agent said in a session.
    fn append_text(&self, session_id: &SessionId, text: &str) {
        if let Ok(mut texts) = self.texts.lock() {
            texts
                .entry(session_id.to_string())
                .or_default()
                .push_str(text);
        }
    }

    /// Everything the agent said in a session.
    fn text(&self, session_id: &SessionId) -> String {
        self.texts
            .lock()
            .ok()
            .and_then(|texts| texts.get(&session_id.to_string()).cloned())
            .unwrap_or_default()
    }
}

/// A live connection to an ACP agent.
pub struct AgentConnection {
    connection: ConnectionTo<Agent>,
    state: Arc<SharedState>,
    events: Option<UnboundedSender<RunEvent>>,
}

impl AgentConnection {
    /// Start the agent and run `body` against it.
    ///
    /// The connection is closed as soon as `body` returns.
    ///
    /// The error of `body` is returned unchanged: the protocol only knows
    /// generic errors, so the original one is kept aside and preferred over
    /// the error of the connection. Without that, a caller could not tell a
    /// rejected implementation from a broken protocol any more.
    pub async fn run<F, Fut, R, E>(
        config: &AgentConfig,
        events: Option<UnboundedSender<RunEvent>>,
        body: F,
    ) -> Result<R, E>
    where
        F: FnOnce(AgentConnection) -> Fut,
        Fut: std::future::Future<Output = Result<R, E>>,
        E: From<AcpError> + Send + 'static + std::fmt::Display,
        agent_client_protocol::Error: From<E>,
    {
        let agent = AcpAgent::from_str(&config.command)
            .map_err(|error| AcpError::Spawn(error.to_string()))?;

        let state = Arc::new(SharedState::default());
        let roots = Arc::new(config.roots.clone());
        let terminals = Arc::new(TerminalRegistry::new(
            roots.first().cloned().unwrap_or_else(|| PathBuf::from(".")),
        ));

        // The error of the body, kept for the caller.
        let failure: Arc<Mutex<Option<E>>> = Arc::new(Mutex::new(None));
        let body_failure = Arc::clone(&failure);

        let notification_state = Arc::clone(&state);
        let notification_events = events.clone();

        let request_state = Arc::clone(&state);
        let request_events = events.clone();

        let read_roots = Arc::clone(&roots);

        let write_roots = Arc::clone(&roots);

        let create_terminals = Arc::clone(&terminals);
        let output_terminals = Arc::clone(&terminals);
        let wait_terminals = Arc::clone(&terminals);
        let kill_terminals = Arc::clone(&terminals);
        let release_terminals = Arc::clone(&terminals);

        Client
            .v1()
            .on_receive_notification(
                async |notification: SessionNotification, _cx| {
                    handle_notification(
                        &notification_state,
                        notification_events.as_ref(),
                        notification,
                    );
                    Ok(())
                },
                agent_client_protocol::on_receive_notification!(),
            )
            .on_receive_request(
                async |request: RequestPermissionRequest, responder, _cx| {
                    handle_permission(&request_state, request_events.as_ref(), request, responder)
                        .await
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_request(
                async |request: ReadTextFileRequest, responder, _cx| {
                    handle_read_text_file(&read_roots, request, responder)
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_request(
                async |request: WriteTextFileRequest, responder, _cx| {
                    handle_write_text_file(&write_roots, request, responder)
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_request(
                async |request: CreateTerminalRequest, responder, _cx| {
                    handle_create_terminal(&create_terminals, request, responder)
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_request(
                async |request: TerminalOutputRequest, responder, _cx| {
                    handle_terminal_output(&output_terminals, request, responder)
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_request(
                async |request: WaitForTerminalExitRequest, responder, _cx| {
                    handle_wait_terminal(&wait_terminals, request, responder)
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_request(
                async |request: KillTerminalRequest, responder, _cx| {
                    handle_kill_terminal(&kill_terminals, request, responder)
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_request(
                async |request: ReleaseTerminalRequest, responder, _cx| {
                    handle_release_terminal(&release_terminals, request, responder)
                },
                agent_client_protocol::on_receive_request!(),
            )
            .connect_with(agent, move |connection: ConnectionTo<Agent>| {
                let future = body(AgentConnection {
                    connection,
                    state: Arc::clone(&state),
                    events: events.clone(),
                });
                let failure = Arc::clone(&body_failure);
                async move {
                    match future.await {
                        Ok(value) => Ok(value),
                        Err(error) => {
                            // Keep the original error; the protocol only
                            // knows generic ones.
                            let message = error.to_string();
                            if let Ok(mut slot) = failure.lock() {
                                *slot = Some(error);
                            }
                            Err(agent_client_protocol::Error::new(
                                agent_client_protocol::ErrorCode::INTERNAL_ERROR,
                                message,
                            ))
                        }
                    }
                }
            })
            .await
            .map_err(|error| {
                // The body may have failed with a more precise error.
                if let Ok(mut slot) = failure.lock()
                    && let Some(original) = slot.take()
                {
                    return original;
                }
                E::from(AcpError::Protocol(error))
            })
    }

    /// Initialise the protocol.
    pub async fn initialize(&self) -> Result<InitializeResponse, AcpError> {
        let response = self
            .connection
            .send_request(InitializeRequest::new(
                agent_client_protocol::schema::ProtocolVersion::V1,
            ))
            .block_task()
            .await?;
        Ok(response)
    }

    /// Open a new session for one agent role.
    pub async fn new_session(&self, cwd: &Path, role: AgentRole) -> Result<AgentSession, AcpError> {
        let response: NewSessionResponse = self
            .connection
            .send_request(NewSessionRequest::new(cwd.to_path_buf()))
            .block_task()
            .await?;
        self.state.set_role(&response.session_id, role);
        self.state.clear_text(&response.session_id);
        let models = response
            .config_options
            .as_deref()
            .map(models_from_options)
            .unwrap_or_default();
        Ok(AgentSession {
            session_id: response.session_id,
            models,
        })
    }

    /// Choose the model the agent must use for a session.
    pub async fn select_model(&self, session_id: &SessionId, model: &str) -> Result<(), AcpError> {
        self.connection
            .send_request(SetSessionConfigOptionRequest::new(
                session_id.clone(),
                crate::models::MODEL_CONFIG_ID,
                SessionConfigOptionValue::value_id(model.to_string()),
            ))
            .block_task()
            .await?;
        Ok(())
    }

    /// Send a prompt and wait for the agent to finish the turn.
    pub async fn prompt(
        &self,
        session_id: &SessionId,
        role: AgentRole,
        text: &str,
    ) -> Result<PromptOutcome, AcpError> {
        self.state.clear_text(session_id);
        if let Some(events) = self.events() {
            let _ = events.send(RunEvent::PromptSent {
                role,
                prompt: text.to_string(),
            });
        }
        let response: PromptResponse = self
            .connection
            .send_request(PromptRequest::new(
                session_id.clone(),
                vec![ContentBlock::Text(TextContent::new(text.to_string()))],
            ))
            .block_task()
            .await?;
        let spoken = self.state.text(session_id);
        Ok(PromptOutcome {
            stop_reason: response.stop_reason,
            text: spoken,
        })
    }

    /// The event sink of this connection, when there is one.
    fn events(&self) -> Option<&UnboundedSender<RunEvent>> {
        self.events.as_ref()
    }
}

/// Forward a session update to the event stream and to the text buffer.
fn handle_notification(
    state: &SharedState,
    events: Option<&UnboundedSender<RunEvent>>,
    notification: SessionNotification,
) {
    let Some(role) = state.role(&notification.session_id) else {
        return;
    };
    match notification.update {
        SessionUpdate::AgentMessageChunk(chunk) => {
            if let ContentBlock::Text(text) = chunk.content {
                state.append_text(&notification.session_id, &text.text);
                if let Some(events) = events {
                    let _ = events.send(RunEvent::AgentMessage {
                        role,
                        text: text.text,
                    });
                }
            }
        }
        SessionUpdate::AgentThoughtChunk(chunk) => {
            if let ContentBlock::Text(text) = chunk.content
                && let Some(events) = events
            {
                let _ = events.send(RunEvent::AgentThought {
                    role,
                    text: text.text,
                });
            }
        }
        SessionUpdate::ToolCall(call) => {
            if let Some(events) = events {
                let _ = events.send(RunEvent::ToolCall {
                    role,
                    title: call.title,
                    status: format!("{:?}", call.status).to_lowercase(),
                });
            }
        }
        SessionUpdate::ToolCallUpdate(update) => {
            if let Some(events) = events {
                let title = update.fields.title.unwrap_or_else(|| {
                    update
                        .fields
                        .name
                        .unwrap_or_else(|| update.tool_call_id.to_string())
                });
                let status = update
                    .fields
                    .status
                    .map(|status| format!("{status:?}").to_lowercase())
                    .unwrap_or_else(|| "updated".to_string());
                let _ = events.send(RunEvent::ToolCall {
                    role,
                    title,
                    status,
                });
            }
        }
        _ => {}
    }
}

/// Approve a permission request automatically.
async fn handle_permission(
    state: &SharedState,
    events: Option<&UnboundedSender<RunEvent>>,
    request: RequestPermissionRequest,
    responder: agent_client_protocol::Responder<
        RequestPermissionResponse,
        agent_client_protocol::V1,
    >,
) -> Result<(), agent_client_protocol::Error> {
    let role = state.role(&request.session_id);
    let option = [
        PermissionOptionKind::AllowOnce,
        PermissionOptionKind::AllowAlways,
    ]
    .iter()
    .find_map(|kind| {
        request
            .options
            .iter()
            .find(|option| option.kind == *kind)
            .map(|option| option.option_id.clone())
    });
    let title = request
        .tool_call
        .fields
        .title
        .clone()
        .unwrap_or_else(|| request.tool_call.tool_call_id.to_string());
    match option {
        Some(option_id) => {
            if let (Some(events), Some(role)) = (events, role) {
                let _ = events.send(RunEvent::Permission {
                    role,
                    title,
                    outcome: "allowed".to_string(),
                });
            }
            responder.respond(RequestPermissionResponse::new(
                RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(option_id)),
            ))
        }
        None => {
            if let (Some(events), Some(role)) = (events, role) {
                let _ = events.send(RunEvent::Permission {
                    role,
                    title,
                    outcome: "cancelled".to_string(),
                });
            }
            responder.respond(RequestPermissionResponse::new(
                RequestPermissionOutcome::Cancelled,
            ))
        }
    }
}

/// Serve `fs/read_text_file` inside the allowed roots.
fn handle_read_text_file(
    roots: &[PathBuf],
    request: ReadTextFileRequest,
    responder: agent_client_protocol::Responder<ReadTextFileResponse, agent_client_protocol::V1>,
) -> Result<(), agent_client_protocol::Error> {
    if !is_allowed(roots, &request.path) {
        return responder.respond(ReadTextFileResponse::new(
            "the requested path is outside of the directories opened by the user".to_string(),
        ));
    }
    let content = std::fs::read_to_string(&request.path).unwrap_or_default();
    let content = match (request.line, request.limit) {
        (Some(line), limit) => {
            let lines: Vec<&str> = content.lines().collect();
            let start = line.saturating_sub(1) as usize;
            let slice = lines.get(start..).unwrap_or(&[]);
            let taken: Vec<&str> = match limit {
                Some(limit) => slice.iter().take(limit as usize).copied().collect(),
                None => slice.to_vec(),
            };
            taken.join("\n")
        }
        (None, Some(limit)) => content
            .lines()
            .take(limit as usize)
            .collect::<Vec<&str>>()
            .join("\n"),
        (None, None) => content,
    };
    responder.respond(ReadTextFileResponse::new(content))
}

/// Serve `fs/write_text_file` inside the allowed roots.
fn handle_write_text_file(
    roots: &[PathBuf],
    request: WriteTextFileRequest,
    responder: agent_client_protocol::Responder<WriteTextFileResponse, agent_client_protocol::V1>,
) -> Result<(), agent_client_protocol::Error> {
    if !is_allowed(roots, &request.path) {
        return responder.respond(WriteTextFileResponse::new());
    }
    if let Some(parent) = request.path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&request.path, &request.content);
    responder.respond(WriteTextFileResponse::new())
}

/// Serve `terminal/create`.
fn handle_create_terminal(
    terminals: &TerminalRegistry,
    request: CreateTerminalRequest,
    responder: agent_client_protocol::Responder<CreateTerminalResponse, agent_client_protocol::V1>,
) -> Result<(), agent_client_protocol::Error> {
    let env: Vec<(String, String)> = request
        .env
        .iter()
        .map(|variable| (variable.name.clone(), variable.value.clone()))
        .collect();
    match terminals.create(
        &request.command,
        &request.args,
        &env,
        request.cwd.as_deref(),
        request
            .output_byte_limit
            .and_then(|limit| usize::try_from(limit).ok()),
    ) {
        Ok(id) => responder.respond(CreateTerminalResponse::new(id)),
        Err(error) => responder.respond(CreateTerminalResponse::new(error.to_string())),
    }
}

/// Serve `terminal/output`.
fn handle_terminal_output(
    terminals: &TerminalRegistry,
    request: TerminalOutputRequest,
    responder: agent_client_protocol::Responder<TerminalOutputResponse, agent_client_protocol::V1>,
) -> Result<(), agent_client_protocol::Error> {
    match terminals.output(&request.terminal_id.to_string()) {
        Ok(output) => {
            let exit_status = output.exit_status.map(|status| {
                TerminalExitStatus::new()
                    .exit_code(status.exit_code)
                    .signal(status.signal)
            });
            responder.respond(
                TerminalOutputResponse::new(output.output, output.truncated)
                    .exit_status(exit_status),
            )
        }
        Err(error) => responder.respond(TerminalOutputResponse::new(error.to_string(), false)),
    }
}

/// Serve `terminal/wait_for_exit`.
fn handle_wait_terminal(
    terminals: &TerminalRegistry,
    request: WaitForTerminalExitRequest,
    responder: agent_client_protocol::Responder<
        WaitForTerminalExitResponse,
        agent_client_protocol::V1,
    >,
) -> Result<(), agent_client_protocol::Error> {
    match terminals.wait_for_exit(&request.terminal_id.to_string()) {
        Ok(status) => responder.respond(WaitForTerminalExitResponse::new(
            TerminalExitStatus::new()
                .exit_code(status.exit_code)
                .signal(status.signal),
        )),
        Err(_error) => {
            responder.respond(WaitForTerminalExitResponse::new(TerminalExitStatus::new()))
        }
    }
}

/// Serve `terminal/kill`.
fn handle_kill_terminal(
    terminals: &TerminalRegistry,
    request: KillTerminalRequest,
    responder: agent_client_protocol::Responder<KillTerminalResponse, agent_client_protocol::V1>,
) -> Result<(), agent_client_protocol::Error> {
    let _ = terminals.kill(&request.terminal_id.to_string());
    responder.respond(KillTerminalResponse::new())
}

/// Serve `terminal/release`.
fn handle_release_terminal(
    terminals: &TerminalRegistry,
    request: ReleaseTerminalRequest,
    responder: agent_client_protocol::Responder<ReleaseTerminalResponse, agent_client_protocol::V1>,
) -> Result<(), agent_client_protocol::Error> {
    let _ = terminals.release(&request.terminal_id.to_string());
    responder.respond(ReleaseTerminalResponse::new())
}

/// Whether a path is inside one of the allowed roots.
fn is_allowed(roots: &[PathBuf], path: &Path) -> bool {
    if roots.is_empty() {
        return false;
    }
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    roots.iter().any(|root| {
        let root = root.canonicalize().unwrap_or_else(|_| root.clone());
        canonical.starts_with(&root)
    })
}

/// Discover the models an agent offers.
///
/// The agent is started, the protocol is initialised and one session is opened
/// in the first allowed root. The model list of that session is returned and
/// the connection is closed again.
///
/// The identifiers are the ones the agent expects in
/// [`AgentConnection::select_model`], so a model picked in the dashboard can
/// be handed to the agent without any translation.
pub async fn discover_models(config: &AgentConfig) -> Result<Vec<ModelChoice>, AcpError> {
    let root = config
        .roots
        .first()
        .cloned()
        .unwrap_or_else(|| PathBuf::from("."));

    AgentConnection::run(config, None, async move |connection: AgentConnection| {
        connection.initialize().await?;
        let session = connection
            .new_session(&root, AgentRole::Implementer)
            .await?;
        Ok(session.models)
    })
    .await
}

/// Errors raised while talking to the agent.
#[derive(Debug, thiserror::Error)]
pub enum AcpError {
    /// The agent process could not be started.
    #[error("the agent could not be started: {0}")]
    Spawn(String),
    /// The protocol failed.
    #[error("the agent protocol failed: {0}")]
    Protocol(#[from] agent_client_protocol::Error),
    /// The agent rejected the requested model.
    #[error("the agent rejected the model {0}")]
    UnknownModel(String),
}

impl From<AcpError> for agent_client_protocol::Error {
    fn from(error: AcpError) -> Self {
        match error {
            AcpError::Protocol(inner) => inner,
            other => agent_client_protocol::Error::new(
                agent_client_protocol::ErrorCode::INTERNAL_ERROR,
                other.to_string(),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_inside_a_root_is_allowed() {
        let root = std::env::temp_dir();
        let allowed = root.join("fsrs-test-file.txt");
        assert!(is_allowed(std::slice::from_ref(&root), &allowed));
    }

    #[test]
    fn a_path_outside_the_roots_is_rejected() {
        let root = std::env::temp_dir().join("fsrs-allowed");
        let outside = std::env::temp_dir().join("fsrs-other-file.txt");
        assert!(!is_allowed(&[root], &outside));
    }

    #[test]
    fn without_roots_nothing_is_allowed() {
        let path = std::env::temp_dir().join("fsrs-test-file.txt");
        assert!(!is_allowed(&[], &path));
    }

    #[test]
    fn the_default_agent_configuration_is_opencode() {
        assert_eq!(AgentConfig::default().command, "opencode acp");
    }
}
