mod interaction;

use std::{
    error::Error,
    path::PathBuf,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use interaction::{CanvasInteraction, InteractionEffect};
use not_news_domain::{GraphSnapshot, Point};
use not_news_platform::{
    FrameInfo, FrameSchedule, PlatformApplication, WindowOptions, run,
    skia_safe::Canvas,
    winit::{
        dpi::PhysicalPosition,
        event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent},
        keyboard::{Key, ModifiersState},
    },
};
use not_news_renderer::{
    SceneAnimation, SceneState, paint_background, paint_graph, paint_grid, resolved_positions,
};
use not_news_store::{CommitOutcome, DurableGraphStore};

struct CanvasApplication {
    store: DurableGraphStore,
    graph: GraphSnapshot,
    interaction: CanvasInteraction,
    modifiers: ModifiersState,
    operation_epoch: u128,
    operation_sequence: u64,
}

impl CanvasApplication {
    fn load(database: PathBuf) -> Result<Self, Box<dyn Error>> {
        let store = DurableGraphStore::open(database)?;
        let graph = store.load()?;
        let interaction = CanvasInteraction::new(resolved_positions(&graph));
        Ok(Self {
            store,
            graph,
            interaction,
            modifiers: ModifiersState::default(),
            operation_epoch: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
            operation_sequence: 0,
        })
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
                match self.store.commit_move(&operation, &command) {
                    Ok(outcome) => self.apply_outcome(outcome),
                    Err(error) => eprintln!("move was not committed: {error}"),
                }
                true
            }
        }
    }

    fn undo(&mut self) -> bool {
        let operation = self.next_operation_id("undo");
        match self.store.undo(&operation) {
            Ok(Some(outcome)) => {
                self.apply_outcome(outcome);
                true
            }
            Ok(None) => false,
            Err(error) => {
                eprintln!("undo was not committed: {error}");
                false
            }
        }
    }

    fn redo(&mut self) -> bool {
        let operation = self.next_operation_id("redo");
        match self.store.redo(&operation) {
            Ok(Some(outcome)) => {
                self.apply_outcome(outcome);
                true
            }
            Ok(None) => false,
            Err(error) => {
                eprintln!("redo was not committed: {error}");
                false
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
}

impl PlatformApplication for CanvasApplication {
    fn window_event(&mut self, event: &WindowEvent) -> bool {
        let now = Instant::now();
        match event {
            WindowEvent::Resized(size) => {
                self.interaction.resize(size.width, size.height);
                true
            }
            WindowEvent::CursorMoved { position, .. } => self.interaction.cursor_moved(
                Point {
                    x: position.x,
                    y: position.y,
                },
                &self.graph,
                now,
            ),
            WindowEvent::CursorLeft { .. } => self.interaction.cursor_left(now),
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => match state {
                ElementState::Pressed => self.interaction.pointer_down(&self.graph, now),
                ElementState::Released => {
                    let effect = self.interaction.pointer_up(&self.graph, now);
                    self.commit_effect(effect)
                }
            },
            WindowEvent::MouseWheel { delta, .. } => self.interaction.scroll(scroll_pixels(*delta)),
            WindowEvent::ModifiersChanged(modifiers) => {
                self.modifiers = modifiers.state();
                false
            }
            WindowEvent::KeyboardInput { event, .. } => self.keyboard_input(event),
            WindowEvent::Focused(false) => self.interaction.cancel_pointer(),
            _ => false,
        }
    }

    fn render(&mut self, canvas: &Canvas, frame: FrameInfo) -> FrameSchedule {
        self.interaction
            .resize(frame.physical_width, frame.physical_height);
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
        self.interaction
            .next_deadline(frame.now)
            .map_or(FrameSchedule::Wait, FrameSchedule::RedrawAt)
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let database = std::env::args_os()
        .nth(1)
        .map_or_else(default_database_path, PathBuf::from);
    let application = CanvasApplication::load(database)?;
    run(application, WindowOptions::default())?;
    Ok(())
}

fn default_database_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../backend/data/graph.sqlite")
}

#[allow(clippy::cast_precision_loss)]
fn physical_scalar(value: u32) -> f32 {
    value as f32
}

fn scroll_pixels(delta: MouseScrollDelta) -> f64 {
    match delta {
        MouseScrollDelta::LineDelta(_, vertical) => f64::from(vertical) * 40.0,
        MouseScrollDelta::PixelDelta(PhysicalPosition { y, .. }) => y,
    }
}
