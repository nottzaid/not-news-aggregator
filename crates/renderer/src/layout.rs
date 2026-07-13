use std::collections::{HashMap, HashSet, VecDeque};

use not_news_domain::{EventId, GraphSnapshot, Point, ResearchEvent};

use crate::{REFERENCE_HEIGHT as REFERENCE_VIEW_HEIGHT, REFERENCE_WIDTH as REFERENCE_VIEW_WIDTH};
const EVENT_COLLISION_RADIUS: f64 = 92.0;

pub fn resolved_positions(graph: &GraphSnapshot) -> HashMap<EventId, Point> {
    let mut positions = generate_positions(graph);
    for (event, placement) in &graph.placements {
        positions.insert(event.clone(), placement.point);
    }
    positions
}

pub fn primary_component_ids(graph: &GraphSnapshot) -> Vec<EventId> {
    let mut primary = Vec::new();
    for component in connected_components(graph) {
        if component.len() > primary.len() {
            primary = component
                .into_iter()
                .map(|event| event.id.clone())
                .collect();
        }
    }
    primary
}

pub fn generate_positions(graph: &GraphSnapshot) -> HashMap<EventId, Point> {
    let components = connected_components(graph);
    let mut positions = HashMap::new();
    for (index, component) in components.iter().enumerate() {
        positions.extend(component_positions(component, cluster_center(index)));
    }
    relax_components(&components, positions)
}

fn connected_components(graph: &GraphSnapshot) -> Vec<Vec<&ResearchEvent>> {
    let by_id: HashMap<_, _> = graph.events.iter().collect();
    let mut adjacency: HashMap<EventId, Vec<EventId>> = graph
        .events
        .keys()
        .cloned()
        .map(|id| (id, Vec::new()))
        .collect();
    for bridge in graph.bridges.values() {
        if !by_id.contains_key(&bridge.from) || !by_id.contains_key(&bridge.to) {
            continue;
        }
        push_unique(&mut adjacency, &bridge.from, &bridge.to);
        push_unique(&mut adjacency, &bridge.to, &bridge.from);
    }

    let mut seen = HashSet::new();
    let mut components = Vec::new();
    for event in graph.events.values() {
        if !seen.insert(event.id.clone()) {
            continue;
        }
        let mut queue = VecDeque::from([event.id.clone()]);
        let mut component = Vec::new();
        while let Some(id) = queue.pop_front() {
            component.push(by_id[&id]);
            for neighbor in &adjacency[&id] {
                if seen.insert(neighbor.clone()) {
                    queue.push_back(neighbor.clone());
                }
            }
        }
        components.push(component);
    }
    components
}

fn push_unique(adjacency: &mut HashMap<EventId, Vec<EventId>>, from: &EventId, to: &EventId) {
    let neighbors = adjacency
        .get_mut(from)
        .expect("validated graph contains every bridge endpoint");
    if !neighbors.contains(to) {
        neighbors.push(to.clone());
    }
}

fn cluster_center(index: usize) -> Point {
    let origin = Point {
        x: REFERENCE_VIEW_WIDTH / 2.0,
        y: REFERENCE_VIEW_HEIGHT / 2.0 - 24.0,
    };
    let slots = [
        (0.0, 0.0),
        (-860.0, 0.0),
        (860.0, 0.0),
        (0.0, -640.0),
        (0.0, 640.0),
        (-860.0, -640.0),
        (860.0, -640.0),
        (-860.0, 640.0),
        (860.0, 640.0),
    ];
    if let Some((x, y)) = slots.get(index) {
        return add(origin, Point { x: *x, y: *y });
    }
    let ring = usize_to_f64((index - slots.len()) / 8 + 2);
    let angle = usize_to_f64((index - slots.len()) % 8) * std::f64::consts::FRAC_PI_4;
    add(
        origin,
        Point {
            x: angle.cos() * 860.0 * ring,
            y: angle.sin() * 640.0 * ring,
        },
    )
}

