mod interaction;
mod release_check;
mod settings;

use std::{
    collections::{HashSet, VecDeque},
    error::Error,
    fs,
    path::{Path, PathBuf},
    time::Duration,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use interaction::{CanvasInteraction, CanvasSubject, InteractionEffect, InteractionFrame};
use not_news_agent::{
    AgentEvent, BridgeUpsert, ResearchHandle, ResearchLaunch, ResearchProcessEvent,
    ResearchTermination, build_research_prompt, hermes_is_available, open_hermes_dashboard,
    opencode_is_available,
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
    application_data_directory, open_external_url, run,
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
    GroqCredentialState, SettingsStore, UserSettings, delete_groq_api_key, groq_api_key,
    groq_credential_state, save_groq_api_key,
};

const MAX_RESEARCH_CHARS: usize = 4_096;
const MAX_RESEARCH_BYTES: usize = 16_384;
const MAX_PREDICATE_CHARS: usize = 96;
const MAX_PREDICATE_BYTES: usize = 384;
const MAX_CREDENTIAL_CHARS: usize = 4_096;
const MAX_CREDENTIAL_BYTES: usize = 16_384;
const CURATION_RELATIONSHIPS_PER_PAGE: usize = 7;
const HERMES_INSTALL_URL: &str = "https://hermes-agent.nousresearch.com";
use not_news_renderer::{
    ChromeControl, Motion, RecordOrbState, SceneAnimation, SceneState, active_metadata_scroll_max,
    hit_active_metadata, hit_activity_surface, hit_activity_toggle, hit_curation_menu,
    hit_fixed_chrome, paint_active_metadata, paint_activity_drawer, paint_background,
    paint_curation_menu, paint_curation_prompt, paint_fixed_chrome, paint_graph, paint_grid,
    paint_research_prompt, paint_status, resolved_positions,
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
        groq: GroqCredentialState,
        hermes_available: bool,
    },
    GroqKey {
        input: Zeroizing<String>,
    },
}

#[derive(Clone, Copy)]
enum SettingsChoice {
    CycleBackend,
    GroqKey,
    Hermes,
    RemoveGroq,
    Close,
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
    research_directory: PathBuf,
    voice_directory: PathBuf,
    voice: VoiceState,
    speech: SpeechWorker,
    research_input: Option<String>,
    research_preedit: String,
    curation: Option<CurationFlow>,
    curation_preedit: String,
    settings_store: SettingsStore,
    user_settings: UserSettings,
    settings: Option<SettingsFlow>,
    settings_preedit: Zeroizing<String>,
    research: Option<ActiveResearch>,
    research_messages: Vec<String>,
    generated_research_events: HashSet<not_news_domain::EventId>,
    auto_follow_research: bool,
    activity_open: bool,
    activity_openness: f64,
    activity_motion: Option<ActivityMotion>,
    record_hold_deadline: Option<Instant>,
    metadata_scroll_event: Option<EventId>,
    metadata_scroll: f64,
    exit_after_present: bool,
}

