use not_news_domain::Point;

pub const REFERENCE_WIDTH: f64 = 1_400.0;
pub const REFERENCE_HEIGHT: f64 = 900.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    pub camera: Point,
    pub zoom: f64,
}

impl Default for Viewport {
    fn default() -> Self {
        Self {
            camera: Point { x: 0.0, y: 0.0 },
            zoom: 1.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewportTransform {
    width: f64,
    height: f64,
    viewport: Viewport,
}

impl ViewportTransform {
    /// Builds the invertible mapping between physical pixels and world space.
    ///
    /// # Panics
    ///
    /// Panics for non-finite or non-positive extents or zoom.
    pub fn new(width: f64, height: f64, viewport: Viewport) -> Self {
        assert!(width.is_finite() && width > 0.0);
        assert!(height.is_finite() && height > 0.0);
        assert!(viewport.zoom.is_finite() && viewport.zoom > 0.0);
        Self {
            width,
            height,
            viewport,
        }
    }

    pub fn scale(self) -> f64 {
        (self.width / REFERENCE_WIDTH).min(self.height / REFERENCE_HEIGHT) * self.viewport.zoom
    }

    pub fn origin(self) -> Point {
        let scale = self.scale();
        Point {
            x: (self.width - REFERENCE_WIDTH * scale) / 2.0,
            y: (self.height - REFERENCE_HEIGHT * scale) / 2.0,
        }
    }

    pub fn screen_to_world(self, screen: Point) -> Point {
        let origin = self.origin();
        let scale = self.scale();
        Point {
            x: (screen.x - origin.x) / scale + self.viewport.camera.x,
            y: (screen.y - origin.y) / scale + self.viewport.camera.y,
        }
    }

    pub fn world_to_screen(self, world: Point) -> Point {
        let origin = self.origin();
        let scale = self.scale();
        Point {
            x: origin.x + (world.x - self.viewport.camera.x) * scale,
            y: origin.y + (world.y - self.viewport.camera.y) * scale,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_view_is_normalization_not_a_world_boundary() {
        let transform = ViewportTransform::new(
            1_280.0,
            720.0,
            Viewport {
                camera: Point {
                    x: -82_000.0,
                    y: 47_000.0,
                },
                zoom: 2.8,
            },
        );
        let world = Point {
            x: 9_000_000.25,
            y: -7_000_000.5,
        };
        let round_trip = transform.screen_to_world(transform.world_to_screen(world));
        assert!((round_trip.x - world.x).abs() < 1e-8);
        assert!((round_trip.y - world.y).abs() < 1e-8);
    }

    #[test]
    fn transform_matches_flutter_reference_fit() {
        let transform = ViewportTransform::new(1_280.0, 720.0, Viewport::default());
        assert!((transform.scale() - 0.8).abs() < f64::EPSILON);
        assert_eq!(transform.origin(), Point { x: 80.0, y: 0.0 });
        assert_eq!(
            transform.world_to_screen(Point { x: 700.0, y: 450.0 }),
            Point { x: 640.0, y: 360.0 }
        );
    }
}
