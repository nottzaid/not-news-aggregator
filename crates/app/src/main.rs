use std::{collections::HashMap, error::Error, path::PathBuf};

use not_news_domain::{EventId, GraphSnapshot, Point};
use not_news_platform::{
    FrameInfo, FrameSchedule, PlatformApplication, WindowOptions, run, skia_safe::Canvas,
};
use not_news_renderer::{Viewport, paint_background, paint_grid, resolved_positions};
use not_news_store::LegacyGraphReader;

struct CanvasApplication {
    _database: PathBuf,
    _graph: GraphSnapshot,
    _positions: HashMap<EventId, Point>,
    viewport: Viewport,
}

impl CanvasApplication {
    fn load(database: PathBuf) -> Result<Self, Box<dyn Error>> {
        let graph = LegacyGraphReader::new(&database).load()?;
        let positions = resolved_positions(&graph);
        Ok(Self {
            _database: database,
            _graph: graph,
            _positions: positions,
            viewport: Viewport::default(),
        })
    }
}

impl PlatformApplication for CanvasApplication {
    fn render(&mut self, canvas: &Canvas, frame: FrameInfo) -> FrameSchedule {
        let width = physical_scalar(frame.physical_width);
        let height = physical_scalar(frame.physical_height);
        paint_background(canvas, width, height);
        paint_grid(canvas, width, height, self.viewport);
        FrameSchedule::Wait
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