fn component_positions(events: &[&ResearchEvent], center: Point) -> HashMap<EventId, Point> {
    let footprint: HashMap<_, _> = events
        .iter()
        .map(|event| (event.id.clone(), event_footprint_radius(event)))
        .collect();
    let radius = 260.0f64.max(usize_to_f64(events.len()).sqrt() * 172.0);
    let mut placed: Vec<(Point, f64)> = Vec::new();
    let mut positions = HashMap::new();

    for (index, event) in events.iter().enumerate() {
        let event_radius = footprint[&event.id];
        let mut random =
            SeededRandom::new(hash_string(&format!("event:{}:{}", event.id.0, event.date)));
        let mut best = center;
        let mut best_score = f64::NEG_INFINITY;
        for _ in 0..320 {
            let distance_from_center = if index == 0 {
                random.next() * 34.0
            } else {
                random.next().powf(0.62) * radius
            };
            let angle = random.next() * std::f64::consts::TAU;
            let candidate = add(
                center,
                Point {
                    x: angle.cos() * distance_from_center,
                    y: angle.sin() * distance_from_center * 0.74,
                },
            );
            let min_distance = placed
                .iter()
                .map(|(point, _)| distance(*point, candidate))
                .reduce(f64::min)
                .unwrap_or(999.0);
            let record_distance = distance(
                candidate,
                Point {
                    x: REFERENCE_VIEW_WIDTH / 2.0,
                    y: REFERENCE_VIEW_HEIGHT - 74.0,
                },
            );
            let record_penalty = if record_distance < 170.0 { 90.0 } else { 0.0 };
            let clearance = placed
                .iter()
                .map(|(point, radius)| distance(*point, candidate) - radius - event_radius)
                .reduce(f64::min)
                .unwrap_or(999.0);
            let score =
                clearance * 1.8 + min_distance * 0.25 - record_penalty + random.next() * 18.0;
            if score > best_score {
                best_score = score;
                best = candidate;
            }
        }
        placed.push((best, event_radius));
        positions.insert(event.id.clone(), best);
    }
    relax_events(events, positions, &footprint, center)
}

fn relax_events(
    events: &[&ResearchEvent],
    mut positions: HashMap<EventId, Point>,
    radius: &HashMap<EventId, f64>,
    center: Point,
) -> HashMap<EventId, Point> {
    if events.len() < 2 {
        return positions;
    }
    let ids: Vec<_> = events.iter().map(|event| event.id.clone()).collect();
    for _ in 0..120 {
        let mut moved = false;
        for left in 0..ids.len() {
            for right in left + 1..ids.len() {
                let a = positions[&ids[left]];
                let b = positions[&ids[right]];
                let minimum = radius[&ids[left]] + radius[&ids[right]] + 34.0;
                if distance(a, b) < minimum {
                    let (a, b) = resolved_pair(a, b, minimum);
                    positions.insert(ids[left].clone(), a);
                    positions.insert(ids[right].clone(), b);
                    moved = true;
                }
            }
        }
        if !moved {
            break;
        }
    }
    let centroid = mean(ids.iter().map(|id| positions[id]));
    let recenter = subtract(center, centroid);
    positions
        .into_iter()
        .map(|(id, point)| (id, add(point, recenter)))
        .collect()
}

struct ComponentFootprint {
    ids: Vec<EventId>,
    center: Point,
    radius: f64,
}

