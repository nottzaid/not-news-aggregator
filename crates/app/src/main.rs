mod hermes_profile;
mod interaction;
mod release_check;
mod settings;

use std::{
    collections::{HashSet, VecDeque},
    error::Error,
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::{self, Read as _},
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, TryRecvError},
    time::Duration,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use fs2::FileExt;
use interaction::{CanvasInteraction, CanvasSubject, InteractionEffect, InteractionFrame};
use not_news_agent::{
    AgentEvent, BridgeUpsert, ResearchHandle, ResearchLaunch, ResearchProcessEvent,
    ResearchTermination, ResolvedEnvironment, browse_is_available, build_research_prompt,
    check_hermes_compatibility, check_tool_capability, curl_is_available, hermes_is_available,
    open_hermes_dashboard,
};
use not_news_audio::{
    Recorder, SpeechCapability, SpeechEvent, SpeechSubmit, SpeechWorker, TranscriptionConfig,
    TranscriptionHandle, default_input_capability,
};
use not_news_domain::{
    BridgeId, DetachRelationship, EventId, GraphSnapshot, Point, PromoteArtifact,
    PromotionRelation, Provenance, RelateEvents,
};
use not_news_platform::{
    FrameInfo, FrameMeasurement, FrameSchedule, PlatformApplication, WindowOptions,
    application_data_directory, hermes_root_directory, open_external_url, run,
    skia_safe::Canvas,
    winit::{
        dpi::PhysicalPosition,
        event::{ElementState, Ime, MouseButton, MouseScrollDelta, WindowEvent},
        keyboard::{Key, ModifiersState, NamedKey},
    },
};
use unicode_segmentation::UnicodeSegmentation;
use zeroize::{Zeroize, Zeroizing};

use settings::{
    CredentialState, EndpointState, browserbase_api_key, browserbase_credential_state,
    delete_browserbase_api_key, delete_exa_api_key, delete_groq_api_key, delete_searxng_url,
    exa_api_key, exa_credential_state, groq_api_key, groq_credential_state,
    retire_backend_selector, save_browserbase_api_key, save_exa_api_key, save_groq_api_key,
    save_searxng_url, searxng_endpoint_state, searxng_url,
};

const MAX_RESEARCH_CHARS: usize = 4_096;
const MAX_RESEARCH_BYTES: usize = 16_384;
const MAX_PREDICATE_CHARS: usize = 96;
const MAX_PREDICATE_BYTES: usize = 384;
const MAX_CREDENTIAL_CHARS: usize = 4_096;
const MAX_CREDENTIAL_BYTES: usize = 16_384;
const MAX_SEARXNG_CONFIG_BYTES: u64 = 1_048_576;
const CURATION_RELATIONSHIPS_PER_PAGE: usize = 7;
const HERMES_INSTALL_URL: &str = "https://hermes-agent.nousresearch.com";
const BROWSE_INSTALL_URL: &str = "https://browse.sh";
const CURL_INSTALL_URL: &str = "https://curl.se/download.html";
const MANROPE_LICENSE: &str = include_str!("../../../assets/fonts/manrope/OFL.txt");
const JETBRAINS_MONO_LICENSE: &str = include_str!("../../../assets/fonts/jetbrainsmono/OFL.txt");
use not_news_renderer::{
    ChromeControl, CurationMenu, Motion, RecordOrbState, SceneAnimation, SceneState,
    ViewportTransform, active_metadata_scroll_max, activity_scroll_max,
    curation_menu_reveal_offset, curation_menu_scroll_max, hit_active_metadata,
    hit_activity_surface, hit_activity_toggle, hit_curation_menu, hit_curation_menu_surface,
    hit_fixed_chrome, paint_active_metadata, paint_activity_drawer, paint_background,
    paint_connection_prompt, paint_credential_prompt, paint_curation_menu, paint_curation_prompt,
    paint_fixed_chrome, paint_graph, paint_grid, paint_research_prompt, paint_status,
    resolved_positions,
};
use not_news_store::{
    CommitOutcome, DurableGraphStore, LegacyGraphReader, ResearchOutputKind, ResearchSessionStatus,
    StoreError,
};

const PERFORMANCE_WARMUP_FRAMES: usize = 60;
const PERFORMANCE_MEASURED_FRAMES: usize = 600;
const PERFORMANCE_P99_LIMIT_MICROS: u128 = 16_667;

struct ActiveResearch {
    session_id: String,
    handle: ResearchHandle,
    next_sequence: u64,
    deferred_bridges: VecDeque<BridgeUpsert>,
    closed: bool,
    scratch_directory: PathBuf,
}

struct ResearchPreflight {
    question: String,
    receiver: Receiver<Result<PreparedResearch, String>>,
}

struct PreparedResearch {
    environment: ResolvedEnvironment,
    evidence: String,
}

struct ApplicationLease {
    file: Option<File>,
    path: PathBuf,
    remove_on_drop: bool,
}

impl ApplicationLease {
    fn acquire(data_directory: &Path) -> io::Result<Self> {
        let parent = data_directory.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)?;
        let name = data_directory
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("not-news-canvas");
        let path = parent.join(format!(".{name}.not-news-instance.lock"));
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let file = options.open(&path)?;
        file.lock_shared()?;
        Ok(Self {
            file: Some(file),
            path,
            remove_on_drop: false,
        })
    }

    fn require_exclusive(&mut self) -> io::Result<()> {
        let file = self.file.as_ref().expect("application lease missing");
        FileExt::unlock(file)?;
        if let Err(error) = file.try_lock_exclusive() {
            file.lock_shared()?;
            return Err(io::Error::new(
                error.kind(),
                "another Not News instance is using this application state",
            ));
        }
        Ok(())
    }

    fn remove_after_exit(&mut self) {
        self.remove_on_drop = true;
    }

    fn restore_shared(&self) -> io::Result<()> {
        let file = self.file.as_ref().expect("application lease missing");
        FileExt::unlock(file)?;
        file.lock_shared()
    }
}

impl Drop for ApplicationLease {
    fn drop(&mut self) {
        if let Some(file) = self.file.take() {
            let _ = FileExt::unlock(&file);
            drop(file);
        }
        if self.remove_on_drop {
            let _ = fs::remove_file(&self.path);
        }
    }
}

fn research_environment(data_directory: &Path) -> Result<ResolvedEnvironment, String> {
    let exa = exa_api_key()
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "EXA_API_KEY is required; store it in Connections (Ctrl+,)".to_owned())?;
    if exa.len() < 8 {
        return Err("The stored Exa key is too short to use or redact safely; replace it in Connections (Ctrl+,)".into());
    }
    let searxng = searxng_url(data_directory)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| {
            "SEARXNG_URL is required; configure it in Connections (Ctrl+,)".to_owned()
        })?;
    validate_searxng_endpoint(&searxng)?;
    let search_url = format!("{searxng}/search");
    let mut secrets = vec![exa.clone()];
    let mut environment = vec![
        (OsString::from("EXA_API_KEY"), exa.into()),
        (OsString::from("SEARXNG_URL"), searxng.clone().into()),
        (OsString::from("AI_NEWS_SEARXNG_URL"), searxng.into()),
        (
            OsString::from("AI_NEWS_SEARXNG_SEARCH_URL"),
            search_url.into(),
        ),
    ];
    if let Some(browserbase) = browserbase_api_key().map_err(|error| error.to_string())? {
        if browserbase.len() < 8 {
            return Err(
                "The stored Browserbase key is too short to use or redact safely; replace or remove it in Connections (Ctrl+,)"
                    .into(),
            );
        }
        secrets.push(browserbase.clone());
        environment.push((OsString::from("BROWSERBASE_API_KEY"), browserbase.into()));
    }
    Ok(ResolvedEnvironment::new(environment, secrets))
}

fn prepare_research(
    data_directory: &Path,
    profile: &hermes_profile::InstalledProfile,
) -> Result<PreparedResearch, String> {
    let compatibility = check_hermes_compatibility(
        &profile.root,
        hermes_profile::PROFILE_ID,
        profile.policy_version,
    )
    .map_err(|error| format!("Hermes compatibility is unproved: {error}"))?;
    check_tool_capability("browse", &["--version"])
        .map_err(|error| format!("Browse capability is unavailable: {error}"))?;
    check_tool_capability("curl", &["--version"])
        .map_err(|error| format!("SearXNG transport is unavailable: {error}"))?;
    let environment = research_environment(data_directory)?;
    Ok(PreparedResearch {
        environment,
        evidence: format!(
            "{}; Browse and curl version commands passed; Exa was resolved from the Not News vault; SearXNG identity, an enabled engine, and JSON search capability passed without dispatching a search. Responsive upstream engines, provider authentication, Browse browser launch, and useful live results remain task evidence.",
            compatibility.evidence
        ),
    })
}

fn validate_searxng_endpoint(searxng: &str) -> Result<(), String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|error| format!("SearXNG validation client failed: {error}"))?;
    let configuration = check_searxng_configuration(&client, searxng);
    let json_capability = check_searxng_json_capability(&client, searxng);
    match (configuration, json_capability) {
        // Deployments may hide /config behind a proxy; a queryless JSON probe
        // that SearXNG answers is sufficient identity and capability evidence.
        (_, Ok(())) => Ok(()),
        (Ok(()), Err(json_error)) => Err(json_error),
        (Err(configuration_error), Err(json_error)) => {
            Err(format!("{configuration_error}; {json_error}"))
        }
    }
}

/// Proves `SearXNG` identity and an enabled engine through `/config`, which
/// dispatches no search.
fn check_searxng_configuration(
    client: &reqwest::blocking::Client,
    searxng: &str,
) -> Result<(), String> {
    let response = client
        .get(format!("{searxng}/config"))
        .send()
        .map_err(|error| format!("SearXNG endpoint is unreachable: {error}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "SearXNG configuration endpoint returned HTTP {}; correct it in Connections (Ctrl+,)",
            response.status()
        ));
    }
    let mut body = Vec::new();
    response
        .take(MAX_SEARXNG_CONFIG_BYTES + 1)
        .read_to_end(&mut body)
        .map_err(|error| format!("SearXNG configuration response could not be read: {error}"))?;
    if body.len() as u64 > MAX_SEARXNG_CONFIG_BYTES {
        return Err("SearXNG configuration response exceeded the 1 MiB readiness limit".into());
    }
    let configuration: serde_json::Value = serde_json::from_slice(&body)
        .map_err(|error| format!("SearXNG configuration endpoint did not return JSON: {error}"))?;
    let identifies_searxng = configuration
        .get("instance_name")
        .is_some_and(serde_json::Value::is_string);
    let has_enabled_engine = configuration
        .get("engines")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|engines| {
            engines.iter().any(|engine| {
                engine
                    .get("enabled")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false)
                    && engine
                        .get("name")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|name| !name.is_empty())
            })
        });
    if !identifies_searxng || !has_enabled_engine {
        return Err(
            "SearXNG configuration response did not identify an instance with an enabled engine"
                .into(),
        );
    }
    Ok(())
}

/// Proves the JSON output format is enabled without dispatching a search:
/// `SearXNG` rejects a disallowed format with 403 before checking for a query,
/// and answers a queryless allowed-format request with 400 "No query".
fn check_searxng_json_capability(
    client: &reqwest::blocking::Client,
    searxng: &str,
) -> Result<(), String> {
    let response = client
        .get(format!("{searxng}/search"))
        .query(&[("format", "json")])
        .send()
        .map_err(|error| format!("SearXNG search endpoint is unreachable: {error}"))?;
    let status = response.status();
    if status == reqwest::StatusCode::FORBIDDEN {
        return Err(
            "SearXNG JSON search is disabled; add `- json` to `search.formats` in its settings.yml"
                .into(),
        );
    }
    if status.is_success() || status == reqwest::StatusCode::BAD_REQUEST {
        return Ok(());
    }
    Err(format!(
        "SearXNG JSON capability probe returned HTTP {status}; correct it in Connections (Ctrl+,)"
    ))
}

enum VoiceState {
    Idle,
    Recording(Recorder),
    Transcribing(TranscriptionHandle),
}

#[derive(Clone, Copy)]
enum CurationMenuStage {
    Actions,
    Detach,
}

#[derive(Clone)]
enum CurationFlow {
    Menu {
        anchor: Point,
        subject: CanvasSubject,
        stage: CurationMenuStage,
        page: usize,
        expected_revision: u64,
    },
    RelateTarget {
        from: EventId,
        expected_revision: u64,
    },
    RelatePredicate {
        from: EventId,
        to: EventId,
        expected_revision: u64,
        input: String,
    },
    PromotePredicate {
        source: EventId,
        artifact_index: usize,
        expected_revision: u64,
        input: String,
    },
}

#[derive(Clone)]
enum CurationChoice {
    Relate,
    Detach,
    Promote,
    Bridge(BridgeId),
    PreviousPage,
    NextPage,
}

enum SettingsFlow {
    Menu {
        browserbase: CredentialMenuState,
        exa: CredentialMenuState,
        groq: CredentialMenuState,
        searxng: EndpointState,
        hermes_available: bool,
        browse_available: bool,
        curl_available: bool,
        page: SettingsPage,
        selected: usize,
    },
    Credential {
        kind: CredentialKind,
        input: Zeroizing<String>,
    },
    SearxngEndpoint {
        input: String,
    },
    EraseConfirmation {
        input: String,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SettingsPage {
    Root,
    Credential(CredentialKind),
    Searxng,
}

impl SettingsPage {
    fn parent(self) -> Option<Self> {
        match self {
            Self::Root => None,
            Self::Credential(_) | Self::Searxng => Some(Self::Root),
        }
    }
}

#[derive(Clone, Copy)]
enum SettingsNavigation {
    Backward,
    Forward,
}

#[derive(Clone)]
enum CredentialMenuState {
    Resolving,
    Ready(CredentialState),
    Saving,
    Removing,
}

impl CredentialMenuState {
    fn label(&self) -> &'static str {
        match self {
            Self::Resolving => "CHECKING OS VAULT",
            Self::Ready(state) => state.label(),
            Self::Saving => "STORING IN OS VAULT",
            Self::Removing => "REMOVING FROM OS VAULT",
        }
    }
}

enum VaultTaskResult {
    Resolved {
        browserbase: CredentialState,
        exa: CredentialState,
        groq: CredentialState,
    },
    Saved {
        kind: CredentialKind,
        result: Result<(), String>,
    },
    Removed {
        kind: CredentialKind,
        result: Result<(), String>,
    },
    EraseCredentials {
        failures: Vec<String>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CredentialKind {
    Browserbase,
    Exa,
    Groq,
}

impl CredentialKind {
    fn title(self) -> &'static str {
        match self {
            Self::Browserbase => "BROWSERBASE CLOUD",
            Self::Exa => "EXA DISCOVERY",
            Self::Groq => "GROQ TRANSCRIPTION",
        }
    }

    fn placeholder(self) -> &'static str {
        match self {
            Self::Browserbase => "Paste or type the Browserbase API key…",
            Self::Exa => "Paste or type the Exa API key…",
            Self::Groq => "Paste or type the Groq API key…",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Browserbase => "Browserbase",
            Self::Exa => "Exa",
            Self::Groq => "Groq",
        }
    }

    fn menu_label(self) -> &'static str {
        match self {
            Self::Browserbase => "BROWSERBASE",
            Self::Exa => "EXA",
            Self::Groq => "GROQ",
        }
    }

    fn removal_consequence(self) -> &'static str {
        match self {
            Self::Browserbase => "CLOUD BROWSING DISABLED; LOCAL BROWSE REMAINS",
            Self::Exa => "RESEARCH DISCOVERY BECOMES INCOMPLETE",
            Self::Groq => "TRANSCRIPTION DISABLED",
        }
    }
}

#[derive(Clone, Copy)]
enum SettingsChoice {
    Hermes,
    Browse,
    Curl,
    OpenSearxng,
    EditSearxng,
    OpenCredential(CredentialKind),
    EditCredential(CredentialKind),
    RemoveCredential(CredentialKind),
    RemoveSearxng,
    EraseAll,
}

impl VoiceState {
    fn is_idle(&self) -> bool {
        matches!(self, Self::Idle)
    }

    fn is_recording(&self) -> bool {
        matches!(self, Self::Recording(_))
    }
}

#[derive(Clone, Copy)]
struct ActivityMotion {
    from: f64,
    to: f64,
    started: Instant,
}

#[derive(Clone, Copy, Default)]
enum PointerOwner {
    #[default]
    None,
    Canvas,
    FixedChrome(ChromeControl),
    ActivityToggle,
    ActivitySurface,
    MetadataSurface,
    ConsumedChrome,
}

struct CanvasApplication {
    store: Option<DurableGraphStore>,
    graph: GraphSnapshot,
    interaction: CanvasInteraction,
    status: Option<String>,
    modifiers: ModifiersState,
    operation_epoch: u128,
    operation_sequence: u64,
    cursor: Option<Point>,
    physical_width: f64,
    physical_height: f64,
    scale_factor: f64,
    pointer_owner: PointerOwner,
    application_lease: Option<ApplicationLease>,
    data_directory: PathBuf,
    research_directory: PathBuf,
    voice_directory: PathBuf,
    voice: VoiceState,
    speech: SpeechWorker,
    research_input: Option<String>,
    research_preedit: String,
    curation: Option<CurationFlow>,
    curation_preedit: String,
    hermes_profile: Option<hermes_profile::InstalledProfile>,
    hermes_root: Option<PathBuf>,
    settings: Option<SettingsFlow>,
    settings_preedit: Zeroizing<String>,
    vault_task: Option<Receiver<VaultTaskResult>>,
    research_preflight: Option<ResearchPreflight>,
    research: Option<ActiveResearch>,
    research_messages: Vec<String>,
    generated_research_events: HashSet<not_news_domain::EventId>,
    auto_follow_research: bool,
    activity_open: bool,
    activity_openness: f64,
    activity_motion: Option<ActivityMotion>,
    activity_scroll: f64,
    record_hold_deadline: Option<Instant>,
    metadata_scroll_event: Option<EventId>,
    metadata_scroll: f64,
    metadata_focus: Option<EventId>,
    curation_scroll: f64,
    settings_scroll: f64,
    exit_after_present: bool,
}

impl CanvasApplication {
    fn load(database: &Path) -> Self {
        let data_directory = database.parent();
        let isolated_hermes_root = data_directory.map(|directory| directory.join("hermes-root"));
        Self::load_with_directories(database, data_directory, isolated_hermes_root.as_deref())
    }

    fn load_with_directories(
        database: &Path,
        data_directory: Option<&Path>,
        hermes_root: Option<&Path>,
    ) -> Self {
        let application_lease = match data_directory.map(ApplicationLease::acquire).transpose() {
            Ok(lease) => lease,
            Err(error) => {
                return Self::unavailable(format!(
                    "Application state could not be leased safely; writes are disabled: {error}"
                ));
            }
        };
        match DurableGraphStore::open(database) {
            Ok(store) => {
                let recovery = store.recover_interrupted_research();
                let graph = match store.load() {
                    Ok(graph) => graph,
                    Err(error) => {
                        return Self::unavailable(format!(
                            "Graph became unreadable during startup; writes are disabled. {}: {error}",
                            database.display()
                        ));
                    }
                };
                let mut status = store.migration_backup().map(|backup| {
                    format!(
                        "Legacy graph migrated after verified backup: {}",
                        backup.display()
                    )
                });
                let recovered_activity = recovery
                    .as_ref()
                    .ok()
                    .and_then(|sessions| sessions.last())
                    .and_then(|session| store.research_activity(&session.id).ok())
                    .unwrap_or_default();
                match &recovery {
                    Ok(recovered) if !recovered.is_empty() => {
                        status = Some(format!(
                            "Recovered {} interrupted research session{}; accepted findings were preserved.",
                            recovered.len(),
                            if recovered.len() == 1 { "" } else { "s" }
                        ));
                    }
                    Err(error) => {
                        status = Some(format!(
                            "Saved graph opened, but research recovery failed: {error}"
                        ));
                    }
                    _ => {}
                }
                if status.is_none() && graph.events.is_empty() {
                    status = Some(
                        "Canvas ready offline. Ctrl+, opens Connections; Ctrl+K researches; right-click curates; dropping a legacy graph.sqlite imports it."
                            .into(),
                    );
                }
                let mut application = Self::with_state(
                    Some(store),
                    graph,
                    status,
                    data_directory,
                    hermes_root,
                    application_lease,
                );
                application.research_messages = recovered_activity;
                application
            }
            Err(error) => Self::unavailable(format!(
                "Graph unavailable; no research is shown or writable. {}: {error}",
                database.display()
            )),
        }
    }

    fn unavailable(status: String) -> Self {
        Self::with_state(
            None,
            GraphSnapshot::default(),
            Some(status),
            None,
            None,
            None,
        )
    }

    fn with_state(
        store: Option<DurableGraphStore>,
        graph: GraphSnapshot,
        status: Option<String>,
        data_directory: Option<&Path>,
        hermes_root: Option<&Path>,
        application_lease: Option<ApplicationLease>,
    ) -> Self {
        let interaction = CanvasInteraction::new(resolved_positions(&graph));
        let profile_install = data_directory.zip(hermes_root).map(|(directory, root)| {
            hermes_profile::install(root, Some(&directory.join("hermes")))
        });
        let retired_setting = data_directory.map(retire_backend_selector);
        let data_directory = data_directory.unwrap_or_else(|| Path::new("."));
        let voice_directory = data_directory.join("voice-scratch");
        let speech = SpeechWorker::from_environment(voice_directory.join("synthesis"));
        let (hermes_profile, profile_error) = match profile_install {
            Some(Ok(home)) => (Some(home), None),
            Some(Err(error)) => (
                None,
                Some(format!("Hermes profile could not be installed: {error}")),
            ),
            None => (None, None),
        };
        Self {
            store,
            graph,
            interaction,
            status: profile_error.or(status).or_else(|| {
                retired_setting.and_then(Result::err).map(|error| {
                    format!("Retired research-backend setting could not be removed: {error}")
                })
            }),
            modifiers: ModifiersState::default(),
            operation_epoch: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
            operation_sequence: 0,
            cursor: None,
            physical_width: 1_280.0,
            physical_height: 800.0,
            scale_factor: 1.0,
            pointer_owner: PointerOwner::None,
            application_lease,
            data_directory: data_directory.to_path_buf(),
            research_directory: data_directory.join("research-scratch"),
            voice_directory,
            voice: VoiceState::Idle,
            speech,
            research_input: None,
            research_preedit: String::new(),
            curation: None,
            curation_preedit: String::new(),
            hermes_profile,
            hermes_root: hermes_root.map(Path::to_path_buf),
            settings: None,
            settings_preedit: Zeroizing::new(String::new()),
            vault_task: None,
            research_preflight: None,
            research: None,
            research_messages: Vec::new(),
            generated_research_events: HashSet::new(),
            auto_follow_research: true,
            activity_open: false,
            activity_openness: 0.0,
            activity_motion: None,
            activity_scroll: 0.0,
            record_hold_deadline: None,
            metadata_scroll_event: None,
            metadata_scroll: 0.0,
            metadata_focus: None,
            curation_scroll: 0.0,
            settings_scroll: 0.0,
            exit_after_present: false,
        }
    }

