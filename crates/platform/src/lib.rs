//! Windows/Linux window, input, and GPU-surface boundary.

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
compile_error!("not-news-platform supports Windows and Linux only");

mod open_gl;

use std::time::Instant;

use skia_safe::Canvas;
use winit::event::WindowEvent;

pub use open_gl::{PlatformError, WindowOptions, run};
pub use skia_safe;
pub use winit;

#[derive(Clone, Copy, Debug)]
pub struct FrameInfo {
    pub physical_width: u32,
    pub physical_height: u32,
    pub scale_factor: f64,
    pub now: Instant,
}

#[derive(Clone, Copy, Debug, Default)]
pub enum FrameSchedule {
    #[default]
    Wait,
    RedrawAt(Instant),
    Exit,
}

/// Safe application side of the native window and GPU boundary.
pub trait PlatformApplication {
    /// Handles an input or lifecycle event and reports whether it changed the
    /// pixels that should be presented.
    fn window_event(&mut self, _event: &WindowEvent) -> bool {
        false
    }

    /// Paints one frame and selects the next animation deadline.
    fn render(&mut self, canvas: &Canvas, frame: FrameInfo) -> FrameSchedule;
}
