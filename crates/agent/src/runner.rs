use std::{
    env,
    ffi::OsString,
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError},
    },
    thread,
    time::{Duration, Instant},
};

use base64::{Engine as _, engine::general_purpose};
use command_group::{CommandGroup, GroupChild};
use serde_json::{Value, json};
use thiserror::Error;
use zeroize::Zeroizing;

use crate::{AgentEvent, AgentProtocolError, parse_output_line};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResearchBackend {
    Hermes,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OutputProtocol {
    HermesLines,
    HermesAcp,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProcessLimits {
    pub total_timeout: Duration,
    pub idle_timeout: Duration,
    pub max_line_bytes: usize,
    pub max_output_bytes: u64,
    pub max_lines: u64,
}

impl Default for ProcessLimits {
    fn default() -> Self {
        Self {
            total_timeout: Duration::from_mins(20),
            idle_timeout: Duration::from_mins(3),
            max_line_bytes: 1024 * 1024,
            max_output_bytes: 32 * 1024 * 1024,
            max_lines: 8_192,
        }
    }
}

pub struct ResearchLaunch {
    pub backend: ResearchBackend,
    program: PathBuf,
    arguments: Vec<OsString>,
    current_directory: PathBuf,
    environment: Vec<(OsString, OsString)>,
    inherit_environment: bool,
    environment_loader: Option<EnvironmentLoader>,
    secret_filter: SecretFilter,
    protocol: OutputProtocol,
    acp_prompt: Option<String>,
    limits: ProcessLimits,
}

type EnvironmentLoader = Box<dyn FnOnce() -> Result<ResolvedEnvironment, String> + Send + 'static>;

pub struct ResolvedEnvironment {
    variables: Vec<(OsString, OsString)>,
    secrets: Vec<String>,
}

impl ResolvedEnvironment {
    pub fn new(
        variables: Vec<(OsString, OsString)>,
        secrets: impl IntoIterator<Item = String>,
    ) -> Self {
        Self {
            variables,
            secrets: secrets.into_iter().collect(),
        }
    }
}

#[derive(Default)]
struct SecretFilter {
    patterns: Zeroizing<Vec<String>>,
}

impl SecretFilter {
    fn extend(&mut self, secrets: impl IntoIterator<Item = String>) {
        for secret in secrets {
            if secret.len() < 8 {
                continue;
            }
            let bytes = secret.as_bytes();
            let encoded = [
                general_purpose::STANDARD.encode(bytes),
                general_purpose::STANDARD_NO_PAD.encode(bytes),
                general_purpose::URL_SAFE.encode(bytes),
                general_purpose::URL_SAFE_NO_PAD.encode(bytes),
                percent_encode(bytes),
                hex_encode(bytes, false),
                hex_encode(bytes, true),
            ];
            self.patterns.extend(encoded);
            self.patterns.push(secret);
        }
        self.patterns
            .sort_by_key(|value| std::cmp::Reverse(value.len()));
        self.patterns.dedup();
    }

    fn sanitize(&self, text: &str) -> String {
        self.patterns
            .iter()
            .fold(text.to_owned(), |output, pattern| {
                output.replace(pattern, "<redacted>")
            })
    }

    fn sanitize_bounded(&self, text: &str, max_chars: usize) -> String {
        redact_ambient(&self.sanitize(text), max_chars)
    }
}

fn percent_encode(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 3);
    for &byte in bytes {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            output.push(char::from(byte));
        } else {
            use std::fmt::Write as _;
            let _ = write!(output, "%{byte:02X}");
        }
    }
    output
}

fn hex_encode(bytes: &[u8], uppercase: bool) -> String {
    use std::fmt::Write as _;
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        if uppercase {
            let _ = write!(output, "{byte:02X}");
        } else {
            let _ = write!(output, "{byte:02x}");
        }
    }
    output
}