    fn apply_outcome(&mut self, outcome: CommitOutcome) {
        let semantic_change = self.graph.events != outcome.snapshot.events
            || self.graph.bridges != outcome.snapshot.bridges
            || self.graph.aliases != outcome.snapshot.aliases;
        let previous = std::mem::replace(&mut self.graph, outcome.snapshot);
        if semantic_change {
            self.interaction
                .graph_committed(&previous, &self.graph, Instant::now());
        } else {
            self.interaction.placement_committed(&self.graph);
        }
    }

    fn apply_semantic_outcome(&mut self, outcome: CommitOutcome, now: Instant) {
        let previous = std::mem::replace(&mut self.graph, outcome.snapshot);
        self.interaction
            .graph_committed(&previous, &self.graph, now);
    }

    #[allow(clippy::too_many_lines)]
    fn settings_choices(&self) -> Vec<(SettingsChoice, String)> {
        let Some(SettingsFlow::Menu {
            browserbase,
            exa,
            groq,
            searxng,
            hermes_available,
            browse_available,
            curl_available,
            page,
            ..
        }) = self.settings.as_ref()
        else {
            return Vec::new();
        };
        match page {
            SettingsPage::Root => vec![
                (
                    SettingsChoice::Hermes,
                    if self.hermes_profile.is_none() {
                        "HERMES PROFILE  ·  INSTALLATION FAILED".into()
                    } else if *hermes_available {
                        "HERMES  ·  EXECUTABLE PRESENT  ·  ACP CHECK ON RESEARCH".into()
                    } else {
                        "HERMES  ·  MISSING  ·  OPEN INSTALL GUIDE".into()
                    },
                ),
                (
                    SettingsChoice::OpenCredential(CredentialKind::Exa),
                    format!("EXA DISCOVERY  ·  {}", exa.label()),
                ),
                (
                    SettingsChoice::OpenSearxng,
                    format!("SEARXNG FRONTIER  ·  {}", searxng.label()),
                ),
                (
                    SettingsChoice::OpenCredential(CredentialKind::Groq),
                    format!("GROQ VOICE  ·  {}", groq.label()),
                ),
                (
                    SettingsChoice::OpenCredential(CredentialKind::Browserbase),
                    format!("BROWSERBASE CLOUD  ·  {}", browserbase.label()),
                ),
                (
                    SettingsChoice::Browse,
                    if *browse_available {
                        "BROWSE  ·  EXECUTABLE PRESENT  ·  BROWSER UNVERIFIED".into()
                    } else {
                        "BROWSE  ·  MISSING  ·  OPEN INSTALL GUIDE".into()
                    },
                ),
                (
                    SettingsChoice::Curl,
                    if *curl_available {
                        "CURL  ·  EXECUTABLE PRESENT  ·  VERSION CHECK ON RESEARCH".into()
                    } else {
                        "CURL  ·  MISSING  ·  OPEN INSTALL GUIDE".into()
                    },
                ),
                (
                    SettingsChoice::EraseAll,
                    "COMPLETE ERASE  ·  GRAPH, SETTINGS, VAULT, OWNED PROFILE".into(),
                ),
            ],
            SettingsPage::Credential(kind) => {
                let state = match kind {
                    CredentialKind::Browserbase => browserbase,
                    CredentialKind::Exa => exa,
                    CredentialKind::Groq => groq,
                };
                let mut choices = vec![(
                    SettingsChoice::EditCredential(*kind),
                    format!(
                        "{} KEY  ·  {}",
                        if matches!(state, CredentialMenuState::Ready(CredentialState::Vault)) {
                            "REPLACE"
                        } else {
                            "CONFIGURE"
                        },
                        state.label()
                    ),
                )];
                if matches!(state, CredentialMenuState::Ready(CredentialState::Vault)) {
                    choices.push((
                        SettingsChoice::RemoveCredential(*kind),
                        format!(
                            "REMOVE {} KEY  ·  {}",
                            kind.menu_label(),
                            kind.removal_consequence()
                        ),
                    ));
                }
                choices
            }
            SettingsPage::Searxng => {
                let mut choices = vec![(
                    SettingsChoice::EditSearxng,
                    format!(
                        "{} ENDPOINT  ·  {}",
                        if matches!(searxng, EndpointState::Saved) {
                            "REPLACE"
                        } else {
                            "CONFIGURE"
                        },
                        searxng.label()
                    ),
                )];
                if matches!(searxng, EndpointState::Saved) {
                    choices.push((
                        SettingsChoice::RemoveSearxng,
                        "REMOVE ENDPOINT  ·  RESEARCH DISCOVERY BECOMES INCOMPLETE".into(),
                    ));
                }
                choices
            }
        }
    }

    fn open_settings(&mut self, now: Instant) -> bool {
        self.interaction.cancel_pointer();
        self.metadata_focus = None;
        self.pointer_owner = PointerOwner::ConsumedChrome;
        self.research_input = None;
        self.research_preedit.clear();
        self.curation = None;
        self.curation_preedit.clear();
        self.settings = Some(SettingsFlow::Menu {
            browserbase: CredentialMenuState::Resolving,
            exa: CredentialMenuState::Resolving,
            groq: CredentialMenuState::Resolving,
            searxng: searxng_endpoint_state(&self.data_directory),
            hermes_available: hermes_is_available(),
            browse_available: browse_is_available(),
            curl_available: curl_is_available(),
            page: SettingsPage::Root,
            selected: 0,
        });
        self.settings_scroll = 0.0;
        self.settings_preedit.zeroize();
        self.start_credential_state_resolution();
        self.interaction.cursor_left(now);
        true
    }

    fn begin_settings_refresh(&mut self) {
        let selected = match self.settings.as_ref() {
            Some(SettingsFlow::Menu { selected, .. }) => *selected,
            Some(
                SettingsFlow::Credential { .. }
                | SettingsFlow::SearxngEndpoint { .. }
                | SettingsFlow::EraseConfirmation { .. },
            )
            | None => 0,
        };
        self.settings = Some(SettingsFlow::Menu {
            browserbase: CredentialMenuState::Resolving,
            exa: CredentialMenuState::Resolving,
            groq: CredentialMenuState::Resolving,
            searxng: searxng_endpoint_state(&self.data_directory),
            hermes_available: hermes_is_available(),
            browse_available: browse_is_available(),
            curl_available: curl_is_available(),
            page: SettingsPage::Root,
            selected,
        });
        let last = self.settings_choices().len().saturating_sub(1);
        if let Some(SettingsFlow::Menu { selected, .. }) = self.settings.as_mut() {
            *selected = (*selected).min(last);
        }
        self.settings_scroll = 0.0;
        self.settings_preedit.zeroize();
        self.start_credential_state_resolution();
    }

    fn start_credential_state_resolution(&mut self) {
        self.start_vault_task(|| VaultTaskResult::Resolved {
            browserbase: browserbase_credential_state(),
            exa: exa_credential_state(),
            groq: groq_credential_state(),
        });
    }

