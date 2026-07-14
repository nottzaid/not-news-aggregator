//! Bounded Windows/Linux speech input, transcription, synthesis, and playback.

mod speech;

pub use speech::{SpeechEvent, SpeechSubmit, SpeechWorker};

use std::{
    env, fs,
    io::{self, Read},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, TryRecvError},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use cpal::{
    Sample, SampleFormat, Stream, StreamConfig,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};
use ringbuf::{
    HeapProd, HeapRb,
    traits::{Consumer, Observer, Producer, Split},
};
use serde::Deserialize;
use thiserror::Error;
use zeroize::Zeroizing;

const RING_SECONDS: usize = 2;
const MAX_WAV_BYTES: u64 = 24 * 1_024 * 1_024;
const MAX_MONO_SAMPLES: u64 = (MAX_WAV_BYTES - 44) / 2;
const TRANSCRIPTION_TIMEOUT: Duration = Duration::from_secs(45);
const MAX_RESPONSE_BYTES: u64 = 64 * 1_024;
const MAX_ERROR_BYTES: u64 = 4 * 1_024;
const DEFAULT_ENDPOINT: &str = "https://api.groq.com/openai/v1/audio/transcriptions";
const DEFAULT_MODEL: &str = "whisper-large-v3-turbo";

#[derive(Debug, Error)]
pub enum AudioError {
    #[error("no default microphone is available")]
    NoInputDevice,
    #[error("the default microphone configuration is unavailable: {0}")]
    DefaultConfiguration(String),
    #[error("microphone sample format {0} is not supported")]
    UnsupportedSampleFormat(String),
    #[error("the recording file could not be created: {0}")]
    CreateRecording(io::Error),
    #[error("the microphone stream could not be created: {0}")]
    BuildStream(String),
    #[error("the microphone stream could not start: {0}")]
    StartStream(String),
    #[error("the recording writer could not start: {0}")]
    StartWriter(io::Error),
    #[error("the recording writer failed: {0}")]
    WriteRecording(String),
    #[error("recording audio arrived faster than it could be preserved; nothing was submitted")]
    CaptureOverflow,
    #[error("recording exceeded the bounded 24 MiB transcription allowance; nothing was submitted")]
    RecordingTooLong,
    #[error("the microphone stopped unexpectedly: {0}")]
    StreamFailure(String),
    #[error("the recording contains no audio")]
    EmptyRecording,
    #[error("GROQ_API_KEY is not configured; voice transcription is unavailable")]
    MissingApiKey,
    #[error("the recording exceeds the bounded 24 MiB transcription allowance")]
    RecordingTooLarge,
    #[error("the transcription client could not be built: {0}")]
    BuildClient(String),
    #[error("the transcription request failed: {0}")]
    Request(String),
    #[error("the transcription service returned HTTP {status}: {detail}")]
    Service { status: u16, detail: String },
    #[error("the transcription service returned an invalid response: {0}")]
    InvalidResponse(String),
    #[error("the transcription was cancelled")]
    Cancelled,
    #[error("the transcription worker could not start: {0}")]
    StartTranscriber(io::Error),
}

/// An active microphone capture. Dropping it cancels capture and removes its WAV.
pub struct Recorder {
    stream: Option<Stream>,
    stop: Arc<AtomicBool>,
    overflow: Arc<AtomicBool>,
    length_limit: Arc<AtomicBool>,
    stream_error: Arc<Mutex<Option<String>>>,
    samples: Arc<AtomicU64>,
    sample_rate: u32,
    path: PathBuf,
    writer: Option<JoinHandle<Result<(), String>>>,
}

#[derive(Clone)]
struct CaptureSignals {
    overflow: Arc<AtomicBool>,
    length_limit: Arc<AtomicBool>,
    samples: Arc<AtomicU64>,
    stream_error: Arc<Mutex<Option<String>>>,
}