impl CanvasApplication {
    fn load(database: &Path) -> Self {
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
                        "Canvas ready. Ctrl+K researches; right-click a finding curates; dropping a legacy graph.sqlite imports it."
                            .into(),
                    );
                }
                let mut application =
                    Self::with_state(Some(store), graph, status, database.parent());
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
        Self::with_state(None, GraphSnapshot::default(), Some(status), None)
    }

    fn with_state(
        store: Option<DurableGraphStore>,
        graph: GraphSnapshot,
        status: Option<String>,
        data_directory: Option<&Path>,
    ) -> Self {
        let interaction = CanvasInteraction::new(resolved_positions(&graph));
        let data_directory = data_directory.unwrap_or_else(|| Path::new("."));
        let voice_directory = data_directory.join("voice-scratch");
        let speech = SpeechWorker::from_environment(voice_directory.join("synthesis"));
        let settings_store = SettingsStore::new(data_directory);
        let (user_settings, settings_error) = match settings_store.load() {
            Ok(settings) => (settings, None),
            Err(error) => (
                UserSettings::default(),
                Some(format!(
                    "Saved capability settings were rejected; using AUTO for this run: {error}"
                )),
            ),
        };
        Self {
            store,
            graph,
            interaction,
            status: status.or(settings_error),
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
            research_directory: data_directory.join("research-scratch"),
            voice_directory,
            voice: VoiceState::Idle,
            speech,
            research_input: None,
            research_preedit: String::new(),
            curation: None,
            curation_preedit: String::new(),
            settings_store,
            user_settings,
            settings: None,
            settings_preedit: Zeroizing::new(String::new()),
            research: None,
            research_messages: Vec::new(),
            generated_research_events: HashSet::new(),
            auto_follow_research: true,
            activity_open: false,
            activity_openness: 0.0,
            activity_motion: None,
            record_hold_deadline: None,
            metadata_scroll_event: None,
            metadata_scroll: 0.0,
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

    fn settings_choices(&self) -> Vec<(SettingsChoice, String)> {
        let Some(SettingsFlow::Menu {
            groq,
            hermes_available,
        }) = self.settings.as_ref()
        else {
            return Vec::new();
        };
        let mut choices = vec![
            (
                SettingsChoice::CycleBackend,
                format!(
                    "RESEARCH BACKEND  ·  {}",
                    self.user_settings.research_backend.label()
                ),
            ),
            (
                SettingsChoice::GroqKey,
                format!("GROQ VOICE  ·  {}", groq.label()),
            ),
            (
                SettingsChoice::Hermes,
                if *hermes_available {
                    "HERMES SETTINGS  ·  OPEN DASHBOARD".into()
                } else {
                    "HERMES  ·  INSTALL / CONFIGURE".into()
                },
            ),
        ];
        if matches!(groq, GroqCredentialState::Vault) {
            choices.push((SettingsChoice::RemoveGroq, "REMOVE GROQ VAULT KEY".into()));
        }
        choices.push((SettingsChoice::Close, "CLOSE CONNECTIONS".into()));
        choices
    }

    fn open_settings(&mut self, now: Instant) -> bool {
        self.interaction.cancel_pointer();
        self.pointer_owner = PointerOwner::ConsumedChrome;
        self.research_input = None;
        self.research_preedit.clear();
        self.curation = None;
        self.curation_preedit.clear();
        let groq = groq_credential_state();
        if let GroqCredentialState::Unavailable(detail) = &groq {
            eprintln!("OS credential vault unavailable: {detail}");
            self.status = Some(
                "Groq voice needs Credential Manager, KWallet, or GNOME Keyring; reopen Connections after starting one."
                    .into(),
            );
        }
        self.settings = Some(SettingsFlow::Menu {
            groq,
            hermes_available: hermes_is_available(),
        });
        self.settings_preedit.zeroize();
        self.interaction.cursor_left(now);
        true
    }

    fn refresh_settings_menu(&mut self) {
        self.settings = Some(SettingsFlow::Menu {
            groq: groq_credential_state(),
            hermes_available: hermes_is_available(),
        });
        self.settings_preedit.zeroize();
    }

    fn settings_anchor(&self) -> Point {
        Point {
            x: (self.physical_width * 0.5 - 170.0).max(12.0),
            y: (self.physical_height * 0.18).max(72.0),
        }
    }

    fn settings_menu_at_cursor(&self) -> Option<usize> {
        let cursor = self.cursor?;
        let count = self.settings_choices().len();
        hit_curation_menu(
            cursor,
            self.physical_width,
            self.physical_height,
            self.scale_factor,
            self.settings_anchor(),
            count,
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
            SettingsFlow::GroqKey { .. } => Some(true),
        }
    }

    fn activate_settings_row(&mut self, row: usize) -> bool {
        let Some((choice, _)) = self.settings_choices().get(row).cloned() else {
            return false;
        };
        match choice {
            SettingsChoice::CycleBackend => {
                let next = self.user_settings.research_backend.next();
                let candidate = UserSettings {
                    research_backend: next,
                };
                match self.settings_store.save(candidate) {
                    Ok(()) => {
                        self.user_settings = candidate;
                        self.status = Some(format!(
                            "Research routing is now {}; environment overrides still take precedence.",
                            next.label()
                        ));
                    }
                    Err(error) => {
                        self.status = Some(format!("Research routing was not saved: {error}"));
                    }
                }
                self.refresh_settings_menu();
            }
            SettingsChoice::GroqKey => {
                self.settings = Some(SettingsFlow::GroqKey {
                    input: Zeroizing::new(String::new()),
                });
                self.settings_preedit.zeroize();
            }
            SettingsChoice::Hermes => {
                if hermes_is_available() {
                    match open_hermes_dashboard() {
                        Ok(()) => {
                            self.status = Some(
                                "Hermes owns provider, model, OAuth, and API-key configuration in the opened dashboard."
                                    .into(),
                            );
                        }
                        Err(error) => {
                            self.status = Some(format!("Hermes dashboard did not open: {error}"));
                        }
                    }
                } else {
                    match open_external_url(HERMES_INSTALL_URL) {
                        Ok(()) => {
                            self.status = Some(
                                "Opened the Hermes installation guide; return here after Hermes is on PATH."
                                    .into(),
                            );
                        }
                        Err(error) => {
                            self.status =
                                Some(format!("Hermes installation guide did not open: {error}"));
                        }
                    }
                }
                self.refresh_settings_menu();
            }
            SettingsChoice::RemoveGroq => {
                match delete_groq_api_key() {
                    Ok(()) => self.status = Some("Groq key removed from the OS vault.".into()),
                    Err(error) => {
                        self.status = Some(format!("Groq vault key was not removed: {error}"));
                    }
                }
                self.refresh_settings_menu();
            }
            SettingsChoice::Close => {
                self.settings = None;
                self.settings_preedit.zeroize();
            }
        }
        true
    }

    fn commit_groq_key(&mut self) -> bool {
        let Some(SettingsFlow::GroqKey { mut input }) = self.settings.take() else {
            return false;
        };
        match save_groq_api_key(input.as_str()) {
            Ok(()) => {
                self.status = Some(
                    "Groq transcription key stored in the operating-system credential vault."
                        .into(),
                );
            }
            Err(error) => self.status = Some(format!("Groq key was not stored: {error}")),
        }
        input.zeroize();
        self.refresh_settings_menu();
        true
    }

    fn settings_keyboard_input(
        &mut self,
        event: &not_news_platform::winit::event::KeyEvent,
    ) -> Option<bool> {
        let menu = matches!(self.settings, Some(SettingsFlow::Menu { .. }));
        let key_input = matches!(self.settings, Some(SettingsFlow::GroqKey { .. }));
        if !menu && !key_input {
            return None;
        }
        if matches!(event.logical_key, Key::Named(NamedKey::Escape)) {
            if key_input {
                self.refresh_settings_menu();
            } else {
                self.settings = None;
                self.settings_preedit.zeroize();
            }
            return Some(true);
        }
        let command = self.modifiers.control_key() || self.modifiers.super_key();
        if menu
            && !command
            && !self.modifiers.alt_key()
            && let Key::Character(key) = &event.logical_key
            && let Some(row) = menu_row_from_key(key)
        {
            return Some(self.activate_settings_row(row));
        }
        if !key_input {
            return Some(false);
        }
        if matches!(event.logical_key, Key::Named(NamedKey::Enter)) {
            return Some(self.commit_groq_key());
        }
        if matches!(event.logical_key, Key::Named(NamedKey::Backspace)) {
            if let Some(SettingsFlow::GroqKey { input }) = self.settings.as_mut() {
                remove_last_grapheme(input);
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
            if let Some(SettingsFlow::GroqKey { input }) = self.settings.as_mut() {
                input.zeroize();
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
        if let Some(SettingsFlow::GroqKey { input }) = self.settings.as_mut() {
            append_bounded_secret(input, text);
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
        let count = self.curation_choices(subject, *stage, *page).len();
        hit_curation_menu(
            cursor,
            self.physical_width,
            self.physical_height,
            self.scale_factor,
            *anchor,
            count,
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
                false
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
        if event.repeat {
            return false;
        }
        if matches!(event.logical_key, Key::Named(NamedKey::Escape)) && self.cancel_voice() {
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
        if matches!(self.settings, Some(SettingsFlow::GroqKey { .. })) {
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
        if self.research.is_some() || !self.voice.is_idle() {
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
        if self.research.is_some() || !self.voice.is_idle() {
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
        if self.research.is_some() {
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
        if self.research.is_some() {
            self.status = Some("Research is already running; press Escape to cancel it.".into());
            return true;
        }
        if self.store.is_none() {
            self.status = Some("Research is unavailable because the graph did not open.".into());
            return true;
        }
        let session_id = self.next_operation_id("research");
        if let Err(error) = self
            .store
            .as_ref()
            .expect("store existence checked")
            .start_research_session(&session_id, question)
        {
            self.status = Some(format!("Research session could not start: {error}"));
            return true;
        }
        let prompt = build_research_prompt(question, &self.graph);
        let scratch_directory = self.research_directory.join(&session_id);
        let launch = match ResearchLaunch::from_configuration(
            &prompt,
            &scratch_directory,
            self.user_settings.research_backend.into(),
        ) {
            Ok(launch) => launch,
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
                return true;
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
                self.generated_research_events.clear();
                self.auto_follow_research = true;
                self.set_activity_open(true, Instant::now());
                self.status = Some("Starting research…".into());
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
        true
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
        if self.auto_follow_research && !self.generated_research_events.is_empty() {
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
        self.research.is_some() || !self.research_messages.is_empty()
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

    fn cursor_moved(&mut self, point: Point, now: Instant) -> bool {
        self.cursor = Some(point);
        if self.record_hold_deadline.is_some()
            && self.chrome_at_cursor() != Some(ChromeControl::Record)
        {
            self.record_hold_deadline = None;
        }
        if self.settings.is_some() {
            self.interaction.cursor_left(now)
        } else if matches!(self.pointer_owner, PointerOwner::Canvas) {
            let changed = self.interaction.cursor_moved(point, &self.graph, now);
            self.stop_auto_follow_if_panning();
            changed
        } else if self.activity_surface_at_cursor(now)
            || self.chrome_at_cursor().is_some()
            || self.metadata_at_cursor()
        {
            self.interaction.cursor_left(now)
        } else {
            self.interaction.cursor_moved(point, &self.graph, now)
        }
    }

    fn mouse_input(&mut self, state: ElementState, now: Instant) -> bool {
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
            if let Some(control) = self.chrome_at_cursor() {
                self.pointer_owner = PointerOwner::FixedChrome(control);
                if control == ChromeControl::Record && self.voice.is_recording() {
                    self.record_hold_deadline = Some(now + Duration::from_millis(500));
                }
                return self.interaction.cursor_left(now);
            }
            if self.metadata_at_cursor() {
                self.pointer_owner = PointerOwner::MetadataSurface;
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
        if self.settings.is_some() {
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

    fn metadata_at_cursor(&self) -> bool {
        let Some(cursor) = self.cursor else {
            return false;
        };
        let Some((active, position)) = self.interaction.active_event_position() else {
            return false;
        };
        hit_active_metadata(
            cursor,
            self.physical_width,
            self.physical_height,
            self.scale_factor,
            &self.graph.events[active],
            position,
        )
    }

    fn scroll_metadata(&mut self, delta: MouseScrollDelta) -> bool {
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
        let next = (self.metadata_scroll + scroll_pixels(delta)).clamp(0.0, maximum);
        let changed = (next - self.metadata_scroll).abs() > f64::EPSILON;
        self.metadata_scroll = next;
        changed
    }

    fn sync_metadata_scroll(&mut self, active: Option<&EventId>) {
        if self.metadata_scroll_event.as_ref() == active {
            return;
        }
        let active = active.cloned();
        self.metadata_scroll_event.clone_from(&active);
        self.metadata_scroll = 0.0;
    }

    fn record_orb_state(&self) -> RecordOrbState {
        if self.research.is_some() || !self.voice.is_idle() {
            RecordOrbState::Busy
        } else {
            RecordOrbState::Idle
        }
    }

    fn paint_composer(&self, canvas: &Canvas, width: f32, height: f32, scale_factor: f32) {
        if let Some(SettingsFlow::GroqKey { input }) = self.settings.as_ref() {
            paint_curation_prompt(
                canvas,
                width,
                height,
                scale_factor,
                "GROQ TRANSCRIPTION",
                "ENTER STORE IN OS VAULT  ·  ESC BACK  ·  CTRL+V PASTE",
                &masked_secret(input),
                &masked_secret(&self.settings_preedit),
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
        if let Some(active) = state.expanded_event.as_ref() {
            paint_active_metadata(
                canvas,
                width,
                height,
                scale_factor,
                &self.graph.events[active],
                state.positions[active],
                scale_scalar(self.metadata_scroll),
            );
        }
        if let Some(status) = self.status.as_deref() {
            paint_status(
                canvas,
                width,
                height,
                scale_factor,
                status,
                self.research.is_some(),
            );
        }
        if self.activity_visible() {
            paint_activity_drawer(
                canvas,
                width,
                height,
                scale_factor,
                &self.research_messages,
                self.research.is_some(),
                scale_scalar(activity_progress),
            );
        }
        self.paint_composer(canvas, width, height, scale_factor);
        self.paint_curation_surface(canvas, width, height, scale_factor);
        paint_fixed_chrome(
            canvas,
            width,
            height,
            scale_factor,
            viewport.zoom,
            self.record_orb_state(),
        );
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
        paint_curation_menu(canvas, width, height, scale_factor, *anchor, title, &items);
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
        paint_curation_menu(
            canvas,
            width,
            height,
            scale_factor,
            self.settings_anchor(),
            "CONNECTIONS  ·  CTRL+,",
            &items,
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
                self.interaction.cursor_left(now)
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
                    false
                } else if self.metadata_at_cursor() {
                    self.scroll_metadata(*delta)
                } else if self.activity_surface_at_cursor(now) {
                    false
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
        self.drain_research();
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
        let voice_deadline = (!self.voice.is_idle()).then(|| frame.now + Duration::from_millis(33));
        let speech_deadline = self
            .speech
            .is_busy()
            .then(|| frame.now + Duration::from_millis(50));
        let activity_deadline = self
            .activity_motion
            .map(|_| frame.now + Duration::from_millis(16));
        interaction_deadline
            .into_iter()
            .chain(research_deadline)
            .chain(voice_deadline)
            .chain(speech_deadline)
            .chain(self.record_hold_deadline)
            .chain(activity_deadline)
            .min()
            .map_or(FrameSchedule::Wait, FrameSchedule::RedrawAt)
    }

    fn text_input_active(&self) -> bool {
        self.research_input.is_some()
            || matches!(self.settings, Some(SettingsFlow::GroqKey { .. }))
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

fn main() -> Result<(), Box<dyn Error>> {
    let options = startup_options(std::env::args_os().skip(1))?;
    if let Some(root) = options.capability_root {
        return run_capability_check(&root);
    }
    if let Some(source) = options.performance_source {
        return run_reference_performance_check(&source);
    }
    if let Some(root) = options.release_smoke {
        let check = release_check::run(&root)?;
        let mut application = CanvasApplication::load(&check.database);
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
            "{{\"release_self_check\":\"pass\",\"version\":\"{}\",\"commit\":\"{}\",\"renderer\":\"{}\",\"events\":{},\"bridges\":{},\"revision\":{}}}",
            env!("CARGO_PKG_VERSION"),
            option_env!("NOT_NEWS_BUILD_COMMIT").unwrap_or("development"),
            report.renderer.as_str(),
            check.imported_events,
            check.imported_bridges,
            check.final_revision,
        );
        return Ok(());
    }
    let mut application = match options.database {
        Ok(database) => CanvasApplication::load(&database),
        Err(error) => CanvasApplication::unavailable(format!(
            "Data directory unavailable; no graph is open: {error}"
        )),
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
        });
    }
    let mut database = None;
    let mut import_legacy = None;
    let mut release_smoke = None;
    let mut performance_source = None;
    let mut capability_root = None;
    let mut index = 0;
    while index < arguments.len() {
        let flag = &arguments[index];
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
        + usize::from(capability_root.is_some());
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
    })
}

fn run_capability_check(root: &Path) -> Result<(), Box<dyn Error>> {
    let settings = SettingsStore::new(root).load()?;
    let opencode = opencode_is_available();
    let hermes = hermes_is_available();
    let selected_ready = match settings.research_backend {
        settings::StoredResearchBackend::Auto => opencode || hermes,
        settings::StoredResearchBackend::OpenCode => opencode,
        settings::StoredResearchBackend::Hermes => hermes,
    };
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
            "selected": settings.research_backend.label().to_ascii_lowercase(),
            "selected_ready": selected_ready,
            "opencode": if opencode { "available" } else { "missing" },
            "hermes": if hermes { "available" } else { "missing" },
            "remediation": "Install and authenticate OpenCode, or install Hermes and configure its provider dashboard."
        },
        "transcription": {
            "environment_override": if std::env::var_os("GROQ_API_KEY").is_some_and(|value| !value.is_empty()) { "configured" } else { "missing" },
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

    #[cfg(unix)]
    use not_news_agent::{OutputProtocol, ProcessLimits, ResearchBackend};
    use tempfile::TempDir;

    use super::*;

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
        let application = CanvasApplication::with_state(None, graph, None, None);
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
}
