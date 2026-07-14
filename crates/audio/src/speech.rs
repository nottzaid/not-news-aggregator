use std::{
    env,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TryRecvError, TrySendError},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use command_group::{CommandGroup, GroupChild};
use serde_json::json;

const DEFAULT_KOKORO_ENDPOINT: &str = "http://127.0.0.1:8890";
const MAX_SYNTHESIZED_BYTES: u64 = 32 * 1_024 * 1_024;
const QUEUE_CAPACITY: usize = 2;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SpeechSubmit {
    Queued,
    Disabled,
    Empty,
    Duplicate,
    Throttled,
    SessionLimit,
    QueueFull,
    Unavailable(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SpeechEvent {
    Played,
    Cancelled,
    Stale,
    Failed(String),
}

#[derive(Clone)]
struct PlayerCommand {
    program: PathBuf,
    prefix: Vec<OsString>,
}

struct SpeechConfig {
    enabled: bool,
    endpoint: String,
    api_key: String,
    model: String,
    voice: String,
    speed: f64,
    scratch_directory: PathBuf,
    player: Option<PlayerCommand>,
    synthesis_timeout: Duration,
    playback_timeout: Duration,
    note_interval: Duration,
    note_limit: usize,
    note_max_age: Duration,
    note_max_chars: usize,
    dedupe_interval: Duration,
}

impl SpeechConfig {
    fn from_environment(scratch_directory: PathBuf) -> Self {
        Self {
            enabled: enabled("AI_NEWS_ENABLE_VOICE", true),
            endpoint: env::var("KOKORO_TTS_BASE_URL")
                .unwrap_or_else(|_| DEFAULT_KOKORO_ENDPOINT.into())
                .trim_end_matches('/')
                .to_owned(),
            api_key: env::var("KOKORO_API_KEY").unwrap_or_default(),
            model: env::var("KOKORO_TTS_MODEL").unwrap_or_else(|_| "kokoro".into()),
            voice: env::var("KOKORO_TTS_VOICE").unwrap_or_else(|_| "af_heart".into()),
            speed: finite_f64("KOKORO_TTS_SPEED", 1.0, 0.5, 2.0),
            scratch_directory,
            player: player_from_environment(),
            synthesis_timeout: bounded_duration("AI_NEWS_VOICE_SYNTH_TIMEOUT", 45.0, 1.0, 120.0),
            playback_timeout: bounded_duration("AI_NEWS_AUDIO_PLAY_TIMEOUT", 20.0, 1.0, 120.0),
            note_interval: bounded_duration("AI_NEWS_VOICE_NOTE_INTERVAL", 35.0, 0.0, 3_600.0),
            note_limit: bounded_usize("AI_NEWS_VOICE_NOTE_LIMIT", 2, 0, 10),
            note_max_age: bounded_duration("AI_NEWS_VOICE_NOTE_MAX_AGE", 12.0, 0.0, 120.0),
            note_max_chars: bounded_usize("AI_NEWS_VOICE_NOTE_MAX_CHARS", 110, 40, 400),
            dedupe_interval: bounded_duration("AI_NEWS_VOICE_DEDUPE_SECONDS", 20.0, 0.0, 600.0),
        }
    }
}

enum Capability {
    Ready,
    Disabled,
    Unavailable(String),
}

struct SpeechRequest {
    generation: u64,
    queued_at: Instant,
    text: String,
}

enum WorkerCommand {
    Speak(SpeechRequest),
    Shutdown,
}

pub struct SpeechWorker {
    command: Option<SyncSender<WorkerCommand>>,
    events: Receiver<SpeechEvent>,
    cancellation: Arc<AtomicU64>,
    shutdown: Arc<AtomicBool>,
    pending: Arc<AtomicUsize>,
    worker: Option<JoinHandle<()>>,
    capability: Capability,
    note_interval: Duration,
    note_limit: usize,
    note_max_chars: usize,
    dedupe_interval: Duration,
    note_count: usize,
    last_note_at: Option<Instant>,
    recent_note: Option<(String, Instant)>,
}

impl SpeechWorker {
    pub fn from_environment(scratch_directory: impl Into<PathBuf>) -> Self {
        Self::start(SpeechConfig::from_environment(scratch_directory.into()))
    }

    /// Constructs a silent capability for embeddings that explicitly prohibit
    /// synthesized output. Research event handling remains unchanged.
    pub fn disabled() -> Self {
        let mut config = SpeechConfig::from_environment(PathBuf::new());
        config.enabled = false;
        config.api_key.clear();
        Self::start(config)
    }

    fn start(config: SpeechConfig) -> Self {
        let (event_sender, events) = mpsc::channel();
        let cancellation = Arc::new(AtomicU64::new(0));
        let shutdown = Arc::new(AtomicBool::new(false));
        let pending = Arc::new(AtomicUsize::new(0));
        let capability = if !config.enabled {
            Capability::Disabled
        } else if config.endpoint.is_empty() {
            Capability::Unavailable("Kokoro synthesis has no configured endpoint".into())
        } else if config.player.is_none() {
            Capability::Unavailable("no local WAV player is available".into())
        } else {
            Capability::Ready
        };
        let note_interval = config.note_interval;
        let note_limit = config.note_limit;
        let note_max_chars = config.note_max_chars;
        let dedupe_interval = config.dedupe_interval;
        let (command, worker) = if matches!(capability, Capability::Ready) {
            let (command_sender, command_receiver) = mpsc::sync_channel(QUEUE_CAPACITY);
            let worker_cancellation = Arc::clone(&cancellation);
            let worker_shutdown = Arc::clone(&shutdown);
            let worker_pending = Arc::clone(&pending);
            let thread = thread::Builder::new()
                .name("kokoro-voice".into())
                .spawn(move || {
                    worker_loop(
                        &config,
                        &command_receiver,
                        &event_sender,
                        &worker_cancellation,
                        &worker_shutdown,
                        &worker_pending,
                    );
                });
            match thread {
                Ok(thread) => (Some(command_sender), Some(thread)),
                Err(error) => {
                    return Self {
                        command: None,
                        events,
                        cancellation,
                        shutdown,
                        pending,
                        worker: None,
                        capability: Capability::Unavailable(format!(
                            "Kokoro voice worker could not start: {error}"
                        )),
                        note_interval,
                        note_limit,
                        note_max_chars,
                        dedupe_interval,
                        note_count: 0,
                        last_note_at: None,
                        recent_note: None,
                    };
                }
            }
        } else {
            (None, None)
        };
        Self {
            command,
            events,
            cancellation,
            shutdown,
            pending,
            worker,
            capability,
            note_interval,
            note_limit,
            note_max_chars,
            dedupe_interval,
            note_count: 0,
            last_note_at: None,
            recent_note: None,
        }
    }

    pub fn reset_session(&mut self) {
        self.cancel_session();
        self.note_count = 0;
        self.last_note_at = None;
        self.recent_note = None;
    }

    pub fn cancel_session(&self) {
        self.cancellation.fetch_add(1, Ordering::AcqRel);
    }

    pub fn submit_note(&mut self, text: &str, now: Instant) -> SpeechSubmit {
        match &self.capability {
            Capability::Disabled => return SpeechSubmit::Disabled,
            Capability::Unavailable(reason) => {
                return SpeechSubmit::Unavailable(reason.clone());
            }
            Capability::Ready => {}
        }
        let cleaned = clean_spoken_text(text);
        if cleaned.is_empty() {
            return SpeechSubmit::Empty;
        }
        let text = truncate_spoken_text(&cleaned, self.note_max_chars);
        let dedupe_key = utterance_key(&text);
        if self
            .recent_note
            .as_ref()
            .is_some_and(|(previous, spoken_at)| {
                *previous == dedupe_key
                    && now.saturating_duration_since(*spoken_at) < self.dedupe_interval
            })
        {
            return SpeechSubmit::Duplicate;
        }
        if self.note_count >= self.note_limit {
            return SpeechSubmit::SessionLimit;
        }
        if self
            .last_note_at
            .is_some_and(|last| now.saturating_duration_since(last) < self.note_interval)
        {
            return SpeechSubmit::Throttled;
        }
        let Some(command) = self.command.as_ref() else {
            return SpeechSubmit::Unavailable("Kokoro voice worker is unavailable".into());
        };
        let request = SpeechRequest {
            generation: self.cancellation.load(Ordering::Acquire),
            queued_at: now,
            text,
        };
        self.pending.fetch_add(1, Ordering::AcqRel);
        match command.try_send(WorkerCommand::Speak(request)) {
            Ok(()) => {
                self.note_count += 1;
                self.last_note_at = Some(now);
                self.recent_note = Some((dedupe_key, now));
                SpeechSubmit::Queued
            }
            Err(TrySendError::Full(_)) => {
                self.pending.fetch_sub(1, Ordering::AcqRel);
                SpeechSubmit::QueueFull
            }
            Err(TrySendError::Disconnected(_)) => {
                self.pending.fetch_sub(1, Ordering::AcqRel);
                SpeechSubmit::Unavailable("Kokoro voice worker stopped".into())
            }
        }
    }

    /// Drains one completed playback result without blocking the UI thread.
    ///
    /// # Errors
    ///
    /// Reports an empty queue or a stopped worker through [`TryRecvError`].
    pub fn try_recv(&self) -> Result<SpeechEvent, TryRecvError> {
        self.events.try_recv()
    }

    pub fn is_busy(&self) -> bool {
        self.pending.load(Ordering::Acquire) > 0
    }
}

impl Drop for SpeechWorker {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        self.cancellation.fetch_add(1, Ordering::AcqRel);
        if let Some(command) = self.command.as_ref() {
            let _ = command.try_send(WorkerCommand::Shutdown);
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn worker_loop(
    config: &SpeechConfig,
    commands: &Receiver<WorkerCommand>,
    events: &mpsc::Sender<SpeechEvent>,
    cancellation: &AtomicU64,
    shutdown: &AtomicBool,
    pending: &AtomicUsize,
) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build();
    while !shutdown.load(Ordering::Acquire) {
        let command = match commands.recv_timeout(Duration::from_millis(50)) {
            Ok(command) => command,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => break,
        };
        let WorkerCommand::Speak(request) = command else {
            break;
        };
        let event = runtime.as_ref().map_or_else(
            |_| SpeechEvent::Failed("Kokoro async runtime could not start".into()),
            |runtime| run_request(config, &request, cancellation, shutdown, runtime),
        );
        pending.fetch_sub(1, Ordering::AcqRel);
        let _ = events.send(event);
    }
}

fn run_request(
    config: &SpeechConfig,
    request: &SpeechRequest,
    cancellation: &AtomicU64,
    shutdown: &AtomicBool,
    runtime: &tokio::runtime::Runtime,
) -> SpeechEvent {
    if cancelled(request, cancellation, shutdown) {
        return SpeechEvent::Cancelled;
    }
    if !config.note_max_age.is_zero() && request.queued_at.elapsed() > config.note_max_age {
        return SpeechEvent::Stale;
    }
    if let Err(error) = fs::create_dir_all(&config.scratch_directory) {
        return SpeechEvent::Failed(format!("voice scratch directory is unavailable: {error}"));
    }
    let audio_path = config.scratch_directory.join(format!(
        "kokoro-{}-{}.wav",
        request.generation,
        request.queued_at.elapsed().as_nanos()
    ));
    let cleanup = ScratchAudio(audio_path.clone());
    match runtime.block_on(synthesize(
        config,
        request,
        &audio_path,
        cancellation,
        shutdown,
    )) {
        Ok(()) => {}
        Err(SynthesisError::Cancelled) => return SpeechEvent::Cancelled,
        Err(SynthesisError::Failed(message)) => return SpeechEvent::Failed(message),
    }
    if cancelled(request, cancellation, shutdown) {
        return SpeechEvent::Cancelled;
    }
    if !config.note_max_age.is_zero() && request.queued_at.elapsed() > config.note_max_age {
        return SpeechEvent::Stale;
    }
    let Some(player) = config.player.as_ref() else {
        return SpeechEvent::Failed("no local WAV player is available".into());
    };
    let result = play(
        player,
        &audio_path,
        config.playback_timeout,
        request,
        cancellation,
        shutdown,
    );
    drop(cleanup);
    result
}

struct ScratchAudio(PathBuf);

impl Drop for ScratchAudio {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

enum SynthesisError {
    Cancelled,
    Failed(String),
}

async fn synthesize(
    config: &SpeechConfig,
    speech: &SpeechRequest,
    output: &Path,
    cancellation: &AtomicU64,
    shutdown: &AtomicBool,
) -> Result<(), SynthesisError> {
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(3))
        .timeout(config.synthesis_timeout)
        .build()
        .map_err(|_| SynthesisError::Failed("Kokoro synthesis client could not start".into()))?;
    let mut request = client
        .post(format!("{}/v1/audio/speech", config.endpoint))
        .json(&json!({
            "model": config.model,
            "input": speech.text,
            "voice": config.voice,
            "response_format": "wav",
            "speed": config.speed,
        }));
    if !config.api_key.is_empty() {
        request = request.bearer_auth(&config.api_key);
    }
    let started = Instant::now();
    let mut response = tokio::select! {
        response = request.send() => response.map_err(|_| {
            SynthesisError::Failed("Kokoro synthesis request failed or timed out".into())
        })?,
        () = wait_for_cancellation(speech, cancellation, shutdown) => {
            return Err(SynthesisError::Cancelled);
        }
        () = tokio::time::sleep(config.synthesis_timeout) => {
            return Err(SynthesisError::Failed("Kokoro synthesis request timed out".into()));
        }
    };
    if !response.status().is_success() {
        return Err(SynthesisError::Failed(format!(
            "Kokoro synthesis returned HTTP {}",
            response.status().as_u16()
        )));
    }
    let mut bytes = Vec::new();
    loop {
        let remaining = config.synthesis_timeout.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            return Err(SynthesisError::Failed(
                "Kokoro synthesis request timed out".into(),
            ));
        }
        let chunk = tokio::select! {
            chunk = response.chunk() => chunk.map_err(|_| {
                SynthesisError::Failed("Kokoro audio response could not be read".into())
            })?,
            () = wait_for_cancellation(speech, cancellation, shutdown) => {
                return Err(SynthesisError::Cancelled);
            }
            () = tokio::time::sleep(remaining) => {
                return Err(SynthesisError::Failed("Kokoro synthesis request timed out".into()));
            }
        };
        let Some(chunk) = chunk else {
            break;
        };
        bytes.extend_from_slice(&chunk);
        if bytes.len() as u64 > MAX_SYNTHESIZED_BYTES {
            return Err(SynthesisError::Failed(
                "Kokoro audio exceeded the bounded 32 MiB allowance".into(),
            ));
        }
    }
    if bytes.len() as u64 > MAX_SYNTHESIZED_BYTES {
        return Err(SynthesisError::Failed(
            "Kokoro audio exceeded the bounded 32 MiB allowance".into(),
        ));
    }
    if bytes.len() < 12 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(SynthesisError::Failed(
            "Kokoro returned an invalid WAV response".into(),
        ));
    }
    fs::write(output, bytes).map_err(|error| {
        SynthesisError::Failed(format!("Kokoro audio could not be staged: {error}"))
    })
}

async fn wait_for_cancellation(
    request: &SpeechRequest,
    cancellation: &AtomicU64,
    shutdown: &AtomicBool,
) {
    loop {
        if cancelled(request, cancellation, shutdown) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn play(
    player: &PlayerCommand,
    path: &Path,
    timeout: Duration,
    request: &SpeechRequest,
    cancellation: &AtomicU64,
    shutdown: &AtomicBool,
) -> SpeechEvent {
    let mut command = Command::new(&player.program);
    command
        .args(&player.prefix)
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let Ok(child) = command.group_spawn() else {
        return SpeechEvent::Failed("local WAV player could not start".into());
    };
    let mut child = ChildProcess::new(child);
    let started = Instant::now();
    loop {
        if cancelled(request, cancellation, shutdown) {
            child.terminate();
            return SpeechEvent::Cancelled;
        }
        if started.elapsed() >= timeout {
            child.terminate();
            return SpeechEvent::Failed("local WAV playback timed out".into());
        }
        match child.child.inner().try_wait() {
            Ok(Some(status)) => {
                child.reaped = true;
                return if status.success() {
                    SpeechEvent::Played
                } else {
                    SpeechEvent::Failed("local WAV player exited unsuccessfully".into())
                };
            }
            Ok(None) => thread::sleep(Duration::from_millis(20)),
            Err(_) => {
                child.terminate();
                return SpeechEvent::Failed("local WAV player state became unreadable".into());
            }
        }
    }
}

struct ChildProcess {
    child: GroupChild,
    reaped: bool,
}

impl ChildProcess {
    fn new(child: GroupChild) -> Self {
        Self {
            child,
            reaped: false,
        }
    }

    fn terminate(&mut self) {
        if !self.reaped {
            let _ = self.child.kill();
            let _ = self.child.wait();
            self.reaped = true;
        }
    }
}

impl Drop for ChildProcess {
    fn drop(&mut self) {
        self.terminate();
    }
}

fn cancelled(request: &SpeechRequest, cancellation: &AtomicU64, shutdown: &AtomicBool) -> bool {
    shutdown.load(Ordering::Acquire) || cancellation.load(Ordering::Acquire) != request.generation
}

fn player_from_environment() -> Option<PlayerCommand> {
    if let Some(program) = env::var_os("AI_NEWS_AUDIO_PLAYER").filter(|path| !path.is_empty()) {
        return Some(PlayerCommand {
            program: PathBuf::from(program),
            prefix: Vec::new(),
        });
    }
    #[cfg(target_os = "windows")]
    {
        find_on_path("powershell.exe")
            .or_else(|| find_on_path("pwsh.exe"))
            .map(|program| PlayerCommand {
                program,
                prefix: vec![
                    "-NoProfile".into(),
                    "-NonInteractive".into(),
                    "-Command".into(),
                    "& { param([string]$AudioPath) $player = New-Object System.Media.SoundPlayer $AudioPath; $player.PlaySync() }"
                        .into(),
                ],
            })
    }
    #[cfg(not(target_os = "windows"))]
    {
        [
            (
                "ffplay",
                ["-nodisp", "-autoexit", "-loglevel", "quiet"].as_slice(),
            ),
            ("pw-play", [].as_slice()),
            ("aplay", ["-q"].as_slice()),
            ("paplay", [].as_slice()),
        ]
        .into_iter()
        .find_map(|(name, arguments)| {
            find_on_path(name).map(|program| PlayerCommand {
                program,
                prefix: arguments.iter().map(OsString::from).collect(),
            })
        })
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

fn clean_spoken_text(text: &str) -> String {
    let bounded = text.chars().take(4_000).collect::<String>();
    let without_urls = strip_markdown_links_and_urls(&bounded);
    without_urls
        .chars()
        .map(|character| match character {
            '*' | '_' | '`' | '#' | '>' | '[' | ']' => ' ',
            _ => character,
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn strip_markdown_links_and_urls(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut index = 0;
    while index < text.len() {
        let remaining = &text[index..];
        if let Some(label_end) = remaining.strip_prefix('[').and_then(|tail| tail.find("]("))
            && let Some(link_end) = remaining[label_end + 3..].find(')')
        {
            output.push_str(&remaining[1..=label_end]);
            index += label_end + 3 + link_end + 1;
            continue;
        }
        if remaining.starts_with("http://") || remaining.starts_with("https://") {
            let length = remaining
                .char_indices()
                .find(|(_, character)| character.is_whitespace())
                .map_or(remaining.len(), |(offset, _)| offset);
            index += length;
            continue;
        }
        let Some(character) = remaining.chars().next() else {
            break;
        };
        output.push(character);
        index += character.len_utf8();
    }
    output
}

fn truncate_spoken_text(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_owned();
    }
    let mut result = String::new();
    for word in text.split_whitespace() {
        let candidate_chars =
            result.chars().count() + usize::from(!result.is_empty()) + word.chars().count();
        if candidate_chars + 1 > max_chars {
            break;
        }
        if !result.is_empty() {
            result.push(' ');
        }
        result.push_str(word);
    }
    let trimmed = result.trim_end_matches([' ', ',', ';', ':', '.']);
    format!("{trimmed}.")
}

fn utterance_key(text: &str) -> String {
    let mut normalized = String::with_capacity(text.len());
    for character in text.chars() {
        if character.is_alphanumeric() {
            normalized.extend(character.to_lowercase());
        } else {
            normalized.push(' ');
        }
    }
    normalized.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn enabled(name: &str, default: bool) -> bool {
    env::var(name).map_or(default, |value| {
        !matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "0" | "false" | "no" | "off"
        )
    })
}

fn bounded_duration(name: &str, default: f64, minimum: f64, maximum: f64) -> Duration {
    Duration::from_secs_f64(finite_f64(name, default, minimum, maximum))
}

fn finite_f64(name: &str, default: f64, minimum: f64, maximum: f64) -> f64 {
    env::var(name)
        .ok()
        .and_then(|value| value.parse::<f64>().ok())
        .filter(|value| value.is_finite())
        .unwrap_or(default)
        .clamp(minimum, maximum)
}

fn bounded_usize(name: &str, default: usize, minimum: usize, maximum: usize) -> usize {
    env::var(name)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(default)
        .clamp(minimum, maximum)
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read, Write},
        net::TcpListener,
    };

    use tempfile::TempDir;

    use super::*;

    const EMPTY_WAV: &[u8] = b"RIFF\x24\0\0\0WAVEfmt \x10\0\0\0\x01\0\x01\0\x40\x1f\0\0\x80\x3e\0\0\x02\0\x10\0data\0\0\0\0";

    #[test]
    fn voice_text_is_cleaned_and_shortened_on_unicode_boundaries() {
        let cleaned = clean_spoken_text(
            "**Done**: see [the source](https://example.test/a) and `code` https://discard.test",
        );
        assert_eq!(cleaned, "Done : see the source and code");
        let shortened = truncate_spoken_text(
            "I found a useful source about the release and I am checking whether it changes the canvas relationships.",
            64,
        );
        assert!(shortened.chars().count() <= 64);
        assert!(shortened.ends_with('.'));
    }

    #[test]
    fn missing_player_is_typed_without_starting_a_worker() {
        let directory = TempDir::new().unwrap();
        let mut worker =
            SpeechWorker::start(test_config(directory.path(), None, "http://127.0.0.1:1"));
        assert_eq!(
            worker.submit_note("A useful note.", Instant::now()),
            SpeechSubmit::Unavailable("no local WAV player is available".into())
        );
        assert!(!worker.is_busy());
    }

    #[cfg(unix)]
    #[test]
    fn source_throttle_limit_and_deduplication_precede_synthesis() {
        let directory = TempDir::new().unwrap();
        let mut config = test_config(
            directory.path(),
            Some(PlayerCommand {
                program: PathBuf::from("/bin/true"),
                prefix: Vec::new(),
            }),
            "http://127.0.0.1:1",
        );
        config.note_interval = Duration::from_secs(35);
        config.note_limit = 2;
        config.dedupe_interval = Duration::from_secs(20);
        let mut worker = SpeechWorker::start(config);
        let now = Instant::now();
        assert_eq!(worker.submit_note("First note.", now), SpeechSubmit::Queued);
        assert_eq!(
            worker.submit_note("First note.", now + Duration::from_secs(1)),
            SpeechSubmit::Duplicate
        );
        assert_eq!(
            worker.submit_note("Different note.", now + Duration::from_secs(1)),
            SpeechSubmit::Throttled
        );
        assert_eq!(
            worker.submit_note("Second note.", now + Duration::from_secs(36)),
            SpeechSubmit::Queued
        );
        assert_eq!(
            worker.submit_note("Third note.", now + Duration::from_secs(72)),
            SpeechSubmit::SessionLimit
        );
        worker.cancel_session();
    }

    #[cfg(unix)]
    #[test]
    fn loopback_synthesis_precedes_playback_and_deletes_audio() {
        use std::os::unix::fs::PermissionsExt;

        let directory = TempDir::new().unwrap();
        let marker = directory.path().join("played");
        let player = directory.path().join("player.sh");
        fs::write(
            &player,
            format!("#!/bin/sh\nprintf played > '{}'\n", marker.display()),
        )
        .unwrap();
        fs::set_permissions(&player, fs::Permissions::from_mode(0o700)).unwrap();
        let (endpoint, body, server) = wav_server(Duration::ZERO);
        let mut worker = SpeechWorker::start(test_config(
            directory.path(),
            Some(PlayerCommand {
                program: player,
                prefix: Vec::new(),
            }),
            &endpoint,
        ));

        assert_eq!(
            worker.submit_note("**Useful** https://discard.test evidence", Instant::now()),
            SpeechSubmit::Queued
        );
        assert_eq!(wait_event(&worker), SpeechEvent::Played);
        server.join().unwrap();
        assert!(marker.exists());
        assert!(
            body.recv_timeout(Duration::from_secs(1))
                .unwrap()
                .contains("Useful evidence")
        );
        assert_eq!(
            fs::read_dir(directory.path().join("scratch"))
                .unwrap()
                .count(),
            0
        );
    }

    #[cfg(unix)]
    #[test]
    fn cancellation_kills_active_playback_and_never_leaves_scratch_audio() {
        use std::os::unix::fs::PermissionsExt;

        let directory = TempDir::new().unwrap();
        let started = directory.path().join("started");
        let completed = directory.path().join("completed");
        let player = directory.path().join("slow-player.sh");
        fs::write(
            &player,
            format!(
                "#!/bin/sh\nprintf started > '{}'\nsleep 10\nprintf completed > '{}'\n",
                started.display(),
                completed.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&player, fs::Permissions::from_mode(0o700)).unwrap();
        let (endpoint, _body, server) = wav_server(Duration::ZERO);
        let mut worker = SpeechWorker::start(test_config(
            directory.path(),
            Some(PlayerCommand {
                program: player,
                prefix: Vec::new(),
            }),
            &endpoint,
        ));
        assert_eq!(
            worker.submit_note("Cancel me.", Instant::now()),
            SpeechSubmit::Queued
        );
        let deadline = Instant::now() + Duration::from_secs(3);
        while !started.exists() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(started.exists());
        worker.cancel_session();
        assert_eq!(wait_event(&worker), SpeechEvent::Cancelled);
        server.join().unwrap();
        thread::sleep(Duration::from_millis(100));
        assert!(!completed.exists());
        assert_eq!(
            fs::read_dir(directory.path().join("scratch"))
                .unwrap()
                .count(),
            0
        );
    }

    #[cfg(unix)]
    #[test]
    fn cancellation_interrupts_an_inflight_synthesis_response() {
        use std::os::unix::fs::PermissionsExt;

        let directory = TempDir::new().unwrap();
        let player = directory.path().join("unused-player.sh");
        fs::write(&player, "#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&player, fs::Permissions::from_mode(0o700)).unwrap();
        let (endpoint, body, server) = wav_server(Duration::from_secs(2));
        let mut config = test_config(
            directory.path(),
            Some(PlayerCommand {
                program: player,
                prefix: Vec::new(),
            }),
            &endpoint,
        );
        config.note_limit = 10;
        let mut worker = SpeechWorker::start(config);
        assert_eq!(
            worker.submit_note("Cancel synthesis.", Instant::now()),
            SpeechSubmit::Queued
        );
        body.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(
            worker.submit_note("Queued second.", Instant::now()),
            SpeechSubmit::Queued
        );
        assert_eq!(
            worker.submit_note("Queued third.", Instant::now()),
            SpeechSubmit::Queued
        );
        assert_eq!(
            worker.submit_note("Rejected fourth.", Instant::now()),
            SpeechSubmit::QueueFull
        );
        let cancelled_at = Instant::now();
        worker.cancel_session();
        assert_eq!(wait_event(&worker), SpeechEvent::Cancelled);
        assert!(cancelled_at.elapsed() < Duration::from_millis(500));
        server.join().unwrap();
        assert_eq!(
            fs::read_dir(directory.path().join("scratch"))
                .unwrap()
                .count(),
            0
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_loopback_synthesis_invokes_and_cancels_real_player_processes() {
        let directory = TempDir::new().unwrap();
        let played = directory.path().join("played");
        let player = powershell_player(format!(
            "Set-Content -LiteralPath '{}' -Value played",
            powershell_literal(&played)
        ));
        let (endpoint, _body, server) = wav_server(Duration::ZERO);
        let mut worker =
            SpeechWorker::start(test_config(directory.path(), Some(player), &endpoint));
        assert_eq!(
            worker.submit_note("Windows playback.", Instant::now()),
            SpeechSubmit::Queued
        );
        assert_eq!(wait_event(&worker), SpeechEvent::Played);
        server.join().unwrap();
        assert!(played.exists());

        let started = directory.path().join("started");
        let completed = directory.path().join("completed");
        let player = powershell_player(format!(
            "Set-Content -LiteralPath '{}' -Value started; Start-Sleep -Seconds 10; Set-Content -LiteralPath '{}' -Value completed",
            powershell_literal(&started),
            powershell_literal(&completed)
        ));
        let (endpoint, _body, server) = wav_server(Duration::ZERO);
        let mut worker =
            SpeechWorker::start(test_config(directory.path(), Some(player), &endpoint));
        assert_eq!(
            worker.submit_note("Cancel Windows playback.", Instant::now()),
            SpeechSubmit::Queued
        );
        let deadline = Instant::now() + Duration::from_secs(3);
        while !started.exists() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(started.exists());
        worker.cancel_session();
        assert_eq!(wait_event(&worker), SpeechEvent::Cancelled);
        server.join().unwrap();
        thread::sleep(Duration::from_millis(100));
        assert!(!completed.exists());
    }

    fn test_config(root: &Path, player: Option<PlayerCommand>, endpoint: &str) -> SpeechConfig {
        SpeechConfig {
            enabled: true,
            endpoint: endpoint.into(),
            api_key: String::new(),
            model: "kokoro".into(),
            voice: "af_heart".into(),
            speed: 1.0,
            scratch_directory: root.join("scratch"),
            player,
            synthesis_timeout: Duration::from_secs(2),
            playback_timeout: Duration::from_secs(3),
            note_interval: Duration::ZERO,
            note_limit: 2,
            note_max_age: Duration::from_secs(2),
            note_max_chars: 110,
            dedupe_interval: Duration::ZERO,
        }
    }

    fn wait_event(worker: &SpeechWorker) -> SpeechEvent {
        let deadline = Instant::now() + Duration::from_secs(4);
        loop {
            match worker.try_recv() {
                Ok(event) => return event,
                Err(TryRecvError::Empty) if Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(10));
                }
                other => panic!("voice worker did not finish: {other:?}"),
            }
        }
    }

    fn wav_server(delay: Duration) -> (String, mpsc::Receiver<String>, JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let (body_sender, body_receiver) = mpsc::channel();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut buffer = [0_u8; 2048];
            let header_end = loop {
                let count = stream.read(&mut buffer).unwrap();
                request.extend_from_slice(&buffer[..count]);
                if let Some(index) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                    break index + 4;
                }
            };
            let headers = String::from_utf8_lossy(&request[..header_end]);
            let length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())
                        .flatten()
                })
                .unwrap();
            while request.len() - header_end < length {
                let count = stream.read(&mut buffer).unwrap();
                request.extend_from_slice(&buffer[..count]);
            }
            body_sender
                .send(String::from_utf8_lossy(&request[header_end..header_end + length]).into())
                .unwrap();
            thread::sleep(delay);
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                EMPTY_WAV.len()
            );
            let _ = stream.write_all(EMPTY_WAV);
        });
        (endpoint, body_receiver, server)
    }

    #[cfg(target_os = "windows")]
    fn powershell_player(command: String) -> PlayerCommand {
        PlayerCommand {
            program: find_on_path("powershell.exe").expect("Windows PowerShell must exist"),
            prefix: vec![
                "-NoProfile".into(),
                "-NonInteractive".into(),
                "-Command".into(),
                format!("& {{ param([string]$AudioPath) {command} }}").into(),
            ],
        }
    }

    #[cfg(target_os = "windows")]
    fn powershell_literal(path: &Path) -> String {
        path.to_string_lossy().replace('\'', "''")
    }
}
