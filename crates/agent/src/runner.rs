use std::{
    env,
    ffi::OsString,
    fs,
    io::{self, Read},
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

use command_group::{CommandGroup, GroupChild};
use serde_json::Value;
use thiserror::Error;

use crate::{AgentEvent, AgentProtocolError, parse_output_line};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResearchBackend {
    OpenCode,
    Hermes,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OutputProtocol {
    OpenCodeJson,
    HermesLines,
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

#[derive(Clone, Debug)]
pub struct ResearchLaunch {
    pub backend: ResearchBackend,
    program: PathBuf,
    arguments: Vec<OsString>,
    current_directory: PathBuf,
    environment: Vec<(OsString, OsString)>,
    protocol: OutputProtocol,
    limits: ProcessLimits,
}

#[derive(Debug, Error)]
pub enum ResearchLaunchError {
    #[error("unsupported research backend {0:?}; use auto, opencode, or hermes")]
    InvalidBackend(String),
    #[error("research question must not be blank")]
    EmptyQuestion,
    #[error(
        "research backend {0:?} is not available on PATH; install OpenCode and run `opencode auth login`, or install and configure Hermes"
    )]
    MissingBackend(String),
    #[error("research scratch directory could not be created: {0}")]
    Scratch(#[source] io::Error),
}

impl ResearchLaunch {
    /// Builds a launch from environment configuration without reading or
    /// copying credential files. Auto mode prefers the authenticated standalone
    /// `OpenCode` surface requested for this project, then Hermes.
    ///
    /// # Errors
    ///
    /// Rejects blank questions, unknown backend names, missing executables, and
    /// an unavailable scratch directory.
    pub fn from_environment(
        prompt: &str,
        scratch_directory: impl Into<PathBuf>,
    ) -> Result<Self, ResearchLaunchError> {
        if prompt.trim().is_empty() {
            return Err(ResearchLaunchError::EmptyQuestion);
        }
        let requested = env::var("AI_NEWS_RESEARCH_BACKEND")
            .unwrap_or_else(|_| "auto".into())
            .to_lowercase();
        let (backend, program) = match requested.as_str() {
            "auto" => find_on_path("opencode")
                .map(|path| (ResearchBackend::OpenCode, path))
                .or_else(|| find_on_path("hermes").map(|path| (ResearchBackend::Hermes, path)))
                .ok_or_else(|| ResearchLaunchError::MissingBackend("opencode or hermes".into()))?,
            "opencode" => (
                ResearchBackend::OpenCode,
                find_on_path("opencode")
                    .ok_or_else(|| ResearchLaunchError::MissingBackend("opencode".into()))?,
            ),
            "hermes" => (
                ResearchBackend::Hermes,
                find_on_path("hermes")
                    .ok_or_else(|| ResearchLaunchError::MissingBackend("hermes".into()))?,
            ),
            other => return Err(ResearchLaunchError::InvalidBackend(other.into())),
        };
        let scratch_directory = scratch_directory.into();
        fs::create_dir_all(&scratch_directory).map_err(ResearchLaunchError::Scratch)?;
        Ok(match backend {
            ResearchBackend::OpenCode => Self::open_code(program, prompt, scratch_directory),
            ResearchBackend::Hermes => Self::hermes(program, prompt, scratch_directory),
        })
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
            protocol,
            limits,
        }
    }

    fn open_code(program: PathBuf, prompt: &str, scratch_directory: PathBuf) -> Self {
        let model = env::var_os("AI_NEWS_OPENCODE_MODEL")
            .unwrap_or_else(|| "opencode-go/mimo-v2.5-pro".into());
        Self {
            backend: ResearchBackend::OpenCode,
            program,
            arguments: vec![
                "run".into(),
                "--pure".into(),
                "--format".into(),
                "json".into(),
                "--model".into(),
                model,
                "--dir".into(),
                scratch_directory.as_os_str().to_owned(),
                "--title".into(),
                "Not News Canvas research".into(),
                prompt.into(),
            ],
            current_directory: scratch_directory,
            environment: Vec::new(),
            protocol: OutputProtocol::OpenCodeJson,
            limits: ProcessLimits::default(),
        }
    }

    fn hermes(program: PathBuf, prompt: &str, scratch_directory: PathBuf) -> Self {
        let provider = env::var_os("HERMES_PROVIDER").unwrap_or_else(|| "opencode-go".into());
        let model = env::var_os("HERMES_MODEL").unwrap_or_else(|| "mimo-v2.5-pro".into());
        let max_turns = env::var_os("HERMES_MAX_TURNS").unwrap_or_else(|| "12".into());
        let mut arguments = Vec::new();
        let mut environment = Vec::new();
        if let Some(home) = hermes_profile_home() {
            environment.push((OsString::from("HERMES_HOME"), home));
        } else if let Some(profile) = env::var_os("HERMES_PROFILE") {
            arguments.extend([OsString::from("--profile"), profile]);
        }
        environment.push((OsString::from("HERMES_MAX_ITERATIONS"), max_turns));
        arguments.extend([
            "--oneshot".into(),
            prompt.into(),
            "--provider".into(),
            provider,
            "--model".into(),
            model,
        ]);
        Self {
            backend: ResearchBackend::Hermes,
            program,
            arguments,
            current_directory: scratch_directory,
            environment,
            protocol: OutputProtocol::HermesLines,
            limits: ProcessLimits::default(),
        }
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
            .spawn(move || supervise(&self, &thread_cancelled, &sender))?;
        Ok(ResearchHandle {
            receiver,
            cancelled,
        })
    }
}

fn hermes_profile_home() -> Option<OsString> {
    if let Some(home) = env::var_os("AI_NEWS_HERMES_HOME").filter(|home| !home.is_empty()) {
        return Some(home);
    }
    if let Some(home) = env::var_os("HERMES_HOME").filter(|home| !home.is_empty()) {
        return Some(home);
    }
    let project_profile = env::current_dir()
        .ok()?
        .join(".hermes")
        .join("profiles")
        .join("ainews");
    project_profile
        .is_dir()
        .then(|| project_profile.into_os_string())
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

fn supervise(
    launch: &ResearchLaunch,
    cancelled: &AtomicBool,
    sender: &Sender<ResearchProcessEvent>,
) {
    let Some((mut child, raw_receiver)) = start_child(launch, sender) else {
        return;
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
                dispatch_line(stream, &bytes, launch.protocol, sender, &mut diagnostics);
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
                    launch.protocol,
                    sender,
                    &mut diagnostics,
                    &mut line_count,
                    launch.limits.max_lines,
                );
                let termination = drain_failure.unwrap_or_else(|| {
                    if status.success() {
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
) -> Option<(ChildGuard, Receiver<RawEvent>)> {
    let mut command = Command::new(&launch.program);
    command
        .args(&launch.arguments)
        .current_dir(&launch.current_directory)
        .stdin(Stdio::null())
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
    Some((child, raw_receiver))
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
    protocol: OutputProtocol,
    sender: &Sender<ResearchProcessEvent>,
    diagnostics: &mut u8,
) {
    let line = String::from_utf8_lossy(bytes);
    let line = clean_terminal_line(&line);
    if line.is_empty() {
        return;
    }
    match stream {
        StreamKind::Stderr => {
            if *diagnostics < 4 {
                *diagnostics += 1;
                let _ = sender.send(ResearchProcessEvent::Diagnostic(redact(&line, 500)));
            }
        }
        StreamKind::Stdout => match protocol {
            OutputProtocol::HermesLines => dispatch_agent_text(&line, sender),
            OutputProtocol::OpenCodeJson => dispatch_open_code_json(&line, sender),
        },
    }
}

fn dispatch_open_code_json(line: &str, sender: &Sender<ResearchProcessEvent>) {
    let value: Value = match serde_json::from_str(line) {
        Ok(value) => value,
        Err(error) => {
            let _ = sender.send(ResearchProcessEvent::ProtocolError(format!(
                "OpenCode emitted malformed JSON: {error}"
            )));
            return;
        }
    };
    if value.get("type").and_then(Value::as_str) != Some("text") {
        return;
    }
    let Some(text) = value
        .get("part")
        .and_then(|part| part.get("text"))
        .and_then(Value::as_str)
    else {
        let _ = sender.send(ResearchProcessEvent::ProtocolError(
            "OpenCode text event omitted part.text".into(),
        ));
        return;
    };
    for line in text.lines() {
        dispatch_agent_text(line, sender);
    }
}

fn dispatch_agent_text(line: &str, sender: &Sender<ResearchProcessEvent>) {
    match parse_output_line(line) {
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
    protocol: OutputProtocol,
    sender: &Sender<ResearchProcessEvent>,
    diagnostics: &mut u8,
    line_count: &mut u64,
    max_lines: u64,
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
                dispatch_line(stream, &bytes, protocol, sender, diagnostics);
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

fn redact(text: &str, max_chars: usize) -> String {
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

fn find_on_path(executable: &str) -> Option<PathBuf> {
    let path = Path::new(executable);
    if path.components().count() > 1 && path.is_file() {
        return Some(path.to_owned());
    }
    let paths = env::var_os("PATH")?;
    for directory in env::split_paths(&paths) {
        #[cfg(target_os = "windows")]
        let candidates = windows_candidates(&directory, executable);
        #[cfg(not(target_os = "windows"))]
        let candidates = vec![directory.join(executable)];
        if let Some(candidate) = candidates.into_iter().find(|candidate| candidate.is_file()) {
            return Some(candidate);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::TryRecvError;

    use tempfile::TempDir;

    use super::*;

    #[test]
    fn opencode_transport_exposes_only_text_parts_to_the_agent_protocol() {
        let (sender, receiver) = mpsc::channel();
        dispatch_open_code_json(
            r#"{"type":"step_start","part":{"type":"step-start"}}"#,
            &sender,
        );
        assert_eq!(receiver.try_recv().unwrap_err(), TryRecvError::Empty);
        dispatch_open_code_json(
            r#"{"type":"text","part":{"text":"AI_NEWS_EVENT: {\"type\":\"session.message\",\"data\":{\"message\":\"Searching\"}}\nAI_NEWS_EVENT: {\"type\":\"session.done\",\"data\":{\"message\":\"Complete\"}}"}}"#,
            &sender,
        );
        assert!(matches!(
            receiver.try_recv().unwrap(),
            ResearchProcessEvent::Output(AgentEvent::SessionMessage(message)) if message == "Searching"
        ));
        assert!(matches!(
            receiver.try_recv().unwrap(),
            ResearchProcessEvent::Output(AgentEvent::SessionDone(message)) if message == "Complete"
        ));
    }

    #[test]
    fn hermes_uses_the_raw_scripting_surface_and_bounds_turns_in_child_environment() {
        let directory = TempDir::new().unwrap();
        let launch = ResearchLaunch::hermes(
            PathBuf::from("hermes"),
            "AI_NEWS_EVENT: typed output only",
            directory.path().to_path_buf(),
        );
        assert!(
            launch
                .arguments
                .iter()
                .any(|argument| argument == "--oneshot")
        );
        assert!(
            !launch
                .arguments
                .iter()
                .any(|argument| argument == "chat" || argument == "--query")
        );
        assert!(
            launch
                .environment
                .iter()
                .any(|(name, value)| { name == "HERMES_MAX_ITERATIONS" && !value.is_empty() })
        );
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
        let launch = ResearchLaunch::from_environment(&prompt, directory.path()).unwrap();
        let handle = launch.spawn().unwrap();
        let deadline = Instant::now() + Duration::from_mins(2);
        let mut saw_event = false;
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
        assert!(saw_event && saw_done, "observed {observed:?}");
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