    fn start_vault_task(&mut self, task: impl FnOnce() -> VaultTaskResult + Send + 'static) {
        if self.vault_task.is_some() {
            return;
        }
        let (sender, receiver) = mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let _ = sender.send(task());
        });
        self.vault_task = Some(receiver);
    }

    fn drain_vault_task(&mut self) -> bool {
        let Some(receiver) = self.vault_task.as_ref() else {
            return false;
        };
        let result = match receiver.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return false,
            Err(TryRecvError::Disconnected) => {
                self.vault_task = None;
                self.status = Some("The credential worker ended without a result.".into());
                return true;
            }
        };
        self.vault_task = None;
        match result {
            VaultTaskResult::Resolved {
                browserbase,
                exa,
                groq,
            } => {
                let unavailable = [
                    (&exa, "Exa"),
                    (&groq, "Groq"),
                    (&browserbase, "Browserbase"),
                ]
                .into_iter()
                .find_map(|(state, label)| match state {
                    CredentialState::Unavailable(detail) => Some((label, detail)),
                    _ => None,
                });
                if let Some((label, detail)) = unavailable {
                    eprintln!("OS credential vault unavailable for {label}: {detail}");
                    self.status = Some(
                        "Research and voice credentials need Credential Manager, KWallet, or GNOME Keyring; reopen Connections after starting one."
                            .into(),
                    );
                }
                if let Some(SettingsFlow::Menu {
                    browserbase: browserbase_menu,
                    exa: exa_menu,
                    groq: groq_menu,
                    ..
                }) = self.settings.as_mut()
                {
                    *browserbase_menu = CredentialMenuState::Ready(browserbase);
                    *exa_menu = CredentialMenuState::Ready(exa);
                    *groq_menu = CredentialMenuState::Ready(groq);
                }
            }
            VaultTaskResult::Saved { kind, result } => {
                self.status = Some(match result {
                    Ok(()) => format!(
                        "{} API key stored in the operating-system credential vault.",
                        kind.label()
                    ),
                    Err(error) => format!(
                        "{} key storage could not be confirmed: {error}",
                        kind.label()
                    ),
                });
                if matches!(self.settings, Some(SettingsFlow::Menu { .. })) {
                    self.begin_settings_refresh();
                }
            }
            VaultTaskResult::Removed { kind, result } => {
                self.status = Some(match result {
                    Ok(()) => format!("{} key removed from the OS vault.", kind.label()),
                    Err(error) => {
                        format!(
                            "{} vault-key removal could not be confirmed: {error}",
                            kind.label()
                        )
                    }
                });
                if matches!(self.settings, Some(SettingsFlow::Menu { .. })) {
                    self.begin_settings_refresh();
                }
            }
            VaultTaskResult::EraseCredentials { failures } => {
                if failures.is_empty() {
                    self.finish_complete_erase();
                } else {
                    self.status = Some(format!(
                        "Complete erase stopped before filesystem deletion because vault removal was incomplete or unconfirmed: {}. Earlier vault deletions may already have completed.",
                        failures.join("; ")
                    ));
                    self.begin_settings_refresh();
                }
            }
        }
        true
    }

    fn settings_selection(&self) -> Option<usize> {
        match self.settings.as_ref()? {
            SettingsFlow::Menu { selected, .. } => Some(*selected),
            SettingsFlow::Credential { .. }
            | SettingsFlow::SearxngEndpoint { .. }
            | SettingsFlow::EraseConfirmation { .. } => None,
        }
    }

    fn navigate_settings(&mut self, direction: SettingsNavigation) -> bool {
        match direction {
            SettingsNavigation::Forward => match self.settings.as_ref() {
                Some(SettingsFlow::Menu { selected, .. }) => self.activate_settings_row(*selected),
                Some(SettingsFlow::Credential { .. }) => self.commit_credential(),
                Some(SettingsFlow::SearxngEndpoint { .. }) => self.commit_searxng_endpoint(),
                Some(SettingsFlow::EraseConfirmation { .. }) => self.commit_complete_erase(),
                None => false,
            },
            SettingsNavigation::Backward => self.retreat_settings(),
        }
    }

    fn retreat_settings(&mut self) -> bool {
        let Some(flow) = self.settings.as_ref() else {
            return false;
        };
        let parent = match flow {
            SettingsFlow::Menu { page, .. } => page.parent(),
            SettingsFlow::Credential { kind, .. } => Some(SettingsPage::Credential(*kind)),
            SettingsFlow::SearxngEndpoint { .. } => Some(SettingsPage::Searxng),
            SettingsFlow::EraseConfirmation { .. } => Some(SettingsPage::Root),
        };
        let Some(parent) = parent else {
            self.settings = None;
            self.settings_preedit.zeroize();
            return true;
        };
        if let Some(SettingsFlow::Menu { page, selected, .. }) = self.settings.as_mut() {
            *page = parent;
            *selected = 0;
            self.settings_scroll = 0.0;
        } else {
            self.settings = Some(SettingsFlow::Menu {
                browserbase: CredentialMenuState::Resolving,
                exa: CredentialMenuState::Resolving,
                groq: CredentialMenuState::Resolving,
                searxng: searxng_endpoint_state(&self.data_directory),
                hermes_available: hermes_is_available(),
                browse_available: browse_is_available(),
                curl_available: curl_is_available(),
                page: parent,
                selected: 0,
            });
            self.settings_scroll = 0.0;
            self.settings_preedit.zeroize();
            self.start_credential_state_resolution();
        }
        true
    }

    fn move_settings_selection(&mut self, direction: isize) -> bool {
        let count = self.settings_choices().len();
        let Some(SettingsFlow::Menu { selected, .. }) = self.settings.as_mut() else {
            return false;
        };
        if count == 0 {
            return false;
        }
        let previous = *selected;
        *selected = if direction < 0 {
            selected.checked_sub(1).unwrap_or(count - 1)
        } else {
            (*selected + 1) % count
        };
        let changed = *selected != previous;
        if changed {
            self.reveal_settings_selection();
        }
        changed
    }

    fn select_settings_row(&mut self, row: usize) -> bool {
        let Some(SettingsFlow::Menu { selected, .. }) = self.settings.as_mut() else {
            return false;
        };
        let changed = *selected != row;
        *selected = row;
        changed
    }

    fn reveal_settings_selection(&mut self) {
        let Some(selected) = self.settings_selection() else {
            return;
        };
        let items = self
            .settings_choices()
            .into_iter()
            .map(|(_, label)| label)
            .collect::<Vec<_>>();
        self.settings_scroll = curation_menu_reveal_offset(
            self.physical_width,
            self.physical_height,
            self.scale_factor,
            self.settings_anchor(),
            &items,
            selected,
            self.settings_scroll,
        );
    }

    fn settings_anchor(&self) -> Point {
        Point {
            x: (self.physical_width * 0.5 - 170.0 * self.scale_factor)
                .max(12.0 * self.scale_factor),
            y: (self.physical_height * 0.18).max(72.0 * self.scale_factor),
        }
    }

    fn settings_menu_at_cursor(&self) -> Option<usize> {
        let cursor = self.cursor?;
        let items = self
            .settings_choices()
            .into_iter()
            .map(|(_, label)| label)
            .collect::<Vec<_>>();
        hit_curation_menu(
            cursor,
            self.physical_width,
            self.physical_height,
            self.scale_factor,
            self.settings_anchor(),
            &items,
            self.settings_scroll,
        )
    }

    fn settings_left_press(&mut self) -> Option<bool> {
        match self.settings.as_ref()? {
            SettingsFlow::Menu { .. } => {
                let Some(row) = self.settings_menu_at_cursor() else {
                    self.settings = None;
                    self.settings_preedit.zeroize();
                    return Some(true);
                };
                Some(self.activate_settings_row(row))
            }
            SettingsFlow::Credential { .. }
            | SettingsFlow::SearxngEndpoint { .. }
            | SettingsFlow::EraseConfirmation { .. } => Some(true),
        }
    }

    #[allow(clippy::too_many_lines)]
    fn activate_settings_row(&mut self, row: usize) -> bool {
        let Some((choice, _)) = self.settings_choices().get(row).cloned() else {
            return false;
        };
        match choice {
            SettingsChoice::Browse => {
                if browse_is_available() {
                    self.status = Some(
                        "Browse executable is present. Research rechecks `browse --version`; browser launch, required skills, and local or Browserbase execution remain unverified until use."
                            .into(),
                    );
                } else {
                    self.open_recovery_guide("Browse", BROWSE_INSTALL_URL);
                }
                self.begin_settings_refresh();
            }
            SettingsChoice::Curl => {
                if curl_is_available() {
                    self.status = Some(
                        "curl executable is present. Research rechecks `curl --version`; the configured SearXNG JSON endpoint is validated separately."
                            .into(),
                    );
                } else {
                    self.open_recovery_guide("curl", CURL_INSTALL_URL);
                }
                self.begin_settings_refresh();
            }
            SettingsChoice::OpenSearxng => {
                if let Some(SettingsFlow::Menu { page, selected, .. }) = self.settings.as_mut() {
                    *page = SettingsPage::Searxng;
                    *selected = 0;
                }
                self.settings_scroll = 0.0;
            }
            SettingsChoice::EditSearxng => {
                let input = searxng_url(&self.data_directory)
                    .ok()
                    .flatten()
                    .unwrap_or_default();
                self.settings = Some(SettingsFlow::SearxngEndpoint { input });
                self.settings_preedit.zeroize();
            }
            SettingsChoice::OpenCredential(kind) => {
                if let Some(SettingsFlow::Menu { page, selected, .. }) = self.settings.as_mut() {
                    *page = SettingsPage::Credential(kind);
                    *selected = 0;
                }
                self.settings_scroll = 0.0;
            }
            SettingsChoice::EditCredential(kind) => {
                if self.vault_task.is_some() {
                    self.status =
                        Some("The credential vault is already processing a request.".into());
                    return true;
                }
                self.settings = Some(SettingsFlow::Credential {
                    kind,
                    input: Zeroizing::new(String::new()),
                });
                self.settings_preedit.zeroize();
            }
            SettingsChoice::Hermes => {
                if let Some(profile) = self.hermes_profile.as_ref()
                    && hermes_is_available()
                {
                    match open_hermes_dashboard(&profile.root, hermes_profile::PROFILE_ID) {
                        Ok(()) => {
                            self.status = Some(
                                "Hermes dashboard launch requested; continued operation is unconfirmed. If no browser opens, run `hermes -p not-news dashboard`. Hermes alone owns provider and model configuration."
                                    .into(),
                            );
                        }
                        Err(error) => {
                            self.status = Some(format!("Hermes dashboard did not open: {error}"));
                        }
                    }
                } else if self.hermes_profile.is_some() {
                    match open_external_url(HERMES_INSTALL_URL) {
                        Ok(()) => {
                            self.status = Some(format!(
                                "Hermes guide launch requested but not confirmed. If no browser opens, copy {HERMES_INSTALL_URL}"
                            ));
                        }
                        Err(error) => {
                            self.status =
                                Some(format!("Hermes installation guide did not open: {error}"));
                        }
                    }
                } else {
                    self.status = Some(
                        "The application-owned Hermes profile could not be installed; see the startup error before configuring providers."
                            .into(),
                    );
                }
                self.begin_settings_refresh();
            }
            SettingsChoice::RemoveCredential(kind) => {
                if self.vault_task.is_some() {
                    self.status =
                        Some("The credential vault is already processing a request.".into());
                    return true;
                }
                if let Some(state) = self.credential_menu_mut(kind) {
                    *state = CredentialMenuState::Removing;
                }
                self.start_vault_task(move || {
                    let result = match kind {
                        CredentialKind::Browserbase => delete_browserbase_api_key(),
                        CredentialKind::Exa => delete_exa_api_key(),
                        CredentialKind::Groq => delete_groq_api_key(),
                    }
                    .map_err(|error| error.to_string());
                    VaultTaskResult::Removed { kind, result }
                });
            }
            SettingsChoice::RemoveSearxng => {
                self.status = Some(match delete_searxng_url(&self.data_directory) {
                    Ok(()) => "SearXNG application setting removed.".into(),
                    Err(error) => {
                        format!("SearXNG-setting removal could not be confirmed: {error}")
                    }
                });
                self.begin_settings_refresh();
            }
            SettingsChoice::EraseAll => {
                if self.research.is_some()
                    || self.research_preflight.is_some()
                    || !self.voice.is_idle()
                    || self.vault_task.is_some()
                {
                    self.status = Some(
                        "Complete erase requires idle research, voice, and credential workers. Cancel them, then retry."
                            .into(),
                    );
                } else {
                    self.settings = Some(SettingsFlow::EraseConfirmation {
                        input: String::new(),
                    });
                    self.settings_preedit.zeroize();
                }
            }
        }
        true
    }

    fn open_recovery_guide(&mut self, label: &str, url: &str) {
        self.status = Some(match open_external_url(url) {
            Ok(()) => format!(
                "{label} guide launch requested but not confirmed. If no browser opens, copy {url}"
            ),
            Err(error) => format!("{label} guide did not open: {error}. Copy {url}"),
        });
    }

    fn commit_searxng_endpoint(&mut self) -> bool {
        let Some(SettingsFlow::SearxngEndpoint { input }) = self.settings.take() else {
            return false;
        };
        self.settings_preedit.zeroize();
        self.status = Some(match save_searxng_url(&self.data_directory, &input) {
            Ok(()) => "SearXNG endpoint saved; research will validate it on use.".into(),
            Err(error) => format!("SearXNG endpoint was not saved: {error}"),
        });
        self.begin_settings_refresh();
        true
    }

    fn credential_menu_mut(&mut self, kind: CredentialKind) -> Option<&mut CredentialMenuState> {
        let SettingsFlow::Menu {
            browserbase,
            exa,
            groq,
            ..
        } = self.settings.as_mut()?
        else {
            return None;
        };
        Some(match kind {
            CredentialKind::Browserbase => browserbase,
            CredentialKind::Exa => exa,
            CredentialKind::Groq => groq,
        })
    }

    fn commit_credential(&mut self) -> bool {
        let Some(SettingsFlow::Credential { kind, input }) = self.settings.take() else {
            return false;
        };
        self.settings = Some(SettingsFlow::Menu {
            browserbase: CredentialMenuState::Resolving,
            exa: CredentialMenuState::Resolving,
            groq: CredentialMenuState::Resolving,
            searxng: searxng_endpoint_state(&self.data_directory),
            hermes_available: hermes_is_available(),
            browse_available: browse_is_available(),
            curl_available: curl_is_available(),
            page: SettingsPage::Root,
            selected: 1,
        });
        if let Some(state) = self.credential_menu_mut(kind) {
            *state = CredentialMenuState::Saving;
        }
        self.settings_preedit.zeroize();
        self.start_vault_task(move || {
            let result = match kind {
                CredentialKind::Browserbase => save_browserbase_api_key(input.as_str()),
                CredentialKind::Exa => save_exa_api_key(input.as_str()),
                CredentialKind::Groq => save_groq_api_key(input.as_str()),
            }
            .map_err(|error| error.to_string());
            VaultTaskResult::Saved { kind, result }
        });
        true
    }

    fn commit_complete_erase(&mut self) -> bool {
        let Some(SettingsFlow::EraseConfirmation { input }) = self.settings.as_ref() else {
            return false;
        };
        if input.trim() != "ERASE" {
            self.status = Some(
                "Complete erase not started: type ERASE exactly, or press Escape to keep all state."
                    .into(),
            );
            return true;
        }
        let Some(lease) = self.application_lease.as_mut() else {
            self.status = Some(
                "Complete erase is unavailable because application-state ownership was not established."
                    .into(),
            );
            return true;
        };
        if let Err(error) = lease.require_exclusive() {
            self.status = Some(format!(
                "Complete erase did not start: {error}. Close every other Not News instance and retry."
            ));
            return true;
        }
        self.settings = Some(SettingsFlow::EraseConfirmation {
            input: "ERASE".into(),
        });
        self.settings_preedit.zeroize();
        self.status = Some(
            "Complete erase is removing the three Not News vault accounts; filesystem state remains until those outcomes are confirmed."
                .into(),
        );
        self.start_vault_task(|| {
            let mut failures = Vec::new();
            for (label, result) in [
                ("Exa", delete_exa_api_key()),
                ("Browserbase", delete_browserbase_api_key()),
                ("Groq", delete_groq_api_key()),
            ] {
                if let Err(error) = result {
                    failures.push(format!("{label}: {error}"));
                }
            }
            VaultTaskResult::EraseCredentials { failures }
        });
        true
    }

    fn finish_complete_erase(&mut self) {
        let result = (|| -> Result<(), String> {
            let root = self
                .hermes_root
                .as_deref()
                .ok_or_else(|| "Hermes-root ownership was not established".to_owned())?;
            hermes_profile::erase_owned(root)
                .map_err(|error| format!("owned Hermes profile: {error}"))?;

            self.speech.cancel_session();
            let store = self
                .store
                .take()
                .ok_or_else(|| "graph store is unavailable".to_owned())?;
            let database = store.path().to_path_buf();
            drop(store);
            if self.data_directory.exists() {
                fs::remove_dir_all(&self.data_directory)
                    .map_err(|error| format!("application data: {error}"))?;
            }
            if !database.starts_with(&self.data_directory) {
                remove_external_graph_family(&database)
                    .map_err(|error| format!("external graph family: {error}"))?;
            }
            Ok(())
        })();

        match result {
            Ok(()) => {
                if let Some(lease) = self.application_lease.as_mut() {
                    lease.remove_after_exit();
                }
                self.graph = GraphSnapshot::default();
                self.settings = None;
                self.status = Some(
                    "Complete erase finished: graph, app settings and scratch state, Not News vault accounts, and the exactly owned Hermes profile were removed. Exiting now."
                        .into(),
                );
                self.exit_after_present = true;
            }
            Err(error) => {
                if let Some(lease) = self.application_lease.as_ref() {
                    let _ = lease.restore_shared();
                }
                self.status = Some(format!(
                    "Complete erase stopped after vault deletion; local deletion is partial or unconfirmed: {error}. No unrelated Hermes profile was selected for deletion."
                ));
                self.begin_settings_refresh();
            }
        }
    }

    #[allow(clippy::too_many_lines)]
    fn settings_keyboard_input(
        &mut self,
        event: &not_news_platform::winit::event::KeyEvent,
    ) -> Option<bool> {
        let menu = matches!(self.settings, Some(SettingsFlow::Menu { .. }));
        let key_input = matches!(self.settings, Some(SettingsFlow::Credential { .. }));
        let endpoint_input = matches!(self.settings, Some(SettingsFlow::SearxngEndpoint { .. }));
        let erase_input = matches!(self.settings, Some(SettingsFlow::EraseConfirmation { .. }));
        if !menu && !key_input && !endpoint_input && !erase_input {
            return None;
        }
        if matches!(event.logical_key, Key::Named(NamedKey::Escape)) {
            return Some(self.navigate_settings(SettingsNavigation::Backward));
        }
        let command = self.modifiers.control_key() || self.modifiers.super_key();
        let unmodified = !command && !self.modifiers.alt_key();
        if unmodified && matches!(event.logical_key, Key::Named(NamedKey::ArrowLeft)) {
            return Some(self.navigate_settings(SettingsNavigation::Backward));
        }
        if unmodified && matches!(event.logical_key, Key::Named(NamedKey::ArrowRight)) {
            return Some(self.navigate_settings(SettingsNavigation::Forward));
        }
        if menu
            && !command
            && !self.modifiers.alt_key()
            && matches!(event.logical_key, Key::Named(NamedKey::ArrowDown))
        {
            return Some(self.move_settings_selection(1));
        }
        if menu
            && !command
            && !self.modifiers.alt_key()
            && matches!(event.logical_key, Key::Named(NamedKey::ArrowUp))
        {
            return Some(self.move_settings_selection(-1));
        }
        if menu && matches!(event.logical_key, Key::Named(NamedKey::Enter)) {
            return Some(self.navigate_settings(SettingsNavigation::Forward));
        }
        if menu
            && !command
            && !self.modifiers.alt_key()
            && let Key::Character(key) = &event.logical_key
            && let Some(row) = menu_row_from_key(key)
        {
            return Some(self.activate_settings_row(row));
        }
        if !key_input && !endpoint_input && !erase_input {
            return Some(false);
        }
        if matches!(event.logical_key, Key::Named(NamedKey::Enter)) {
            return Some(self.navigate_settings(SettingsNavigation::Forward));
        }
        if matches!(event.logical_key, Key::Named(NamedKey::Backspace)) {
            match self.settings.as_mut() {
                Some(SettingsFlow::Credential { input, .. }) => remove_last_grapheme(input),
                Some(
                    SettingsFlow::SearxngEndpoint { input }
                    | SettingsFlow::EraseConfirmation { input },
                ) => remove_last_grapheme(input),
                _ => {}
            }
            return Some(true);
        }
        if command
            && matches!(&event.logical_key, Key::Character(key) if key.eq_ignore_ascii_case("v"))
        {
            let result = arboard::Clipboard::new().and_then(|mut clipboard| clipboard.get_text());
            return Some(match result {
                Ok(text) => {
                    let mut secret = Zeroizing::new(text);
                    self.append_settings_text(secret.as_str());
                    secret.zeroize();
                    true
                }
                Err(error) => {
                    self.status = Some(format!("Clipboard text is unavailable: {error}"));
                    true
                }
            });
        }
        if command
            && matches!(&event.logical_key, Key::Character(key) if key.eq_ignore_ascii_case("u"))
        {
            match self.settings.as_mut() {
                Some(SettingsFlow::Credential { input, .. }) => input.zeroize(),
                Some(
                    SettingsFlow::SearxngEndpoint { input }
                    | SettingsFlow::EraseConfirmation { input },
                ) => input.clear(),
                _ => {}
            }
            self.settings_preedit.zeroize();
            return Some(true);
        }
        if !command
            && !self.modifiers.alt_key()
            && let Some(text) = event.text.as_deref()
        {
            self.append_settings_text(text);
            return Some(true);
        }
        Some(false)
    }

    fn append_settings_text(&mut self, text: &str) {
        match self.settings.as_mut() {
            Some(SettingsFlow::Credential { input, .. }) => append_bounded_secret(input, text),
            Some(
                SettingsFlow::SearxngEndpoint { input } | SettingsFlow::EraseConfirmation { input },
            ) => append_bounded_text(input, text),
            _ => {}
        }
    }

    fn curation_choices(
        &self,
        subject: &CanvasSubject,
        stage: CurationMenuStage,
        page: usize,
    ) -> Vec<(CurationChoice, String)> {
        let Some(event) = self.graph.events.get(&subject.event) else {
            return Vec::new();
        };
        match stage {
            CurationMenuStage::Actions => {
                let mut choices = vec![(
                    CurationChoice::Relate,
                    format!("RELATE FROM {}", menu_text(&event.title, 34)),
                )];
                if self
                    .graph
                    .bridges
                    .values()
                    .any(|bridge| bridge.from == subject.event || bridge.to == subject.event)
                {
                    choices.push((CurationChoice::Detach, "DETACH ONE RELATIONSHIP…".into()));
                }
                if let Some(index) = subject.artifact_index
                    && let Some(artifact) = event.artifacts.get(index)
                {
                    choices.push((
                        CurationChoice::Promote,
                        format!("PROMOTE {}", menu_text(&artifact.text, 37)),
                    ));
                }
                choices
            }
            CurationMenuStage::Detach => {
                let relationships = self
                    .graph
                    .bridges
                    .values()
                    .filter(|bridge| bridge.from == subject.event || bridge.to == subject.event)
                    .collect::<Vec<_>>();
                let last_page = relationships
                    .len()
                    .saturating_sub(1)
                    .checked_div(CURATION_RELATIONSHIPS_PER_PAGE)
                    .unwrap_or(0);
                let page = page.min(last_page);
                let start = page * CURATION_RELATIONSHIPS_PER_PAGE;
                let end = (start + CURATION_RELATIONSHIPS_PER_PAGE).min(relationships.len());
                let mut choices = Vec::with_capacity(CURATION_RELATIONSHIPS_PER_PAGE + 2);
                if page > 0 {
                    choices.push((
                        CurationChoice::PreviousPage,
                        "← PREVIOUS RELATIONSHIPS".into(),
                    ));
                }
                choices.extend(relationships[start..end].iter().map(|bridge| {
                    let direction = if bridge.from == subject.event {
                        format!("→ {}", event_title(&self.graph, &bridge.to))
                    } else {
                        format!("← {}", event_title(&self.graph, &bridge.from))
                    };
                    (
                        CurationChoice::Bridge(bridge.id.clone()),
                        menu_text(&format!("{direction}  ·  {}", bridge.label), 49),
                    )
                }));
                if page < last_page {
                    choices.push((CurationChoice::NextPage, "MORE RELATIONSHIPS →".into()));
                }
                choices
            }
        }
    }

    fn open_curation_menu(&mut self, now: Instant) -> bool {
        self.interaction.cancel_pointer();
        self.metadata_focus = None;
        let Some(anchor) = self.cursor else {
            return false;
        };
        let Some(subject) = self.interaction.subject_at_cursor(&self.graph, now) else {
            let changed = self.curation.take().is_some();
            self.curation_preedit.clear();
            return changed;
        };
        self.research_input = None;
        self.research_preedit.clear();
        self.curation = Some(CurationFlow::Menu {
            anchor,
            subject,
            stage: CurationMenuStage::Actions,
            page: 0,
            expected_revision: self.graph.revision,
        });
        self.curation_scroll = 0.0;
        self.curation_preedit.clear();
        true
    }

    fn curation_menu_at_cursor(&self) -> Option<usize> {
        let cursor = self.cursor?;
        let CurationFlow::Menu {
            anchor,
            subject,
            stage,
            page,
            ..
        } = self.curation.as_ref()?
        else {
            return None;
        };
        let items = self
            .curation_choices(subject, *stage, *page)
            .into_iter()
            .map(|(_, label)| label)
            .collect::<Vec<_>>();
        hit_curation_menu(
            cursor,
            self.physical_width,
            self.physical_height,
            self.scale_factor,
            *anchor,
            &items,
            self.curation_scroll,
        )
    }

    fn curation_menu_surface_at_cursor(&self) -> bool {
        let Some(cursor) = self.cursor else {
            return false;
        };
        let Some(CurationFlow::Menu {
            anchor,
            subject,
            stage,
            page,
            ..
        }) = self.curation.as_ref()
        else {
            return false;
        };
        let items = self
            .curation_choices(subject, *stage, *page)
            .into_iter()
            .map(|(_, label)| label)
            .collect::<Vec<_>>();
        hit_curation_menu_surface(
            cursor,
            self.physical_width,
            self.physical_height,
            self.scale_factor,
            *anchor,
            &items,
            self.curation_scroll,
        )
    }

    fn curation_left_press(&mut self, now: Instant) -> Option<bool> {
        let flow = self.curation.clone()?;
        match flow {
            CurationFlow::Menu { .. } => {
                let Some(row) = self.curation_menu_at_cursor() else {
                    self.curation = None;
                    self.curation_preedit.clear();
                    return Some(true);
                };
                Some(self.activate_curation_row(row, now))
            }
            CurationFlow::RelateTarget {
                from,
                expected_revision,
            } => {
                let Some(target) = self.interaction.subject_at_cursor(&self.graph, now) else {
                    self.status = Some("Choose an exact target finding, or press Escape.".into());
                    return Some(true);
                };
                if target.event == from {
                    self.status = Some("A finding cannot be related to itself.".into());
                    return Some(true);
                }
                self.curation = Some(CurationFlow::RelatePredicate {
                    from,
                    to: target.event,
                    expected_revision,
                    input: String::new(),
                });
                self.curation_preedit.clear();
                self.status = None;
                Some(true)
            }
            CurationFlow::RelatePredicate { .. } | CurationFlow::PromotePredicate { .. } => {
                Some(false)
            }
        }
    }

    fn activate_curation_row(&mut self, row: usize, now: Instant) -> bool {
        let Some(CurationFlow::Menu {
            anchor,
            subject,
            stage,
            page,
            expected_revision,
        }) = self.curation.clone()
        else {
            return false;
        };
        let choices = self.curation_choices(&subject, stage, page);
        let Some((choice, _)) = choices.get(row).cloned() else {
            return false;
        };
        self.activate_curation_choice(choice, anchor, subject, stage, page, expected_revision, now)
    }

    #[allow(clippy::too_many_arguments)]
    fn activate_curation_choice(
        &mut self,
        choice: CurationChoice,
        anchor: Point,
        subject: CanvasSubject,
        stage: CurationMenuStage,
        page: usize,
        expected_revision: u64,
        now: Instant,
    ) -> bool {
        match choice {
            CurationChoice::Relate => {
                self.curation = Some(CurationFlow::RelateTarget {
                    from: subject.event,
                    expected_revision,
                });
                self.status =
                    Some("Choose the exact target finding; movement remains disabled.".into());
                true
            }
            CurationChoice::Detach if matches!(stage, CurationMenuStage::Actions) => {
                self.curation = Some(CurationFlow::Menu {
                    anchor,
                    subject,
                    stage: CurationMenuStage::Detach,
                    page: 0,
                    expected_revision,
                });
                self.curation_scroll = 0.0;
                true
            }
            CurationChoice::Promote => {
                let Some(artifact_index) = subject.artifact_index else {
                    return false;
                };
                self.curation = Some(CurationFlow::PromotePredicate {
                    source: subject.event,
                    artifact_index,
                    expected_revision,
                    input: String::new(),
                });
                self.curation_preedit.clear();
                self.status = None;
                true
            }
            CurationChoice::Bridge(bridge_id) => {
                self.commit_detachment(bridge_id, expected_revision, now)
            }
            CurationChoice::PreviousPage if matches!(stage, CurationMenuStage::Detach) => {
                self.curation = Some(CurationFlow::Menu {
                    anchor,
                    subject,
                    stage,
                    page: page.saturating_sub(1),
                    expected_revision,
                });
                self.curation_scroll = 0.0;
                true
            }
            CurationChoice::NextPage if matches!(stage, CurationMenuStage::Detach) => {
                self.curation = Some(CurationFlow::Menu {
                    anchor,
                    subject,
                    stage,
                    page: page.saturating_add(1),
                    expected_revision,
                });
                self.curation_scroll = 0.0;
                true
            }
            CurationChoice::Detach | CurationChoice::PreviousPage | CurationChoice::NextPage => {
                false
            }
        }
    }

    fn commit_detachment(
        &mut self,
        bridge_id: BridgeId,
        expected_revision: u64,
        now: Instant,
    ) -> bool {
        let operation = self.next_operation_id("detach");
        let command = DetachRelationship {
            bridge_id,
            expected_revision,
        };
        let result = self
            .store
            .as_ref()
            .ok_or_else(|| "the graph store is unavailable".to_owned())
            .and_then(|store| {
                store
                    .commit_detachment(&operation, &command)
                    .map_err(|error| error.to_string())
            });
        match result {
            Ok(outcome) => {
                self.apply_semantic_outcome(outcome, now);
                self.curation = None;
                self.status =
                    Some("Relationship detached; Ctrl+Z restores that exact fact.".into());
            }
            Err(error) => self.status = Some(format!("Relationship was not detached: {error}")),
        }
        true
    }

    fn commit_relation_input(&mut self, now: Instant) -> bool {
        let Some(CurationFlow::RelatePredicate {
            from,
            to,
            expected_revision,
            input,
        }) = self.curation.clone()
        else {
            return false;
        };
        if input.trim().is_empty() {
            self.status = Some("Name what the relationship means before committing it.".into());
            return true;
        }
        let operation = self.next_operation_id("relate");
        let command = RelateEvents {
            bridge_id: BridgeId(format!("{operation}:bridge")),
            from,
            to,
            predicate: input,
            provenance: Provenance::User,
            expected_revision,
        };
        let result = self
            .store
            .as_ref()
            .ok_or_else(|| "the graph store is unavailable".to_owned())
            .and_then(|store| {
                store
                    .commit_relation(&operation, &command)
                    .map_err(|error| error.to_string())
            });
        match result {
            Ok(outcome) => {
                self.apply_semantic_outcome(outcome, now);
                self.curation = None;
                self.curation_preedit.clear();
                self.status = Some("Relationship committed; movement was not changed.".into());
            }
            Err(error) => self.status = Some(format!("Relationship was not committed: {error}")),
        }
        true
    }

    fn commit_promotion_input(&mut self, now: Instant) -> bool {
        let Some(CurationFlow::PromotePredicate {
            source,
            artifact_index,
            expected_revision,
            input,
        }) = self.curation.clone()
        else {
            return false;
        };
        let Some(source_event) = self.graph.events.get(&source) else {
            self.status = Some("The selected source finding no longer exists.".into());
            return true;
        };
        let Some(artifact) = source_event.artifacts.get(artifact_index) else {
            self.status = Some("The selected artifact no longer exists.".into());
            return true;
        };
        let artifact_url = artifact.url.clone();
        let date = source_event.date.clone();
        let operation = self.next_operation_id("promote");
        let relation = (!input.trim().is_empty()).then(|| PromotionRelation {
            bridge_id: BridgeId(format!("{operation}:bridge")),
            predicate: input,
        });
        let command = PromoteArtifact {
            source_event: source,
            artifact_url,
            promoted_id: EventId(format!("{operation}:event")),
            date,
            relation,
            expected_revision,
        };
        let result = self
            .store
            .as_ref()
            .ok_or_else(|| "the graph store is unavailable".to_owned())
            .and_then(|store| {
                store
                    .commit_artifact_promotion(&operation, &command)
                    .map_err(|error| error.to_string())
            });
        match result {
            Ok(outcome) => {
                self.apply_semantic_outcome(outcome, now);
                self.curation = None;
                self.curation_preedit.clear();
                self.status = Some("Artifact promoted into a durable finding.".into());
            }
            Err(error) => self.status = Some(format!("Artifact was not promoted: {error}")),
        }
        true
    }

    fn next_operation_id(&mut self, kind: &str) -> String {
        self.operation_sequence = self
            .operation_sequence
            .checked_add(1)
            .expect("a process cannot emit 2^64 user gestures");
        format!(
            "rust-{kind}-{}-{}-{}",
            std::process::id(),
            self.operation_epoch,
            self.operation_sequence
        )
    }

    fn commit_effect(&mut self, effect: InteractionEffect) -> bool {
        match effect {
            InteractionEffect::Unchanged => false,
            InteractionEffect::PixelsChanged => true,
            InteractionEffect::Move(command) => {
                let operation = self.next_operation_id("move");
                let result = self
                    .store
                    .as_ref()
                    .ok_or("the graph store is unavailable".to_owned())
                    .and_then(|store| {
                        store
                            .commit_move(&operation, &command)
                            .map_err(|error| error.to_string())
                    });
                match result {
                    Ok(outcome) => {
                        self.apply_outcome(outcome);
                        self.status = None;
                    }
                    Err(error) => self.status = Some(format!("Move was not committed: {error}")),
                }
                true
            }
            InteractionEffect::OpenUrl(url) => {
                if let Err(error) = open_external_url(&url) {
                    self.status = Some(format!("Source could not be opened: {error}"));
                    return true;
                }
                self.status = Some(format!(
                    "Source launch requested but not confirmed. If no browser opens, copy {url}"
                ));
                true
            }
        }
    }

    fn undo(&mut self) -> bool {
        let operation = self.next_operation_id("undo");
        let Some(store) = self.store.as_ref() else {
            self.status = Some("Undo is unavailable because the graph did not open.".into());
            return true;
        };
        match store.undo(&operation) {
            Ok(Some(outcome)) => {
                self.apply_outcome(outcome);
                self.status = None;
                true
            }
            Ok(None) => false,
            Err(error) => {
                self.status = Some(format!("Undo was not committed: {error}"));
                true
            }
        }
    }

    fn redo(&mut self) -> bool {
        let operation = self.next_operation_id("redo");
        let Some(store) = self.store.as_ref() else {
            self.status = Some("Redo is unavailable because the graph did not open.".into());
            return true;
        };
        match store.redo(&operation) {
            Ok(Some(outcome)) => {
                self.apply_outcome(outcome);
                self.status = None;
                true
            }
            Ok(None) => false,
            Err(error) => {
                self.status = Some(format!("Redo was not committed: {error}"));
                true
            }
        }
    }

    fn curation_keyboard_input(
        &mut self,
        event: &not_news_platform::winit::event::KeyEvent,
        now: Instant,
    ) -> Option<bool> {
        let flow = self.curation.clone()?;
        if matches!(event.logical_key, Key::Named(NamedKey::Escape)) {
            self.curation = None;
            self.curation_preedit.clear();
            self.status = None;
            return Some(true);
        }
        let command = self.modifiers.control_key() || self.modifiers.super_key();
        if matches!(&flow, CurationFlow::Menu { .. })
            && !command
            && !self.modifiers.alt_key()
            && let Key::Character(key) = &event.logical_key
            && let Some(row) = menu_row_from_key(key)
        {
            return Some(self.activate_curation_row(row, now));
        }
        if matches!(&flow, CurationFlow::RelatePredicate { .. })
            && matches!(event.logical_key, Key::Named(NamedKey::Enter))
        {
            self.curation_preedit.clear();
            return Some(self.commit_relation_input(now));
        }
        if matches!(&flow, CurationFlow::PromotePredicate { .. })
            && matches!(event.logical_key, Key::Named(NamedKey::Enter))
        {
            self.curation_preedit.clear();
            return Some(self.commit_promotion_input(now));
        }
        if !matches!(
            &flow,
            CurationFlow::RelatePredicate { .. } | CurationFlow::PromotePredicate { .. }
        ) {
            return Some(false);
        }
        if matches!(event.logical_key, Key::Named(NamedKey::Backspace)) {
            if let Some(input) = self.curation_input_mut() {
                remove_last_grapheme(input);
            }
            return Some(true);
        }
        if command
            && matches!(&event.logical_key, Key::Character(key) if key.eq_ignore_ascii_case("v"))
        {
            return Some(self.paste_curation_input());
        }
        if command
            && matches!(&event.logical_key, Key::Character(key) if key.eq_ignore_ascii_case("u"))
        {
            if let Some(input) = self.curation_input_mut() {
                input.clear();
            }
            self.curation_preedit.clear();
            return Some(true);
        }
        if !command
            && !self.modifiers.alt_key()
            && let Some(text) = event.text.as_deref()
        {
            self.append_curation_text(text);
            return Some(true);
        }
        Some(false)
    }

    fn curation_input_mut(&mut self) -> Option<&mut String> {
        match self.curation.as_mut()? {
            CurationFlow::RelatePredicate { input, .. }
            | CurationFlow::PromotePredicate { input, .. } => Some(input),
            CurationFlow::Menu { .. } | CurationFlow::RelateTarget { .. } => None,
        }
    }

    fn append_curation_text(&mut self, text: &str) {
        if let Some(input) = self.curation_input_mut() {
            append_bounded_predicate(input, text);
        }
    }

    fn paste_curation_input(&mut self) -> bool {
        let result = arboard::Clipboard::new().and_then(|mut clipboard| clipboard.get_text());
        match result {
            Ok(text) => {
                self.append_curation_text(&text);
                true
            }
            Err(error) => {
                self.status = Some(format!("Clipboard text is unavailable: {error}"));
                true
            }
        }
    }

    fn metadata_keyboard_input(
        &mut self,
        event: &not_news_platform::winit::event::KeyEvent,
        now: Instant,
    ) -> Option<bool> {
        if self.metadata_focus.is_some() {
            return Some(match event.logical_key {
                Key::Named(NamedKey::Escape) => self.release_metadata_focus(now),
                Key::Named(NamedKey::Tab) if !event.repeat => self.release_metadata_focus(now),
                Key::Named(NamedKey::ArrowDown) => self.scroll_metadata_by(36.0),
                Key::Named(NamedKey::ArrowUp) => self.scroll_metadata_by(-36.0),
                Key::Named(NamedKey::PageDown) => self.scroll_metadata_by(240.0),
                Key::Named(NamedKey::PageUp) => self.scroll_metadata_by(-240.0),
                Key::Named(NamedKey::Home) => {
                    let changed = self.metadata_scroll > 0.0;
                    self.metadata_scroll = 0.0;
                    changed
                }
                Key::Named(NamedKey::End) => {
                    let Some((active, _)) = self.interaction.active_event_position() else {
                        return Some(false);
                    };
                    let maximum = active_metadata_scroll_max(
                        self.physical_width,
                        self.physical_height,
                        self.scale_factor,
                        &self.graph.events[active],
                    );
                    let changed = (self.metadata_scroll - maximum).abs() > f64::EPSILON;
                    self.metadata_scroll = maximum;
                    changed
                }
                _ => false,
            });
        }
        if matches!(event.logical_key, Key::Named(NamedKey::Tab))
            && !event.repeat
            && !self.modifiers.control_key()
            && !self.modifiers.alt_key()
            && !self.modifiers.super_key()
            && self.metadata_has_overflow()
        {
            return Some(self.focus_metadata());
        }
        None
    }

    #[allow(clippy::too_many_lines)]
    fn keyboard_input(
        &mut self,
        event: &not_news_platform::winit::event::KeyEvent,
        now: Instant,
    ) -> bool {
        if event.state != ElementState::Pressed {
            return false;
        }
        if let Some(changed) = self.settings_keyboard_input(event) {
            return changed;
        }
        if let Some(changed) = self.curation_keyboard_input(event, now) {
            return changed;
        }
        if self.research_input.is_some() {
            match &event.logical_key {
                Key::Named(NamedKey::Enter) => {
                    self.research_preedit.clear();
                    let question = self.research_input.take().unwrap_or_default();
                    return self.start_research(&question);
                }
                Key::Named(NamedKey::Escape) => {
                    self.research_input = None;
                    self.research_preedit.clear();
                    return true;
                }
                Key::Named(NamedKey::Backspace) => {
                    if let Some(input) = self.research_input.as_mut() {
                        remove_last_grapheme(input);
                    }
                    return true;
                }
                _ => {}
            }
            let command = self.modifiers.control_key() || self.modifiers.super_key();
            if command
                && matches!(&event.logical_key, Key::Character(key) if key.eq_ignore_ascii_case("v"))
            {
                return self.paste_research_input();
            }
            if command
                && matches!(&event.logical_key, Key::Character(key) if key.eq_ignore_ascii_case("u"))
            {
                self.research_input.as_mut().unwrap().clear();
                self.research_preedit.clear();
                return true;
            }
            if !command
                && !self.modifiers.alt_key()
                && let Some(text) = event.text.as_deref()
            {
                self.append_research_text(text);
                return true;
            }
            return false;
        }
        if let Some(changed) = self.metadata_keyboard_input(event, now) {
            return changed;
        }
        if event.repeat {
            return false;
        }
        if matches!(event.logical_key, Key::Named(NamedKey::Escape)) && self.cancel_voice() {
            return true;
        }
        if matches!(event.logical_key, Key::Named(NamedKey::Escape))
            && self.research_preflight.take().is_some()
        {
            self.status =
                Some("Research readiness cancelled; no research session was created.".into());
            return true;
        }
        if matches!(event.logical_key, Key::Named(NamedKey::Escape))
            && let Some(active) = self.research.as_ref()
            && !active.closed
        {
            active.handle.cancel();
            self.speech.cancel_session();
            self.status = Some("Cancelling research…".into());
            return true;
        }
        let Key::Character(character) = &event.logical_key else {
            return false;
        };
        let command = self.modifiers.control_key() || self.modifiers.super_key();
        if command && character == "," {
            return self.open_settings(now);
        }
        if command && character.eq_ignore_ascii_case("e") {
            if self.open_curation_menu(now) {
                return true;
            }
            self.status = Some("Hover a finding or source artifact, then press Ctrl+E.".into());
            return true;
        }
        if (command && character.eq_ignore_ascii_case("k")) || (!command && character == "/") {
            self.research_input = Some(String::new());
            self.research_preedit.clear();
            return true;
        }
        if !command {
            return false;
        }
        if character.eq_ignore_ascii_case("z") {
            if self.modifiers.shift_key() {
                self.redo()
            } else {
                self.undo()
            }
        } else if character.eq_ignore_ascii_case("y") {
            self.redo()
        } else {
            false
        }
    }

    fn append_research_text(&mut self, text: &str) {
        let Some(input) = self.research_input.as_mut() else {
            return;
        };
        append_bounded_text(input, text);
    }

    fn paste_research_input(&mut self) -> bool {
        let result = arboard::Clipboard::new().and_then(|mut clipboard| clipboard.get_text());
        match result {
            Ok(text) => {
                self.append_research_text(&text);
                true
            }
            Err(error) => {
                self.status = Some(format!("Clipboard text is unavailable: {error}"));
                true
            }
        }
    }

    fn ime_input(&mut self, ime: &Ime) -> bool {
        if matches!(
            self.settings,
            Some(
                SettingsFlow::Credential { .. }
                    | SettingsFlow::SearxngEndpoint { .. }
                    | SettingsFlow::EraseConfirmation { .. }
            )
        ) {
            match ime {
                Ime::Preedit(text, _) => {
                    self.settings_preedit =
                        Zeroizing::new(text.chars().take(MAX_CREDENTIAL_CHARS).collect());
                    return true;
                }
                Ime::Commit(text) => {
                    self.settings_preedit.zeroize();
                    self.append_settings_text(text);
                    return true;
                }
                Ime::Disabled => {
                    let changed = !self.settings_preedit.is_empty();
                    self.settings_preedit.zeroize();
                    return changed;
                }
                Ime::Enabled => return false,
            }
        }
        if matches!(
            self.curation,
            Some(CurationFlow::RelatePredicate { .. } | CurationFlow::PromotePredicate { .. })
        ) {
            match ime {
                Ime::Preedit(text, _) => {
                    self.curation_preedit = text.chars().take(96).collect();
                    return true;
                }
                Ime::Commit(text) => {
                    self.curation_preedit.clear();
                    self.append_curation_text(text);
                    return true;
                }
                Ime::Disabled => {
                    let changed = !self.curation_preedit.is_empty();
                    self.curation_preedit.clear();
                    return changed;
                }
                Ime::Enabled => return false,
            }
        }
        if self.research_input.is_none() {
            return false;
        }
        match ime {
            Ime::Preedit(text, _) => {
                self.research_preedit = text.chars().take(256).collect();
                true
            }
            Ime::Commit(text) => {
                self.research_preedit.clear();
                self.append_research_text(text);
                true
            }
            Ime::Disabled => {
                let changed = !self.research_preedit.is_empty();
                self.research_preedit.clear();
                changed
            }
            Ime::Enabled => false,
        }
    }

    fn chrome_at_cursor(&self) -> Option<ChromeControl> {
        self.cursor.and_then(|cursor| {
            hit_fixed_chrome(
                cursor,
                self.physical_width,
                self.physical_height,
                self.scale_factor,
            )
        })
    }

    fn activate_chrome(&mut self, control: ChromeControl) -> bool {
        match control {
            ChromeControl::ZoomOut => self.interaction.zoom_by(1.0 / 1.18),
            ChromeControl::ZoomIn => self.interaction.zoom_by(1.18),
            ChromeControl::ResetZoom => self.interaction.reset_zoom(),
            ChromeControl::ZoomLabel => false,
            ChromeControl::Record => self.toggle_voice(),
            ChromeControl::Clear => self.clear_canvas(),
        }
    }

    fn clear_canvas(&mut self) -> bool {
        if self.research.is_some() || self.research_preflight.is_some() || !self.voice.is_idle() {
            self.status = Some(
                "Canvas cannot be cleared while recording, transcribing, or researching; press Escape to cancel first."
                    .into(),
            );
            return true;
        }
        let operation_id = self.next_operation_id("clear");
        let result = self
            .store
            .as_ref()
            .ok_or_else(|| "the graph store is unavailable".to_owned())
            .and_then(|store| {
                store
                    .clear(&operation_id)
                    .map_err(|error| error.to_string())
            });
        match result {
            Ok(graph) => {
                self.graph = graph;
                self.interaction = CanvasInteraction::new(resolved_positions(&self.graph));
                self.interaction.resize(
                    physical_dimension(self.physical_width),
                    physical_dimension(self.physical_height),
                );
                self.research_input = None;
                self.research_preedit.clear();
                self.curation = None;
                self.curation_preedit.clear();
                self.research_messages.clear();
                self.activity_scroll = 0.0;
                self.speech.cancel_session();
                self.generated_research_events.clear();
                self.auto_follow_research = true;
                self.activity_open = false;
                self.activity_openness = 0.0;
                self.activity_motion = None;
                self.status = Some("Canvas cleared.".into());
            }
            Err(error) => self.status = Some(format!("Canvas was not cleared: {error}")),
        }
        true
    }

    fn import_legacy(&mut self, source: &Path) -> bool {
        if self.research.is_some() || self.research_preflight.is_some() || !self.voice.is_idle() {
            self.status = Some(
                "Legacy import is unavailable while recording, transcribing, or researching."
                    .into(),
            );
            return true;
        }
        let result = self
            .store
            .as_ref()
            .ok_or_else(|| "the graph store is unavailable".to_owned())
            .and_then(|store| {
                store
                    .import_legacy(source)
                    .map_err(|error| error.to_string())
            });
        match result {
            Ok(graph) => {
                let events = graph.events.len();
                let bridges = graph.bridges.len();
                self.graph = graph;
                self.interaction = CanvasInteraction::new(resolved_positions(&self.graph));
                self.interaction.resize(
                    physical_dimension(self.physical_width),
                    physical_dimension(self.physical_height),
                );
                self.research_input = None;
                self.research_preedit.clear();
                self.curation = None;
                self.curation_preedit.clear();
                self.research_messages.clear();
                self.activity_scroll = 0.0;
                self.speech.cancel_session();
                self.generated_research_events.clear();
                self.auto_follow_research = true;
                self.activity_open = false;
                self.activity_openness = 0.0;
                self.activity_motion = None;
                self.metadata_scroll_event = None;
                self.metadata_scroll = 0.0;
                self.status = Some(format!(
                    "Imported {events} events and {bridges} relationships; the source database was not changed."
                ));
            }
            Err(error) => self.status = Some(format!("Legacy graph was not imported: {error}")),
        }
        true
    }

    fn toggle_voice(&mut self) -> bool {
        if self.research.is_some() || self.research_preflight.is_some() {
            self.status = Some("Research is already running; press Escape to cancel it.".into());
            return true;
        }
        match std::mem::replace(&mut self.voice, VoiceState::Idle) {
            VoiceState::Idle => self.start_recording(),
            VoiceState::Recording(recorder) => self.finish_recording(recorder),
            transcribing @ VoiceState::Transcribing(_) => {
                self.voice = transcribing;
                self.status =
                    Some("Transcription is already running; press Escape to cancel it.".into());
                true
            }
        }
    }

    fn start_recording(&mut self) -> bool {
        if let Err(error) = Self::transcription_config() {
            self.status = Some(format!(
                "Voice research unavailable: {error}. Press Ctrl+, to configure Groq."
            ));
            return true;
        }
        let recording_id = self.next_operation_id("voice");
        let path = self.voice_directory.join(format!("{recording_id}.wav"));
        match Recorder::start(path) {
            Ok(recorder) => {
                self.voice = VoiceState::Recording(recorder);
                self.status = Some("Listening... tap again to research.".into());
            }
            Err(error) => {
                self.status = Some(format!("Could not start recording: {error}"));
            }
        }
        true
    }

    fn finish_recording(&mut self, recorder: Recorder) -> bool {
        self.status = Some("Transcribing with Groq Whisper v3 Turbo...".into());
        let result = recorder.stop().and_then(|recording| {
            Self::transcription_config()
                .map_err(|_| not_news_audio::AudioError::MissingApiKey)
                .and_then(|config| TranscriptionHandle::start(recording, config))
        });
        match result {
            Ok(handle) => self.voice = VoiceState::Transcribing(handle),
            Err(error) => {
                self.status = Some(format!("Recording failed: {error}"));
            }
        }
        true
    }

    fn transcription_config() -> Result<TranscriptionConfig, String> {
        groq_api_key()
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "no Groq API key is configured".to_owned())
            .and_then(|key| {
                TranscriptionConfig::from_api_key(key).map_err(|error| error.to_string())
            })
    }

    fn cancel_voice(&mut self) -> bool {
        match std::mem::replace(&mut self.voice, VoiceState::Idle) {
            VoiceState::Idle => false,
            VoiceState::Recording(recorder) => {
                recorder.cancel();
                self.status = Some("Recording cancelled.".into());
                true
            }
            VoiceState::Transcribing(handle) => {
                handle.cancel();
                self.status = Some("Transcription cancelled.".into());
                true
            }
        }
    }

    fn drain_voice(&mut self) -> bool {
        let outcome = match &self.voice {
            VoiceState::Idle => return false,
            VoiceState::Recording(recorder) => recorder.failure().map(Err),
            VoiceState::Transcribing(handle) => match handle.try_recv() {
                Ok(None) => None,
                Ok(Some(transcript)) => Some(Ok(transcript)),
                Err(error) => Some(Err(error)),
            },
        };
        let Some(outcome) = outcome else {
            return false;
        };
        let previous = std::mem::replace(&mut self.voice, VoiceState::Idle);
        if let VoiceState::Recording(recorder) = previous {
            recorder.cancel();
        }
        match outcome {
            Ok(transcript) => {
                self.status = Some(format!("Transcript: {transcript}"));
                self.start_research(&transcript);
            }
            Err(error) => self.status = Some(format!("Recording failed: {error}")),
        }
        true
    }

    fn start_research(&mut self, question: &str) -> bool {
        let question = question.trim();
        if question.is_empty() {
            self.status = Some("Research question cannot be empty.".into());
            return true;
        }
        if self.research.is_some() || self.research_preflight.is_some() {
            self.status = Some(
                "Research readiness or execution is already active; press Escape to cancel it."
                    .into(),
            );
            return true;
        }
        if self.store.is_none() {
            self.status = Some("Research is unavailable because the graph did not open.".into());
            return true;
        }
        let Some(profile) = self.hermes_profile.as_ref() else {
            self.status = Some(
                "Research unavailable: the Not News Hermes profile is unavailable; open Connections for the startup diagnostic."
                    .into(),
            );
            return true;
        };
        let profile = hermes_profile::InstalledProfile {
            root: profile.root.clone(),
            home: profile.home.clone(),
            policy_version: profile.policy_version,
        };
        let data_directory = self.data_directory.clone();
        let (sender, receiver) = mpsc::sync_channel(1);
        if let Err(error) = std::thread::Builder::new()
            .name("research-readiness".into())
            .spawn(move || {
                let _ = sender.send(prepare_research(&data_directory, &profile));
            })
        {
            self.status = Some(format!(
                "Research readiness worker could not start: {error}"
            ));
            return true;
        }
        self.research_preflight = Some(ResearchPreflight {
            question: question.to_owned(),
            receiver,
        });
        self.status = Some(
            "Checking Hermes ACP/profile compatibility, Browse, curl, the Not News vault, and SearXNG before creating a research session…"
                .into(),
        );
        true
    }

    fn drain_research_preflight(&mut self) -> bool {
        let Some(preflight) = self.research_preflight.as_ref() else {
            return false;
        };
        let result = match preflight.receiver.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return false,
            Err(TryRecvError::Disconnected) => {
                self.research_preflight = None;
                self.status = Some("Research readiness worker stopped without evidence.".into());
                return true;
            }
        };
        let question = self
            .research_preflight
            .take()
            .expect("preflight existence checked")
            .question;
        match result {
            Ok(prepared) => self.begin_research(&question, prepared),
            Err(error) => {
                self.status = Some(format!(
                    "Research did not start; the offline canvas remains available. {error}"
                ));
            }
        }
        true
    }

    fn begin_research(&mut self, question: &str, prepared: PreparedResearch) {
        let PreparedResearch {
            environment,
            evidence,
        } = prepared;
        let session_id = self.next_operation_id("research");
        if let Err(error) = self
            .store
            .as_ref()
            .expect("store existence checked")
            .start_research_session(&session_id, question)
        {
            self.status = Some(format!("Research session could not start: {error}"));
            return;
        }
        let prompt = build_research_prompt(question, &self.graph);
        let scratch_directory = self.research_directory.join(&session_id);
        let Some(profile) = self.hermes_profile.as_ref() else {
            let error = "the application-owned Hermes profile is unavailable";
            let _ = self
                .store
                .as_ref()
                .expect("store existence checked")
                .finish_research_session(&session_id, 0, ResearchSessionStatus::Error, error);
            self.status = Some(format!("Research unavailable: {error}."));
            return;
        };
        let launch = match ResearchLaunch::for_hermes_profile(
            &prompt,
            &scratch_directory,
            &profile.root,
            hermes_profile::PROFILE_ID,
        ) {
            Ok(launch) => launch.with_resolved_environment(environment),
            Err(error) => {
                let _ = self
                    .store
                    .as_ref()
                    .expect("store existence checked")
                    .finish_research_session(
                        &session_id,
                        0,
                        ResearchSessionStatus::Error,
                        &error.to_string(),
                    );
                self.status = Some(format!("Research backend unavailable: {error}"));
                return;
            }
        };
        match launch.spawn() {
            Ok(handle) => {
                self.speech.reset_session();
                self.research = Some(ActiveResearch {
                    session_id,
                    handle,
                    next_sequence: 0,
                    deferred_bridges: VecDeque::new(),
                    closed: false,
                    scratch_directory,
                });
                self.research_messages.clear();
                self.activity_scroll = 0.0;
                self.generated_research_events.clear();
                self.auto_follow_research = true;
                self.set_activity_open(true, Instant::now());
                self.status = Some(
                    "Readiness passed at the declared layers; starting research. Hermes provider authentication and live discovery remain task evidence."
                        .into(),
                );
                self.record_research_message(
                    ResearchOutputKind::Message,
                    &format!("Preflight evidence: {evidence}"),
                );
            }
            Err(error) => {
                let _ = self
                    .store
                    .as_ref()
                    .expect("store existence checked")
                    .finish_research_session(
                        &session_id,
                        0,
                        ResearchSessionStatus::Error,
                        &error.to_string(),
                    );
                self.status = Some(format!("Research supervisor could not start: {error}"));
            }
        }
    }

    fn drain_research(&mut self) -> bool {
        let mut events = Vec::new();
        if let Some(active) = self.research.as_ref() {
            while let Ok(event) = active.handle.try_recv() {
                events.push(event);
            }
        }
        let changed = !events.is_empty();
        for event in events {
            self.handle_research_event(event);
        }
        changed
    }

    fn drain_speech(&mut self) -> bool {
        let mut changed = false;
        while let Ok(event) = self.speech.try_recv() {
            match event {
                SpeechEvent::Failed(message) => {
                    self.status = Some(format!(
                        "Voice playback unavailable: {message}. Research continues."
                    ));
                    changed = true;
                }
                SpeechEvent::Played | SpeechEvent::Cancelled | SpeechEvent::Stale => {}
            }
        }
        changed
    }

    fn handle_research_event(&mut self, event: ResearchProcessEvent) {
        if self.research.as_ref().is_some_and(|active| active.closed)
            && !matches!(event, ResearchProcessEvent::Finished(_))
        {
            return;
        }
        match event {
            ResearchProcessEvent::Started { backend, .. } => {
                self.record_research_message(
                    ResearchOutputKind::Message,
                    &format!("{backend:?} research started."),
                );
            }
            ResearchProcessEvent::Output(output) => self.accept_agent_output(output),
            ResearchProcessEvent::ProtocolError(message) => {
                self.record_research_message(ResearchOutputKind::ProtocolError, &message);
            }
            ResearchProcessEvent::Diagnostic(message) => {
                self.record_research_message(
                    ResearchOutputKind::ProtocolError,
                    &format!("Research backend: {message}"),
                );
            }
            ResearchProcessEvent::Finished(termination) => {
                self.finish_process(termination);
            }
        }
    }

    fn accept_agent_output(&mut self, output: AgentEvent) {
        match output {
            AgentEvent::SessionMessage(message) => {
                self.record_research_message(ResearchOutputKind::Message, &message);
            }
            AgentEvent::VoiceNote(message) => {
                if self.record_research_message(ResearchOutputKind::VoiceNote, &message) {
                    match self.speech.submit_note(&message, Instant::now()) {
                        SpeechSubmit::Unavailable(reason) => {
                            self.status = Some(format!(
                                "Voice note logged; playback unavailable: {reason}. Research continues."
                            ));
                        }
                        SpeechSubmit::QueueFull => {
                            self.status = Some(
                                "Voice note logged; the bounded playback queue was full. Research continues."
                                    .into(),
                            );
                        }
                        SpeechSubmit::Queued
                        | SpeechSubmit::Disabled
                        | SpeechSubmit::Empty
                        | SpeechSubmit::Duplicate
                        | SpeechSubmit::Throttled
                        | SpeechSubmit::SessionLimit => {}
                    }
                }
            }
            AgentEvent::EventUpsert(event) => {
                let Some((session, sequence)) = self.research_cursor() else {
                    return;
                };
                let result = self
                    .store
                    .as_ref()
                    .ok_or_else(|| "graph store is unavailable".to_owned())
                    .and_then(|store| {
                        store
                            .accept_research_event(&session, sequence, &event)
                            .map_err(|error| error.to_string())
                    });
                match result {
                    Ok(outcome) => {
                        let canonical = not_news_domain::EventId(outcome.canonical_key.clone());
                        let generated = !self.graph.events.contains_key(&canonical)
                            && outcome.snapshot.events.contains_key(&canonical);
                        self.interaction.graph_committed(
                            &self.graph,
                            &outcome.snapshot,
                            Instant::now(),
                        );
                        self.advance_research_cursor();
                        self.graph = outcome.snapshot;
                        if generated {
                            self.generated_research_events.insert(canonical);
                        }
                        self.focus_generated_research();
                        self.status = Some(format!("Accepted finding: {}", event.title));
                        self.retry_deferred_bridges();
                    }
                    Err(error) => self.abort_research(format!(
                        "Finding was not committed; research stopped: {error}"
                    )),
                }
            }
            AgentEvent::BridgeUpsert(bridge) => self.accept_or_defer_bridge(bridge),
            AgentEvent::SessionDone(message) => {
                self.reject_unresolved_bridges();
                self.close_research(ResearchSessionStatus::Done, &message, true);
            }
            AgentEvent::SessionError(message) => {
                self.close_research(ResearchSessionStatus::Error, &message, true);
            }
        }
    }

    fn accept_or_defer_bridge(&mut self, bridge: BridgeUpsert) {
        let Some((session, sequence)) = self.research_cursor() else {
            return;
        };
        let result = self.store.as_ref().map_or_else(
            || Err(StoreError::MissingResearchSession(session.clone())),
            |store| {
                store.accept_research_bridge(
                    &session,
                    sequence,
                    &bridge.from,
                    &bridge.to,
                    &bridge.label,
                )
            },
        );
        match result {
            Ok(outcome) => {
                self.interaction
                    .graph_committed(&self.graph, &outcome.snapshot, Instant::now());
                self.advance_research_cursor();
                self.graph = outcome.snapshot;
                self.focus_generated_research();
                self.status = Some(format!("Accepted relationship: {}", bridge.label));
            }
            Err(StoreError::MissingResearchEndpoint(_)) => {
                let message = format!(
                    "Deferred relationship {} → {} until both events exist.",
                    bridge.from.0, bridge.to.0
                );
                if self.record_research_message(ResearchOutputKind::ProtocolError, &message)
                    && let Some(active) = self.research.as_mut()
                {
                    active.deferred_bridges.push_back(bridge);
                }
            }
            Err(error) => self.record_or_abort_bridge_error(&error),
        }
    }

    fn retry_deferred_bridges(&mut self) {
        let mut pending = self
            .research
            .as_mut()
            .map(|active| std::mem::take(&mut active.deferred_bridges))
            .unwrap_or_default();
        while let Some(bridge) = pending.pop_front() {
            self.accept_or_defer_bridge(bridge);
            if self.research.as_ref().is_none_or(|active| active.closed) {
                break;
            }
        }
        if let Some(active) = self.research.as_mut() {
            active.deferred_bridges.extend(pending);
        }
    }

    fn reject_unresolved_bridges(&mut self) {
        let deferred = self
            .research
            .as_mut()
            .map(|active| std::mem::take(&mut active.deferred_bridges))
            .unwrap_or_default();
        for bridge in deferred {
            self.record_research_message(
                ResearchOutputKind::ProtocolError,
                &format!(
                    "Ignored unresolved relationship {} → {}.",
                    bridge.from.0, bridge.to.0
                ),
            );
        }
    }

    fn record_or_abort_bridge_error(&mut self, error: &StoreError) {
        let message = format!("Ignored invalid relationship: {error}");
        if !self.record_research_message(ResearchOutputKind::ProtocolError, &message) {
            self.abort_research(message);
        }
    }

    fn record_research_message(&mut self, kind: ResearchOutputKind, message: &str) -> bool {
        let Some((session, sequence)) = self.research_cursor() else {
            return false;
        };
        let result = self
            .store
            .as_ref()
            .ok_or_else(|| "graph store is unavailable".to_owned())
            .and_then(|store| {
                store
                    .record_research_output(&session, sequence, kind, message)
                    .map_err(|error| error.to_string())
            });
        match result {
            Ok(_) => {
                let previous_scroll_max = self.activity_scroll_maximum();
                let preserve_visible_history = self.activity_scroll > f64::EPSILON;
                self.advance_research_cursor();
                self.status = Some(message.to_owned());
                if self
                    .research_messages
                    .last()
                    .is_none_or(|previous| previous != message)
                {
                    if self.research_messages.len() >= 80 {
                        self.research_messages.remove(0);
                    }
                    self.research_messages.push(message.to_owned());
                }
                if preserve_visible_history {
                    let scroll_max = self.activity_scroll_maximum();
                    self.activity_scroll = (self.activity_scroll
                        + (scroll_max - previous_scroll_max))
                        .clamp(0.0, scroll_max);
                }
                true
            }
            Err(error) => {
                self.abort_research(format!("Research output could not be recorded: {error}"));
                false
            }
        }
    }

    fn research_cursor(&self) -> Option<(String, u64)> {
        self.research
            .as_ref()
            .filter(|active| !active.closed)
            .map(|active| (active.session_id.clone(), active.next_sequence))
    }

    fn advance_research_cursor(&mut self) {
        if let Some(active) = self.research.as_mut() {
            active.next_sequence = active
                .next_sequence
                .checked_add(1)
                .expect("one process cannot emit 2^64 research outputs");
        }
    }

    fn close_research(
        &mut self,
        state: ResearchSessionStatus,
        message: &str,
        cancel_process: bool,
    ) {
        let Some((session, sequence)) = self.research_cursor() else {
            return;
        };
        match self.store.as_ref().map_or_else(
            || Err(StoreError::MissingResearchSession(session.clone())),
            |store| store.finish_research_session(&session, sequence, state, message),
        ) {
            Ok(_) => {
                self.advance_research_cursor();
                if let Some(active) = self.research.as_mut() {
                    active.closed = true;
                    if cancel_process {
                        active.handle.cancel();
                    }
                }
                self.status = Some(message.to_owned());
            }
            Err(error) => self.abort_research(format!(
                "Research could not be finalized; accepted findings remain: {error}"
            )),
        }
    }

    fn abort_research(&mut self, message: String) {
        if let Some((session, sequence)) = self.research_cursor()
            && let Some(store) = self.store.as_ref()
            && store
                .finish_research_session(&session, sequence, ResearchSessionStatus::Error, &message)
                .is_ok()
        {
            self.advance_research_cursor();
        }
        if let Some(active) = self.research.as_mut() {
            active.closed = true;
            active.handle.cancel();
        }
        self.status = Some(message);
    }

    fn finish_process(&mut self, termination: ResearchTermination) {
        let already_closed = self.research.as_ref().is_none_or(|active| active.closed);
        if !already_closed {
            match termination {
                ResearchTermination::Completed => {
                    self.reject_unresolved_bridges();
                    self.close_research(ResearchSessionStatus::Done, "Research completed.", false);
                }
                other => self.close_research(
                    ResearchSessionStatus::Error,
                    &termination_message(&other),
                    false,
                ),
            }
        }
        if let Some(active) = self.research.take() {
            let _ = std::fs::remove_dir_all(active.scratch_directory);
        }
        self.generated_research_events.clear();
    }

    fn focus_generated_research(&mut self) {
        if self.metadata_focus.is_none()
            && self.auto_follow_research
            && !self.generated_research_events.is_empty()
        {
            self.interaction
                .focus_events(&self.generated_research_events, Instant::now());
        }
    }

    fn stop_auto_follow_if_panning(&mut self) {
        if self.interaction.manual_pan_active() {
            self.auto_follow_research = false;
            self.generated_research_events.clear();
        }
    }

    fn activity_visible(&self) -> bool {
        self.research.is_some()
            || self.research_preflight.is_some()
            || !self.research_messages.is_empty()
    }

    fn activity_progress(&mut self, now: Instant) -> f64 {
        let Some(motion) = self.activity_motion else {
            return self.activity_openness;
        };
        let linear = (now.saturating_duration_since(motion.started).as_secs_f64()
            / Motion::PANEL.as_secs_f64())
        .clamp(0.0, 1.0);
        self.activity_openness =
            motion.from + (motion.to - motion.from) * Motion::ease_out_cubic(linear);
        if linear >= 1.0 {
            self.activity_openness = motion.to;
            self.activity_motion = None;
        }
        self.activity_openness
    }

    fn set_activity_open(&mut self, open: bool, now: Instant) {
        let current = self.activity_progress(now);
        let target = if open { 1.0 } else { 0.0 };
        self.activity_open = open;
        if (current - target).abs() < f64::EPSILON {
            self.activity_openness = target;
            self.activity_motion = None;
        } else {
            self.activity_motion = Some(ActivityMotion {
                from: current,
                to: target,
                started: now,
            });
        }
    }

    fn activity_toggle_at_cursor(&mut self, now: Instant) -> bool {
        let Some(cursor) = self.cursor else {
            return false;
        };
        let progress = self.activity_progress(now);
        self.activity_visible()
            && hit_activity_toggle(cursor, self.physical_width, self.scale_factor, progress)
    }

    fn activity_surface_at_cursor(&mut self, now: Instant) -> bool {
        let Some(cursor) = self.cursor else {
            return false;
        };
        let progress = self.activity_progress(now);
        self.activity_visible()
            && hit_activity_surface(
                cursor,
                self.physical_width,
                self.physical_height,
                self.scale_factor,
                progress,
            )
    }

    fn activity_scroll_maximum(&self) -> f64 {
        activity_scroll_max(
            self.physical_width,
            self.physical_height,
            self.scale_factor,
            &self.research_messages,
        )
    }

    fn scroll_activity(&mut self, delta: MouseScrollDelta) -> bool {
        let maximum = self.activity_scroll_maximum();
        let next =
            (self.activity_scroll + scroll_pixels(delta) / self.scale_factor).clamp(0.0, maximum);
        let changed = (next - self.activity_scroll).abs() > f64::EPSILON;
        self.activity_scroll = next;
        changed
    }

    fn cursor_moved(&mut self, point: Point, now: Instant) -> bool {
        let previous_curation_row = self.curation_menu_at_cursor();
        self.cursor = Some(point);
        if self.metadata_focus.is_some() {
            return self.interaction.retain_active();
        }
        if self.record_hold_deadline.is_some()
            && (self.metadata_at_cursor(now)
                || self.chrome_at_cursor() != Some(ChromeControl::Record))
        {
            self.record_hold_deadline = None;
        }
        if matches!(self.curation, Some(CurationFlow::Menu { .. })) {
            let selection_changed = previous_curation_row != self.curation_menu_at_cursor();
            if self.curation_menu_surface_at_cursor() {
                self.interaction.cursor_left(now) || selection_changed
            } else {
                self.interaction.cursor_moved(point, &self.graph, now) || selection_changed
            }
        } else if self.settings.is_some() {
            let selection_changed = self
                .settings_menu_at_cursor()
                .is_some_and(|row| self.select_settings_row(row));
            self.interaction.cursor_left(now) || selection_changed
        } else if matches!(self.pointer_owner, PointerOwner::Canvas) {
            let changed = self.interaction.cursor_moved(point, &self.graph, now);
            self.stop_auto_follow_if_panning();
            changed
        } else if self.activity_surface_at_cursor(now) {
            self.interaction.cursor_left(now)
        } else if self.metadata_at_cursor(now) {
            self.interaction.retain_active()
        } else if self.chrome_at_cursor().is_some() {
            self.interaction.cursor_left(now)
        } else {
            self.interaction.cursor_moved(point, &self.graph, now)
        }
    }

    fn mouse_input(&mut self, state: ElementState, now: Instant) -> bool {
        if self.metadata_focus.is_some() {
            self.pointer_owner = if state == ElementState::Pressed {
                PointerOwner::MetadataSurface
            } else {
                PointerOwner::None
            };
            return self.interaction.retain_active();
        }
        if state == ElementState::Pressed {
            if let Some(changed) = self.settings_left_press() {
                self.pointer_owner = PointerOwner::ConsumedChrome;
                return changed;
            }
            if let Some(changed) = self.curation_left_press(now) {
                self.pointer_owner = PointerOwner::ConsumedChrome;
                return changed;
            }
            if self.activity_toggle_at_cursor(now) {
                self.pointer_owner = PointerOwner::ActivityToggle;
                return self.interaction.cursor_left(now);
            }
            if self.activity_surface_at_cursor(now) {
                self.pointer_owner = PointerOwner::ActivitySurface;
                return self.interaction.cursor_left(now);
            }
            if self.metadata_at_cursor(now) {
                self.pointer_owner = PointerOwner::MetadataSurface;
                return self.interaction.retain_active();
            }
            if let Some(control) = self.chrome_at_cursor() {
                self.pointer_owner = PointerOwner::FixedChrome(control);
                if control == ChromeControl::Record && self.voice.is_recording() {
                    self.record_hold_deadline = Some(now + Duration::from_millis(500));
                }
                return self.interaction.cursor_left(now);
            }
            self.pointer_owner = PointerOwner::Canvas;
            return self.interaction.pointer_down(&self.graph, now);
        }

        self.record_hold_deadline = None;
        match std::mem::take(&mut self.pointer_owner) {
            PointerOwner::Canvas => {
                let effect = self.interaction.pointer_up(&self.graph, now);
                self.commit_effect(effect)
            }
            PointerOwner::ActivityToggle if self.activity_toggle_at_cursor(now) => {
                self.set_activity_open(!self.activity_open, now);
                true
            }
            PointerOwner::FixedChrome(pressed)
                if self
                    .chrome_at_cursor()
                    .is_some_and(|released| pressed == released) =>
            {
                self.activate_chrome(pressed)
            }
            PointerOwner::None
            | PointerOwner::ActivityToggle
            | PointerOwner::ActivitySurface
            | PointerOwner::MetadataSurface
            | PointerOwner::ConsumedChrome
            | PointerOwner::FixedChrome(_) => false,
        }
    }

    fn right_mouse_input(&mut self, state: ElementState, now: Instant) -> bool {
        if self.settings.is_some() || self.metadata_focus.is_some() {
            self.pointer_owner = PointerOwner::ConsumedChrome;
            return state == ElementState::Pressed;
        }
        if state == ElementState::Pressed {
            self.pointer_owner = PointerOwner::ConsumedChrome;
            self.open_curation_menu(now)
        } else {
            false
        }
    }

    fn resolve_record_hold(&mut self, now: Instant) -> bool {
        if self
            .record_hold_deadline
            .is_none_or(|deadline| now < deadline)
        {
            return false;
        }
        self.record_hold_deadline = None;
        if matches!(
            self.pointer_owner,
            PointerOwner::FixedChrome(ChromeControl::Record)
        ) && self.voice.is_recording()
        {
            self.pointer_owner = PointerOwner::ConsumedChrome;
            return self.cancel_voice();
        }
        false
    }

    fn active_metadata_screen_position(&self, now: Instant) -> Option<(EventId, Point)> {
        let (active, world) = self
            .interaction
            .active_event_position_at(&self.graph, now)?;
        let transform = ViewportTransform::new(
            self.physical_width,
            self.physical_height,
            self.interaction.viewport(),
        );
        Some((active.clone(), transform.world_to_screen(world)))
    }

    fn metadata_at_cursor(&self, now: Instant) -> bool {
        let Some(cursor) = self.cursor else {
            return false;
        };
        let Some((active, position)) = self.active_metadata_screen_position(now) else {
            return false;
        };
        hit_active_metadata(
            cursor,
            self.physical_width,
            self.physical_height,
            self.scale_factor,
            &self.graph.events[&active],
            position,
        )
    }

    fn scroll_metadata_by(&mut self, logical_delta: f64) -> bool {
        let Some((active, _)) = self.interaction.active_event_position() else {
            return false;
        };
        let active = active.clone();
        self.sync_metadata_scroll(Some(&active));
        let event = &self.graph.events[&active];
        let maximum = active_metadata_scroll_max(
            self.physical_width,
            self.physical_height,
            self.scale_factor,
            event,
        );
        let next = (self.metadata_scroll + logical_delta).clamp(0.0, maximum);
        let changed = (next - self.metadata_scroll).abs() > f64::EPSILON;
        self.metadata_scroll = next;
        changed
    }

    fn scroll_metadata(&mut self, delta: MouseScrollDelta) -> bool {
        self.scroll_metadata_by(-scroll_pixels(delta) / self.scale_factor)
    }

    fn metadata_has_overflow(&self) -> bool {
        let Some((active, _)) = self.interaction.active_event_position() else {
            return false;
        };
        active_metadata_scroll_max(
            self.physical_width,
            self.physical_height,
            self.scale_factor,
            &self.graph.events[active],
        ) > 0.0
    }

    fn focus_metadata(&mut self) -> bool {
        let Some((active, _)) = self.interaction.active_event_position() else {
            return false;
        };
        let active = active.clone();
        if !self.metadata_has_overflow() {
            return false;
        }
        let changed = self.metadata_focus.as_ref() != Some(&active);
        self.metadata_focus = Some(active);
        self.interaction.freeze_view() || changed
    }

    fn release_metadata_focus(&mut self, now: Instant) -> bool {
        if self.metadata_focus.take().is_none() {
            return false;
        }
        if self.metadata_at_cursor(now) {
            self.interaction.retain_active();
        } else if let Some(cursor) = self.cursor {
            self.interaction.cursor_moved(cursor, &self.graph, now);
        } else {
            self.interaction.cursor_left(now);
        }
        true
    }

    fn sync_metadata_scroll(&mut self, active: Option<&EventId>) {
        if self.metadata_scroll_event.as_ref() == active {
            return;
        }
        let active = active.cloned();
        if self.metadata_focus.as_ref() != active.as_ref() {
            self.metadata_focus = None;
        }
        self.metadata_scroll_event.clone_from(&active);
        self.metadata_scroll = 0.0;
    }

    fn scroll_settings_menu(&mut self, delta: MouseScrollDelta) -> bool {
        let items = self
            .settings_choices()
            .into_iter()
            .map(|(_, label)| label)
            .collect::<Vec<_>>();
        let maximum = curation_menu_scroll_max(
            self.physical_width,
            self.physical_height,
            self.scale_factor,
            self.settings_anchor(),
            &items,
        );
        let next =
            (self.settings_scroll - scroll_pixels(delta) / self.scale_factor).clamp(0.0, maximum);
        let changed = (next - self.settings_scroll).abs() > f64::EPSILON;
        self.settings_scroll = next;
        changed
    }

    fn scroll_curation_menu(&mut self, delta: MouseScrollDelta) -> bool {
        let Some(CurationFlow::Menu {
            anchor,
            subject,
            stage,
            page,
            ..
        }) = self.curation.as_ref()
        else {
            return false;
        };
        let items = self
            .curation_choices(subject, *stage, *page)
            .into_iter()
            .map(|(_, label)| label)
            .collect::<Vec<_>>();
        let maximum = curation_menu_scroll_max(
            self.physical_width,
            self.physical_height,
            self.scale_factor,
            *anchor,
            &items,
        );
        let next =
            (self.curation_scroll - scroll_pixels(delta) / self.scale_factor).clamp(0.0, maximum);
        let changed = (next - self.curation_scroll).abs() > f64::EPSILON;
        self.curation_scroll = next;
        changed
    }

    fn curation_menu_can_scroll(&self) -> bool {
        let Some(CurationFlow::Menu {
            anchor,
            subject,
            stage,
            page,
            ..
        }) = self.curation.as_ref()
        else {
            return false;
        };
        let items = self
            .curation_choices(subject, *stage, *page)
            .into_iter()
            .map(|(_, label)| label)
            .collect::<Vec<_>>();
        curation_menu_scroll_max(
            self.physical_width,
            self.physical_height,
            self.scale_factor,
            *anchor,
            &items,
        ) > 0.0
    }

    fn record_orb_state(&self) -> RecordOrbState {
        if self.research.is_some() || self.research_preflight.is_some() || !self.voice.is_idle() {
            RecordOrbState::Busy
        } else {
            RecordOrbState::Idle
        }
    }

    fn paint_composer(&self, canvas: &Canvas, width: f32, height: f32, scale_factor: f32) {
        if let Some(SettingsFlow::Credential { kind, input }) = self.settings.as_ref() {
            paint_credential_prompt(
                canvas,
                width,
                height,
                scale_factor,
                kind.title(),
                &masked_secret(input),
                &masked_secret(&self.settings_preedit),
                kind.placeholder(),
            );
        } else if let Some(SettingsFlow::SearxngEndpoint { input }) = self.settings.as_ref() {
            paint_connection_prompt(
                canvas,
                width,
                height,
                scale_factor,
                input,
                &self.settings_preedit,
            );
        } else if let Some(SettingsFlow::EraseConfirmation { input }) = self.settings.as_ref() {
            paint_curation_prompt(
                canvas,
                width,
                height,
                scale_factor,
                "COMPLETE ERASE",
                "TYPE ERASE  ·  →/ENTER DELETE  ·  ←/ESC KEEP EVERYTHING",
                input,
                &self.settings_preedit,
            );
        } else if let Some(CurationFlow::RelatePredicate { input, .. }) = self.curation.as_ref() {
            paint_curation_prompt(
                canvas,
                width,
                height,
                scale_factor,
                "RELATE",
                "ENTER COMMIT  ·  ESC CANCEL  ·  CTRL+V PASTE",
                input,
                &self.curation_preedit,
            );
        } else if let Some(CurationFlow::PromotePredicate { input, .. }) = self.curation.as_ref() {
            paint_curation_prompt(
                canvas,
                width,
                height,
                scale_factor,
                "PROMOTE",
                "ENTER PROMOTE  ·  OPTIONAL TEXT RELATES  ·  ESC CANCEL",
                input,
                &self.curation_preedit,
            );
        } else if let Some(prompt) = self.research_input.as_deref() {
            paint_research_prompt(
                canvas,
                width,
                height,
                scale_factor,
                prompt,
                &self.research_preedit,
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn paint_foreground(
        &self,
        canvas: &Canvas,
        width: f32,
        height: f32,
        scale_factor: f32,
        viewport: not_news_renderer::Viewport,
        state: &InteractionFrame,
        activity_progress: f64,
    ) {
        if let Some(status) = self.status.as_deref() {
            paint_status(
                canvas,
                width,
                height,
                scale_factor,
                status,
                self.research.is_some() || self.research_preflight.is_some(),
            );
        }
        paint_fixed_chrome(
            canvas,
            width,
            height,
            scale_factor,
            viewport.zoom,
            self.record_orb_state(),
        );
        if let Some(active) = state.expanded_event.as_ref() {
            let screen_position =
                ViewportTransform::new(f64::from(width), f64::from(height), viewport)
                    .world_to_screen(state.positions[active]);
            paint_active_metadata(
                canvas,
                width,
                height,
                scale_factor,
                &self.graph.events[active],
                screen_position,
                scale_scalar(self.metadata_scroll),
                self.metadata_focus.as_ref() == Some(active),
            );
        }
        if self.activity_visible() {
            paint_activity_drawer(
                canvas,
                width,
                height,
                scale_factor,
                &self.research_messages,
                self.research.is_some() || self.research_preflight.is_some(),
                scale_scalar(activity_progress),
                scale_scalar(self.activity_scroll),
            );
        }
        self.paint_composer(canvas, width, height, scale_factor);
        self.paint_curation_surface(canvas, width, height, scale_factor);
        self.paint_settings_surface(canvas, width, height, scale_factor);
    }

    fn paint_frame_layers(
        &self,
        canvas: &Canvas,
        frame: FrameInfo,
        viewport: not_news_renderer::Viewport,
        state: &InteractionFrame,
        activity_progress: f64,
    ) {
        let width = physical_scalar(frame.physical_width);
        let height = physical_scalar(frame.physical_height);
        let scale_factor = scale_scalar(frame.scale_factor);
        let scene_state = SceneState {
            animation: SceneAnimation {
                bridge_flow: state.bridge_flow,
            },
            bridge_event: state.bridge_event.as_ref(),
            expanded_event: state.expanded_event.as_ref(),
            expansion_progress: state.expansion_progress,
            collapsing_event: state.collapsing_event.as_ref(),
            collapse_progress: state.collapse_progress,
        };
        paint_background(canvas, width, height);
        paint_grid(canvas, width, height, viewport);
        paint_graph(
            canvas,
            width,
            height,
            viewport,
            &self.graph,
            &state.positions,
            scene_state,
        );
        self.paint_foreground(
            canvas,
            width,
            height,
            scale_factor,
            viewport,
            state,
            activity_progress,
        );
    }

    fn paint_curation_surface(&self, canvas: &Canvas, width: f32, height: f32, scale_factor: f32) {
        let Some(CurationFlow::Menu {
            anchor,
            subject,
            stage,
            page,
            ..
        }) = self.curation.as_ref()
        else {
            return;
        };
        let title = match stage {
            CurationMenuStage::Actions => "CURATE",
            CurationMenuStage::Detach => "DETACH EXACT RELATIONSHIP",
        };
        let items = self
            .curation_choices(subject, *stage, *page)
            .into_iter()
            .map(|(_, label)| label)
            .collect::<Vec<_>>();
        paint_curation_menu(
            canvas,
            width,
            height,
            scale_factor,
            *anchor,
            CurationMenu {
                title,
                items: &items,
                selected: self.curation_menu_at_cursor(),
                scroll_offset: scale_scalar(self.curation_scroll),
            },
        );
    }

    fn paint_settings_surface(&self, canvas: &Canvas, width: f32, height: f32, scale_factor: f32) {
        if !matches!(self.settings, Some(SettingsFlow::Menu { .. })) {
            return;
        }
        let items = self
            .settings_choices()
            .into_iter()
            .map(|(_, label)| label)
            .collect::<Vec<_>>();
        let title = match self.settings.as_ref() {
            Some(SettingsFlow::Menu {
                page: SettingsPage::Credential(kind),
                ..
            }) => format!("←  CONNECTIONS  ·  {}", kind.title()),
            Some(SettingsFlow::Menu {
                page: SettingsPage::Searxng,
                ..
            }) => "←  CONNECTIONS  ·  SEARXNG FRONTIER".into(),
            _ => "CONNECTIONS  ·  CTRL+,".into(),
        };
        paint_curation_menu(
            canvas,
            width,
            height,
            scale_factor,
            self.settings_anchor(),
            CurationMenu {
                title: &title,
                items: &items,
                selected: self.settings_selection(),
                scroll_offset: scale_scalar(self.settings_scroll),
            },
        );
    }
}

impl PlatformApplication for CanvasApplication {
    fn window_event(&mut self, event: &WindowEvent) -> bool {
        let now = Instant::now();
        match event {
            WindowEvent::Resized(size) => {
                self.physical_width = f64::from(size.width);
                self.physical_height = f64::from(size.height);
                self.interaction.resize(size.width, size.height);
                true
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                self.scale_factor = *scale_factor;
                true
            }
            WindowEvent::CursorMoved { position, .. } => {
                let point = Point {
                    x: position.x,
                    y: position.y,
                };
                self.cursor_moved(point, now)
            }
            WindowEvent::CursorLeft { .. } => {
                self.cursor = None;
                self.record_hold_deadline = None;
                if self.metadata_focus.is_some() {
                    self.interaction.retain_active()
                } else {
                    self.interaction.cursor_left(now)
                }
            }
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => self.mouse_input(*state, now),
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Right,
                ..
            } => self.right_mouse_input(*state, now),
            WindowEvent::MouseWheel { delta, .. } => {
                if self.settings.is_some() {
                    self.scroll_settings_menu(*delta)
                } else if matches!(self.curation, Some(CurationFlow::Menu { .. }))
                    && self.curation_menu_surface_at_cursor()
                    && self.curation_menu_can_scroll()
                {
                    self.scroll_curation_menu(*delta)
                } else if self.activity_surface_at_cursor(now) {
                    self.scroll_activity(*delta)
                } else if self.metadata_focus.is_some() || self.metadata_at_cursor(now) {
                    self.scroll_metadata(*delta)
                } else if matches!(self.curation, Some(CurationFlow::Menu { .. })) {
                    self.cursor.is_some_and(|cursor| {
                        self.interaction.scroll_at(scroll_pixels(*delta), cursor)
                    })
                } else {
                    self.interaction.scroll(scroll_pixels(*delta))
                }
            }
            WindowEvent::ModifiersChanged(modifiers) => {
                self.modifiers = modifiers.state();
                false
            }
            WindowEvent::KeyboardInput { event, .. } => self.keyboard_input(event, now),
            WindowEvent::Ime(ime) => self.ime_input(ime),
            WindowEvent::DroppedFile(path) => self.import_legacy(path),
            WindowEvent::Focused(false) => {
                self.pointer_owner = PointerOwner::None;
                self.record_hold_deadline = None;
                let changed = !self.research_preedit.is_empty()
                    || !self.curation_preedit.is_empty()
                    || !self.settings_preedit.is_empty();
                self.research_preedit.clear();
                self.curation_preedit.clear();
                self.settings_preedit.zeroize();
                self.interaction.cancel_pointer() || changed
            }
            _ => false,
        }
    }

    fn render(&mut self, canvas: &Canvas, frame: FrameInfo) -> FrameSchedule {
        self.resolve_record_hold(frame.now);
        self.drain_voice();
        self.drain_speech();
        self.drain_research_preflight();
        self.drain_research();
        self.drain_vault_task();
        let activity_progress = self.activity_progress(frame.now);
        self.interaction
            .resize(frame.physical_width, frame.physical_height);
        self.physical_width = f64::from(frame.physical_width);
        self.physical_height = f64::from(frame.physical_height);
        self.scale_factor = frame.scale_factor;
        let state = self.interaction.frame(&self.graph, frame.now);
        self.sync_metadata_scroll(state.expanded_event.as_ref());
        let viewport = self.interaction.viewport();
        self.paint_frame_layers(canvas, frame, viewport, &state, activity_progress);
        if self.exit_after_present {
            return FrameSchedule::Exit;
        }
        let interaction_deadline = self.interaction.next_deadline(frame.now);
        let research_deadline = self
            .research
            .as_ref()
            .map(|_| frame.now + Duration::from_millis(33));
        let preflight_deadline = self
            .research_preflight
            .as_ref()
            .map(|_| frame.now + Duration::from_millis(100));
        let voice_deadline = (!self.voice.is_idle()).then(|| frame.now + Duration::from_millis(33));
        let speech_deadline = self
            .speech
            .is_busy()
            .then(|| frame.now + Duration::from_millis(50));
        let activity_deadline = self
            .activity_motion
            .map(|_| frame.now + Duration::from_millis(16));
        let vault_deadline = self
            .vault_task
            .as_ref()
            .map(|_| frame.now + Duration::from_millis(100));
        interaction_deadline
            .into_iter()
            .chain(research_deadline)
            .chain(preflight_deadline)
            .chain(voice_deadline)
            .chain(speech_deadline)
            .chain(self.record_hold_deadline)
            .chain(activity_deadline)
            .chain(vault_deadline)
            .min()
            .map_or(FrameSchedule::Wait, FrameSchedule::RedrawAt)
    }

    fn text_input_active(&self) -> bool {
        self.research_input.is_some()
            || matches!(
                self.settings,
                Some(
                    SettingsFlow::Credential { .. }
                        | SettingsFlow::SearxngEndpoint { .. }
                        | SettingsFlow::EraseConfirmation { .. }
                )
            )
            || matches!(
                self.curation,
                Some(CurationFlow::RelatePredicate { .. } | CurationFlow::PromotePredicate { .. })
            )
    }
}

struct PerformanceApplication {
    inner: CanvasApplication,
    frame_index: usize,
}

impl PlatformApplication for PerformanceApplication {
    fn render(&mut self, canvas: &Canvas, frame: FrameInfo) -> FrameSchedule {
        let cycle = self.frame_index % 472;
        let offset = if cycle < 236 { cycle } else { 472 - cycle };
        let cursor = Point {
            x: 40.0 + usize_to_f64(offset) * 5.0,
            y: 400.0,
        };
        let _ = self.inner.cursor_moved(cursor, frame.now);
        if self.frame_index == 0 {
            let _ = self.inner.mouse_input(ElementState::Pressed, frame.now);
        }
        let _ = self.inner.render(canvas, frame);
        self.frame_index += 1;
        if self.frame_index >= PERFORMANCE_WARMUP_FRAMES + PERFORMANCE_MEASURED_FRAMES {
            FrameSchedule::Exit
        } else {
            FrameSchedule::RedrawAt(frame.now)
        }
    }
}

fn remove_external_graph_family(database: &Path) -> io::Result<()> {
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let mut path = database.as_os_str().to_owned();
        path.push(suffix);
        match fs::remove_file(PathBuf::from(path)) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    let Some(parent) = database.parent() else {
        return Ok(());
    };
    let Some(stem) = database.file_stem().and_then(|stem| stem.to_str()) else {
        return Ok(());
    };
    let prefix = format!("{stem}.pre-rust-v");
    for entry in fs::read_dir(parent)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if entry.file_type()?.is_file()
            && name.starts_with(&prefix)
            && (name.ends_with(".sqlite") || name.ends_with(".sqlite.tmp"))
        {
            fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    let options = startup_options(std::env::args_os().skip(1))?;
    if options.show_licenses {
        println!(
            "Manrope\n=======\n{MANROPE_LICENSE}\n\nJetBrains Mono\n===============\n{JETBRAINS_MONO_LICENSE}"
        );
        return Ok(());
    }
    if let Some(root) = options.capability_root {
        return run_capability_check(&root);
    }
    if let Some(source) = options.performance_source {
        return run_reference_performance_check(&source);
    }
    if let Some(root) = options.release_smoke {
        let check = release_check::run(&root)?;
        let empty_database = root.join("empty-launch.sqlite");
        let mut application = CanvasApplication::load(&empty_database);
        if !application.graph.events.is_empty() {
            return Err("release self-check empty launch created nonempty research".into());
        }
        application.exit_after_present = true;
        let report = run(
            application,
            WindowOptions {
                visible: false,
                force_software: std::env::var_os("NOT_NEWS_FORCE_SOFTWARE").is_some(),
                ..WindowOptions::default()
            },
        )?;
        println!(
            "{{\"release_self_check\":\"pass\",\"version\":\"{}\",\"commit\":\"{}\",\"renderer\":\"{}\",\"empty_launch\":true,\"events\":{},\"bridges\":{},\"revision\":{}}}",
            env!("CARGO_PKG_VERSION"),
            option_env!("NOT_NEWS_BUILD_COMMIT").unwrap_or("development"),
            report.renderer.as_str(),
            check.imported_events,
            check.imported_bridges,
            check.final_revision,
        );
        return Ok(());
    }
    let mut application = match (
        options.database,
        application_data_directory("not-news-canvas"),
        hermes_root_directory(),
    ) {
        (Ok(database), Ok(data_directory), Ok(hermes_root)) => {
            CanvasApplication::load_with_directories(
                &database,
                Some(&data_directory),
                Some(&hermes_root),
            )
        }
        (Err(error), _, _) | (_, Err(error), _) | (_, _, Err(error)) => {
            CanvasApplication::unavailable(format!(
                "Data directory unavailable; no graph is open: {error}"
            ))
        }
    };
    if let Some(source) = options.import_legacy {
        application.import_legacy(&source);
    }
    let _ = run(application, WindowOptions::default())?;
    Ok(())
}

struct StartupOptions {
    database: Result<PathBuf, std::io::Error>,
    import_legacy: Option<PathBuf>,
    release_smoke: Option<PathBuf>,
    performance_source: Option<PathBuf>,
    capability_root: Option<PathBuf>,
    show_licenses: bool,
}

fn startup_options(
    arguments: impl IntoIterator<Item = std::ffi::OsString>,
) -> Result<StartupOptions, std::io::Error> {
    let arguments = arguments.into_iter().collect::<Vec<_>>();
    if arguments.len() == 1 && !arguments[0].to_string_lossy().starts_with('-') {
        return Ok(StartupOptions {
            database: Ok(PathBuf::from(&arguments[0])),
            import_legacy: None,
            release_smoke: None,
            performance_source: None,
            capability_root: None,
            show_licenses: false,
        });
    }
    let mut database = None;
    let mut import_legacy = None;
    let mut release_smoke = None;
    let mut performance_source = None;
    let mut capability_root = None;
    let mut show_licenses = false;
    let mut index = 0;
    while index < arguments.len() {
        let flag = &arguments[index];
        if flag == "--licenses" && !show_licenses {
            show_licenses = true;
            index += 1;
            continue;
        }
        let value = arguments.get(index + 1).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("{} requires a path", flag.to_string_lossy()),
            )
        })?;
        if flag == "--database" && database.is_none() {
            database = Some(PathBuf::from(value));
        } else if flag == "--import-legacy" && import_legacy.is_none() {
            import_legacy = Some(PathBuf::from(value));
        } else if flag == "--release-self-check" && release_smoke.is_none() {
            release_smoke = Some(PathBuf::from(value));
        } else if flag == "--performance-check" && performance_source.is_none() {
            performance_source = Some(PathBuf::from(value));
        } else if flag == "--capability-check" && capability_root.is_none() {
            capability_root = Some(PathBuf::from(value));
        } else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("unknown or repeated option {}", flag.to_string_lossy()),
            ));
        }
        index += 2;
    }
    let check_count = usize::from(release_smoke.is_some())
        + usize::from(performance_source.is_some())
        + usize::from(capability_root.is_some())
        + usize::from(show_licenses);
    if check_count > 1 || check_count == 1 && (database.is_some() || import_legacy.is_some()) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "release, performance, and capability checks are exclusive",
        ));
    }
    Ok(StartupOptions {
        database: database.map_or_else(default_database_path, Ok),
        import_legacy,
        release_smoke,
        performance_source,
        capability_root,
        show_licenses,
    })
}

