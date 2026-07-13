use not_news_domain::{Point, ResearchEvent};

#[derive(Clone, Debug, PartialEq)]
pub struct ArtifactLayout {
    pub artifact_index: usize,
    pub lines: Vec<String>,
    pub offset: Point,
    pub radius: f64,
}

impl ArtifactLayout {
    pub fn collision_radius(&self) -> f64 {
        self.radius + 26.0
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ArtifactMetrics {
    pub artifacts: Vec<ArtifactLayout>,
    pub radius: f64,
}

pub fn layout_artifacts(event: &ResearchEvent) -> ArtifactMetrics {
    if event.artifacts.len() <= 1 {
        return ArtifactMetrics {
            artifacts: Vec::new(),
            radius: 42.0,
        };
    }

    let mut random = SeededRandom::new(hash_string(&event.id.0));
    let artifacts: Vec<_> = event
        .artifacts
        .iter()
        .enumerate()
        .map(|(artifact_index, artifact)| {
            let lines = optimal_artifact_lines(&artifact.text);
            let radius = text_radius(&lines, 11.0, 16.0);
            MutableArtifactLayout {
                artifact_index,
                lines,
                radius,
                x: 0.0,
                y: 0.0,
            }
        })
        .collect();
    let largest = artifacts
        .iter()
        .map(|artifact| artifact.radius)
        .fold(0.0, f64::max);
    let ring = 150.0 + largest * 0.38;
    let rotation = -std::f64::consts::FRAC_PI_2 + (random.next() - 0.5) * 0.55;
    let artifact_count = usize_to_f64(artifacts.len());
    let mut mutable: Vec<_> = artifacts
        .into_iter()
        .enumerate()
        .map(|(index, mut artifact)| {
            let angle = rotation
                + (usize_to_f64(index) * std::f64::consts::TAU) / artifact_count
                + (random.next() - 0.5) * 0.18;
            let distance = ring + (random.next() - 0.5) * 18.0;
            artifact.x = angle.cos() * distance;
            artifact.y = angle.sin() * distance;
            artifact
        })
        .collect();

    for _ in 0..80 {
        for artifact in &mut mutable {
            let center_distance = artifact.x.hypot(artifact.y);
            let safe_distance = if center_distance == 0.0 {
                1.0
            } else {
                center_distance
            };
            let minimum = 82.0 + artifact.radius;
            if safe_distance < minimum {
                let push = (minimum - safe_distance) * 0.5;
                artifact.x += (artifact.x / safe_distance) * push;
                artifact.y += (artifact.y / safe_distance) * push;
            }
        }

        for a_index in 0..mutable.len() {
            for b_index in (a_index + 1)..mutable.len() {
                let (before_b, from_b) = mutable.split_at_mut(b_index);
                let a = &mut before_b[a_index];
                let b = &mut from_b[0];
                let dx = b.x - a.x;
                let dy = b.y - a.y;
                let distance = dx.hypot(dy);
                let safe_distance = if distance == 0.0 { 1.0 } else { distance };
                let minimum = a.collision_radius() + b.collision_radius() + 26.0;
                if safe_distance < minimum {
                    let push = (minimum - safe_distance) * 0.5;
                    let nx = dx / safe_distance;
                    let ny = dy / safe_distance;
                    a.x -= nx * push;
                    a.y -= ny * push;
                    b.x += nx * push;
                    b.y += ny * push;
                }
            }
        }
    }

    let mut radius = 132.0f64;
    let artifacts = mutable
        .into_iter()
        .map(|artifact| {
            let offset = Point {
                x: artifact.x,
                y: artifact.y,
            };
            radius = radius.max(offset.x.hypot(offset.y) + artifact.radius + 34.0);
            ArtifactLayout {
                artifact_index: artifact.artifact_index,
                lines: artifact.lines,
                offset,
                radius: artifact.radius,
            }
        })
        .collect();
    ArtifactMetrics { artifacts, radius }
}

fn optimal_artifact_lines(text: &str) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }
    let font_size = 11.0;
    let char_width = font_size * 0.54;
    let line_height = font_size * 1.12;
    let ideal = (usize_to_f64(utf16_len(text)) * line_height / char_width).sqrt();
    let lower = 6usize.max(rounded_usize((ideal - 6.0).max(0.0)));
    let upper = rounded_usize(ideal + 8.0);
    let mut best = wrap_lines(text, rounded_usize(ideal));
    let mut best_radius = block_circumradius(&best, char_width, line_height);
    for width in lower..=upper {
        let lines = wrap_lines(text, width);
        let radius = block_circumradius(&lines, char_width, line_height);
        if radius < best_radius {
            best_radius = radius;
            best = lines;
        }
    }
    best
}

