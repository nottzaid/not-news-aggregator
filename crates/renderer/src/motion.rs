use std::time::Duration;

pub struct Motion;

impl Motion {
    pub const LAYOUT: Duration = Duration::from_millis(220);
    pub const CAMERA: Duration = Duration::from_millis(560);
    pub const BRIDGE_FLOW: Duration = Duration::from_secs(7);
    pub const ARTIFACT_HOVER_IN: Duration = Duration::from_millis(170);
    pub const ARTIFACT_HOVER_OUT: Duration = Duration::from_millis(130);
    pub const RECONCILIATION_PULSE: Duration = Duration::from_millis(1_600);
    pub const COLLAPSE_DELAY: Duration = Duration::from_millis(180);
    pub const PANEL: Duration = Duration::from_millis(180);

    pub fn ease_out_cubic(t: f64) -> f64 {
        cubic_transform(t, 0.215, 0.61, 0.355, 1.0)
    }

    pub fn ease_in_out_cubic(t: f64) -> f64 {
        cubic_transform(t, 0.645, 0.045, 0.355, 1.0)
    }
}

/// Flutter's `Cubic.transformInternal`, including its 0.001 x-error bound.
///
/// # Panics
///
/// Panics unless `t` lies in the closed unit interval.
pub fn cubic_transform(t: f64, x1: f64, y1: f64, x2: f64, y2: f64) -> f64 {
    assert!((0.0..=1.0).contains(&t));
    if t <= 0.0 {
        return 0.0;
    }
    if t >= 1.0 {
        return 1.0;
    }
    let evaluate = |a: f64, b: f64, m: f64| {
        3.0 * a * (1.0 - m) * (1.0 - m) * m + 3.0 * b * (1.0 - m) * m * m + m * m * m
    };
    let (mut start, mut end) = (0.0, 1.0);
    loop {
        let midpoint = f64::midpoint(start, end);
        let estimate = evaluate(x1, x2, midpoint);
        if (t - estimate).abs() < 0.001 {
            return evaluate(y1, y2, midpoint);
        }
        if estimate < t {
            start = midpoint;
        } else {
            end = midpoint;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flutter_curve_port_preserves_endpoints_and_character() {
        assert!(Motion::ease_out_cubic(0.0).abs() < f64::EPSILON);
        assert!((Motion::ease_out_cubic(1.0) - 1.0).abs() < f64::EPSILON);
        assert!(Motion::ease_out_cubic(0.5) > 0.85);
        let cases = [
            (0.25, 0.087_394_673_824_310_3),
            (0.5, 0.516_875),
            (0.75, 0.931_120_035_648_346),
        ];
        for (t, expected) in cases {
            assert!((Motion::ease_in_out_cubic(t) - expected).abs() < f64::EPSILON);
        }
    }
}