impl Recorder {
    /// Starts recording the default microphone as mono signed 16-bit PCM.
    ///
    /// # Errors
    ///
    /// Reports absent or inaccessible devices, unsupported formats, filesystem
    /// failures, and stream startup failures without leaving a recording behind.
    pub fn start(path: impl AsRef<Path>) -> Result<Self, AudioError> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(AudioError::CreateRecording)?;
        }
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or(AudioError::NoInputDevice)?;
        let supported = device
            .default_input_config()
            .map_err(|error| AudioError::DefaultConfiguration(error.to_string()))?;
        let sample_format = supported.sample_format();
        let config: StreamConfig = supported.into();
        if config.channels == 0 {
            return Err(AudioError::DefaultConfiguration(
                "the device reported zero channels".into(),
            ));
        }

        let capacity = usize::try_from(config.sample_rate)
            .unwrap_or(usize::MAX / RING_SECONDS)
            .saturating_mul(RING_SECONDS)
            .max(1);
        let ring = HeapRb::<i16>::new(capacity);
        let (producer, consumer) = ring.split();
        let stop = Arc::new(AtomicBool::new(false));
        let overflow = Arc::new(AtomicBool::new(false));
        let length_limit = Arc::new(AtomicBool::new(false));
        let stream_error = Arc::new(Mutex::new(None));
        let samples = Arc::new(AtomicU64::new(0));

        let writer_stop = Arc::clone(&stop);
        let sample_rate = config.sample_rate;
        let wav_writer = create_wav_writer(&path, sample_rate)?;
        let writer = thread::Builder::new()
            .name("voice-wav-writer".into())
            .spawn(move || write_consumer(wav_writer, consumer, &writer_stop));
        let writer = match writer {
            Ok(writer) => writer,
            Err(error) => {
                let _ = fs::remove_file(&path);
                return Err(AudioError::StartWriter(error));
            }
        };

        let channels = usize::from(config.channels);
        let signals = CaptureSignals {
            overflow: Arc::clone(&overflow),
            length_limit: Arc::clone(&length_limit),
            samples: Arc::clone(&samples),
            stream_error: Arc::clone(&stream_error),
        };
        let stream = build_stream(
            &device,
            &config,
            sample_format,
            channels,
            producer,
            &signals,
        );
        let mut recorder = Self {
            stream: None,
            stop,
            overflow,
            length_limit,
            stream_error,
            samples,
            sample_rate,
            path,
            writer: Some(writer),
        };
        recorder.stream = Some(match stream {
            Ok(stream) => stream,
            Err(error) => {
                recorder.cancel_inner();
                return Err(error);
            }
        });
        let Some(stream) = recorder.stream.as_ref() else {
            recorder.cancel_inner();
            return Err(AudioError::BuildStream(
                "stream construction returned no stream".into(),
            ));
        };
        if let Err(error) = stream.play() {
            recorder.cancel_inner();
            return Err(AudioError::StartStream(error.to_string()));
        }
        Ok(recorder)
    }

    /// Returns a terminal capture failure that the UI can surface immediately.
    pub fn failure(&self) -> Option<AudioError> {
        if self.writer.as_ref().is_some_and(JoinHandle::is_finished) {
            return Some(AudioError::WriteRecording(
                "recording writer stopped unexpectedly".into(),
            ));
        }
        if self.overflow.load(Ordering::Acquire) {
            return Some(AudioError::CaptureOverflow);
        }
        if self.length_limit.load(Ordering::Acquire) {
            return Some(AudioError::RecordingTooLong);
        }
        self.stream_error
            .lock()
            .ok()
            .and_then(|slot| slot.clone())
            .map(AudioError::StreamFailure)
    }

    /// Stops capture, finalizes the WAV, and transfers deletion ownership.
    ///
    /// # Errors
    ///
    /// Deletes and rejects empty, truncated, overlong, or unfinalized audio.
    pub fn stop(mut self) -> Result<RecordedAudio, AudioError> {
        self.stream.take();
        self.stop.store(true, Ordering::Release);
        let Some(writer) = self.writer.take() else {
            self.remove_file();
            return Err(AudioError::WriteRecording(
                "recording writer was no longer active".into(),
            ));
        };
        let writer_result = writer
            .join()
            .map_err(|_| AudioError::WriteRecording("writer thread panicked".into()))?;
        if let Err(error) = writer_result {
            self.remove_file();
            return Err(AudioError::WriteRecording(error));
        }
        if let Some(error) = self.failure() {
            self.remove_file();
            return Err(error);
        }
        let samples = self.samples.load(Ordering::Acquire);
        if samples == 0 {
            self.remove_file();
            return Err(AudioError::EmptyRecording);
        }
        Ok(RecordedAudio {
            path: std::mem::take(&mut self.path),
            sample_rate: self.sample_rate,
            samples,
        })
    }

    /// Cancels capture and removes all temporary audio.
    pub fn cancel(mut self) {
        self.cancel_inner();
    }

    fn cancel_inner(&mut self) {
        self.stream.take();
        self.stop.store(true, Ordering::Release);
        if let Some(writer) = self.writer.take() {
            let _ = writer.join();
        }
        self.remove_file();
    }

    fn remove_file(&self) {
        if !self.path.as_os_str().is_empty() {
            let _ = fs::remove_file(&self.path);
        }
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        self.cancel_inner();
    }
}

