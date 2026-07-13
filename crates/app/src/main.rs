mod interaction;

use std::{error::Error, path::PathBuf, time::Instant};

use interaction::CanvasInteraction;
use not_news_domain::{GraphSnapshot, Point};
use not_news_platform::{
    FrameInfo, FrameSchedule, PlatformApplication, WindowOptions, run,
    skia_safe::Canvas,
    winit::{
        dpi::PhysicalPosition,
        event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent},
    },
};
use not_news_renderer::{
    SceneAnimation, SceneState, paint_background, paint_graph, paint_grid, resolved_positions,
};
use not_news_store::LegacyGraphReader;

struct CanvasApplication {
    _database: PathBuf,
    graph: GraphSnapshot,
    interaction: CanvasInteraction,
}

impl CanvasApplication {
    fn load(database: PathBuf) -> Result<Self, Box<dyn Error>> {
        let graph = LegacyGraphReader::new(&database).load()?;
        let interaction = CanvasInteraction::new(resolved_positions(&graph));
        Ok(Self {
            _database: database,
            graph,
            interaction,
        })
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
                ElementState::Released => self
                    .interaction
                    .pointer_up(&mut self.graph, now)
                    .unwrap_or_else(|error| {
                        eprintln!("move rejected: {error}");
                        false
                    }),
            },
            WindowEvent::MouseWheel { delta, .. } => self.interaction.scroll(scroll_pixels(*delta)),
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
