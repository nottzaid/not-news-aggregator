pub mod background;
pub mod grid;
pub mod layout;
pub mod motion;
pub mod palette;
pub mod viewport;

pub use background::{BackgroundError, paint_background, render_background_png};
pub use grid::paint_grid;
pub use layout::{generate_positions, primary_component_ids, resolved_positions};
pub use motion::{Motion, cubic_transform};
pub use viewport::{REFERENCE_HEIGHT, REFERENCE_WIDTH, Viewport, ViewportTransform};