fn build_stream(
    device: &cpal::Device,
    config: &StreamConfig,
    format: SampleFormat,
    channels: usize,
    producer: HeapProd<i16>,
    signals: &CaptureSignals,
) -> Result<Stream, AudioError> {
    let error_slot = Arc::clone(&signals.stream_error);
    let error_callback = move |error: cpal::Error| {
        if let Ok(mut slot) = error_slot.lock()
            && slot.is_none()
        {
            *slot = Some(error.to_string());
        }
    };
    macro_rules! typed_stream {
        ($sample:ty, $convert:expr) => {{
            let mut producer = producer;
            let overflow = Arc::clone(&signals.overflow);
            let length_limit = Arc::clone(&signals.length_limit);
            let samples = Arc::clone(&signals.samples);
            device.build_input_stream(
                config.clone(),
                move |data: &[$sample], _| {
                    push_frames(
                        data,
                        channels,
                        &mut producer,
                        &overflow,
                        &length_limit,
                        &samples,
                        $convert,
                    );
                },
                error_callback,
                Some(Duration::from_secs(5)),
            )
        }};
    }
    let result = match format {
        SampleFormat::F32 => typed_stream!(f32, f32_to_i16),
        SampleFormat::I16 => typed_stream!(i16, |sample| sample),
        SampleFormat::U16 => typed_stream!(u16, u16_to_i16),
        other => return Err(AudioError::UnsupportedSampleFormat(other.to_string())),
    };
    result.map_err(|error| AudioError::BuildStream(error.to_string()))
}

fn push_frames<T: Copy>(
    data: &[T],
    channels: usize,
    producer: &mut HeapProd<i16>,
    overflow: &AtomicBool,
    length_limit: &AtomicBool,
    samples: &AtomicU64,
    convert: impl Fn(T) -> i16,
) {
    for frame in data.chunks_exact(channels) {
        let next = samples.fetch_add(1, Ordering::AcqRel);
        if next >= MAX_MONO_SAMPLES {
            length_limit.store(true, Ordering::Release);
            continue;
        }
        let sum = frame
            .iter()
            .map(|sample| i64::from(convert(*sample)))
            .sum::<i64>();
        let mono = sum / i64::try_from(frame.len()).unwrap_or(1);
        let mono = i16::try_from(mono).unwrap_or(if mono.is_negative() {
            i16::MIN
        } else {
            i16::MAX
        });
        if producer.try_push(mono).is_err() {
            overflow.store(true, Ordering::Release);
        }
    }
}

fn f32_to_i16(sample: f32) -> i16 {
    i16::from_sample(sample.clamp(-1.0, 1.0))
}

fn u16_to_i16(sample: u16) -> i16 {
    i16::from_sample(sample)
}

fn create_wav_writer(
    path: &Path,
    sample_rate: u32,
) -> Result<hound::WavWriter<io::BufWriter<fs::File>>, AudioError> {
    let specification = hound::WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    hound::WavWriter::create(path, specification)
        .map_err(|error| AudioError::CreateRecording(io::Error::other(error)))
}

fn write_consumer(
    mut writer: hound::WavWriter<io::BufWriter<fs::File>>,
    mut consumer: ringbuf::HeapCons<i16>,
    stop: &AtomicBool,
) -> Result<(), String> {
    loop {
        let mut wrote = false;
        while let Some(sample) = consumer.try_pop() {
            writer
                .write_sample(sample)
                .map_err(|error| error.to_string())?;
            wrote = true;
        }
        if stop.load(Ordering::Acquire) && consumer.is_empty() {
            break;
        }
        if !wrote {
            thread::park_timeout(Duration::from_millis(2));
        }
    }
    writer.finalize().map_err(|error| error.to_string())
}

/// A finalized WAV that removes itself unless ownership is consumed by transcription.
pub struct RecordedAudio {
    path: PathBuf,
    sample_rate: u32,
    samples: u64,
}

impl RecordedAudio {
    pub fn duration(&self) -> Duration {
        let samples = u32::try_from(self.samples).unwrap_or(u32::MAX);
        Duration::from_secs_f64(f64::from(samples) / f64::from(self.sample_rate))
    }
}

