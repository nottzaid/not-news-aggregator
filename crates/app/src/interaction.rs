use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};

use not_news_domain::{EventId, GraphSnapshot, MoveNode, Point};
use not_news_renderer::{
    Motion, Viewport, ViewportTransform, expanded_positions, layout_artifacts, resolved_positions,
};

const ACTIVE_MOTION: Duration = Duration::from_millis(220);
const COLLAPSE_GRACE: Duration = Duration::from_millis(180);
const FRAME_INTERVAL: Duration = Duration::from_nanos(16_666_667);
const BRIDGE_CYCLE_SECONDS: f32 = 7.0;
const EVENT_HIT_RADIUS: f64 = 54.0;
const DRAG_THRESHOLD: f64 = 6.0;
const PAN_THRESHOLD: f64 = 4.0;

#[derive(Clone, Debug)]
struct ActiveMotion {
    from_positions: HashMap<EventId, Point>,
    to_positions: HashMap<EventId, Point>,
    from_active: Option<EventId>,
    to_active: Option<EventId>,
    started: Instant,
}

#[derive(Clone, Copy, Debug)]
struct CameraMotion {
    from: Point,
    to: Point,
    started: Instant,
}

#[derive(Clone, Debug)]
enum PointerGesture {
    Drag {
        event: EventId,
        screen_start: Point,
        origin: Point,
        position: Point,
    },
    Pan {
        screen_start: Point,
        camera_start: Point,
        moved: bool,
    },
}

