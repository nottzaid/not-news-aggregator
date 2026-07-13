use not_news_domain::Point as WorldPoint;
use skia_safe::{Canvas, Paint, Point};

use crate::{Viewport, ViewportTransform, palette};

const MINOR_STEP: f64 = 48.0;
const MAJOR_STEP: f64 = 240.0;
const CULL_MARGIN_PIXELS: f64 = 220.0;

/// Paints Flutter's world-space grid over the currently visible unbounded area.
///
/// # Panics
///
/// Panics when the viewport dimensions or zoom are invalid, matching
/// [`ViewportTransform::new`].
pub fn paint_grid(canvas: &Canvas, width: f32, height: f32, viewport: Viewport) {
    let transform = ViewportTransform::new(f64::from(width), f64::from(height), viewport);
    let scale = transform.scale();
    let top_left = transform.screen_to_world(WorldPoint { x: 0.0, y: 0.0 });
    let bottom_right = transform.screen_to_world(WorldPoint {
        x: f64::from(width),
        y: f64::from(height),
    });
    let margin = CULL_MARGIN_PIXELS / scale;
    let left = top_left.x - margin;
    let top = top_left.y - margin;
    let right = bottom_right.x + margin;
    let bottom = bottom_right.y + margin;
    let origin = transform.origin();

    canvas.save();
    canvas.translate((scalar(origin.x), scalar(origin.y)));
    canvas.scale((scalar(scale), scalar(scale)));
    canvas.translate((scalar(-viewport.camera.x), scalar(-viewport.camera.y)));

    let mut minor = Paint::default();
    minor.set_color(palette::color(palette::GRID));
    minor.set_stroke_width(1.0);
    paint_lines(canvas, &minor, left, top, right, bottom, MINOR_STEP);

    let mut major = Paint::default();
    major.set_color(palette::color(palette::GRID_MAJOR));
    major.set_stroke_width(1.0);
    paint_lines(canvas, &major, left, top, right, bottom, MAJOR_STEP);
    canvas.restore();
}

fn paint_lines(
    canvas: &Canvas,
    paint: &Paint,
    left: f64,
    top: f64,
    right: f64,
    bottom: f64,
    step: f64,
) {
    let start_x = (left / step).floor() * step;
    let start_y = (top / step).floor() * step;
    let mut x = start_x;
    while x <= right {
        canvas.draw_line(
            Point::new(scalar(x), scalar(start_y)),
            Point::new(scalar(x), scalar(bottom)),
            paint,
        );
        x += step;
    }
    let mut y = start_y;
    while y <= bottom {
        canvas.draw_line(
            Point::new(scalar(start_x), scalar(y)),
            Point::new(scalar(right), scalar(y)),
            paint,
        );
        y += step;
    }
}

#[allow(clippy::cast_possible_truncation)]
fn scalar(value: f64) -> f32 {
    assert!(value.is_finite() && value >= f64::from(f32::MIN) && value <= f64::from(f32::MAX));
    value as f32
}

#[cfg(test)]
mod tests {
    use skia_safe::surfaces;

    use super::*;

    #[test]
    fn grid_renders_after_large_unbounded_camera_translation() {
        let mut surface = surfaces::raster_n32_premul((320, 180)).unwrap();
        paint_grid(
            surface.canvas(),
            320.0,
            180.0,
            Viewport {
                camera: WorldPoint {
                    x: 7_000_000.0,
                    y: -9_000_000.0,
                },
                zoom: 2.8,
            },
        );
        let image = surface.image_snapshot();
        let pixels = image.peek_pixels().unwrap();
        assert!(pixels.bytes().unwrap().iter().any(|channel| *channel != 0));
    }
}
