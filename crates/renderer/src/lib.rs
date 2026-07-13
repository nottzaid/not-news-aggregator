pub mod artifacts;
pub mod background;
pub mod grid;
pub mod layout;
pub mod motion;
pub mod palette;
pub mod scene;
pub mod viewport;

pub use artifacts::{ArtifactLayout, ArtifactMetrics, layout_artifacts};
pub use background::{BackgroundError, paint_background, render_background_png};
pub use grid::paint_grid;
pub use layout::{
    expanded_positions, generate_positions, primary_component_ids, resolved_positions,
};
pub use motion::{Motion, cubic_transform};
pub use scene::{SceneAnimation, SceneState, paint_closed_graph, paint_graph};
pub use viewport::{REFERENCE_HEIGHT, REFERENCE_WIDTH, Viewport, ViewportTransform};