fn relax_components(
    components: &[Vec<&ResearchEvent>],
    mut positions: HashMap<EventId, Point>,
) -> HashMap<EventId, Point> {
    if components.len() < 2 {
        return positions;
    }
    let footprints: Vec<_> = components
        .iter()
        .map(|events| component_footprint(events, &positions))
        .collect();
    let mut centers: Vec<_> = footprints.iter().map(|item| item.center).collect();
    for _ in 0..160 {
        let mut moved = false;
        for left in 0..footprints.len() {
            for right in left + 1..footprints.len() {
                let minimum = footprints[left].radius + footprints[right].radius + 190.0;
                if distance(centers[left], centers[right]) < minimum {
                    let (a, b) = resolved_pair(centers[left], centers[right], minimum);
                    centers[left] = a;
                    centers[right] = b;
                    moved = true;
                }
            }
        }
        if !moved {
            break;
        }
    }
    for (index, footprint) in footprints.iter().enumerate() {
        let delta = subtract(centers[index], footprint.center);
        for id in &footprint.ids {
            positions.insert(id.clone(), add(positions[id], delta));
        }
    }
    positions
}

fn component_footprint(
    events: &[&ResearchEvent],
    positions: &HashMap<EventId, Point>,
) -> ComponentFootprint {
    let ids: Vec<_> = events.iter().map(|event| event.id.clone()).collect();
    let center = mean(ids.iter().map(|id| positions[id]));
    let radius = events.iter().fold(0.0f64, |radius, event| {
        radius.max(distance(positions[&event.id], center) + event_footprint_radius(event))
    });
    ComponentFootprint {
        ids,
        center,
        radius,
    }
}

fn event_footprint_radius(event: &ResearchEvent) -> f64 {
    let lines = wrap_lines(&event.title, 18);
    let title_height = usize_to_f64(lines.len()) * 15.0;
    let width = lines
        .iter()
        .map(|line| usize_to_f64(utf16_len(line)))
        .reduce(f64::max)
        .unwrap_or_default()
        * 13.0
        * 0.55;
    let width = width.min(142.0);
    let height = 42.0 + title_height + 24.0;
    EVENT_COLLISION_RADIUS.max((width.mul_add(width * 0.25, height * height)).sqrt() + 12.0)
}

fn wrap_lines(content: &str, max_units: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in content.split(' ') {
        let trial = if line.is_empty() {
            word.to_owned()
        } else {
            format!("{line} {word}")
        };
        if utf16_len(&trial) > max_units && !line.is_empty() {
            lines.push(line);
            line = String::from(word);
        } else {
            line = trial;
        }
    }
    lines.push(line);
    lines
}

fn utf16_len(value: &str) -> usize {
    value.encode_utf16().count()
}

fn hash_string(value: &str) -> u32 {
    value.encode_utf16().fold(2_166_136_261, |hash, unit| {
        (hash ^ u32::from(unit)).wrapping_mul(16_777_619)
    })
}

struct SeededRandom(u32);

impl SeededRandom {
    fn new(seed: u32) -> Self {
        Self(seed)
    }

    fn next(&mut self) -> f64 {
        self.0 = 1_664_525u32
            .wrapping_mul(self.0)
            .wrapping_add(1_013_904_223);
        f64::from(self.0) / 4_294_967_296.0
    }
}

fn resolved_pair(a: Point, b: Point, minimum: f64) -> (Point, Point) {
    let delta = subtract(b, a);
    let current = distance(a, b);
    if current >= minimum {
        return (a, b);
    }
    let push = (minimum - current) * 0.5;
    let direction = if current == 0.0 {
        Point { x: 1.0, y: 0.0 }
    } else {
        scale(delta, 1.0 / current)
    };
    (
        add(a, scale(direction, -push)),
        add(b, scale(direction, push)),
    )
}

fn mean(points: impl IntoIterator<Item = Point>) -> Point {
    let (sum, count) = points
        .into_iter()
        .fold((Point { x: 0.0, y: 0.0 }, 0usize), |(sum, count), point| {
            (add(sum, point), count + 1)
        });
    scale(sum, 1.0 / usize_to_f64(count))
}

fn usize_to_f64(value: usize) -> f64 {
    f64::from(u32::try_from(value).expect("canvas collection exceeds supported size"))
}