fn run_capability_check(root: &Path) -> Result<(), Box<dyn Error>> {
    let profile = hermes_profile::install(&root.join("hermes"), None)?;
    let hermes = hermes_is_available();
    let searxng = searxng_url(root)?;
    let microphone = default_input_capability();
    let speech = SpeechWorker::from_environment(root.join("voice-capability-scratch"));
    let (kokoro_state, kokoro_detail) = match speech.capability() {
        SpeechCapability::Ready => ("configured", None),
        SpeechCapability::Disabled => ("disabled", None),
        SpeechCapability::Unavailable(reason) => ("unavailable", Some(reason)),
    };
    let report = serde_json::json!({
        "capability_check": "pass",
        "version": env!("CARGO_PKG_VERSION"),
        "commit": option_env!("NOT_NEWS_BUILD_COMMIT").unwrap_or("development"),
        "research": {
            "runtime": "hermes",
            "executable": if hermes { "present" } else { "missing" },
            "compatibility": "deferred-to-exact-pre-research-acp-check",
            "provider": "owned-by-hermes-unverified",
            "profile": profile.home,
            "profile_policy": "installed-v2",
            "remediation": "Install Hermes, then open Connections to configure any provider Hermes supports."
        },
        "discovery": {
            "exa_configuration": "deferred-os-vault-probe",
            "searxng_endpoint": if searxng.is_some() { "configured" } else { "missing" },
            "browse_executable": if browse_is_available() { "present" } else { "missing" },
            "browse_end_to_end": "unverified",
            "curl_executable": if curl_is_available() { "present" } else { "missing" },
            "browserbase_configuration": "optional-deferred-os-vault-probe",
            "os_vault_probe": "deferred-to-connections-to-avoid-an-unattended-unlock-prompt",
            "remediation": "Open Connections (Ctrl+,) to store an Exa key and SearXNG endpoint; install Browse CLI for local browser retrieval."
        },
        "transcription": {
            "configuration": "deferred-os-vault-probe",
            "os_vault_probe": "deferred-to-connections-to-avoid-an-unattended-unlock-prompt",
            "remediation": "Open Connections (Ctrl+,) to store a Groq key in the operating-system vault."
        },
        "microphone": match microphone {
            Ok(()) => serde_json::json!({"state": "available", "permission_probe": "deferred-until-recording"}),
            Err(error) => serde_json::json!({"state": "unavailable", "detail": error.to_string()}),
        },
        "kokoro": {
            "state": kokoro_state,
            "detail": kokoro_detail,
            "endpoint_probe": "deferred-until-a-bounded-voice-note-request",
            "remediation": "Start the configured Kokoro endpoint and install a local WAV player, or disable voice notes explicitly."
        }
    });
    println!("{}", serde_json::to_string(&report)?);
    Ok(())
}