impl Drop for RecordedAudio {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Groq transcription settings. The API key is deliberately opaque to callers.
pub struct TranscriptionConfig {
    api_key: Zeroizing<String>,
    endpoint: String,
    model: String,
}

impl TranscriptionConfig {
    /// Reads the existing Groq credential and optional model override.
    ///
    /// # Errors
    ///
    /// Returns an actionable capability error when no key is configured.
    pub fn from_environment() -> Result<Self, AudioError> {
        let api_key = env::var("GROQ_API_KEY")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .ok_or(AudioError::MissingApiKey)?;
        Self::from_api_key(api_key)
    }

    /// Builds settings from a key retrieved from the operating-system vault.
    /// The key is zeroed when the transcription job releases its configuration.
    ///
    /// # Errors
    ///
    /// Rejects an empty key before any recording or network request begins.
    pub fn from_api_key(api_key: String) -> Result<Self, AudioError> {
        if api_key.trim().is_empty() {
            return Err(AudioError::MissingApiKey);
        }
        let model = env::var("STT_GROQ_MODEL")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .or_else(|| {
                env::var("GROQ_WHISPER_MODEL")
                    .ok()
                    .filter(|value| !value.trim().is_empty())
            })
            .unwrap_or_else(|| DEFAULT_MODEL.into());
        Ok(Self {
            api_key: Zeroizing::new(api_key),
            endpoint: DEFAULT_ENDPOINT.into(),
            model,
        })
    }
}

pub struct TranscriptionHandle {
    receiver: Receiver<Result<String, AudioError>>,
    cancelled: Arc<AtomicBool>,
}

impl TranscriptionHandle {
    /// Starts a bounded background upload. The recording is always deleted.
    ///
    /// # Errors
    ///
    /// Returns only when the worker thread itself cannot be started.
    pub fn start(
        recording: RecordedAudio,
        config: TranscriptionConfig,
    ) -> Result<Self, AudioError> {
        let (sender, receiver) = mpsc::channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = Arc::clone(&cancelled);
        thread::Builder::new()
            .name("voice-transcription".into())
            .spawn(move || {
                let result = transcribe(&recording, &config, &worker_cancelled);
                drop(recording);
                let _ = sender.send(result);
            })
            .map_err(AudioError::StartTranscriber)?;
        Ok(Self {
            receiver,
            cancelled,
        })
    }

    /// Receives a finished transcript without blocking the render thread.
    ///
    /// # Errors
    ///
    /// Returns transcription, service, cancellation, and worker failures.
    pub fn try_recv(&self) -> Result<Option<String>, AudioError> {
        match self.receiver.try_recv() {
            Ok(result) => result.map(Some),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => Err(AudioError::InvalidResponse(
                "transcription worker ended without a result".into(),
            )),
        }
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }
}

impl Drop for TranscriptionHandle {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[derive(Deserialize)]
struct Transcript {
    text: String,
}

fn transcribe(
    recording: &RecordedAudio,
    config: &TranscriptionConfig,
    cancelled: &AtomicBool,
) -> Result<String, AudioError> {
    if cancelled.load(Ordering::Acquire) {
        return Err(AudioError::Cancelled);
    }
    let metadata = fs::metadata(&recording.path).map_err(AudioError::CreateRecording)?;
    if metadata.len() > MAX_WAV_BYTES {
        return Err(AudioError::RecordingTooLarge);
    }
    let audio = fs::read(&recording.path).map_err(AudioError::CreateRecording)?;
    let form = reqwest::blocking::multipart::Form::new()
        .text("model", config.model.clone())
        .part(
            "file",
            reqwest::blocking::multipart::Part::bytes(audio)
                .file_name("research-recording.wav")
                .mime_str("audio/wav")
                .map_err(|error| AudioError::BuildClient(error.to_string()))?,
        );
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(TRANSCRIPTION_TIMEOUT)
        .build()
        .map_err(|error| AudioError::BuildClient(error.to_string()))?;
    let response = client
        .post(&config.endpoint)
        .bearer_auth(config.api_key.as_str())
        .header(reqwest::header::ACCEPT, "application/json")
        .multipart(form)
        .send()
        .map_err(|error| AudioError::Request(error.to_string()))?;
    if cancelled.load(Ordering::Acquire) {
        return Err(AudioError::Cancelled);
    }
    let status = response.status();
    let limit = if status.is_success() {
        MAX_RESPONSE_BYTES
    } else {
        MAX_ERROR_BYTES
    };
    let mut body = String::new();
    response
        .take(limit)
        .read_to_string(&mut body)
        .map_err(|error| AudioError::InvalidResponse(error.to_string()))?;
    if !status.is_success() {
        return Err(AudioError::Service {
            status: status.as_u16(),
            detail: compact_service_error(&body),
        });
    }
    let transcript: Transcript = serde_json::from_str(&body)
        .map_err(|error| AudioError::InvalidResponse(error.to_string()))?;
    let text = transcript.text.trim();
    if text.is_empty() {
        return Err(AudioError::InvalidResponse(
            "transcript text was empty".into(),
        ));
    }
    Ok(text.to_owned())
}

fn compact_service_error(body: &str) -> String {
    body.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read, Write},
        net::TcpListener,
    };

