use skia_safe::{
    Canvas, Color4f, EncodedImageFormat, Paint, PaintCap, Point, Rect, TileMode,
    canvas::PointMode,
    gradient::{Colors, Gradient, Interpolation, shaders},
    surfaces,
};

use crate::palette::{
    DATA, INK_0, INK_1, INK_2, PLUM, SIGNAL, color, color4f, flutter_gradient_color4f,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackgroundError {
    InvalidSize,
    SurfaceAllocation,
    PngEncoding,
}

/// Paints the viewport-space background inherited from the Flutter renderer.
///
/// # Panics
///
/// Panics when either extent is non-finite or non-positive. The platform
/// adapter must suppress zero-sized frames before acquiring a surface.
pub fn paint_background(canvas: &Canvas, width: f32, height: f32) {
    assert!(width.is_finite() && width > 0.0);
    assert!(height.is_finite() && height > 0.0);
    let rect = Rect::from_xywh(0.0, 0.0, width, height);

    let base_colors = [color4f(INK_0), color4f(INK_1), color4f(INK_2)];
    let base_stops = [0.0, 0.55, 1.0];
    let base = gradient(&base_colors, Some(&base_stops));
    let mut paint = Paint::default();
    paint.set_shader(shaders::linear_gradient(
        ((width / 2.0, 0.0), (width / 2.0, height)),
        &base,
        None,
    ));
    canvas.draw_rect(rect, &paint);

    let extent = width.max(height);
    let glows = [
        ((0.14, 0.12), flutter_gradient_color4f(SIGNAL, 0.21), 0.62),
        ((0.86, 0.18), flutter_gradient_color4f(PLUM, 0.16), 0.58),
        ((0.80, 0.88), flutter_gradient_color4f(DATA, 0.15), 0.66),
    ];
    for ((x, y), glow_color, radius_factor) in glows {
        let center = Point::new(width * x, height * y);
        let radius = extent * radius_factor;
        let glow_colors = [glow_color, Color4f::new(0.0, 0.0, 0.0, 0.0)];
        let glow = gradient(&glow_colors, None);
        paint.set_shader(shaders::radial_gradient((center, radius), &glow, None));
        canvas.draw_circle(center, radius, &paint);
    }

    let vignette_colors = [color4f(0x0006_090f), color4f(0x5406_090f), color4f(INK_0)];
    let vignette_stops = [0.0, 0.62, 1.0];
    let vignette = gradient(&vignette_colors, Some(&vignette_stops));
    let center = Point::new(width * 0.5, height * 0.47);
    paint.set_shader(shaders::radial_gradient(
        (center, width.min(height) * 1.38),
        &vignette,
        None,
    ));
    canvas.draw_rect(rect, &paint);

    let mut grain = Vec::new();
    let mut random = FlutterGrain::new();
    let spacing = 50.0;
    let mut y = -spacing;
    while y < height + spacing {
        let mut x = -spacing;
        while x < width + spacing {
            grain.push(Point::new(
                skia_scalar(f64::from(x) + (random.next() - 0.5) * 30.0),
                skia_scalar(f64::from(y) + (random.next() - 0.5) * 30.0),
            ));
            x += spacing;
        }
        y += spacing;
    }
    paint.set_shader(None);
    paint.set_color(color(0x0cff_ffff));
    paint.set_stroke_width(1.25);
    paint.set_stroke_cap(PaintCap::Round);
    canvas.draw_points(PointMode::Points, &grain, &paint);
}

/// Renders a deterministic raster oracle without opening a native window.
///
/// # Errors
///
/// Returns an error for invalid dimensions, raster-surface allocation
/// failure, or PNG encoding failure.
pub fn render_background_png(width: i32, height: i32) -> Result<Vec<u8>, BackgroundError> {
    if width <= 0 || height <= 0 {
        return Err(BackgroundError::InvalidSize);
    }
    let mut surface =
        surfaces::raster_n32_premul((width, height)).ok_or(BackgroundError::SurfaceAllocation)?;
    paint_background(
        surface.canvas(),
        dimension_to_scalar(width),
        dimension_to_scalar(height),
    );
    let image = surface.image_snapshot();
    let png = image
        .encode(None, EncodedImageFormat::PNG, None)
        .ok_or(BackgroundError::PngEncoding)?;
    Ok(png.as_bytes().to_vec())
}

fn gradient<'a>(colors: &'a [Color4f], stops: Option<&'a [f32]>) -> Gradient<'a> {
    Gradient::new(
        Colors::new(colors, stops, TileMode::Clamp, None),
        Interpolation::default(),
    )
}

#[allow(clippy::cast_precision_loss)]
fn dimension_to_scalar(value: i32) -> f32 {
    debug_assert!(value > 0);
    value as f32
}

#[allow(clippy::cast_possible_truncation)]
fn skia_scalar(value: f64) -> f32 {
    debug_assert!(
        value.is_finite() && value >= f64::from(f32::MIN) && value <= f64::from(f32::MAX)
    );
    value as f32
}

struct FlutterGrain {
    state: u64,
}

impl FlutterGrain {
    fn new() -> Self {
        Self {
            state: 2_654_435_769,
        }
    }

    fn next(&mut self) -> f64 {
        self.state = (self.state * 1_664_525 + 1_013_904_223) & 0x7fff_ffff;
        let state = u32::try_from(self.state).expect("grain state is masked to 31 bits");
        f64::from(state) / 2_147_483_647.0
    }
}

#[cfg(test)]
mod tests {
    use skia_safe::{Data, Image, ImageInfo, image::CachingHint};

    use super::*;

    #[test]
    fn background_raster_is_deterministic_and_nonempty() {
        let first = render_background_png(320, 180).unwrap();
        let second = render_background_png(320, 180).unwrap();
        assert_eq!(first, second);
        assert!(first.starts_with(b"\x89PNG\r\n\x1a\n"));
        assert!(first.len() > 10_000);
    }

    #[test]
    fn flutter_grain_sequence_is_stable() {
        let mut random = FlutterGrain::new();
        let values = [random.next(), random.next(), random.next()];
        let expected = [
            0.521_998_377_759_940_2,
            0.821_472_207_001_164_6,
            0.496_857_832_417_291_53,
        ];
        for (actual, expected) in values.into_iter().zip(expected) {
            assert!((actual - expected).abs() < f64::EPSILON);
        }
    }

    #[test]
    fn background_stays_within_flutter_raster_budget() {
        const FLUTTER_PNG: &[u8] = include_bytes!("../../../test/goldens/background-320x180.png");
        let rust_png = render_background_png(320, 180).unwrap();
        let flutter = decode_png(FLUTTER_PNG, 320, 180);
        let rust = decode_png(&rust_png, 320, 180);
        let mut changed_pixels = 0usize;
        let mut absolute_delta = 0u64;
        let mut maximum_delta = 0u8;
        for (flutter, rust) in flutter.chunks_exact(4).zip(rust.chunks_exact(4)) {
            let mut changed = false;
            for (&expected, &actual) in flutter.iter().zip(rust) {
                let delta = expected.abs_diff(actual);
                changed |= delta != 0;
                absolute_delta += u64::from(delta);
                maximum_delta = maximum_delta.max(delta);
            }
            changed_pixels += usize::from(changed);
        }
        let channel_count = u64::try_from(flutter.len()).unwrap();
        let mean_absolute_delta = ratio(absolute_delta, channel_count);
        assert!(
            mean_absolute_delta <= 1.0 && maximum_delta <= 8,
            "Flutter/Rust background drift: {changed_pixels}/57600 pixels changed; \
             mean channel delta {mean_absolute_delta:.6}; max {maximum_delta}"
        );
    }

    fn decode_png(png: &[u8], width: i32, height: i32) -> Vec<u8> {
        let image = Image::from_encoded(Data::new_copy(png)).unwrap();
        assert_eq!((image.width(), image.height()), (width, height));
        let info = ImageInfo::new_n32_premul((width, height), None);
        let row_bytes = info.min_row_bytes();
        let mut pixels = vec![0; info.compute_byte_size(row_bytes)];
        assert!(image.read_pixels(&info, &mut pixels, row_bytes, (0, 0), CachingHint::Disallow,));
        pixels
    }

    #[allow(clippy::cast_precision_loss)]
    fn ratio(numerator: u64, denominator: u64) -> f64 {
        numerator as f64 / denominator as f64
    }
}