fn run_reference_performance_check(source: &Path) -> Result<(), Box<dyn Error>> {
    let source_before = fs::read(source)?;
    let legacy = LegacyGraphReader::new(source).load()?;
    if legacy.events.len() != 71 {
        return Err(format!(
            "reference performance corpus must contain 71 events, found {}",
            legacy.events.len()
        )
        .into());
    }
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "not-news-performance-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir(&root)?;
    let cleanup = TemporaryDirectory(root.clone());
    let database = root.join("graph.sqlite");
    let store = DurableGraphStore::open(&database)?;
    let imported = store.import_legacy(source)?;
    if imported.events != legacy.events
        || imported.bridges != legacy.bridges
        || imported.aliases != legacy.aliases
        || imported.placements != legacy.placements
    {
        return Err("performance import changed the reference graph".into());
    }
    drop(store);

    let mut application = CanvasApplication::load(&database);
    application.status = None;
    application.speech = SpeechWorker::disabled();
    let report = run(
        PerformanceApplication {
            inner: application,
            frame_index: 0,
        },
        WindowOptions {
            visible: false,
            frame_measurement: Some(FrameMeasurement {
                skip: PERFORMANCE_WARMUP_FRAMES,
            }),
            ..WindowOptions::default()
        },
    )?;
    if fs::read(source)? != source_before {
        return Err("performance check modified its reference database".into());
    }
    if report.measured_frames != PERFORMANCE_MEASURED_FRAMES {
        return Err(format!(
            "performance check measured {} frames instead of {PERFORMANCE_MEASURED_FRAMES}",
            report.measured_frames
        )
        .into());
    }
    let p99 = report
        .p99_frame_time
        .ok_or("performance check produced no frame samples")?;
    let application_p99 = report
        .p99_application_time
        .ok_or("performance check produced no application samples")?;
    let presentation_p99 = report
        .p99_presentation_time
        .ok_or("performance check produced no presentation samples")?;
    let passed = p99.as_micros() <= PERFORMANCE_P99_LIMIT_MICROS;
    println!(
        "{{\"performance_check\":\"{}\",\"version\":\"{}\",\"commit\":\"{}\",\"renderer\":\"{}\",\"events\":{},\"frames\":{},\"p99_frame_micros\":{},\"p99_input_paint_micros\":{},\"p99_present_micros\":{},\"limit_micros\":{}}}",
        if passed { "pass" } else { "fail" },
        env!("CARGO_PKG_VERSION"),
        option_env!("NOT_NEWS_BUILD_COMMIT").unwrap_or("development"),
        report.renderer.as_str(),
        legacy.events.len(),
        report.measured_frames,
        p99.as_micros(),
        application_p99.as_micros(),
        presentation_p99.as_micros(),
        PERFORMANCE_P99_LIMIT_MICROS,
    );
    drop(cleanup);
    if !passed {
        return Err("p99 carrying-frame time exceeds 16.667 ms".into());
    }
    Ok(())
}