    use tempfile::TempDir;

    use super::*;

    #[test]
    fn interleaved_stereo_is_mixed_to_mono_without_overwrite() {
        let ring = HeapRb::<i16>::new(3);
        let (mut producer, mut consumer) = ring.split();
        let overflow = AtomicBool::new(false);
        let length = AtomicBool::new(false);
        let samples = AtomicU64::new(0);
        push_frames(
            &[10_i16, 30, -30, 10, 100, -100],
            2,
            &mut producer,
            &overflow,
            &length,
            &samples,
            |sample| sample,
        );
        assert_eq!(consumer.try_pop(), Some(20));
        assert_eq!(consumer.try_pop(), Some(-10));
        assert_eq!(consumer.try_pop(), Some(0));
        assert!(!overflow.load(Ordering::Acquire));
        assert_eq!(samples.load(Ordering::Acquire), 3);
    }

    #[test]
    fn full_capture_queue_fails_instead_of_overwriting_earlier_speech() {
        let ring = HeapRb::<i16>::new(1);
        let (mut producer, mut consumer) = ring.split();
        let overflow = AtomicBool::new(false);
        push_frames(
            &[1_i16, 2],
            1,
            &mut producer,
            &overflow,
            &AtomicBool::new(false),
            &AtomicU64::new(0),
            |sample| sample,
        );
        assert_eq!(consumer.try_pop(), Some(1));
        assert!(overflow.load(Ordering::Acquire));
    }

    #[test]
    fn authenticated_multipart_transcription_is_bounded_and_deletes_the_wav() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/audio", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut request = Vec::new();
            let mut chunk = [0_u8; 4_096];
            let header_end;
            loop {
                let count = socket.read(&mut chunk).unwrap();
                request.extend_from_slice(&chunk[..count]);
                if let Some(index) = request.windows(4).position(|part| part == b"\r\n\r\n") {
                    header_end = index + 4;
                    break;
                }
            }
            let headers = String::from_utf8_lossy(&request[..header_end]).to_string();
            let content_length = headers
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length: ")
                        .and_then(|value| value.trim().parse::<usize>().ok())
                })
                .unwrap();
            while request.len() - header_end < content_length {
                let count = socket.read(&mut chunk).unwrap();
                request.extend_from_slice(&chunk[..count]);
            }
            assert!(
                headers
                    .to_ascii_lowercase()
                    .contains("authorization: bearer test-secret")
            );
            let body = &request[header_end..header_end + content_length];
            assert!(
                body.windows(DEFAULT_MODEL.len())
                    .any(|part| part == DEFAULT_MODEL.as_bytes())
            );
            assert!(body.windows(4).any(|part| part == b"RIFF"));
            let response = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 27\r\nConnection: close\r\n\r\n{\"text\":\"investigate this\"}";
            socket.write_all(response).unwrap();
        });

        let directory = TempDir::new().unwrap();
        let path = directory.path().join("voice.wav");
        let specification = hound::WavSpec {
            channels: 1,
            sample_rate: 16_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(&path, specification).unwrap();
        writer.write_sample(123_i16).unwrap();
        writer.finalize().unwrap();
        let recording = RecordedAudio {
            path: path.clone(),
            sample_rate: 16_000,
            samples: 1,
        };
        let handle = TranscriptionHandle::start(
            recording,
            TranscriptionConfig {
                api_key: String::from("test-secret").into(),
                endpoint,
                model: DEFAULT_MODEL.into(),
            },
        )
        .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        let transcript = loop {
            if let Some(transcript) = handle.try_recv().unwrap() {
                break transcript;
            }
            assert!(std::time::Instant::now() < deadline);
            thread::sleep(Duration::from_millis(5));
        };
        assert_eq!(transcript, "investigate this");
        assert!(!path.exists());
        server.join().unwrap();
    }

    #[test]
    fn api_key_never_appears_in_configuration_debug_or_capability_errors() {
        assert!(!std::any::type_name::<TranscriptionConfig>().contains("secret"));
        assert_eq!(
            AudioError::MissingApiKey.to_string(),
            "GROQ_API_KEY is not configured; voice transcription is unavailable"
        );
    }
}