fn wrap_lines(content: &str, max_chars: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in content.split(' ') {
        let trial = if line.is_empty() {
            word.into()
        } else {
            format!("{line} {word}")
        };
        if utf16_len(&trial) > max_chars && !line.is_empty() {
            lines.push(line);
            line = word.into();
        } else {
            line = trial;
        }
    }
    lines.push(line);
    lines
}

fn block_circumradius(lines: &[String], char_width: f64, line_height: f64) -> f64 {
    if lines.is_empty() {
        return 0.0;
    }
    let longest = lines.iter().map(|line| utf16_len(line)).max().unwrap();
    let width = usize_to_f64(longest) * char_width;
    let height = usize_to_f64(lines.len()) * line_height;
    width.hypot(height) * 0.5
}

fn text_radius(lines: &[String], font_size: f64, padding: f64) -> f64 {
    if lines.is_empty() {
        return padding;
    }
    let longest = lines.iter().map(|line| utf16_len(line)).max().unwrap();
    let estimated_width = usize_to_f64(longest) * font_size * 0.54;
    let estimated_height = usize_to_f64(lines.len()) * font_size * 1.12;
    estimated_width.hypot(estimated_height) * 0.5 + padding
}

fn utf16_len(value: &str) -> usize {
    value.encode_utf16().count()
}

fn hash_string(value: &str) -> u32 {
    value.encode_utf16().fold(2_166_136_261, |hash, unit| {
        (hash ^ u32::from(unit)).wrapping_mul(16_777_619)
    })
}

struct SeededRandom {
    state: u32,
}

impl SeededRandom {
    fn new(state: u32) -> Self {
        Self { state }
    }

    fn next(&mut self) -> f64 {
        self.state = 1_664_525u32
            .wrapping_mul(self.state)
            .wrapping_add(1_013_904_223);
        f64::from(self.state) / 4_294_967_296.0
    }
}

struct MutableArtifactLayout {
    artifact_index: usize,
    lines: Vec<String>,
    radius: f64,
    x: f64,
    y: f64,
}

impl MutableArtifactLayout {
    fn collision_radius(&self) -> f64 {
        self.radius + 26.0
    }
}

#[allow(clippy::cast_precision_loss)]
fn usize_to_f64(value: usize) -> f64 {
    value as f64
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]
fn rounded_usize(value: f64) -> usize {
    assert!(value.is_finite() && value >= 0.0 && value <= usize::MAX as f64);
    value.round() as usize
}

#[cfg(test)]
mod tests {
    use not_news_domain::{EventId, ResearchEvent, SourceArtifact};

    use super::*;

    #[test]
    fn layout_is_deterministic_and_separates_variable_text_footprints() {
        let event = ResearchEvent {
            id: EventId("artifact-oracle".into()),
            title: "Artifact oracle".into(),
            date: "Jul 14, 2026".into(),
            color: 0xffe8_a44c,
            summary: "Summary".into(),
            source_label: "Source".into(),
            artifacts: [
                ("Official model release", "Official"),
                (
                    "Independent report with a materially longer finding",
                    "Report",
                ),
                ("Concise synthesis", "Summary"),
            ]
            .into_iter()
            .enumerate()
            .map(|(index, (text, source))| SourceArtifact {
                text: text.into(),
                source: source.into(),
                url: format!("https://example.com/{index}"),
            })
            .collect(),
            url: None,
        };
        let first = layout_artifacts(&event);
        let second = layout_artifacts(&event);
        assert_eq!(first, second);
        assert_eq!(first.artifacts.len(), 3);
        for (index, a) in first.artifacts.iter().enumerate() {
            assert!(a.offset.x.hypot(a.offset.y) >= 82.0 + a.radius);
            for b in first.artifacts.iter().skip(index + 1) {
                let distance = (b.offset.x - a.offset.x).hypot(b.offset.y - a.offset.y);
                assert!(distance >= a.collision_radius() + b.collision_radius() + 26.0);
            }
        }
    }
}