struct TemporaryDirectory(PathBuf);

impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[allow(clippy::cast_precision_loss)]
fn usize_to_f64(value: usize) -> f64 {
    value as f64
}

fn default_database_path() -> Result<PathBuf, std::io::Error> {
    Ok(application_data_directory("not-news-canvas")?.join("graph.sqlite"))
}

#[allow(clippy::cast_precision_loss)]
fn physical_scalar(value: u32) -> f32 {
    value as f32
}

#[allow(clippy::cast_possible_truncation)]
fn scale_scalar(value: f64) -> f32 {
    value as f32
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn physical_dimension(value: f64) -> u32 {
    value.round().clamp(1.0, f64::from(u32::MAX)) as u32
}

fn scroll_pixels(delta: MouseScrollDelta) -> f64 {
    match delta {
        MouseScrollDelta::LineDelta(_, vertical) => f64::from(vertical) * 40.0,
        MouseScrollDelta::PixelDelta(PhysicalPosition { y, .. }) => y,
    }
}

fn append_bounded_text(input: &mut String, text: &str) {
    let mut characters = input.chars().count();
    for character in text.chars() {
        let character = match character {
            '\n' | '\r' | '\t' => ' ',
            character if character.is_control() => continue,
            character => character,
        };
        if characters >= MAX_RESEARCH_CHARS
            || input.len() + character.len_utf8() > MAX_RESEARCH_BYTES
        {
            break;
        }
        input.push(character);
        characters += 1;
    }
}

fn append_bounded_predicate(input: &mut String, text: &str) {
    let mut characters = input.chars().count();
    for character in text.chars() {
        let character = match character {
            '\n' | '\r' | '\t' => ' ',
            character if character.is_control() => continue,
            character => character,
        };
        if characters >= MAX_PREDICATE_CHARS
            || input.len() + character.len_utf8() > MAX_PREDICATE_BYTES
        {
            break;
        }
        input.push(character);
        characters += 1;
    }
}

fn append_bounded_secret(input: &mut String, text: &str) {
    let mut characters = input.chars().count();
    for character in text.chars() {
        if character.is_control() || character.is_whitespace() {
            continue;
        }
        if characters >= MAX_CREDENTIAL_CHARS
            || input.len() + character.len_utf8() > MAX_CREDENTIAL_BYTES
        {
            break;
        }
        input.push(character);
        characters += 1;
    }
}

fn masked_secret(secret: &str) -> String {
    let length = secret.graphemes(true).count();
    let visible = length.min(32);
    let mut masked = "●".repeat(visible);
    if length > visible {
        masked.push('…');
    }
    masked
}

fn menu_text(value: &str, maximum: usize) -> String {
    if value.chars().count() <= maximum {
        return value.to_owned();
    }
    let end = value
        .char_indices()
        .nth(maximum.saturating_sub(1))
        .map_or(value.len(), |(index, _)| index);
    format!("{}…", &value[..end])
}

fn menu_row_from_key(key: &str) -> Option<usize> {
    let [digit] = key.as_bytes() else {
        return None;
    };
    (b'1'..=b'9')
        .contains(digit)
        .then(|| usize::from(*digit - b'1'))
}

fn event_title(graph: &GraphSnapshot, event: &EventId) -> String {
    graph.events.get(event).map_or_else(
        || menu_text(&event.0, 22),
        |event| menu_text(&event.title, 22),
    )
}

fn remove_last_grapheme(input: &mut String) {
    if let Some((index, _)) = input.grapheme_indices(true).next_back() {
        input.truncate(index);
    }
}

fn termination_message(termination: &ResearchTermination) -> String {
    match termination {
        ResearchTermination::Completed => "Research completed.".into(),
        ResearchTermination::ExitFailure(code) => code.map_or_else(
            || "Research backend exited unsuccessfully.".into(),
            |code| format!("Research backend exited with status {code}."),
        ),
        ResearchTermination::SpawnFailure(error) => {
            format!("Research backend could not start: {error}")
        }
        ResearchTermination::Cancelled => "Research was cancelled.".into(),
        ResearchTermination::TimedOut => "Research exceeded its total time limit.".into(),
        ResearchTermination::IdleTimedOut => {
            "Research backend stopped producing output and was terminated.".into()
        }
        ResearchTermination::OutputLimit => {
            "Research backend exceeded its bounded output allowance.".into()
        }
        ResearchTermination::IoFailure(error) => {
            format!("Research process communication failed: {error}")
        }
    }
}

#[cfg(test)]
mod app_tests {
    #[cfg(unix)]
    use std::ffi::OsString;
    use std::{
        io::{Read, Write},
        net::TcpListener,
    };

    #[cfg(unix)]
    use not_news_agent::{OutputProtocol, ProcessLimits, ResearchBackend};
    use tempfile::TempDir;

    use super::*;

    /// Serves one canned exchange per `(request-line prefix, status, body)`
    /// element in order, recording every request for later inspection.
    fn mock_searxng(
        exchanges: Vec<(&'static str, &'static str, &'static str)>,
    ) -> (std::net::SocketAddr, std::thread::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let mut requests = Vec::new();
            for (prefix, status, body) in exchanges {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0_u8; 2_048];
                let read = stream.read(&mut request).unwrap();
                let request = String::from_utf8_lossy(&request[..read]).into_owned();
                assert!(
                    request.starts_with(prefix),
                    "expected a request starting with {prefix:?}, got {request:?}"
                );
                write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
                requests.push(request);
            }
            requests
        });
        (address, server)
    }

    const VALID_CONFIG: &str =
        r#"{"instance_name":"SearXNG","engines":[{"name":"arxiv","enabled":true}]}"#;
    const NO_QUERY: &str = r#"{"error":"No query"}"#;

    #[test]
    fn searxng_gate_probes_identity_and_json_capability_without_dispatching_a_search() {
        let (address, server) = mock_searxng(vec![
            ("GET /config HTTP/1.1\r\n", "200 OK", VALID_CONFIG),
            (
                "GET /search?format=json HTTP/1.1\r\n",
                "400 Bad Request",
                NO_QUERY,
            ),
        ]);

        validate_searxng_endpoint(&format!("http://{address}")).unwrap();

        let requests = server.join().unwrap();
        assert_eq!(requests.len(), 2);
        assert!(
            requests.iter().all(|request| !request.contains("q=")),
            "no probe may carry a query that dispatches a search: {requests:?}"
        );
    }

    /// Regression: an instance whose `/config` looks healthy but whose
    /// `search.formats` lacks `json` must not pass readiness; preflight used
    /// to wave it through and every session search then failed at use time.
    #[test]
    fn searxng_gate_rejects_a_json_disabled_instance_with_a_settings_directive() {
        let (address, server) = mock_searxng(vec![
            ("GET /config HTTP/1.1\r\n", "200 OK", VALID_CONFIG),
            ("GET /search?format=json HTTP/1.1\r\n", "403 Forbidden", ""),
        ]);

        let error = validate_searxng_endpoint(&format!("http://{address}")).unwrap_err();
        assert!(error.contains("SearXNG JSON search is disabled"), "{error}");
        assert!(error.contains("search.formats"), "{error}");
        server.join().unwrap();
    }

    /// Deployments may hide `/config` behind a proxy; the queryless JSON
    /// probe then carries both identity and capability evidence on its own.
    #[test]
    fn searxng_gate_falls_back_to_the_json_probe_when_config_is_hidden() {
        let (address, server) = mock_searxng(vec![
            ("GET /config HTTP/1.1\r\n", "404 Not Found", ""),
            (
                "GET /search?format=json HTTP/1.1\r\n",
                "400 Bad Request",
                NO_QUERY,
            ),
        ]);

        validate_searxng_endpoint(&format!("http://{address}")).unwrap();
        server.join().unwrap();
    }

    #[test]
    fn searxng_gate_rejects_generic_json_without_searching_it() {
        let (address, server) = mock_searxng(vec![
            ("GET /config HTTP/1.1\r\n", "200 OK", r#"{"results":[]}"#),
            ("GET /search?format=json HTTP/1.1\r\n", "404 Not Found", ""),
        ]);

        let error = validate_searxng_endpoint(&format!("http://{address}")).unwrap_err();
        assert!(
            error.contains("identify an instance with an enabled engine"),
            "{error}"
        );
        assert!(error.contains("HTTP 404"), "{error}");
        server.join().unwrap();
    }

    /// Live end-to-end gates, exercised manually against a real deployment:
    /// `NOT_NEWS_TEST_SEARXNG_URL=http://127.0.0.1:8889 cargo test -- --ignored`.
    fn live_searxng_url() -> Option<String> {
        std::env::var("NOT_NEWS_TEST_SEARXNG_URL").ok()
    }

    #[test]
    #[ignore = "requires a live SearXNG with JSON search enabled"]
    fn searxng_gate_accepts_a_live_json_capable_instance() {
        let Some(url) = live_searxng_url() else {
            return;
        };
        validate_searxng_endpoint(&url).unwrap();
    }

    #[test]
    #[ignore = "requires a live SearXNG with JSON search disabled"]
    fn searxng_gate_rejects_a_live_json_disabled_instance() {
        let Some(url) = live_searxng_url() else {
            return;
        };
        let error = validate_searxng_endpoint(&url).unwrap_err();
        assert!(error.contains("SearXNG JSON search is disabled"), "{error}");
    }

    #[test]
    fn first_launch_is_empty_while_malformed_data_is_visibly_disabled() {
        let directory = TempDir::new().unwrap();
        let fresh = CanvasApplication::load(&directory.path().join("fresh.sqlite"));
        assert!(fresh.store.is_some());
        assert!(fresh.graph.events.is_empty());
        assert!(
            fresh.status.as_deref().is_some_and(|status| {
                status.contains("Ctrl+K") && status.contains("right-click")
            })
        );

        let malformed = directory.path().join("malformed.sqlite");
        std::fs::write(&malformed, b"not a SQLite graph").unwrap();
        let failed = CanvasApplication::load(&malformed);
        assert!(failed.store.is_none());
        assert!(failed.graph.events.is_empty());
        assert!(failed.status.as_deref().is_some_and(|status| {
            status.contains("Graph unavailable")
                && status.contains("no research is shown or writable")
        }));
    }

    #[test]
    fn custom_graph_path_does_not_fork_the_application_hermes_profile() {
        let directory = TempDir::new().unwrap();
        let graph_directory = directory.path().join("portable-graph");
        let application_directory = directory.path().join("stable-application-data");
        fs::create_dir_all(&graph_directory).unwrap();
        fs::create_dir_all(&application_directory).unwrap();
        let hermes_root = directory.path().join("hermes-root");
        let application = CanvasApplication::load_with_directories(
            &graph_directory.join("graph.sqlite"),
            Some(&application_directory),
            Some(&hermes_root),
        );
        assert_eq!(
            application
                .hermes_profile
                .as_ref()
                .map(|profile| &profile.home),
            Some(&hermes_root.join("profiles/not-news"))
        );
        assert!(!graph_directory.join("hermes").exists());
    }

    #[test]
    fn settled_canvas_waits_without_polling_and_arms_only_real_deadlines() {
        let now = Instant::now();
        let mut application = CanvasApplication::unavailable("Visible failure".into());
        let mut surface =
            not_news_platform::skia_safe::surfaces::raster_n32_premul((640, 360)).unwrap();
        let frame = FrameInfo {
            physical_width: 640,
            physical_height: 360,
            scale_factor: 1.0,
            now,
        };
        assert!(matches!(
            application.render(surface.canvas(), frame),
            FrameSchedule::Wait
        ));
        let deadline = now + Duration::from_millis(500);
        application.record_hold_deadline = Some(deadline);
        assert!(matches!(
            application.render(surface.canvas(), frame),
            FrameSchedule::RedrawAt(scheduled) if scheduled == deadline
        ));
        assert!(matches!(
            application.render(
                surface.canvas(),
                FrameInfo {
                    now: deadline,
                    ..frame
                },
            ),
            FrameSchedule::Wait
        ));
    }

    #[test]
    fn resize_and_dpr_churn_preserve_camera_and_idle_schedule() {
        let now = Instant::now();
        let mut application = CanvasApplication::unavailable("Visible failure".into());
        let initial_viewport = application.interaction.viewport();
        let mut surface =
            not_news_platform::skia_safe::surfaces::raster_n32_premul((1_920, 1_200)).unwrap();
        for (width, height, scale_factor) in [
            (640, 360, 1.0),
            (1_536, 864, 1.25),
            (1_920, 1_080, 2.0),
            (800, 1_200, 1.5),
            (1_280, 800, 1.0),
        ] {
            assert!(matches!(
                application.render(
                    surface.canvas(),
                    FrameInfo {
                        physical_width: width,
                        physical_height: height,
                        scale_factor,
                        now,
                    },
                ),
                FrameSchedule::Wait
            ));
        }
        assert_eq!(application.interaction.viewport(), initial_viewport);
        assert_eq!(
            (application.physical_width, application.physical_height),
            (1_280.0, 800.0)
        );
        assert!((application.scale_factor - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn composer_bounds_unicode_and_commits_ime_without_splitting_graphemes() {
        let mut input = String::from("A👨‍👩‍👧‍👦e\u{301}");
        remove_last_grapheme(&mut input);
        assert_eq!(input, "A👨‍👩‍👧‍👦");
        remove_last_grapheme(&mut input);
        assert_eq!(input, "A");

        let mut bounded = String::new();
        append_bounded_text(&mut bounded, &format!("{}\nignored\0", "界".repeat(6_000)));
        assert_eq!(bounded.chars().count(), MAX_RESEARCH_CHARS);
        assert!(bounded.len() <= MAX_RESEARCH_BYTES);
        assert!(!bounded.contains('\0'));

        let mut application = CanvasApplication::unavailable("Visible failure".into());
        application.research_input = Some("Ask ".into());
        assert!(application.text_input_active());
        assert!(application.ime_input(&Ime::Preedit("研究".into(), Some((6, 6)))));
        assert_eq!(application.research_preedit, "研究");
        assert!(application.ime_input(&Ime::Commit("研究課題".into())));
        assert_eq!(application.research_input.as_deref(), Some("Ask 研究課題"));
        assert!(application.research_preedit.is_empty());
    }

    #[test]
    fn connections_exposes_owned_credentials_without_a_backend_selector() {
        let directory = TempDir::new().unwrap();
        let mut application = CanvasApplication::load(&directory.path().join("graph.sqlite"));
        application.settings = Some(SettingsFlow::Menu {
            browserbase: CredentialMenuState::Ready(CredentialState::Missing),
            exa: CredentialMenuState::Ready(CredentialState::Vault),
            groq: CredentialMenuState::Ready(CredentialState::Missing),
            searxng: EndpointState::Saved,
            hermes_available: true,
            browse_available: true,
            curl_available: true,
            page: SettingsPage::Root,
            selected: 0,
        });
        let labels = application
            .settings_choices()
            .into_iter()
            .map(|(_, label)| label)
            .collect::<Vec<_>>();
        assert!(labels[0].starts_with("HERMES  ·  EXECUTABLE PRESENT"));
        assert!(labels[1].starts_with("EXA DISCOVERY"));
        assert!(labels[2].starts_with("SEARXNG FRONTIER"));
        assert!(labels[3].starts_with("GROQ VOICE"));
        assert!(labels[4].starts_with("BROWSERBASE CLOUD"));
        assert!(labels[5].starts_with("BROWSE  ·  EXECUTABLE PRESENT"));
        assert!(labels[6].starts_with("CURL  ·  EXECUTABLE PRESENT"));
        assert!(labels.last().unwrap().starts_with("COMPLETE ERASE"));
        assert_eq!(labels.len(), 8);
        assert!(labels.iter().all(|label| !label.starts_with("REMOVE")));
        assert!(labels.iter().all(|label| !label.contains("BACKEND")
            && !label.contains("OPENCODE")
            && !label.contains("CLOSE")));
        assert_eq!(application.settings_selection(), Some(0));
        assert!(application.move_settings_selection(-1));
        assert_eq!(application.settings_selection(), Some(labels.len() - 1));
        assert!(application.move_settings_selection(1));
        assert_eq!(application.settings_selection(), Some(0));
        assert!(application.select_settings_row(1));
        assert_eq!(application.settings_selection(), Some(1));
        assert!(application.navigate_settings(SettingsNavigation::Forward));
        let detail = application
            .settings_choices()
            .into_iter()
            .map(|(_, label)| label)
            .collect::<Vec<_>>();
        assert!(detail[0].starts_with("REPLACE KEY  ·  OS VAULT"));
        assert!(detail[1].contains("REMOVE EXA KEY"));
        assert!(detail[1].contains("RESEARCH DISCOVERY BECOMES INCOMPLETE"));
        assert_eq!(detail.len(), 2);
        assert!(application.navigate_settings(SettingsNavigation::Backward));
        assert!(matches!(
            application.settings,
            Some(SettingsFlow::Menu {
                page: SettingsPage::Root,
                ..
            })
        ));
        assert!(application.navigate_settings(SettingsNavigation::Backward));
        assert!(application.settings.is_none());
    }

    #[test]
    fn credential_vault_wait_never_blocks_the_ui_reducer() {
        let mut application = CanvasApplication::unavailable("Visible failure".into());
        application.settings = Some(SettingsFlow::Menu {
            browserbase: CredentialMenuState::Ready(CredentialState::Missing),
            exa: CredentialMenuState::Ready(CredentialState::Missing),
            groq: CredentialMenuState::Saving,
            searxng: EndpointState::Missing,
            hermes_available: true,
            browse_available: true,
            curl_available: true,
            page: SettingsPage::Root,
            selected: 1,
        });
        let labels = application
            .settings_choices()
            .into_iter()
            .map(|(_, label)| label)
            .collect::<Vec<_>>();
        assert!(labels[3].contains("STORING IN OS VAULT"));

        let (release, blocked) = mpsc::sync_channel(0);
        application.start_vault_task(move || {
            blocked.recv().unwrap();
            VaultTaskResult::Saved {
                kind: CredentialKind::Groq,
                result: Ok(()),
            }
        });
        assert!(!application.drain_vault_task());
        assert!(application.vault_task.is_some());

        application.settings = None;
        release.send(()).unwrap();
        for _ in 0..100 {
            if application.drain_vault_task() {
                break;
            }
            std::thread::yield_now();
        }
        assert!(application.vault_task.is_none());
        assert_eq!(
            application.status.as_deref(),
            Some("Groq API key stored in the operating-system credential vault.")
        );
    }

    #[test]
    fn complete_erase_requires_single_instance_and_preserves_unrelated_hermes_state() {
        let directory = TempDir::new().unwrap();
        let data = directory.path().join("app-data");
        let external = directory.path().join("external");
        let database = external.join("graph.sqlite");
        let hermes = directory.path().join("hermes");
        fs::create_dir_all(&external).unwrap();
        fs::create_dir_all(hermes.join("profiles/default")).unwrap();
        fs::write(hermes.join("profiles/default/sentinel"), "keep").unwrap();

        let mut application =
            CanvasApplication::load_with_directories(&database, Some(&data), Some(&hermes));
        fs::write(
            external.join("graph.pre-rust-v4-1-1.sqlite"),
            "owned migration backup",
        )
        .unwrap();
        let second = ApplicationLease::acquire(&data).unwrap();
        assert!(
            application
                .application_lease
                .as_mut()
                .unwrap()
                .require_exclusive()
                .is_err()
        );
        drop(second);
        application
            .application_lease
            .as_mut()
            .unwrap()
            .require_exclusive()
            .unwrap();
        let lease_path = application.application_lease.as_ref().unwrap().path.clone();

        application.finish_complete_erase();

        assert!(application.exit_after_present);
        assert!(!data.exists());
        assert!(!database.exists());
        assert!(!external.join("graph.pre-rust-v4-1-1.sqlite").exists());
        assert!(!hermes.join("profiles/not-news").exists());
        assert_eq!(
            fs::read_to_string(hermes.join("profiles/default/sentinel")).unwrap(),
            "keep"
        );
        assert!(lease_path.exists());
        drop(application);
        assert!(!lease_path.exists());
    }

    #[test]
    fn explicit_curation_flow_reaches_canvas_and_sqlite_without_a_drag() {
        let directory = TempDir::new().unwrap();
        let database = directory.path().join("graph.sqlite");
        let store = DurableGraphStore::open(&database).unwrap();
        store.start_research_session("seed", "Seed").unwrap();
        let finding = |id: &str, artifacts| not_news_domain::ResearchEvent {
            id: EventId(id.into()),
            title: format!("Finding {id}"),
            date: "Jul 14, 2026".into(),
            color: 0xff4c_9be8,
            summary: "Durable finding.".into(),
            source_label: "Primary".into(),
            artifacts,
            url: Some(format!("https://example.test/{id}")),
        };
        store
            .accept_research_event(
                "seed",
                0,
                &finding(
                    "a",
                    vec![not_news_domain::SourceArtifact {
                        text: "Paper".into(),
                        source: "Journal".into(),
                        url: "https://example.test/paper".into(),
                    }],
                ),
            )
            .unwrap();
        store
            .accept_research_event("seed", 1, &finding("b", Vec::new()))
            .unwrap();
        drop(store);

        let mut application = CanvasApplication::load(&database);
        let before_placements = application.graph.placements.clone();
        application.curation = Some(CurationFlow::RelatePredicate {
            from: EventId("a".into()),
            to: EventId("b".into()),
            expected_revision: application.graph.revision,
            input: "Supports with primary evidence".into(),
        });
        assert!(application.commit_relation_input(Instant::now()));
        assert_eq!(application.graph.placements, before_placements);
        assert!(application.graph.bridges.values().any(|bridge| {
            bridge.provenance == Provenance::User
                && bridge.label == "Supports with primary evidence"
        }));

        application.curation = Some(CurationFlow::PromotePredicate {
            source: EventId("a".into()),
            artifact_index: 0,
            expected_revision: application.graph.revision,
            input: String::new(),
        });
        assert!(application.commit_promotion_input(Instant::now()));
        assert_eq!(application.graph.events.len(), 3);
        assert_eq!(
            application.store.as_ref().unwrap().load().unwrap(),
            application.graph
        );
        assert!(application.undo());
        assert_eq!(application.graph.events.len(), 2);
    }

    #[test]
    fn metadata_surface_retains_hover_and_focused_reading_freezes_canvas_selection() {
        let event_id = EventId("active".into());
        let mut graph = GraphSnapshot::default();
        graph.events.insert(
            event_id.clone(),
            not_news_domain::ResearchEvent {
                id: event_id.clone(),
                title: "Long finding".into(),
                date: "Jul 18, 2026".into(),
                color: 0xff4c_9be8,
                summary: "Long metadata remains readable while its surface owns input. ".repeat(80),
                source_label: "Primary".into(),
                artifacts: Vec::new(),
                url: None,
            },
        );
        graph.placements.insert(
            event_id.clone(),
            not_news_domain::Placement {
                point: Point { x: 900.0, y: 500.0 },
                pinned: true,
            },
        );
        let mut application = CanvasApplication::with_state(None, graph, None, None, None, None);
        application.interaction.resize(1_280, 800);
        let transform = ViewportTransform::new(1_280.0, 800.0, application.interaction.viewport());
        let node = transform.world_to_screen(Point { x: 900.0, y: 500.0 });
        let opened_at = Instant::now();
        assert!(application.cursor_moved(node, opened_at));
        let settled_at = opened_at + Duration::from_millis(220);
        assert_eq!(
            application
                .interaction
                .frame(&application.graph, settled_at)
                .expanded_event
                .as_ref(),
            Some(&event_id)
        );

        let card = Point { x: 100.0, y: 100.0 };
        application.cursor_moved(card, settled_at);
        assert!(application.metadata_at_cursor(settled_at));
        assert_eq!(
            application
                .interaction
                .frame(&application.graph, settled_at + Duration::from_millis(500))
                .expanded_event
                .as_ref(),
            Some(&event_id),
            "entering the metadata surface must cancel its collapse deadline"
        );

        assert!(application.focus_metadata());
        assert!(application.scroll_metadata_by(240.0));
        let scroll = application.metadata_scroll;
        application.cursor_moved(
            Point {
                x: 1_200.0,
                y: 700.0,
            },
            settled_at + Duration::from_millis(600),
        );
        assert!((application.metadata_scroll - scroll).abs() < f64::EPSILON);
        assert_eq!(
            application
                .interaction
                .frame(&application.graph, settled_at + Duration::from_secs(2))
                .expanded_event
                .as_ref(),
            Some(&event_id),
            "focused reading must ignore canvas hover movement"
        );
    }

    #[test]
    fn metadata_composited_over_fixed_chrome_also_owns_the_overlap() {
        let event_id = EventId("active".into());
        let mut graph = GraphSnapshot::default();
        graph.events.insert(
            event_id.clone(),
            not_news_domain::ResearchEvent {
                id: event_id.clone(),
                title: "Long finding".into(),
                date: "Jul 18, 2026".into(),
                color: 0xff4c_9be8,
                summary: "Metadata intentionally overlaps persistent chrome. ".repeat(80),
                source_label: "Primary".into(),
                artifacts: Vec::new(),
                url: None,
            },
        );
        graph.placements.insert(
            event_id.clone(),
            not_news_domain::Placement {
                point: Point { x: 300.0, y: 200.0 },
                pinned: true,
            },
        );
        let mut application = CanvasApplication::with_state(None, graph, None, None, None, None);
        application.interaction.resize(1_280, 800);
        let transform = ViewportTransform::new(1_280.0, 800.0, application.interaction.viewport());
        let node = transform.world_to_screen(Point { x: 300.0, y: 200.0 });
        let opened_at = Instant::now();
        assert!(application.cursor_moved(node, opened_at));
        let settled_at = opened_at + Duration::from_millis(220);
        assert_eq!(
            application
                .interaction
                .frame(&application.graph, settled_at)
                .expanded_event
                .as_ref(),
            Some(&event_id)
        );

        let overlap = Point {
            x: 1_160.0,
            y: 750.0,
        };
        application.cursor_moved(overlap, settled_at);
        assert!(application.metadata_at_cursor(settled_at));
        assert_eq!(application.chrome_at_cursor(), Some(ChromeControl::ZoomIn));
        let zoom = application.interaction.viewport().zoom;
        application.mouse_input(ElementState::Pressed, settled_at);
        assert!(matches!(
            application.pointer_owner,
            PointerOwner::MetadataSurface
        ));
        application.mouse_input(ElementState::Released, settled_at);
        assert!((application.interaction.viewport().zoom - zoom).abs() < f64::EPSILON);
    }

    #[test]
    fn activity_wheel_moves_from_latest_to_oldest_and_back() {
        let mut application =
            CanvasApplication::with_state(None, GraphSnapshot::default(), None, None, None, None);
        application.research_messages = (0..24)
            .map(|index| format!("Hermes activity {index}: retained research evidence."))
            .collect();
        assert!(application.activity_scroll_maximum() > 0.0);
        assert!(application.scroll_activity(MouseScrollDelta::LineDelta(0.0, 1.0)));
        assert!(application.activity_scroll > 0.0);
        assert!(application.scroll_activity(MouseScrollDelta::LineDelta(0.0, -100.0)));
        assert!(application.activity_scroll.abs() < f64::EPSILON);
    }

    #[test]
    fn every_curation_hover_row_change_requests_a_repaint() {
        let subject = EventId("subject".into());
        let peer = EventId("peer".into());
        let event = |id: EventId, artifacts| not_news_domain::ResearchEvent {
            id,
            title: "Finding".into(),
            date: "Jul 18, 2026".into(),
            color: 0xff4c_9be8,
            summary: "Finding".into(),
            source_label: "Primary".into(),
            artifacts,
            url: None,
        };
        let mut graph = GraphSnapshot::default();
        graph.events.insert(
            subject.clone(),
            event(
                subject.clone(),
                vec![not_news_domain::SourceArtifact {
                    text: "Source artifact".into(),
                    source: "Primary".into(),
                    url: "https://example.test/source".into(),
                }],
            ),
        );
        graph
            .events
            .insert(peer.clone(), event(peer.clone(), Vec::new()));
        graph.placements.insert(
            subject.clone(),
            not_news_domain::Placement {
                point: Point { x: 900.0, y: 700.0 },
                pinned: true,
            },
        );
        graph.placements.insert(
            peer.clone(),
            not_news_domain::Placement {
                point: Point {
                    x: 1_100.0,
                    y: 700.0,
                },
                pinned: true,
            },
        );
        let bridge = BridgeId("subject::peer".into());
        graph.bridges.insert(
            bridge.clone(),
            not_news_domain::EventBridge {
                id: bridge,
                from: subject.clone(),
                to: peer,
                label: "Supports".into(),
                provenance: Provenance::User,
            },
        );
        let expected_revision = graph.revision;
        let mut application = CanvasApplication::with_state(None, graph, None, None, None, None);
        application.curation = Some(CurationFlow::Menu {
            anchor: Point { x: 120.0, y: 100.0 },
            subject: CanvasSubject {
                event: subject,
                artifact_index: Some(0),
            },
            stage: CurationMenuStage::Actions,
            page: 0,
            expected_revision,
        });
        let now = Instant::now();

        for (row, y) in [166.0, 214.0, 262.0].into_iter().enumerate() {
            assert!(
                application.cursor_moved(Point { x: 140.0, y }, now),
                "moving to Curate row {row} must invalidate the frame even when the canvas beneath it does not change"
            );
            assert_eq!(application.curation_menu_at_cursor(), Some(row));
        }
        assert!(application.cursor_moved(Point { x: 20.0, y: 20.0 }, now));
        assert_eq!(application.curation_menu_at_cursor(), None);
        assert!(application.cursor_moved(Point { x: 140.0, y: 166.0 }, now));
        assert_eq!(application.curation_menu_at_cursor(), Some(0));
        assert!(!application.cursor_moved(Point { x: 141.0, y: 167.0 }, now));

        let peer_screen = ViewportTransform::new(
            application.physical_width,
            application.physical_height,
            application.interaction.viewport(),
        )
        .world_to_screen(Point {
            x: 1_100.0,
            y: 700.0,
        });
        assert!(application.cursor_moved(peer_screen, now + Duration::from_millis(1)));
        assert_eq!(
            application
                .interaction
                .active_event_position()
                .map(|(event, _)| event.clone()),
            Some(EventId("peer".into())),
            "canvas hover outside the Curate surface must remain live"
        );
    }

    #[test]
    fn dense_detachment_menu_pages_every_exact_relationship_and_owns_its_number_keys() {
        let event = |id: EventId| not_news_domain::ResearchEvent {
            id,
            title: "Finding".into(),
            date: "Jul 14, 2026".into(),
            color: 0xff4c_9be8,
            summary: "Finding".into(),
            source_label: "Primary".into(),
            artifacts: Vec::new(),
            url: None,
        };
        let hub = EventId("hub".into());
        let mut graph = GraphSnapshot::default();
        graph.events.insert(hub.clone(), event(hub.clone()));
        for index in 0..10 {
            let peer = EventId(format!("peer-{index}"));
            graph.events.insert(peer.clone(), event(peer.clone()));
            let id = BridgeId(format!("bridge-{index}"));
            graph.bridges.insert(
                id.clone(),
                not_news_domain::EventBridge {
                    id,
                    from: hub.clone(),
                    to: peer,
                    label: format!("relationship {index}"),
                    provenance: Provenance::User,
                },
            );
        }
        let application = CanvasApplication::with_state(None, graph, None, None, None, None);
        let subject = CanvasSubject {
            event: hub,
            artifact_index: None,
        };
        let first = application.curation_choices(&subject, CurationMenuStage::Detach, 0);
        let second = application.curation_choices(&subject, CurationMenuStage::Detach, 1);
        assert_eq!(
            first
                .iter()
                .filter(|(choice, _)| matches!(choice, CurationChoice::Bridge(_)))
                .count(),
            7
        );
        assert!(matches!(first.last(), Some((CurationChoice::NextPage, _))));
        assert!(matches!(
            second.first(),
            Some((CurationChoice::PreviousPage, _))
        ));
        assert_eq!(
            second
                .iter()
                .filter(|(choice, _)| matches!(choice, CurationChoice::Bridge(_)))
                .count(),
            3
        );
        assert!(
            !second
                .iter()
                .any(|(choice, _)| matches!(choice, CurationChoice::NextPage))
        );
        assert_eq!(menu_row_from_key("1"), Some(0));
        assert_eq!(menu_row_from_key("9"), Some(8));
        assert_eq!(menu_row_from_key("0"), None);
        assert_eq!(menu_row_from_key("12"), None);
    }

    #[test]
    fn interrupted_research_reopens_with_accepted_activity_not_a_blank_failure() {
        let directory = TempDir::new().unwrap();
        let database = directory.path().join("graph.sqlite");
        let first = CanvasApplication::load(&database);
        let store = first.store.as_ref().unwrap();
        store
            .start_research_session("interrupted", "Investigate")
            .unwrap();
        store
            .record_research_output(
                "interrupted",
                0,
                ResearchOutputKind::Message,
                "Searching primary sources",
            )
            .unwrap();
        drop(first);

        let reopened = CanvasApplication::load(&database);
        assert_eq!(reopened.research_messages, ["Searching primary sources"]);
        assert!(reopened.status.as_deref().is_some_and(|message| {
            message.contains("interrupted research session")
                && message.contains("accepted findings were preserved")
        }));
    }

    #[test]
    fn clear_click_commits_the_empty_graph_before_resetting_the_view() {
        let directory = TempDir::new().unwrap();
        let database = directory.path().join("graph.sqlite");
        let store = DurableGraphStore::open(&database).unwrap();
        store.start_research_session("seed", "Seed event").unwrap();
        store
            .accept_research_event(
                "seed",
                0,
                &not_news_domain::ResearchEvent {
                    id: not_news_domain::EventId("seed-event".into()),
                    title: "Seed".into(),
                    date: "Jul 14, 2026".into(),
                    color: 0xff4c_9be8,
                    summary: "Durable seed.".into(),
                    source_label: "Primary".into(),
                    artifacts: Vec::new(),
                    url: Some("https://example.test/seed".into()),
                },
            )
            .unwrap();
        store
            .finish_research_session("seed", 1, ResearchSessionStatus::Done, "Done")
            .unwrap();
        drop(store);

        let mut application = CanvasApplication::load(&database);
        let before_revision = application.graph.revision;
        assert_eq!(application.graph.events.len(), 1);
        assert!(application.clear_canvas());
        assert!(application.graph.events.is_empty());
        assert_eq!(application.graph.revision, before_revision + 1);
        assert_eq!(application.status.as_deref(), Some("Canvas cleared."));
        assert!(
            application
                .store
                .as_ref()
                .unwrap()
                .load()
                .unwrap()
                .events
                .is_empty()
        );
        assert!(matches!(
            application
                .store
                .as_ref()
                .unwrap()
                .research_activity("seed"),
            Err(StoreError::MissingResearchSession(_))
        ));
    }

    #[test]
    fn dropped_legacy_graph_imports_into_pristine_state_without_touching_source() {
        let directory = TempDir::new().unwrap();
        let source = directory.path().join("legacy.sqlite");
        let source_store = DurableGraphStore::open(&source).unwrap();
        source_store.start_research_session("seed", "Seed").unwrap();
        source_store
            .accept_research_event(
                "seed",
                0,
                &not_news_domain::ResearchEvent {
                    id: not_news_domain::EventId("legacy-event".into()),
                    title: "Legacy".into(),
                    date: "Jul 14, 2026".into(),
                    color: 0xff4c_9be8,
                    summary: "Imported without source mutation.".into(),
                    source_label: "Primary".into(),
                    artifacts: Vec::new(),
                    url: None,
                },
            )
            .unwrap();
        drop(source_store);
        let source_before = std::fs::read(&source).unwrap();
        let mut application = CanvasApplication::load(&directory.path().join("destination.sqlite"));

        assert!(application.import_legacy(&source));
        assert!(
            application
                .graph
                .events
                .contains_key(&not_news_domain::EventId("legacy-event".into()))
        );
        assert!(application.status.as_deref().is_some_and(|status| {
            status.contains("Imported 1 event")
                && status.contains("source database was not changed")
        }));
        assert_eq!(std::fs::read(&source).unwrap(), source_before);
    }

    #[test]
    fn startup_arguments_separate_import_source_from_destination() {
        let options = startup_options([
            std::ffi::OsString::from("--database"),
            std::ffi::OsString::from("destination.sqlite"),
            std::ffi::OsString::from("--import-legacy"),
            std::ffi::OsString::from("legacy.sqlite"),
        ])
        .unwrap();
        assert_eq!(
            options.database.unwrap(),
            PathBuf::from("destination.sqlite")
        );
        assert_eq!(options.import_legacy, Some(PathBuf::from("legacy.sqlite")));
        assert!(startup_options([std::ffi::OsString::from("--import-legacy")]).is_err());

        let licenses = startup_options([std::ffi::OsString::from("--licenses")]).unwrap();
        assert!(licenses.show_licenses);
        assert!(
            startup_options([
                std::ffi::OsString::from("--licenses"),
                std::ffi::OsString::from("--database"),
                std::ffi::OsString::from("destination.sqlite"),
            ])
            .is_err()
        );

        let performance = startup_options([
            std::ffi::OsString::from("--performance-check"),
            std::ffi::OsString::from("reference.sqlite"),
        ])
        .unwrap();
        assert_eq!(
            performance.performance_source,
            Some(PathBuf::from("reference.sqlite"))
        );
        assert!(
            startup_options([
                std::ffi::OsString::from("--performance-check"),
                std::ffi::OsString::from("reference.sqlite"),
                std::ffi::OsString::from("--database"),
                std::ffi::OsString::from("destination.sqlite"),
            ])
            .is_err()
        );

        let capabilities = startup_options([
            std::ffi::OsString::from("--capability-check"),
            std::ffi::OsString::from("empty-data"),
        ])
        .unwrap();
        assert_eq!(
            capabilities.capability_root,
            Some(PathBuf::from("empty-data"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn process_parser_store_and_canvas_accept_an_early_bridge_without_polling_snapshots() {
        let directory = TempDir::new().unwrap();
        let mut application = CanvasApplication::load(&directory.path().join("graph.sqlite"));
        application.speech = SpeechWorker::disabled();
        let session_id = "integration-session".to_owned();
        application
            .store
            .as_ref()
            .unwrap()
            .start_research_session(&session_id, "Investigate")
            .unwrap();
        let script = r#"
printf '%s\n' 'AI_NEWS_EVENT: {"type":"event.upsert","data":{"id":"a","title":"A","date":"Jul 14, 2026","color":4283218390,"summary":"A finding.","sourceLabel":"Primary","artifacts":[],"url":"https://example.test/a"}}'
printf '%s\n' 'AI_NEWS_EVENT: {"type":"bridge.upsert","data":{"from":"a","to":"b","label":"Supports"}}'
printf '%s\n' 'AI_NEWS_EVENT: {"type":"voice.note","data":{"message":"The relationship is now supported by two exact findings."}}'
printf '%s\n' 'AI_NEWS_EVENT: {"type":"event.upsert","data":{"id":"b","title":"B","date":"Jul 14, 2026","color":4283218390,"summary":"Another finding.","sourceLabel":"Primary","artifacts":[],"url":"https://example.test/b"}}'
printf '%s\n' 'AI_NEWS_EVENT: {"type":"session.done","data":{"message":"Complete."}}'
"#;
        let scratch = directory.path().join("scratch");
        std::fs::create_dir_all(&scratch).unwrap();
        let handle = ResearchLaunch::command(
            ResearchBackend::Hermes,
            "/bin/sh",
            [OsString::from("-c"), OsString::from(script)],
            &scratch,
            OutputProtocol::HermesLines,
            ProcessLimits::default(),
        )
        .spawn()
        .unwrap();
        application.research = Some(ActiveResearch {
            session_id,
            handle,
            next_sequence: 0,
            deferred_bridges: VecDeque::new(),
            closed: false,
            scratch_directory: scratch,
        });

        let deadline = Instant::now() + Duration::from_secs(3);
        while application.research.is_some() && Instant::now() < deadline {
            application.drain_research();
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            application.research.is_none(),
            "research process did not finish"
        );
        assert_eq!(application.graph.events.len(), 2);
        assert_eq!(application.graph.bridges.len(), 1);
        assert_eq!(application.graph.revision, 3);
        assert_eq!(application.status.as_deref(), Some("Complete."));

        let connection =
            rusqlite::Connection::open(application.store.as_ref().unwrap().path()).unwrap();
        let output_kinds = connection
            .prepare(
                "SELECT kind FROM research_output_log WHERE session_id='integration-session' \
                 ORDER BY output_sequence",
            )
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(
            output_kinds,
            [
                "message",
                "event",
                "protocol_error",
                "voice_note",
                "event",
                "bridge",
                "done"
            ]
        );
    }

    #[test]
    #[ignore = "uses the locally authenticated external research subscription and web"]
    fn live_hermes_activity_and_graph_mutations_cross_the_installed_acp_boundary() {
        let root = std::env::var_os("AI_NEWS_HERMES_ROOT")
            .expect("AI_NEWS_HERMES_ROOT must name the exact live-test Hermes root");
        let directory = TempDir::new().unwrap();
        let graph_path = directory.path().join("graph.sqlite");
        let mut application = CanvasApplication::load(&graph_path);
        save_searxng_url(directory.path(), "http://127.0.0.1:8889").unwrap();
        application.hermes_profile = Some(hermes_profile::InstalledProfile {
            home: PathBuf::from(&root).join("profiles/not-news"),
            root: PathBuf::from(root),
            policy_version: hermes_profile::POLICY_VERSION,
        });
        application.speech = SpeechWorker::disabled();

        application.start_research(
            "Find current primary evidence about the expected Kimi K3 model and explain what is confirmed versus speculative.",
        );
        let deadline = Instant::now() + Duration::from_mins(10);
        let mut saw_tool_activity = false;
        let mut saw_graph_before_process_exit = false;
        while application.research.is_some() && Instant::now() < deadline {
            application.drain_research();
            saw_tool_activity |= application
                .research_messages
                .iter()
                .any(|message| message.starts_with("Hermes · "));
            saw_graph_before_process_exit |=
                !application.graph.events.is_empty() && application.research.is_some();
            std::thread::sleep(Duration::from_millis(10));
        }

        assert!(
            application.research.is_none(),
            "live research did not finish"
        );
        assert!(saw_tool_activity, "Hermes tool activity stayed invisible");
        assert!(
            saw_graph_before_process_exit,
            "no graph point crossed while Hermes was still running"
        );
        assert!(!application.graph.events.is_empty());
        let connection = rusqlite::Connection::open(&graph_path).unwrap();
        let durable_events: i64 = connection
            .query_row("SELECT COUNT(*) FROM events", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            usize::try_from(durable_events).unwrap(),
            application.graph.events.len()
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT status FROM research_sessions ORDER BY started_at DESC LIMIT 1",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "done"
        );
    }
}