#[derive(Debug, Error)]
pub enum ResearchLaunchError {
    #[error("research question must not be blank")]
    EmptyQuestion,
    #[error(
        "Hermes is not available on PATH; install Hermes, then configure its provider dashboard"
    )]
    MissingHermes,
    #[error("research scratch directory could not be created: {0}")]
    Scratch(#[source] io::Error),
    #[error("isolated Hermes home could not be created: {0}")]
    IsolatedHome(#[source] io::Error),
}

#[derive(Debug, Error)]
pub enum HermesDashboardError {
    #[error("Hermes is not available on PATH")]
    Missing,
    #[error("Hermes dashboard could not start: {0}")]
    Start(#[source] io::Error),
    #[error("Hermes dashboard reaper could not start: {0}")]
    Reaper(#[source] io::Error),
}

pub fn hermes_is_available() -> bool {
    find_on_path("hermes").is_some()
}

pub fn browse_is_available() -> bool {
    find_on_path("browse").is_some()
}

pub fn curl_is_available() -> bool {
    find_on_path("curl").is_some()
}

/// Starts Hermes's own provider/model/credential dashboard. Not News never
/// reads, mirrors, or narrows the provider registry that Hermes owns.
///
/// # Errors
///
/// Reports a missing executable, a failed dashboard spawn, or failure to
/// create the process-reaping thread.
pub fn open_hermes_dashboard(
    hermes_root: &Path,
    profile_id: &str,
) -> Result<(), HermesDashboardError> {
    let program = find_on_path("hermes").ok_or(HermesDashboardError::Missing)?;
    let mut command = hermes_dashboard_command(&program, hermes_root, profile_id);
    let mut child = command.spawn().map_err(HermesDashboardError::Start)?;
    thread::Builder::new()
        .name("hermes-dashboard".into())
        .spawn(move || {
            if let Err(error) = child.wait() {
                eprintln!("Hermes dashboard process could not be reaped: {error}");
            }
        })
        .map_err(HermesDashboardError::Reaper)?;
    Ok(())
}

fn hermes_dashboard_command(program: &Path, root: &Path, profile_id: &str) -> Command {
    let mut command = Command::new(program);
    command
        .arg("-p")
        .arg(profile_id)
        .arg("dashboard")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command.env("HERMES_HOME", root);
    command
}

impl ResearchLaunch {
    /// Builds a Hermes launch bound to the application-installed profile.
    /// Provider, model, OAuth, and API-key choices remain entirely inside that
    /// profile; the application supplies no competing backend or model switch.
    ///
    /// # Errors
    ///
    /// Rejects blank questions, a missing Hermes executable, and an unusable
    /// scratch directory.
    pub fn for_hermes_profile(
        prompt: &str,
        scratch_directory: impl Into<PathBuf>,
        hermes_root: impl Into<PathBuf>,
        profile_id: &str,
    ) -> Result<Self, ResearchLaunchError> {
        if prompt.trim().is_empty() {
            return Err(ResearchLaunchError::EmptyQuestion);
        }
        let program = find_on_path("hermes").ok_or(ResearchLaunchError::MissingHermes)?;
        let scratch_directory = scratch_directory.into();
        create_private_dir_all(&scratch_directory).map_err(ResearchLaunchError::Scratch)?;
        let hermes_root = hermes_root.into();
        let isolated_home = hermes_root.join("profiles").join(profile_id).join("home");
        prepare_isolated_home(&isolated_home).map_err(ResearchLaunchError::IsolatedHome)?;
        Ok(Self::hermes(
            program,
            prompt,
            scratch_directory,
            hermes_root,
            profile_id,
            &isolated_home,
        ))
    }

    pub fn command(
        backend: ResearchBackend,
        program: impl Into<PathBuf>,
        arguments: impl IntoIterator<Item = OsString>,
        current_directory: impl Into<PathBuf>,
        protocol: OutputProtocol,
        limits: ProcessLimits,
    ) -> Self {
        Self {
            backend,
            program: program.into(),
            arguments: arguments.into_iter().collect(),
            current_directory: current_directory.into(),
            environment: Vec::new(),
            inherit_environment: true,
            environment_loader: None,
            secret_filter: SecretFilter::default(),
            protocol,
            acp_prompt: None,
            limits,
        }
    }

    fn hermes(
        program: PathBuf,
        prompt: &str,
        scratch_directory: PathBuf,
        hermes_root: PathBuf,
        profile_id: &str,
        isolated_home: &Path,
    ) -> Self {
        let max_turns = env::var_os("HERMES_MAX_TURNS").unwrap_or_else(|| "12".into());
        let mut environment = isolated_runtime_environment(isolated_home);
        let mut secret_filter = SecretFilter::default();
        secret_filter.extend(
            environment
                .iter()
                .filter(|(name, _)| {
                    name.to_str()
                        .is_some_and(|name| name.to_ascii_uppercase().contains("PROXY"))
                })
                .map(|(_, value)| value.to_string_lossy().into_owned()),
        );
        environment.extend([
            (OsString::from("HERMES_HOME"), hermes_root.into_os_string()),
            (OsString::from("HERMES_MAX_ITERATIONS"), max_turns),
            (OsString::from("HERMES_YOLO_MODE"), "1".into()),
            (OsString::from("HERMES_ACCEPT_HOOKS"), "1".into()),
            // Current Hermes otherwise restores the account HOME for host
            // terminal and execute_code subprocesses. Not News inputs must
            // originate in Connections, so its child tools stay profile-local.
            (OsString::from("TERMINAL_HOME_MODE"), "profile".into()),
        ]);
        Self {
            backend: ResearchBackend::Hermes,
            program,
            arguments: vec!["-p".into(), profile_id.into(), "acp".into()],
            current_directory: scratch_directory,
            environment,
            inherit_environment: false,
            environment_loader: None,
            secret_filter,
            protocol: OutputProtocol::HermesAcp,
            acp_prompt: Some(prompt.to_owned()),
            limits: ProcessLimits {
                // ACP emits one JSON-RPC notification per streamed token. The
                // byte ceiling remains authoritative; the larger line count
                // prevents normal private-thought chunks from exhausting a
                // limit intended for agent-authored output records.
                max_lines: 131_072,
                ..ProcessLimits::default()
            },
        }
    }

    /// Defers credential retrieval until the research supervisor thread, so a
    /// locked desktop vault can never stall the window event loop.
    #[must_use]
    pub fn with_environment_loader(
        mut self,
        loader: impl FnOnce() -> Result<ResolvedEnvironment, String> + Send + 'static,
    ) -> Self {
        self.environment_loader = Some(Box::new(loader));
        self
    }

    #[must_use]
    pub fn with_resolved_environment(mut self, environment: ResolvedEnvironment) -> Self {
        self.secret_filter.extend(environment.secrets);
        self.environment.extend(environment.variables);
        self
    }

    /// Starts the process-group supervisor and returns its nonblocking event
    /// channel.
    ///
    /// # Errors
    ///
    /// Returns an error only when the supervisor thread itself cannot start;
    /// child spawn failures arrive as terminal process events.
    pub fn spawn(self) -> Result<ResearchHandle, io::Error> {
        let cancelled = Arc::new(AtomicBool::new(false));
        let thread_cancelled = Arc::clone(&cancelled);
        let (sender, receiver) = mpsc::channel();
        thread::Builder::new()
            .name("research-process".into())
            .spawn(move || {
                let mut launch = self;
                if let Some(loader) = launch.environment_loader.take() {
                    match loader() {
                        Ok(environment) => launch = launch.with_resolved_environment(environment),
                        Err(error) => {
                            let _ = sender.send(ResearchProcessEvent::Finished(
                                ResearchTermination::IoFailure(format!(
                                    "research configuration unavailable: {error}"
                                )),
                            ));
                            return;
                        }
                    }
                }
                supervise(&launch, &thread_cancelled, &sender);
            })?;
        Ok(ResearchHandle {
            receiver,
            cancelled,
        })
    }
}

pub(crate) fn prepare_isolated_home(home: &Path) -> io::Result<()> {
    create_private_dir_all(home)?;
    #[cfg(target_os = "linux")]
    for directory in ["config", "data", "state", "cache"] {
        create_private_dir_all(&home.join(".xdg").join(directory))?;
    }
    #[cfg(target_os = "windows")]
    for directory in ["AppData/Roaming", "AppData/Local", "Temp"] {
        create_private_dir_all(&home.join(directory))?;
    }
    Ok(())
}

fn create_private_dir_all(path: &Path) -> io::Result<()> {
    if path.is_dir() {
        return Ok(());
    }
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        create_private_dir_all(parent)?;
    }
    match fs::create_dir(path) {
        Ok(()) => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
            }
            Ok(())
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists && path.is_dir() => Ok(()),
        Err(error) => Err(error),
    }
}

pub(crate) fn isolated_runtime_environment(home: &Path) -> Vec<(OsString, OsString)> {
    let mut environment = env::vars_os()
        .filter(|(name, _)| inherited_runtime_variable(name))
        .collect::<Vec<_>>();
    environment.push(("HOME".into(), home.as_os_str().to_owned()));
    #[cfg(target_os = "linux")]
    {
        let xdg = home.join(".xdg");
        environment.extend([
            (
                "XDG_CONFIG_HOME".into(),
                xdg.join("config").into_os_string(),
            ),
            ("XDG_DATA_HOME".into(), xdg.join("data").into_os_string()),
            ("XDG_STATE_HOME".into(), xdg.join("state").into_os_string()),
            ("XDG_CACHE_HOME".into(), xdg.join("cache").into_os_string()),
        ]);
    }
    #[cfg(target_os = "windows")]
    environment.extend([
        ("USERPROFILE".into(), home.as_os_str().to_owned()),
        (
            "APPDATA".into(),
            home.join("AppData/Roaming").into_os_string(),
        ),
        (
            "LOCALAPPDATA".into(),
            home.join("AppData/Local").into_os_string(),
        ),
        ("TEMP".into(), home.join("Temp").into_os_string()),
        ("TMP".into(), home.join("Temp").into_os_string()),
    ]);
    environment
}

fn inherited_runtime_variable(name: &std::ffi::OsStr) -> bool {
    matches!(
        name.to_str(),
        Some(
            "PATH"
                | "LANG"
                | "LC_ALL"
                | "LC_CTYPE"
                | "TZ"
                | "SSL_CERT_FILE"
                | "SSL_CERT_DIR"
                | "HTTP_PROXY"
                | "HTTPS_PROXY"
                | "ALL_PROXY"
                | "NO_PROXY"
                | "http_proxy"
                | "https_proxy"
                | "all_proxy"
                | "no_proxy"
                | "TMPDIR"
                | "SYSTEMROOT"
                | "SystemRoot"
                | "WINDIR"
                | "COMSPEC"
                | "PATHEXT"
        )
    )
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResearchTermination {
    Completed,
    ExitFailure(Option<i32>),
    SpawnFailure(String),
    Cancelled,
    TimedOut,
    IdleTimedOut,
    OutputLimit,
    IoFailure(String),
}

#[derive(Debug)]
pub enum ResearchProcessEvent {
    Started {
        backend: ResearchBackend,
        process_id: u32,
    },
    Output(AgentEvent),
    ProtocolError(String),
    Diagnostic(String),
    Finished(ResearchTermination),
}

pub struct ResearchHandle {
    receiver: Receiver<ResearchProcessEvent>,
    cancelled: Arc<AtomicBool>,
}

impl ResearchHandle {
    /// Receives an immediately available event without blocking.
    ///
    /// # Errors
    ///
    /// Reports an empty queue or a stopped supervisor through [`TryRecvError`].
    pub fn try_recv(&self) -> Result<ResearchProcessEvent, TryRecvError> {
        self.receiver.try_recv()
    }

    /// Waits no longer than `timeout` for the next process event.
    ///
    /// # Errors
    ///
    /// Reports timeout or a stopped supervisor through [`RecvTimeoutError`].
    pub fn recv_timeout(
        &self,
        timeout: Duration,
    ) -> Result<ResearchProcessEvent, RecvTimeoutError> {
        self.receiver.recv_timeout(timeout)
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }
}

impl Drop for ResearchHandle {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[derive(Clone, Copy)]
enum StreamKind {
    Stdout,
    Stderr,
}

enum RawEvent {
    Activity,
    Line {
        stream: StreamKind,
        bytes: Vec<u8>,
        truncated: bool,
    },
    OutputLimit,
    ReadFailure(io::Error),
}

enum AcpWrite {
    Line(String),
}

enum ProtocolState {
    Lines,
    Acp(AcpProtocol),
}

struct AcpProtocol {
    writer: Sender<AcpWrite>,
    prompt: String,
    current_directory: PathBuf,
    message_buffer: String,
    max_line_bytes: usize,
    completed: bool,
}

impl ProtocolState {
    fn new(launch: &ResearchLaunch, writer: Option<Sender<AcpWrite>>) -> Result<Self, String> {
        match launch.protocol {
            OutputProtocol::HermesLines => Ok(Self::Lines),
            OutputProtocol::HermesAcp => {
                let writer = writer.ok_or_else(|| "Hermes ACP stdin is unavailable".to_owned())?;
                let prompt = launch
                    .acp_prompt
                    .clone()
                    .ok_or_else(|| "Hermes ACP prompt is unavailable".to_owned())?;
                let state = AcpProtocol {
                    writer,
                    prompt,
                    current_directory: launch.current_directory.clone(),
                    message_buffer: String::new(),
                    max_line_bytes: launch.limits.max_line_bytes,
                    completed: false,
                };
                state.send(&json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "initialize",
                    "params": {
                        "protocolVersion": 1,
                        "clientCapabilities": {
                            "fs": {
                                "readTextFile": false,
                                "writeTextFile": false
                            }
                        },
                        "clientInfo": {
                            "name": "not-news",
                            "title": "Not News",
                            "version": env!("CARGO_PKG_VERSION")
                        }
                    }
                }))?;
                Ok(Self::Acp(state))
            }
        }
    }

    fn dispatch_stdout(
        &mut self,
        line: &str,
        sender: &Sender<ResearchProcessEvent>,
        filter: &SecretFilter,
    ) -> Result<(), String> {
        match self {
            Self::Lines => {
                dispatch_agent_text(line, sender, filter);
                Ok(())
            }
            Self::Acp(state) => state.dispatch(line, sender, filter),
        }
    }

    fn is_acp(&self) -> bool {
        matches!(self, Self::Acp(_))
    }

    fn completed(&self) -> bool {
        matches!(
            self,
            Self::Acp(AcpProtocol {
                completed: true,
                ..
            })
        )
    }
}

impl AcpProtocol {
    fn send(&self, message: &Value) -> Result<(), String> {
        self.writer
            .send(AcpWrite::Line(message.to_string()))
            .map_err(|_| "Hermes ACP input channel closed".to_owned())
    }

    fn dispatch(
        &mut self,
        line: &str,
        sender: &Sender<ResearchProcessEvent>,
        filter: &SecretFilter,
    ) -> Result<(), String> {
        let message: Value = serde_json::from_str(line)
            .map_err(|error| format!("Hermes ACP emitted invalid JSON: {error}"))?;
        if let Some(method) = message.get("method").and_then(Value::as_str) {
            return self.dispatch_method(method, &message, sender, filter);
        }
        let Some(id) = message.get("id").and_then(Value::as_u64) else {
            return Ok(());
        };
        if let Some(error) = message.get("error") {
            let detail = error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("unknown JSON-RPC error");
            return Err(format!(
                "Hermes ACP request {id} failed: {}",
                filter.sanitize_bounded(detail, 500)
            ));
        }
        match id {
            1 => self.send(&json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "session/new",
                "params": {
                    "cwd": self.current_directory.to_string_lossy(),
                    "mcpServers": []
                }
            })),
            2 => {
                let session_id = message
                    .pointer("/result/sessionId")
                    .and_then(Value::as_str)
                    .filter(|value| !value.trim().is_empty())
                    .ok_or_else(|| "Hermes ACP did not return a session id".to_owned())?;
                self.send(&json!({
                    "jsonrpc": "2.0",
                    "id": 3,
                    "method": "session/prompt",
                    "params": {
                        "sessionId": session_id,
                        "prompt": [{"type": "text", "text": self.prompt}]
                    }
                }))
            }
            3 => {
                self.flush_message(sender, filter);
                self.completed = true;
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn dispatch_method(
        &mut self,
        method: &str,
        message: &Value,
        sender: &Sender<ResearchProcessEvent>,
        filter: &SecretFilter,
    ) -> Result<(), String> {
        match method {
            "session/update" => self.dispatch_update(message, sender, filter),
            "session/request_permission" => {
                if let Some(id) = message.get("id") {
                    self.send(&json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {"outcome": {"outcome": "cancelled"}}
                    }))?;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn dispatch_update(
        &mut self,
        message: &Value,
        sender: &Sender<ResearchProcessEvent>,
        filter: &SecretFilter,
    ) -> Result<(), String> {
        let Some(update) = message.pointer("/params/update") else {
            return Ok(());
        };
        match update.get("sessionUpdate").and_then(Value::as_str) {
            Some("agent_message_chunk") => {
                if let Some(text) = update.pointer("/content/text").and_then(Value::as_str) {
                    self.push_message(text, sender, filter)?;
                }
            }
            Some("tool_call") => {
                let title = update
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or("using a research tool");
                dispatch_agent_text(
                    &format!("Hermes · {}", filter.sanitize_bounded(title, 240)),
                    sender,
                    filter,
                );
            }
            Some("plan") => {
                if let Some(content) = update
                    .get("entries")
                    .and_then(Value::as_array)
                    .and_then(|entries| {
                        entries.iter().find(|entry| {
                            entry.get("status").and_then(Value::as_str) == Some("in_progress")
                        })
                    })
                    .and_then(|entry| entry.get("content"))
                    .and_then(Value::as_str)
                {
                    dispatch_agent_text(
                        &format!("Hermes plan · {}", filter.sanitize_bounded(content, 240)),
                        sender,
                        filter,
                    );
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn push_message(
        &mut self,
        text: &str,
        sender: &Sender<ResearchProcessEvent>,
        filter: &SecretFilter,
    ) -> Result<(), String> {
        self.message_buffer.push_str(text);
        if self.message_buffer.len() > self.max_line_bytes {
            return Err("Hermes ACP response line exceeded its bounded allowance".into());
        }
        while let Some(newline) = self.message_buffer.find('\n') {
            let remainder = self.message_buffer.split_off(newline + 1);
            let mut line = std::mem::replace(&mut self.message_buffer, remainder);
            line.truncate(newline);
            let line = line.trim_end_matches('\r').trim();
            if !line.is_empty() {
                dispatch_agent_text(line, sender, filter);
            }
        }
        Ok(())
    }

    fn flush_message(&mut self, sender: &Sender<ResearchProcessEvent>, filter: &SecretFilter) {
        let line = std::mem::take(&mut self.message_buffer);
        let line = line.trim();
        if !line.is_empty() {
            dispatch_agent_text(line, sender, filter);
        }
    }
}

struct ChildGuard {
    child: GroupChild,
    reaped: bool,
}

impl ChildGuard {
    fn terminate(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        self.reaped = true;
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if !self.reaped {
            self.terminate();
        }
    }
}

#[allow(clippy::too_many_lines)]
fn supervise(
    launch: &ResearchLaunch,
    cancelled: &AtomicBool,
    sender: &Sender<ResearchProcessEvent>,
) {
    let Some((mut child, raw_receiver, acp_writer)) = start_child(launch, sender) else {
        return;
    };
    let mut protocol = match ProtocolState::new(launch, acp_writer) {
        Ok(protocol) => protocol,
        Err(error) => {
            terminate_with(&mut child, sender, ResearchTermination::IoFailure(error));
            return;
        }
    };
    let started = Instant::now();
    let mut last_activity = started;
    let mut line_count = 0_u64;
    let mut diagnostics = 0_u8;
    loop {
        let now = Instant::now();
        if let Some(reason) =
            requested_termination(cancelled, now, started, last_activity, launch.limits)
        {
            terminate_with(&mut child, sender, reason);
            return;
        }

        match raw_receiver.recv_timeout(Duration::from_millis(20)) {
            Ok(RawEvent::Activity) => last_activity = Instant::now(),
            Ok(RawEvent::Line {
                stream,
                bytes,
                truncated,
            }) => {
                line_count += 1;
                if truncated || line_count > launch.limits.max_lines {
                    terminate_with(&mut child, sender, ResearchTermination::OutputLimit);
                    return;
                }
                if let Err(error) = dispatch_line(
                    stream,
                    &bytes,
                    &mut protocol,
                    sender,
                    &mut diagnostics,
                    &launch.secret_filter,
                ) {
                    terminate_with(&mut child, sender, ResearchTermination::IoFailure(error));
                    return;
                }
                if protocol.completed() {
                    terminate_with(&mut child, sender, ResearchTermination::Completed);
                    return;
                }
            }
            Ok(RawEvent::OutputLimit) => {
                terminate_with(&mut child, sender, ResearchTermination::OutputLimit);
                return;
            }
            Ok(RawEvent::ReadFailure(error)) => {
                terminate_with(
                    &mut child,
                    sender,
                    ResearchTermination::IoFailure(error.to_string()),
                );
                return;
            }
            Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => {}
        }

        match child.child.try_wait() {
            Ok(Some(status)) => {
                child.reaped = true;
                let drain_failure = drain_after_exit(
                    &raw_receiver,
                    &mut protocol,
                    sender,
                    &mut diagnostics,
                    &mut line_count,
                    launch.limits.max_lines,
                    &launch.secret_filter,
                );
                let termination = drain_failure.unwrap_or_else(|| {
                    if protocol.is_acp() && !protocol.completed() {
                        ResearchTermination::IoFailure(
                            "Hermes ACP ended before returning the research result".into(),
                        )
                    } else if status.success() {
                        ResearchTermination::Completed
                    } else {
                        ResearchTermination::ExitFailure(status.code())
                    }
                });
                let _ = sender.send(ResearchProcessEvent::Finished(termination));
                return;
            }
            Ok(None) => {}
            Err(error) => {
                terminate_with(
                    &mut child,
                    sender,
                    ResearchTermination::IoFailure(error.to_string()),
                );
                return;
            }
        }
    }
}

fn start_child(
    launch: &ResearchLaunch,
    sender: &Sender<ResearchProcessEvent>,
) -> Option<(ChildGuard, Receiver<RawEvent>, Option<Sender<AcpWrite>>)> {
    let mut command = Command::new(&launch.program);
    if !launch.inherit_environment {
        command.env_clear();
    }
    command
        .args(&launch.arguments)
        .current_dir(&launch.current_directory)
        .stdin(if launch.protocol == OutputProtocol::HermesAcp {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in &launch.environment {
        command.env(key, value);
    }
    let child = match command.group_spawn() {
        Ok(child) => child,
        Err(error) => {
            let _ = sender.send(ResearchProcessEvent::Finished(
                ResearchTermination::SpawnFailure(error.to_string()),
            ));
            return None;
        }
    };
    let mut child = ChildGuard {
        child,
        reaped: false,
    };
    if sender
        .send(ResearchProcessEvent::Started {
            backend: launch.backend,
            process_id: child.child.id(),
        })
        .is_err()
    {
        return None;
    }

    let (raw_sender, raw_receiver) = mpsc::channel();
    let total = Arc::new(AtomicU64::new(0));
    let acp_writer = child
        .child
        .inner()
        .stdin
        .take()
        .map(|stdin| spawn_acp_writer(stdin, raw_sender.clone()));
    if let Some(stdout) = child.child.inner().stdout.take() {
        spawn_reader(
            stdout,
            StreamKind::Stdout,
            raw_sender.clone(),
            Arc::clone(&total),
            launch.limits,
        );
    }
    if let Some(stderr) = child.child.inner().stderr.take() {
        spawn_reader(
            stderr,
            StreamKind::Stderr,
            raw_sender.clone(),
            total,
            launch.limits,
        );
    }
    drop(raw_sender);
    Some((child, raw_receiver, acp_writer))
}

fn spawn_acp_writer(
    mut stdin: impl Write + Send + 'static,
    raw_sender: Sender<RawEvent>,
) -> Sender<AcpWrite> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        while let Ok(AcpWrite::Line(line)) = receiver.recv() {
            if let Err(error) = writeln!(stdin, "{line}").and_then(|()| stdin.flush()) {
                let _ = raw_sender.send(RawEvent::ReadFailure(error));
                return;
            }
        }
    });
    sender
}

fn requested_termination(
    cancelled: &AtomicBool,
    now: Instant,
    started: Instant,
    last_activity: Instant,
    limits: ProcessLimits,
) -> Option<ResearchTermination> {
    if cancelled.load(Ordering::Acquire) {
        Some(ResearchTermination::Cancelled)
    } else if now.duration_since(started) >= limits.total_timeout {
        Some(ResearchTermination::TimedOut)
    } else if now.duration_since(last_activity) >= limits.idle_timeout {
        Some(ResearchTermination::IdleTimedOut)
    } else {
        None
    }
}

fn terminate_with(
    child: &mut ChildGuard,
    sender: &Sender<ResearchProcessEvent>,
    reason: ResearchTermination,
) {
    child.terminate();
    let _ = sender.send(ResearchProcessEvent::Finished(reason));
}

fn spawn_reader(
    reader: impl Read + Send + 'static,
    stream: StreamKind,
    sender: Sender<RawEvent>,
    total: Arc<AtomicU64>,
    limits: ProcessLimits,
) {
    thread::spawn(move || {
        if let Err(error) = read_bounded(reader, stream, &sender, &total, limits) {
            let _ = sender.send(RawEvent::ReadFailure(error));
        }
    });
}

fn read_bounded(
    mut reader: impl Read,
    stream: StreamKind,
    sender: &Sender<RawEvent>,
    total: &AtomicU64,
    limits: ProcessLimits,
) -> Result<(), io::Error> {
    let mut buffer = [0_u8; 8192];
    let mut line = Vec::new();
    let mut truncated = false;
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            if !line.is_empty() || truncated {
                let _ = sender.send(RawEvent::Line {
                    stream,
                    bytes: line,
                    truncated,
                });
            }
            return Ok(());
        }
        let previous = total.fetch_add(count as u64, Ordering::AcqRel);
        if previous.saturating_add(count as u64) > limits.max_output_bytes {
            let _ = sender.send(RawEvent::OutputLimit);
            return Ok(());
        }
        let _ = sender.send(RawEvent::Activity);
        for &byte in &buffer[..count] {
            if byte == b'\n' {
                let _ = sender.send(RawEvent::Line {
                    stream,
                    bytes: std::mem::take(&mut line),
                    truncated,
                });
                truncated = false;
            } else if line.len() < limits.max_line_bytes {
                line.push(byte);
            } else {
                truncated = true;
            }
        }
    }
}

fn dispatch_line(
    stream: StreamKind,
    bytes: &[u8],
    protocol: &mut ProtocolState,
    sender: &Sender<ResearchProcessEvent>,
    diagnostics: &mut u8,
    filter: &SecretFilter,
) -> Result<(), String> {
    let line = String::from_utf8_lossy(bytes);
    let line = clean_terminal_line(&line);
    if line.is_empty() {
        return Ok(());
    }
    match stream {
        StreamKind::Stderr => {
            let report = !protocol.is_acp()
                || line.contains("[ERROR]")
                || line.to_ascii_lowercase().contains("traceback");
            if report && *diagnostics < 4 {
                *diagnostics += 1;
                let _ = sender.send(ResearchProcessEvent::Diagnostic(
                    filter.sanitize_bounded(&line, 500),
                ));
            }
            Ok(())
        }
        StreamKind::Stdout => protocol.dispatch_stdout(&line, sender, filter),
    }
}

fn dispatch_agent_text(line: &str, sender: &Sender<ResearchProcessEvent>, filter: &SecretFilter) {
    let line = filter.sanitize(line);
    match parse_output_line(&line) {
        Ok(output) => {
            let _ = sender.send(ResearchProcessEvent::Output(output));
        }
        Err(error) => {
            let _ = sender.send(ResearchProcessEvent::ProtocolError(protocol_error(&error)));
        }
    }
}

fn protocol_error(error: &AgentProtocolError) -> String {
    format!("Ignored invalid research output: {error}")
}

fn drain_after_exit(
    receiver: &Receiver<RawEvent>,
    protocol: &mut ProtocolState,
    sender: &Sender<ResearchProcessEvent>,
    diagnostics: &mut u8,
    line_count: &mut u64,
    max_lines: u64,
    filter: &SecretFilter,
) -> Option<ResearchTermination> {
    loop {
        match receiver.recv_timeout(Duration::from_millis(100)) {
            Ok(RawEvent::Activity) => {}
            Ok(RawEvent::Line {
                stream,
                bytes,
                truncated,
            }) => {
                *line_count += 1;
                if truncated || *line_count > max_lines {
                    return Some(ResearchTermination::OutputLimit);
                }
                if let Err(error) =
                    dispatch_line(stream, &bytes, protocol, sender, diagnostics, filter)
                {
                    return Some(ResearchTermination::IoFailure(error));
                }
            }
            Ok(RawEvent::OutputLimit) => return Some(ResearchTermination::OutputLimit),
            Ok(RawEvent::ReadFailure(error)) => {
                return Some(ResearchTermination::IoFailure(error.to_string()));
            }
            Err(RecvTimeoutError::Disconnected) => return None,
            Err(RecvTimeoutError::Timeout) => {
                return Some(ResearchTermination::IoFailure(
                    "output readers did not close after process exit".into(),
                ));
            }
        }
    }
}

fn clean_terminal_line(line: &str) -> String {
    let mut clean = String::with_capacity(line.len());
    let mut escape = false;
    for character in line.trim().chars() {
        if escape {
            if ('@'..='~').contains(&character) {
                escape = false;
            }
        } else if character == '\u{1b}' {
            escape = true;
        } else if !character.is_control() || character == '\t' {
            clean.push(character);
        }
    }
    clean.trim().to_owned()
}

fn redact_ambient(text: &str, max_chars: usize) -> String {
    let mut redacted = text.to_owned();
    for (key, value) in env::vars() {
        let upper = key.to_ascii_uppercase();
        if (upper.contains("KEY")
            || upper.contains("TOKEN")
            || upper.contains("SECRET")
            || upper.contains("PASSWORD"))
            && value.len() >= 8
        {
            redacted = redacted.replace(&value, "<redacted>");
        }
    }
    if redacted.chars().count() <= max_chars {
        redacted
    } else {
        redacted.chars().take(max_chars).collect::<String>() + "…"
    }
}

pub(crate) fn find_on_path(executable: &str) -> Option<PathBuf> {
    let path = Path::new(executable);
    if path.components().count() > 1 && is_executable_candidate(path) {
        return Some(path.to_owned());
    }
    let paths = env::var_os("PATH")?;
    for directory in env::split_paths(&paths) {
        #[cfg(target_os = "windows")]
        let candidates = windows_candidates(&directory, executable);
        #[cfg(not(target_os = "windows"))]
        let candidates = vec![directory.join(executable)];
        if let Some(candidate) = candidates
            .into_iter()
            .find(|candidate| is_executable_candidate(candidate))
        {
            return Some(candidate);
        }
    }
    None
}

fn is_executable_candidate(path: &Path) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(windows)]
    {
        true
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn hermes_uses_acp_streaming_and_bounds_turns_in_child_environment() {
        let directory = TempDir::new().unwrap();
        let launch = ResearchLaunch::hermes(
            PathBuf::from("hermes"),
            "AI_NEWS_EVENT: typed output only",
            directory.path().to_path_buf(),
            directory.path().join("hermes"),
            "not-news",
            &directory.path().join("hermes/profiles/not-news/home"),
        );
        assert_eq!(
            launch.arguments,
            [
                OsString::from("-p"),
                OsString::from("not-news"),
                OsString::from("acp")
            ]
        );
        assert_eq!(
            launch.acp_prompt.as_deref(),
            Some("AI_NEWS_EVENT: typed output only")
        );
        assert_eq!(launch.protocol, OutputProtocol::HermesAcp);
        assert!(!launch.inherit_environment);
        assert!(launch.environment.iter().any(|(name, value)| {
            name == "HOME"
                && value
                    == directory
                        .path()
                        .join("hermes/profiles/not-news/home")
                        .as_os_str()
        }));
        assert!(
            launch
                .environment
                .iter()
                .any(|(name, value)| { name == "HERMES_MAX_ITERATIONS" && !value.is_empty() })
        );
        assert!(launch.environment.iter().any(|(name, value)| {
            name == "HERMES_YOLO_MODE" && value == std::ffi::OsStr::new("1")
        }));
        assert!(
            !launch
                .arguments
                .iter()
                .any(|argument| argument == "--provider" || argument == "--model"),
            "Hermes must use its own configured provider and model unless explicitly overridden"
        );
    }

    #[test]
    fn acp_stream_exposes_tools_and_coalesces_typed_message_chunks() {
        let directory = TempDir::new().unwrap();
        let launch = ResearchLaunch::hermes(
            PathBuf::from("hermes"),
            "research this",
            directory.path().to_path_buf(),
            directory.path().join("hermes"),
            "not-news",
            &directory.path().join("hermes/profiles/not-news/home"),
        );
        let (write_sender, write_receiver) = mpsc::channel();
        let (event_sender, event_receiver) = mpsc::channel();
        let mut protocol = ProtocolState::new(&launch, Some(write_sender)).unwrap();
        let mut filter = SecretFilter::default();
        filter.extend(["vault-secret-123456".to_owned()]);

        let AcpWrite::Line(initialize) = write_receiver.recv().unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&initialize).unwrap()["method"],
            "initialize"
        );
        protocol
            .dispatch_stdout(
                r#"{"jsonrpc":"2.0","id":1,"result":{}}"#,
                &event_sender,
                &filter,
            )
            .unwrap();
        let AcpWrite::Line(new_session) = write_receiver.recv().unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&new_session).unwrap()["method"],
            "session/new"
        );
        protocol
            .dispatch_stdout(
                r#"{"jsonrpc":"2.0","id":2,"result":{"sessionId":"session-a"}}"#,
                &event_sender,
                &filter,
            )
            .unwrap();
        let AcpWrite::Line(prompt) = write_receiver.recv().unwrap();
        let prompt: Value = serde_json::from_str(&prompt).unwrap();
        assert_eq!(prompt["method"], "session/prompt");
        assert_eq!(prompt["params"]["prompt"][0]["text"], "research this");

        protocol
            .dispatch_stdout(
                r#"{"jsonrpc":"2.0","method":"session/update","params":{"update":{"sessionUpdate":"agent_thought_chunk","content":{"type":"text","text":"private reasoning"}}}}"#,
                &event_sender,
                &filter,
            )
            .unwrap();
        assert!(event_receiver.try_recv().is_err());
        protocol
            .dispatch_stdout(
                r#"{"jsonrpc":"2.0","method":"session/update","params":{"update":{"sessionUpdate":"tool_call","title":"web search: primary sources"}}}"#,
                &event_sender,
                &filter,
            )
            .unwrap();
        assert!(matches!(
            event_receiver.recv().unwrap(),
            ResearchProcessEvent::Output(AgentEvent::SessionMessage(message))
                if message == "Hermes · web search: primary sources"
        ));

        for chunk in [
            "AI_NEWS_EVENT: {\"type\":\"session.message\",\"data\":{\"message\":\"vault-secret-",
            "123456\"}}\nAI_NEWS_EVENT: {\"type\":\"session.done\",\"data\":{\"message\":\"Complete.\"}}",
        ] {
            let update = json!({
                "jsonrpc": "2.0",
                "method": "session/update",
                "params": {
                    "update": {
                        "sessionUpdate": "agent_message_chunk",
                        "content": {"type": "text", "text": chunk}
                    }
                }
            });
            protocol
                .dispatch_stdout(&update.to_string(), &event_sender, &filter)
                .unwrap();
        }
        assert!(matches!(
            event_receiver.recv().unwrap(),
            ResearchProcessEvent::Output(AgentEvent::SessionMessage(message))
                if message == "<redacted>"
        ));
        protocol
            .dispatch_stdout(
                r#"{"jsonrpc":"2.0","id":3,"result":{"stopReason":"end_turn"}}"#,
                &event_sender,
                &filter,
            )
            .unwrap();
        assert!(protocol.completed());
        assert!(matches!(
            event_receiver.recv().unwrap(),
            ResearchProcessEvent::Output(AgentEvent::SessionDone(message))
                if message == "Complete."
        ));
    }

    #[test]
    fn hermes_dashboard_keeps_credentials_inside_hermes() {
        let command =
            hermes_dashboard_command(Path::new("hermes"), Path::new("/hermes"), "research");
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            ["-p", "research", "dashboard"]
        );
        assert!(command.get_envs().any(|(name, value)| {
            name == "HERMES_HOME" && value == Some(std::ffi::OsStr::new("/hermes"))
        }));
    }

    #[cfg(unix)]
    fn shell_launch(script: &str, limits: ProcessLimits, directory: &Path) -> ResearchLaunch {
        ResearchLaunch::command(
            ResearchBackend::Hermes,
            "/bin/sh",
            [OsString::from("-c"), OsString::from(script)],
            directory,
            OutputProtocol::HermesLines,
            limits,
        )
    }

    #[cfg(unix)]
    fn terminal(handle: &ResearchHandle) -> ResearchTermination {
        loop {
            if let ResearchProcessEvent::Finished(termination) = handle
                .recv_timeout(Duration::from_secs(3))
                .expect("supervisor should finish")
            {
                return termination;
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn completed_process_drains_typed_output_and_sanitized_diagnostics() {
        let directory = TempDir::new().unwrap();
        let launch = shell_launch(
            "printf '%s\\n' 'AI_NEWS_EVENT: {\"type\":\"session.message\",\"data\":{\"message\":\"Searching\"}}'; printf 'diagnostic\\n' >&2",
            ProcessLimits::default(),
            directory.path(),
        );
        let handle = launch.spawn().unwrap();
        let mut saw_output = false;
        let mut saw_diagnostic = false;
        loop {
            match handle.recv_timeout(Duration::from_secs(3)).unwrap() {
                ResearchProcessEvent::Output(AgentEvent::SessionMessage(message)) => {
                    assert_eq!(message, "Searching");
                    saw_output = true;
                }
                ResearchProcessEvent::Diagnostic(message) => {
                    assert_eq!(message, "diagnostic");
                    saw_diagnostic = true;
                }
                ResearchProcessEvent::Finished(termination) => {
                    assert_eq!(termination, ResearchTermination::Completed);
                    break;
                }
                _ => {}
            }
        }
        assert!(saw_output && saw_diagnostic);
    }

    #[cfg(unix)]
    #[test]
    fn deferred_environment_is_resolved_inside_the_supervisor() {
        let directory = TempDir::new().unwrap();
        let launch = shell_launch(
            r#"test "$NOT_NEWS_TEST_CREDENTIAL" = "opaque-value""#,
            ProcessLimits::default(),
            directory.path(),
        )
        .with_environment_loader(|| {
            Ok(ResolvedEnvironment::new(
                vec![(
                    OsString::from("NOT_NEWS_TEST_CREDENTIAL"),
                    OsString::from("opaque-value"),
                )],
                ["opaque-value".to_owned()],
            ))
        });

        assert_eq!(
            terminal(&launch.spawn().unwrap()),
            ResearchTermination::Completed
        );
    }

    #[cfg(unix)]
    #[test]
    fn line_total_and_idle_limits_terminate_the_whole_job() {
        let directory = TempDir::new().unwrap();
        let line_limited = shell_launch(
            "printf '123456789\\n'",
            ProcessLimits {
                max_line_bytes: 4,
                ..ProcessLimits::default()
            },
            directory.path(),
        )
        .spawn()
        .unwrap();
        assert_eq!(terminal(&line_limited), ResearchTermination::OutputLimit);

        let idle_limited = shell_launch(
            "sleep 2",
            ProcessLimits {
                total_timeout: Duration::from_secs(2),
                idle_timeout: Duration::from_millis(80),
                ..ProcessLimits::default()
            },
            directory.path(),
        )
        .spawn()
        .unwrap();
        assert_eq!(terminal(&idle_limited), ResearchTermination::IdleTimedOut);
    }

    #[cfg(unix)]
    #[test]
    fn cancellation_kills_descendants_before_they_can_escape() {
        let directory = TempDir::new().unwrap();
        let marker = directory.path().join("orphaned");
        let script = format!("(sleep 1; printf orphan > '{}') & wait", marker.display());
        let handle = shell_launch(&script, ProcessLimits::default(), directory.path())
            .spawn()
            .unwrap();
        loop {
            if matches!(
                handle.recv_timeout(Duration::from_secs(1)).unwrap(),
                ResearchProcessEvent::Started { .. }
            ) {
                break;
            }
        }
        handle.cancel();
        assert_eq!(terminal(&handle), ResearchTermination::Cancelled);
        thread::sleep(Duration::from_millis(1_200));
        assert!(!marker.exists(), "a descendant survived process-group kill");
    }

    #[cfg(unix)]
    #[test]
    fn missing_executable_is_a_terminal_event_not_a_panic() {
        let directory = TempDir::new().unwrap();
        let handle = ResearchLaunch::command(
            ResearchBackend::Hermes,
            directory.path().join("does-not-exist"),
            [],
            directory.path(),
            OutputProtocol::HermesLines,
            ProcessLimits::default(),
        )
        .spawn()
        .unwrap();
        assert!(matches!(
            terminal(&handle),
            ResearchTermination::SpawnFailure(_)
        ));
    }

    #[test]
    #[ignore = "uses the locally authenticated external research subscription"]
    fn configured_live_backend_crosses_the_real_transport() {
        let directory = TempDir::new().unwrap();
        let prompt = crate::build_research_prompt(
            "Transport verification only: without external tools, emit one event about the \
             Rust language, then finish the session.",
            &not_news_domain::GraphSnapshot::default(),
        );
        let root = std::env::var_os("AI_NEWS_HERMES_ROOT")
            .expect("AI_NEWS_HERMES_ROOT must name the exact live-test Hermes root");
        let launch =
            ResearchLaunch::for_hermes_profile(&prompt, directory.path(), root, "not-news")
                .unwrap();
        let handle = launch.spawn().unwrap();
        let deadline = Instant::now() + Duration::from_mins(2);
        let mut saw_event = false;
        let mut saw_voice_note = false;
        let mut saw_done = false;
        let mut termination = None;
        let mut observed = Vec::new();
        while Instant::now() < deadline {
            match handle.recv_timeout(Duration::from_secs(2)) {
                Ok(ResearchProcessEvent::Output(AgentEvent::EventUpsert(event))) => {
                    observed.push(format!("event:{}", event.id.0));
                    saw_event = true;
                }
                Ok(ResearchProcessEvent::Output(AgentEvent::SessionDone(message))) => {
                    observed.push(format!("done:{message}"));
                    saw_done = true;
                }
                Ok(ResearchProcessEvent::Output(AgentEvent::VoiceNote(message))) => {
                    observed.push(format!("voice:{message}"));
                    saw_voice_note = true;
                }
                Ok(ResearchProcessEvent::Finished(reason)) => {
                    observed.push(format!("finished:{reason:?}"));
                    termination = Some(reason);
                    break;
                }
                Ok(event) => observed.push(format!("{event:?}")),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        assert!(
            saw_event && saw_voice_note && saw_done,
            "observed {observed:?}"
        );
        assert_eq!(
            termination,
            Some(ResearchTermination::Completed),
            "observed {observed:?}"
        );
    }
}

#[cfg(target_os = "windows")]
fn windows_candidates(directory: &Path, executable: &str) -> Vec<PathBuf> {
    if Path::new(executable).extension().is_some() {
        return vec![directory.join(executable)];
    }
    env::var_os("PATHEXT").map_or_else(
        || vec![directory.join(format!("{executable}.exe"))],
        |extensions| {
            extensions
                .to_string_lossy()
                .split(';')
                .filter(|extension| !extension.is_empty())
                .map(|extension| directory.join(format!("{executable}{extension}")))
                .collect()
        },
    )
}
