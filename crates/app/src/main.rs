mod interaction;

use std::{
    error::Error,
    path::{Path, PathBuf},
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use interaction::{CanvasInteraction, InteractionEffect};
use not_news_domain::{GraphSnapshot, Point};
use not_news_platform::{
    FrameInfo, FrameSchedule, PlatformApplication, WindowOptions, application_data_directory,
    open_external_url, run,
    skia_safe::Canvas,
    winit::{
        dpi::PhysicalPosition,
        event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent},
        keyboard::{Key, ModifiersState},
    },
};
use not_news_renderer::{
    ChromeControl, SceneAnimation, SceneState, hit_fixed_chrome, paint_active_metadata,
    paint_background, paint_fixed_chrome, paint_graph, paint_grid, paint_status,
    resolved_positions,
};
use not_news_store::{CommitOutcome, DurableGraphStore};

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
    chrome_press: Option<ChromeControl>,
    canvas_pointer_active: bool,
}

impl CanvasApplication {
    fn load(database: &Path) -> Self {
        match DurableGraphStore::open(database) {
            Ok(store) => {
                let graph = match store.load() {
                    Ok(graph) => graph,
                    Err(error) => {
                        return Self::unavailable(format!(
                            "Graph became unreadable during startup; writes are disabled. {}: {error}",
                            database.display()
                        ));
                    }
                };
                let status = store.migration_backup().map(|backup| {
                    format!(
                        "Legacy graph migrated after verified backup: {}",
                        backup.display()
                    )
                });
                Self::with_state(Some(store), graph, status)
            }
            Err(error) => Self::unavailable(format!(
                "Graph unavailable; no research is shown or writable. {}: {error}",
                database.display()
            )),
        }
    }

    fn unavailable(status: String) -> Self {
        Self::with_state(None, GraphSnapshot::default(), Some(status))
    }

    fn with_state(
        store: Option<DurableGraphStore>,
        graph: GraphSnapshot,
        status: Option<String>,
    ) -> Self {
        let interaction = CanvasInteraction::new(resolved_positions(&graph));
        Self {
            store,
            graph,
            interaction,
            status,
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
            chrome_press: None,
            canvas_pointer_active: false,
        }
    }

    fn apply_outcome(&mut self, outcome: CommitOutcome) {
        self.graph = outcome.snapshot;
        self.interaction.placement_committed(&self.graph);
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

    fn keyboard_input(&mut self, event: &not_news_platform::winit::event::KeyEvent) -> bool {
        if event.state != ElementState::Pressed || event.repeat {
            return false;
        }
        let Key::Character(character) = &event.logical_key else {
            return false;
        };
        let command = self.modifiers.control_key() || self.modifiers.super_key();
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
            ChromeControl::Record => {
                self.status = Some("Voice research is not implemented in this Rust build.".into());
                true
            }
            ChromeControl::Clear => {
                self.status = Some("Canvas clearing is not implemented in this Rust build.".into());
                true
            }
        }
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
                self.cursor = Some(point);
                if self.canvas_pointer_active {
                    self.interaction.cursor_moved(point, &self.graph, now)
                } else if self.chrome_at_cursor().is_some() {
                    self.interaction.cursor_left(now)
                } else {
                    self.interaction.cursor_moved(point, &self.graph, now)
                }
            }
            WindowEvent::CursorLeft { .. } => {
                self.cursor = None;
                self.interaction.cursor_left(now)
            }
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => match state {
                ElementState::Pressed => {
                    if let Some(control) = self.chrome_at_cursor() {
                        self.chrome_press = Some(control);
                        self.interaction.cursor_left(now)
                    } else {
                        self.canvas_pointer_active = true;
                        self.interaction.pointer_down(&self.graph, now)
                    }
                }
                ElementState::Released => {
                    if self.canvas_pointer_active {
                        self.canvas_pointer_active = false;
                        let effect = self.interaction.pointer_up(&self.graph, now);
                        self.commit_effect(effect)
                    } else {
                        let pressed = self.chrome_press.take();
                        match (pressed, self.chrome_at_cursor()) {
                            (Some(pressed), Some(released)) if pressed == released => {
                                self.activate_chrome(released)
                            }
                            _ => false,
                        }
                    }
                }
            },
            WindowEvent::MouseWheel { delta, .. } => self.interaction.scroll(scroll_pixels(*delta)),
            WindowEvent::ModifiersChanged(modifiers) => {
                self.modifiers = modifiers.state();
                false
            }
            WindowEvent::KeyboardInput { event, .. } => self.keyboard_input(event),
            WindowEvent::Focused(false) => {
                self.chrome_press = None;
                self.canvas_pointer_active = false;
                self.interaction.cancel_pointer()
            }
            _ => false,
        }
    }

    fn render(&mut self, canvas: &Canvas, frame: FrameInfo) -> FrameSchedule {
        self.interaction
            .resize(frame.physical_width, frame.physical_height);
        self.physical_width = f64::from(frame.physical_width);
        self.physical_height = f64::from(frame.physical_height);
        self.scale_factor = frame.scale_factor;
        let width = physical_scalar(frame.physical_width);
        let height = physical_scalar(frame.physical_height);
        let state = self.interaction.frame(&self.graph, frame.now);
        let viewport = self.interaction.viewport();
        paint_background(canvas, width, height);
        paint_grid(canvas, width, height, viewport);
        paint_graph(
            canvas,
            width,
            height,
            viewport,
            &self.graph,
            &state.positions,
            SceneState {
                animation: SceneAnimation {
                    bridge_flow: state.bridge_flow,
                },
                bridge_event: state.bridge_event.as_ref(),
                expanded_event: state.expanded_event.as_ref(),
                expansion_progress: state.expansion_progress,
                collapsing_event: state.collapsing_event.as_ref(),
                collapse_progress: state.collapse_progress,
            },
        );
        if let Some(active) = state.expanded_event.as_ref() {
            paint_active_metadata(
                canvas,
                width,
                height,
                scale_scalar(frame.scale_factor),
                &self.graph.events[active],
                state.positions[active],
            );
        }
        if let Some(status) = self.status.as_deref() {
            paint_status(
                canvas,
                width,
                height,
                scale_scalar(frame.scale_factor),
                status,
                false,
            );
        }
        paint_fixed_chrome(
            canvas,
            width,
            height,
            scale_scalar(frame.scale_factor),
            viewport.zoom,
        );
        self.interaction
            .next_deadline(frame.now)
            .map_or(FrameSchedule::Wait, FrameSchedule::RedrawAt)
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let database = std::env::args_os()
        .nth(1)
        .map_or_else(default_database_path, |path| Ok(PathBuf::from(path)));
    let application = match database {
        Ok(database) => CanvasApplication::load(&database),
        Err(error) => CanvasApplication::unavailable(format!(
            "Data directory unavailable; no graph is open: {error}"
        )),
    };
    run(application, WindowOptions::default())?;
    Ok(())
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

fn scroll_pixels(delta: MouseScrollDelta) -> f64 {
    match delta {
        MouseScrollDelta::LineDelta(_, vertical) => f64::from(vertical) * 40.0,
        MouseScrollDelta::PixelDelta(PhysicalPosition { y, .. }) => y,
    }
}

#[cfg(test)]
mod app_tests {
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn first_launch_is_empty_while_malformed_data_is_visibly_disabled() {
        let directory = TempDir::new().unwrap();
        let fresh = CanvasApplication::load(&directory.path().join("fresh.sqlite"));
        assert!(fresh.store.is_some());
        assert!(fresh.graph.events.is_empty());
        assert!(fresh.status.is_none());

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
}