#[derive(Clone, Debug)]
pub struct InteractionFrame {
    pub positions: HashMap<EventId, Point>,
    pub expanded_event: Option<EventId>,
    pub expansion_progress: f32,
    pub collapsing_event: Option<EventId>,
    pub collapse_progress: f32,
    pub bridge_event: Option<EventId>,
    pub bridge_flow: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum InteractionEffect {
    Unchanged,
    PixelsChanged,
    Move(MoveNode),
    OpenUrl(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CanvasSubject {
    pub event: EventId,
    pub artifact_index: Option<usize>,
}

#[derive(Clone, Debug)]
pub struct CanvasInteraction {
    base_positions: HashMap<EventId, Point>,
    settled_positions: HashMap<EventId, Point>,
    viewport: Viewport,
    width: f64,
    height: f64,
    cursor: Option<Point>,
    active: Option<EventId>,
    motion: Option<ActiveMotion>,
    camera_motion: Option<CameraMotion>,
    collapse_at: Option<Instant>,
    bridge_epoch: Option<Instant>,
    pointer: Option<PointerGesture>,
}

impl CanvasInteraction {
    pub fn new(base_positions: HashMap<EventId, Point>) -> Self {
        let settled_positions = base_positions.clone();
        Self {
            base_positions,
            settled_positions,
            viewport: Viewport::default(),
            width: 1_280.0,
            height: 800.0,
            cursor: None,
            active: None,
            motion: None,
            camera_motion: None,
            collapse_at: None,
            bridge_epoch: None,
            pointer: None,
        }
    }

    pub fn viewport(&self) -> Viewport {
        self.viewport
    }

    pub fn active_event_position(&self) -> Option<(&EventId, Point)> {
        let event = self.active.as_ref()?;
        self.base_positions
            .get(event)
            .map(|position| (event, *position))
    }

    pub fn subject_at_cursor(&self, graph: &GraphSnapshot, now: Instant) -> Option<CanvasSubject> {
        let screen = self.cursor?;
        let positions = self.current_positions(graph, now);
        let world = self.transform().screen_to_world(screen);
        if let Some(active_id) = self.active.as_ref()
            && let Some(event) = graph.events.get(active_id)
            && event.artifacts.len() > 1
            && let Some(artifact_index) = hit_artifact_index(
                world,
                positions[active_id],
                event,
                f64::from(self.active_paint_ease(now)),
            )
        {
            return Some(CanvasSubject {
                event: active_id.clone(),
                artifact_index: Some(artifact_index),
            });
        }
        let event = hit_event(world, graph, &positions)?;
        let artifact_index = (graph.events[&event].artifacts.len() == 1).then_some(0);
        Some(CanvasSubject {
            event,
            artifact_index,
        })
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width > 0 && height > 0 {
            self.width = f64::from(width);
            self.height = f64::from(height);
        }
    }

    pub fn cursor_moved(&mut self, screen: Point, graph: &GraphSnapshot, now: Instant) -> bool {
        self.cursor = Some(screen);
        let scale = self.transform().scale();
        if let Some(pointer) = &mut self.pointer {
            match pointer {
                PointerGesture::Drag {
                    screen_start,
                    origin,
                    position,
                    ..
                } => {
                    let delta = subtract(screen, *screen_start);
                    if *position == *origin && length(delta) <= DRAG_THRESHOLD {
                        return false;
                    }
                    let next = add(*origin, scale_point(delta, 1.0 / scale));
                    if *position == next {
                        return false;
                    }
                    *position = next;
                    self.collapse_at = None;
                    return true;
                }
                PointerGesture::Pan {
                    screen_start,
                    camera_start,
                    moved,
                } => {
                    let delta = subtract(screen, *screen_start);
                    *moved |= length(delta) > PAN_THRESHOLD;
                    self.camera_motion = None;
                    let next = subtract(*camera_start, scale_point(delta, 1.0 / scale));
                    if self.viewport.camera == next {
                        return false;
                    }
                    self.viewport.camera = next;
                    return true;
                }
            }
        }
        self.hover(screen, graph, now)
    }

    pub fn cursor_left(&mut self, now: Instant) -> bool {
        self.cursor = None;
        if self.pointer.is_none() && self.active.is_some() && self.collapse_at.is_none() {
            self.collapse_at = Some(now + COLLAPSE_GRACE);
            return true;
        }
        false
    }

    pub fn pointer_down(&mut self, graph: &GraphSnapshot, now: Instant) -> bool {
        let Some(screen) = self.cursor else {
            return false;
        };
        self.collapse_at = None;
        let positions = self.current_positions(graph, now);
        let world = self.transform().screen_to_world(screen);
        self.pointer = if let Some(event) = hit_event(world, graph, &positions) {
            let origin = positions[&event];
            Some(PointerGesture::Drag {
                event,
                screen_start: screen,
                origin,
                position: origin,
            })
        } else {
            Some(PointerGesture::Pan {
                screen_start: screen,
                camera_start: self.viewport.camera,
                moved: false,
            })
        };
        false
    }

    pub fn pointer_up(&mut self, graph: &GraphSnapshot, now: Instant) -> InteractionEffect {
        let Some(pointer) = self.pointer.take() else {
            return InteractionEffect::Unchanged;
        };
        match pointer {
            PointerGesture::Drag {
                event,
                origin,
                position,
                ..
            } if position != origin => {
                let version = graph
                    .placement_versions
                    .get(&event)
                    .copied()
                    .unwrap_or_default();
                InteractionEffect::Move(MoveNode {
                    event_id: event.clone(),
                    destination: position,
                    expected_placement_version: version,
                })
            }
            PointerGesture::Drag { .. } | PointerGesture::Pan { moved: false, .. } => {
                self.tap(graph, now)
            }
            PointerGesture::Pan { moved: true, .. } => InteractionEffect::Unchanged,
        }
    }

    pub fn placement_committed(&mut self, graph: &GraphSnapshot) {
        self.base_positions = resolved_positions(graph);
        self.settled_positions =
            expanded_positions(graph, &self.base_positions, self.active.as_ref());
        self.motion = None;
    }

    pub fn graph_committed(
        &mut self,
        previous: &GraphSnapshot,
        next: &GraphSnapshot,
        now: Instant,
    ) {
        let mut from_positions = self.current_positions(previous, now);
        let next_base = resolved_positions(next);
        let previous_active = self.active.clone();
        if self
            .active
            .as_ref()
            .is_some_and(|active| !next.events.contains_key(active))
        {
            self.active = None;
        }
        let to_positions = expanded_positions(next, &next_base, self.active.as_ref());
        for (event, point) in &to_positions {
            from_positions.entry(event.clone()).or_insert(*point);
        }
        let changed = next.events.keys().any(|event| {
            from_positions
                .get(event)
                .zip(to_positions.get(event))
                .is_some_and(|(from, to)| distance(*from, *to) > f64::EPSILON)
        });
        self.base_positions = next_base;
        self.settled_positions.clone_from(&to_positions);
        self.motion = changed.then(|| ActiveMotion {
            from_positions,
            to_positions,
            from_active: previous_active,
            to_active: self.active.clone(),
            started: now,
        });
    }

    pub fn focus_events(&mut self, events: &HashSet<EventId>, now: Instant) -> bool {
        let points = events
            .iter()
            .filter_map(|event| self.base_positions.get(event))
            .copied()
            .collect::<Vec<_>>();
        if points.is_empty() {
            return false;
        }
        let center = points
            .iter()
            .fold(Point { x: 0.0, y: 0.0 }, |sum, point| add(sum, *point));
        let count = f64::from(
            u32::try_from(points.len()).expect("a rendered graph cannot contain 2^32 events"),
        );
        let transform = self.transform();
        let target = Point {
            x: center.x / count - self.width / transform.scale() / 2.0,
            y: center.y / count - self.height / transform.scale() / 2.0,
        };
        if distance(self.viewport.camera, target) < 2.0 {
            return false;
        }
        self.camera_motion = Some(CameraMotion {
            from: self.viewport.camera,
            to: target,
            started: now,
        });
        true
    }

    pub fn manual_pan_active(&self) -> bool {
        matches!(self.pointer, Some(PointerGesture::Pan { moved: true, .. }))
    }

    pub fn cancel_pointer(&mut self) -> bool {
        self.pointer.take().is_some()
    }

    pub fn scroll(&mut self, vertical_pixels: f64) -> bool {
        let Some(anchor) = self.cursor else {
            return false;
        };
        if vertical_pixels == 0.0 || !vertical_pixels.is_finite() {
            return false;
        }
        let factor = (vertical_pixels * 0.0016).exp();
        self.set_zoom(self.viewport.zoom * factor, anchor)
    }

    pub fn zoom_by(&mut self, factor: f64) -> bool {
        if !factor.is_finite() || factor <= 0.0 {
            return false;
        }
        self.set_zoom(
            self.viewport.zoom * factor,
            Point {
                x: self.width / 2.0,
                y: self.height / 2.0,
            },
        )
    }

    pub fn reset_zoom(&mut self) -> bool {
        self.set_zoom(
            1.0,
            Point {
                x: self.width / 2.0,
                y: self.height / 2.0,
            },
        )
    }

    pub fn frame(&mut self, graph: &GraphSnapshot, now: Instant) -> InteractionFrame {
        self.update_camera_motion(now);
        if self.collapse_at.is_some_and(|deadline| now >= deadline) {
            self.collapse_at = None;
            self.set_active(None, graph, now);
        }

        let mut positions = self.current_positions(graph, now);
        if let Some(PointerGesture::Drag {
            event, position, ..
        }) = &self.pointer
        {
            positions.insert(event.clone(), *position);
        }
        let (expanded_event, expansion_progress, collapsing_event, collapse_progress) =
            self.expansion_state(now);
        let bridge_event = expanded_event.clone().or_else(|| collapsing_event.clone());
        let bridge_flow = self.bridge_epoch.map_or(0.0, |epoch| {
            (now.saturating_duration_since(epoch).as_secs_f32() / BRIDGE_CYCLE_SECONDS) % 1.0
        });

        if self.motion_complete(now) {
            self.motion = None;
            if self.active.is_none() {
                self.bridge_epoch = None;
            }
        }

        InteractionFrame {
            positions,
            expanded_event,
            expansion_progress,
            collapsing_event,
            collapse_progress,
            bridge_event,
            bridge_flow,
        }
    }

    pub fn next_deadline(&self, now: Instant) -> Option<Instant> {
        let mut deadline = self.collapse_at;
        if let Some(motion) = &self.motion {
            let end = motion.started + ACTIVE_MOTION;
            if now < end {
                deadline = Some(earlier(deadline, (now + FRAME_INTERVAL).min(end)));
            }
        }
        if let Some(motion) = self.camera_motion {
            let end = motion.started + Motion::CAMERA;
            if now < end {
                deadline = Some(earlier(deadline, (now + FRAME_INTERVAL).min(end)));
            }
        }
        if self.bridge_epoch.is_some() {
            deadline = Some(earlier(deadline, now + FRAME_INTERVAL));
        }
        deadline
    }

    fn hover(&mut self, screen: Point, graph: &GraphSnapshot, now: Instant) -> bool {
        let positions = self.current_positions(graph, now);
        let world = self.transform().screen_to_world(screen);
        if let Some(active_id) = self.active.as_ref()
            && let Some(event) = graph.events.get(active_id)
            && event.artifacts.len() > 1
            && protected_active_path(
                world,
                positions[active_id],
                event,
                f64::from(self.active_paint_ease(now)),
            )
        {
            let changed = self.collapse_at.take().is_some();
            return changed;
        }

        if let Some(event) = hit_event(world, graph, &positions) {
            self.collapse_at = None;
            self.set_active(Some(&event), graph, now)
        } else if self.active.is_some() && self.collapse_at.is_none() {
            self.collapse_at = Some(now + COLLAPSE_GRACE);
            true
        } else {
            false
        }
    }

    fn tap(&mut self, graph: &GraphSnapshot, now: Instant) -> InteractionEffect {
        let Some(screen) = self.cursor else {
            return InteractionEffect::Unchanged;
        };
        let positions = self.current_positions(graph, now);
        let world = self.transform().screen_to_world(screen);
        if let Some(active_id) = self.active.as_ref()
            && let Some(event) = graph.events.get(active_id)
            && event.artifacts.len() > 1
            && let Some(url) = hit_artifact_url(
                world,
                positions[active_id],
                event,
                f64::from(self.active_paint_ease(now)),
            )
        {
            return InteractionEffect::OpenUrl(url);
        }
        match hit_event(world, graph, &positions) {
            Some(event_id) => {
                let event = &graph.events[&event_id];
                if event.artifacts.len() <= 1
                    && let Some(url) = event
                        .url
                        .clone()
                        .or_else(|| event.artifacts.first().map(|artifact| artifact.url.clone()))
                {
                    return InteractionEffect::OpenUrl(url);
                }
                let changed = if self.active.as_ref() == Some(&event_id) {
                    self.set_active(None, graph, now)
                } else {
                    self.set_active(Some(&event_id), graph, now)
                };
                if changed {
                    InteractionEffect::PixelsChanged
                } else {
                    InteractionEffect::Unchanged
                }
            }
            None => {
                if self.set_active(None, graph, now) {
                    InteractionEffect::PixelsChanged
                } else {
                    InteractionEffect::Unchanged
                }
            }
        }
    }

    fn set_active(&mut self, next: Option<&EventId>, graph: &GraphSnapshot, now: Instant) -> bool {
        if self.active.as_ref() == next {
            return false;
        }
        let from_positions = self.current_positions(graph, now);
        let to_positions = expanded_positions(graph, &self.base_positions, next);
        let previous = self.active.clone();
        self.active = next.cloned();
        self.settled_positions.clone_from(&to_positions);
        self.motion = Some(ActiveMotion {
            from_positions,
            to_positions,
            from_active: previous.clone(),
            to_active: next.cloned(),
            started: now,
        });
        if next.is_some() || previous.is_some() {
            self.bridge_epoch.get_or_insert(now);
        }
        true
    }

    fn current_positions(&self, graph: &GraphSnapshot, now: Instant) -> HashMap<EventId, Point> {
        let Some(motion) = &self.motion else {
            return self.settled_positions.clone();
        };
        if self.motion_complete(now) {
            return motion.to_positions.clone();
        }
        let progress = f64::from(motion_progress(motion.started, now));
        graph
            .events
            .keys()
            .map(|event| {
                let from = motion.from_positions[event];
                let to = motion.to_positions[event];
                (event.clone(), lerp_point(from, to, progress))
            })
            .collect()
    }

    fn expansion_state(&self, now: Instant) -> (Option<EventId>, f32, Option<EventId>, f32) {
        let Some(motion) = &self.motion else {
            return (
                self.active.clone(),
                if self.active.is_some() { 1.0 } else { 0.0 },
                None,
                0.0,
            );
        };
        if self.motion_complete(now) {
            return (
                motion.to_active.clone(),
                if motion.to_active.is_some() { 1.0 } else { 0.0 },
                None,
                0.0,
            );
        }
        if motion.from_active == motion.to_active {
            return (
                motion.to_active.clone(),
                if motion.to_active.is_some() { 1.0 } else { 0.0 },
                None,
                0.0,
            );
        }
        let progress = motion_progress(motion.started, now);
        (
            motion.to_active.clone(),
            progress,
            motion.from_active.clone(),
            1.0 - progress,
        )
    }

    fn active_paint_ease(&self, now: Instant) -> f32 {
        let (_, progress, _, _) = self.expansion_state(now);
        scalar(Motion::ease_out_cubic(f64::from(progress)))
    }

    fn motion_complete(&self, now: Instant) -> bool {
        self.motion
            .as_ref()
            .is_some_and(|motion| now.saturating_duration_since(motion.started) >= ACTIVE_MOTION)
    }

    fn set_zoom(&mut self, zoom: f64, anchor: Point) -> bool {
        let next_zoom = zoom.clamp(0.35, 2.8);
        if (next_zoom - self.viewport.zoom).abs() < 0.002 {
            return false;
        }
        self.camera_motion = None;
        let world_anchor = self.transform().screen_to_world(anchor);
        let next = Viewport {
            camera: self.viewport.camera,
            zoom: next_zoom,
        };
        let next_transform = ViewportTransform::new(self.width, self.height, next);
        let origin = next_transform.origin();
        let camera = Point {
            x: world_anchor.x - (anchor.x - origin.x) / next_transform.scale(),
            y: world_anchor.y - (anchor.y - origin.y) / next_transform.scale(),
        };
        self.viewport = Viewport {
            camera,
            zoom: next_zoom,
        };
        true
    }

    fn transform(&self) -> ViewportTransform {
        ViewportTransform::new(self.width, self.height, self.viewport)
    }

    fn update_camera_motion(&mut self, now: Instant) {
        let Some(motion) = self.camera_motion else {
            return;
        };
        let linear = (now.saturating_duration_since(motion.started).as_secs_f64()
            / Motion::CAMERA.as_secs_f64())
        .clamp(0.0, 1.0);
        self.viewport.camera =
            lerp_point(motion.from, motion.to, Motion::ease_in_out_cubic(linear));
        if linear >= 1.0 {
            self.camera_motion = None;
        }
    }
}

fn motion_progress(started: Instant, now: Instant) -> f32 {
    let linear = (now.saturating_duration_since(started).as_secs_f64()
        / ACTIVE_MOTION.as_secs_f64())
    .clamp(0.0, 1.0);
    scalar(Motion::ease_out_cubic(linear))
}

fn hit_event(
    world: Point,
    graph: &GraphSnapshot,
    positions: &HashMap<EventId, Point>,
) -> Option<EventId> {
    graph.events.keys().find_map(|event| {
        (distance(world, positions[event]) <= EVENT_HIT_RADIUS).then(|| event.clone())
    })
}

fn protected_active_path(
    world: Point,
    center: Point,
    event: &not_news_domain::ResearchEvent,
    ease: f64,
) -> bool {
    if distance(world, center) <= 46.0 {
        return true;
    }
    layout_artifacts(event).artifacts.iter().any(|artifact| {
        let end = add(center, scale_point(artifact.offset, ease));
        let radius = artifact.radius * lerp(0.2, 1.0, ease) + 4.0;
        distance(world, end) <= radius || distance_to_segment(world, center, end) <= 14.0
    })
}

fn hit_artifact_url(
    world: Point,
    center: Point,
    event: &not_news_domain::ResearchEvent,
    ease: f64,
) -> Option<String> {
    hit_artifact_index(world, center, event, ease)
        .map(|artifact_index| event.artifacts[artifact_index].url.clone())
}

fn hit_artifact_index(
    world: Point,
    center: Point,
    event: &not_news_domain::ResearchEvent,
    ease: f64,
) -> Option<usize> {
    layout_artifacts(event)
        .artifacts
        .iter()
        .find_map(|artifact| {
            let position = add(center, scale_point(artifact.offset, ease));
            let radius = artifact.radius * lerp(0.2, 1.0, ease);
            (distance(world, position) <= radius).then_some(artifact.artifact_index)
        })
}

fn distance_to_segment(point: Point, start: Point, end: Point) -> f64 {
    let line = subtract(end, start);
    let length_squared = line.x.mul_add(line.x, line.y * line.y);
    if length_squared == 0.0 {
        return distance(point, start);
    }
    let relative = subtract(point, start);
    let projection =
        (relative.x.mul_add(line.x, relative.y * line.y) / length_squared).clamp(0.0, 1.0);
    distance(point, add(start, scale_point(line, projection)))
}

fn earlier(current: Option<Instant>, candidate: Instant) -> Instant {
    current.map_or(candidate, |deadline| deadline.min(candidate))
}

#[allow(clippy::cast_possible_truncation)]
fn scalar(value: f64) -> f32 {
    value as f32
}

fn add(a: Point, b: Point) -> Point {
    Point {
        x: a.x + b.x,
        y: a.y + b.y,
    }
}
fn subtract(a: Point, b: Point) -> Point {
    Point {
        x: a.x - b.x,
        y: a.y - b.y,
    }
}
fn scale_point(point: Point, scale: f64) -> Point {
    Point {
        x: point.x * scale,
        y: point.y * scale,
    }
}
fn length(point: Point) -> f64 {
    point.x.hypot(point.y)
}
fn distance(a: Point, b: Point) -> f64 {
    length(subtract(a, b))
}
fn lerp(from: f64, to: f64, progress: f64) -> f64 {
    from + (to - from) * progress
}
fn lerp_point(from: Point, to: Point, progress: f64) -> Point {
    Point {
        x: lerp(from.x, to.x, progress),
        y: lerp(from.y, to.y, progress),
    }
}

#[cfg(test)]
mod tests {
    use indexmap::IndexMap;
    use not_news_domain::{BridgeId, EventBridge, Provenance, ResearchEvent, SourceArtifact};

    use super::*;

    fn graph() -> GraphSnapshot {
        let a = EventId("a".into());
        let b = EventId("b".into());
        let artifact = |name: &str| SourceArtifact {
            text: name.into(),
            source: "source".into(),
            url: format!("https://{name}.test"),
        };
        GraphSnapshot {
            events: IndexMap::from([
                (
                    a.clone(),
                    ResearchEvent {
                        id: a.clone(),
                        title: "A".into(),
                        date: "today".into(),
                        color: 0xff00_0000,
                        summary: "a".into(),
                        source_label: "s".into(),
                        artifacts: vec![artifact("one"), artifact("two")],
                        url: None,
                    },
                ),
                (
                    b.clone(),
                    ResearchEvent {
                        id: b.clone(),
                        title: "B".into(),
                        date: "today".into(),
                        color: 0xff00_0000,
                        summary: "b".into(),
                        source_label: "s".into(),
                        artifacts: vec![],
                        url: Some("https://b.test".into()),
                    },
                ),
            ]),
            bridges: IndexMap::from([(
                BridgeId("ab".into()),
                EventBridge {
                    id: BridgeId("ab".into()),
                    from: a,
                    to: b,
                    label: "related".into(),
                    provenance: Provenance::Legacy,
                },
            )]),
            ..GraphSnapshot::default()
        }
    }

    fn interaction() -> CanvasInteraction {
        CanvasInteraction::new(HashMap::from([
            (EventId("a".into()), Point { x: 400.0, y: 400.0 }),
            (EventId("b".into()), Point { x: 900.0, y: 400.0 }),
        ]))
    }

    #[test]
    fn wheel_zoom_keeps_the_world_point_under_the_pointer() {
        let mut interaction = interaction();
        let now = Instant::now();
        let cursor = Point { x: 317.0, y: 229.0 };
        interaction.cursor_moved(cursor, &graph(), now);
        let before = interaction.transform().screen_to_world(cursor);
        assert!(interaction.scroll(-120.0));
        let after = interaction.transform().screen_to_world(cursor);
        assert!(distance(before, after) < 1.0e-9);
    }

    #[test]
    fn conventional_wheel_direction_zooms_toward_then_away_from_the_canvas() {
        let mut interaction = interaction();
        let now = Instant::now();
        interaction.cursor_moved(Point { x: 317.0, y: 229.0 }, &graph(), now);

        let initial = interaction.viewport().zoom;
        assert!(interaction.scroll(40.0));
        let forward = interaction.viewport().zoom;
        assert!(forward > initial, "wheel-forward/up must zoom in");

        assert!(interaction.scroll(-40.0));
        assert!(
            (interaction.viewport().zoom - initial).abs() < 1.0e-12,
            "equal wheel-back/down input must restore the prior zoom"
        );
    }

    #[test]
    fn hover_expands_then_gracefully_collapses_on_a_deadline() {
        let graph = graph();
        let mut interaction = interaction();
        let now = Instant::now();
        let screen = interaction
            .transform()
            .world_to_screen(Point { x: 400.0, y: 400.0 });
        assert!(interaction.cursor_moved(screen, &graph, now));
        let open = interaction.frame(&graph, now + ACTIVE_MOTION);
        assert_eq!(open.expanded_event, Some(EventId("a".into())));
        assert!((open.expansion_progress - 1.0).abs() < f32::EPSILON);

        interaction.cursor_moved(Point { x: 10.0, y: 10.0 }, &graph, now + ACTIVE_MOTION);
        let collapse_deadline = now + ACTIVE_MOTION + COLLAPSE_GRACE;
        let before = interaction.frame(
            &graph,
            collapse_deadline
                .checked_sub(Duration::from_millis(1))
                .unwrap(),
        );
        assert_eq!(before.expanded_event, Some(EventId("a".into())));
        let collapsing = interaction.frame(&graph, now + ACTIVE_MOTION + COLLAPSE_GRACE);
        assert_eq!(collapsing.collapsing_event, Some(EventId("a".into())));
        assert!((collapsing.collapse_progress - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn drag_threshold_commits_only_placement_and_never_a_relationship() {
        let mut graph = graph();
        let before_bridges = graph.bridges.clone();
        let mut interaction = interaction();
        let now = Instant::now();
        let start = interaction
            .transform()
            .world_to_screen(Point { x: 400.0, y: 400.0 });
        interaction.cursor_moved(start, &graph, now);
        interaction.pointer_down(&graph, now);
        interaction.cursor_moved(add(start, Point { x: 5.0, y: 0.0 }), &graph, now);
        interaction.pointer_up(&graph, now);
        assert!(graph.placements.is_empty());

        interaction.cursor_moved(start, &graph, now);
        interaction.pointer_down(&graph, now);
        interaction.cursor_moved(add(start, Point { x: 70.0, y: 10.0 }), &graph, now);
        let InteractionEffect::Move(command) = interaction.pointer_up(&graph, now) else {
            panic!("drag must produce a placement command");
        };
        graph.apply_move(&command).unwrap();
        interaction.placement_committed(&graph);
        assert_eq!(graph.bridges, before_bridges);
        assert!(graph.placements[&EventId("a".into())].pinned);
    }

    #[test]
    fn panning_is_not_clamped_to_the_reference_frame() {
        let graph = graph();
        let mut interaction = interaction();
        let now = Instant::now();
        interaction.cursor_moved(Point { x: 10.0, y: 10.0 }, &graph, now);
        interaction.pointer_down(&graph, now);
        interaction.cursor_moved(
            Point {
                x: 1_000_010.0,
                y: -2_000_000.0,
            },
            &graph,
            now,
        );
        assert!(interaction.viewport.camera.x.abs() > 100_000.0);
        assert!(interaction.viewport.camera.y.abs() > 100_000.0);
    }

    #[test]
    fn click_resolves_direct_and_expanded_artifact_urls_without_shell_semantics() {
        let graph = graph();
        let mut interaction = interaction();
        let now = Instant::now();
        let direct = interaction
            .transform()
            .world_to_screen(Point { x: 900.0, y: 400.0 });
        interaction.cursor_moved(direct, &graph, now);
        interaction.pointer_down(&graph, now);
        assert_eq!(
            interaction.pointer_up(&graph, now),
            InteractionEffect::OpenUrl("https://b.test".into())
        );

        let center = Point { x: 400.0, y: 400.0 };
        let event = &graph.events[&EventId("a".into())];
        let node = interaction.transform().world_to_screen(center);
        interaction.cursor_moved(node, &graph, now);
        interaction.cursor_moved(node, &graph, now + ACTIVE_MOTION);
        let artifact = &layout_artifacts(event).artifacts[0];
        let artifact_screen = interaction
            .transform()
            .world_to_screen(add(center, artifact.offset));
        interaction.cursor_moved(artifact_screen, &graph, now + ACTIVE_MOTION);
        assert_eq!(
            interaction.subject_at_cursor(&graph, now + ACTIVE_MOTION),
            Some(CanvasSubject {
                event: EventId("a".into()),
                artifact_index: Some(0),
            })
        );
        interaction.pointer_down(&graph, now + ACTIVE_MOTION);
        assert_eq!(
            interaction.pointer_up(&graph, now + ACTIVE_MOTION),
            InteractionEffect::OpenUrl("https://one.test".into())
        );
    }

    #[test]
    fn chrome_zoom_uses_view_center_anchor_and_reset_preserves_its_world_point() {
        let mut interaction = interaction();
        interaction.resize(1_280, 800);
        interaction.viewport.camera = Point { x: 250.0, y: -90.0 };
        let center = Point { x: 640.0, y: 400.0 };
        let before = interaction.transform().screen_to_world(center);
        assert!(interaction.zoom_by(1.18));
        assert_eq!(interaction.transform().screen_to_world(center), before);
        assert!(interaction.reset_zoom());
        assert!((interaction.viewport.zoom - 1.0).abs() < f64::EPSILON);
        assert_eq!(interaction.transform().screen_to_world(center), before);
    }

    #[test]
    fn generated_cluster_focus_matches_flutter_target_and_camera_curve() {
        let positions = HashMap::from([
            (
                EventId("a".into()),
                Point {
                    x: 1_600.0,
                    y: 900.0,
                },
            ),
            (
                EventId("b".into()),
                Point {
                    x: 1_800.0,
                    y: 700.0,
                },
            ),
        ]);
        let mut interaction = CanvasInteraction::new(positions);
        interaction.resize(1_400, 900);
        let now = Instant::now();
        assert!(interaction.focus_events(
            &HashSet::from([EventId("a".into()), EventId("b".into())]),
            now
        ));
        interaction.frame(&GraphSnapshot::default(), now + Motion::CAMERA / 2);
        let midpoint = interaction.viewport().camera;
        assert!((midpoint.x - 516.875).abs() < 0.001);
        assert!((midpoint.y - 180.906_25).abs() < 0.001);

        interaction.frame(&GraphSnapshot::default(), now + Motion::CAMERA);
        assert_eq!(
            interaction.viewport().camera,
            Point {
                x: 1_000.0,
                y: 350.0
            }
        );
    }
}