fn distance(a: Point, b: Point) -> f64 {
    let delta = subtract(b, a);
    delta.x.hypot(delta.y)
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

fn scale(point: Point, factor: f64) -> Point {
    Point {
        x: point.x * factor,
        y: point.y * factor,
    }
}

#[cfg(test)]
mod tests {
    use indexmap::IndexMap;
    use not_news_domain::{BridgeId, EventBridge, Provenance};

    use super::*;

    fn event(id: &str, title: &str) -> ResearchEvent {
        ResearchEvent {
            id: EventId(id.to_owned()),
            title: title.to_owned(),
            date: "2026-07-14".to_owned(),
            color: 0xff44_6688,
            summary: "Summary".to_owned(),
            source_label: "Source".to_owned(),
            artifacts: Vec::new(),
            url: None,
        }
    }

    fn graph(events: Vec<ResearchEvent>, edges: &[(&str, &str)]) -> GraphSnapshot {
        GraphSnapshot {
            events: events
                .into_iter()
                .map(|event| (event.id.clone(), event))
                .collect(),
            bridges: edges
                .iter()
                .enumerate()
                .map(|(index, (from, to))| {
                    let id = BridgeId(format!("edge-{index}"));
                    (
                        id.clone(),
                        EventBridge {
                            id,
                            from: EventId((*from).to_owned()),
                            to: EventId((*to).to_owned()),
                            label: "same topic".to_owned(),
                            provenance: Provenance::Legacy,
                        },
                    )
                })
                .collect(),
            ..GraphSnapshot::default()
        }
    }

    #[test]
    fn generated_layout_is_deterministic_and_reserves_label_footprints() {
        let events: Vec<_> = (0..9)
            .map(|index| {
                event(
                    &format!("dense-{index}"),
                    &format!("European AI sovereignty component with long label {index}"),
                )
            })
            .collect();
        let edges: Vec<_> = (0..8)
            .map(|index| (format!("dense-{index}"), format!("dense-{}", index + 1)))
            .collect();
        let edge_refs: Vec<_> = edges
            .iter()
            .map(|(from, to)| (from.as_str(), to.as_str()))
            .collect();
        let graph = graph(events, &edge_refs);
        let first = generate_positions(&graph);
        assert_eq!(first, generate_positions(&graph));

        let events: Vec<_> = graph.events.values().collect();
        for left in 0..events.len() {
            for right in left + 1..events.len() {
                let minimum =
                    event_footprint_radius(events[left]) + event_footprint_radius(events[right]);
                assert!(
                    distance(first[&events[left].id], first[&events[right].id]) >= minimum,
                    "{} overlaps {}",
                    events[left].id.0,
                    events[right].id.0
                );
            }
        }
    }

    #[test]
    fn disconnected_components_are_separated_while_saved_placements_win() {
        let events = vec![
            event("cosmos-a", "Cosmos A"),
            event("cosmos-b", "Cosmos B"),
            event("kimi-a", "Kimi A"),
            event("kimi-b", "Kimi B"),
        ];
        let mut graph = graph(events, &[("cosmos-a", "cosmos-b"), ("kimi-a", "kimi-b")]);
        let generated = generate_positions(&graph);
        let cosmos = mean([
            generated[&EventId("cosmos-a".to_owned())],
            generated[&EventId("cosmos-b".to_owned())],
        ]);
        let kimi = mean([
            generated[&EventId("kimi-a".to_owned())],
            generated[&EventId("kimi-b".to_owned())],
        ]);
        assert!(distance(cosmos, kimi) > 620.0);

        let saved = Point {
            x: -4_000.0,
            y: 7_000.0,
        };
        graph.placements = IndexMap::from([(
            EventId("cosmos-a".to_owned()),
            not_news_domain::Placement {
                point: saved,
                pinned: true,
            },
        )]);
        assert_eq!(
            resolved_positions(&graph)[&EventId("cosmos-a".to_owned())],
            saved
        );
    }
}
