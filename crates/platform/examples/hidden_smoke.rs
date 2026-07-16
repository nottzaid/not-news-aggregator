use not_news_platform::{
    FrameInfo, FrameSchedule, PlatformApplication, WindowOptions, run, skia_safe::Canvas,
};
use not_news_renderer::{Viewport, paint_background, paint_grid};

struct HiddenSmoke;

impl PlatformApplication for HiddenSmoke {
    fn render(&mut self, canvas: &Canvas, frame: FrameInfo) -> FrameSchedule {
        let width = physical_scalar(frame.physical_width);
        let height = physical_scalar(frame.physical_height);
        paint_background(canvas, width, height);
        paint_grid(canvas, width, height, Viewport::default());
        FrameSchedule::Exit
    }
}

fn main() -> Result<(), not_news_platform::PlatformError> {
    run(
        HiddenSmoke,
        WindowOptions {
            title: "Not News hidden surface smoke".to_owned(),
            logical_width: 320.0,
            logical_height: 180.0,
            visible: false,
            force_software: std::env::var_os("NOT_NEWS_FORCE_SOFTWARE").is_some(),
            frame_measurement: None,
        },
    )
    .map(|_| ())
}

#[allow(clippy::cast_precision_loss)]
fn physical_scalar(value: u32) -> f32 {
    value as f32
}
