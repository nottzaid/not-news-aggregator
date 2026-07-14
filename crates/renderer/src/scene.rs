use std::{cell::RefCell, collections::HashMap, hash::BuildHasher};

use not_news_domain::{EventId, GraphSnapshot, Point as WorldPoint};
use skia_safe::{
    BlurStyle, Canvas, Color4f, FontArguments, FontMgr, FontStyle, Paint, PaintCap, PaintStyle,
    Path, PathBuilder, PathMeasure, Point, Rect, TileMode,
    font_arguments::{VariationPosition, variation_position::Coordinate},
    gradient::{Colors, Gradient, Interpolation, shaders},
    textlayout::{
        FontCollection, ParagraphBuilder, ParagraphStyle, TextAlign, TextDirection, TextStyle,
        TypefaceFontProvider,
    },
};

use crate::{
    ArtifactLayout, ArtifactMetrics, Motion, Viewport, ViewportTransform, layout_artifacts,
    palette::{
        BRIDGE, BRIDGE_HIGHLIGHT, DATA, PANEL_RAISED, PLUM, SIGNAL, TEXT, TEXT_DIM, color, color4f,
        color4f_with_alpha, flutter_gradient_color4f,
    },
};

const CULL_MARGIN_PIXELS: f64 = 220.0;
const EVENT_LABEL_RADIUS: f32 = 124.0;
const EVENT_NODE_RADIUS: f32 = 22.0;
const BRIDGE_DASH: f32 = 8.0;
const BRIDGE_GAP: f32 = 12.0;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SceneAnimation {
    /// Normalized phase used by Flutter's continuously flowing bridge dashes.
    pub bridge_flow: f32,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SceneState<'a> {
    pub animation: SceneAnimation,
    /// Event whose bridges retain emphasis while an activation transition runs.
    pub bridge_event: Option<&'a EventId>,
    pub expanded_event: Option<&'a EventId>,
    pub expansion_progress: f32,
    /// Previously active event fading inward during an active-event handoff.
    pub collapsing_event: Option<&'a EventId>,
    pub collapse_progress: f32,
}

/// Paints Flutter's closed bridge, event-node, and label layers over the world grid.
pub fn paint_closed_graph<S: BuildHasher>(
    canvas: &Canvas,
    width: f32,
    height: f32,
    viewport: Viewport,
    graph: &GraphSnapshot,
    positions: &HashMap<EventId, WorldPoint, S>,
    animation: SceneAnimation,
) {
    paint_graph(
        canvas,
        width,
        height,
        viewport,
        graph,
        positions,
        SceneState {
            animation,
            ..SceneState::default()
        },
    );
}

/// Paints graph layers for a closed or expanding event state.
///
/// # Panics
///
/// Panics unless the expansion progress, viewport extents, and zoom are finite
/// and positive where applicable.
pub fn paint_graph<S: BuildHasher>(
    canvas: &Canvas,
    width: f32,
    height: f32,
    viewport: Viewport,
    graph: &GraphSnapshot,
    positions: &HashMap<EventId, WorldPoint, S>,
    state: SceneState<'_>,
) {
    assert!((0.0..=1.0).contains(&state.expansion_progress));
    assert!((0.0..=1.0).contains(&state.collapse_progress));
    let transform = ViewportTransform::new(f64::from(width), f64::from(height), viewport);
    let visible = visible_world_rect(transform, width, height);
    let origin = transform.origin();

    canvas.save();
    canvas.translate((scalar(origin.x), scalar(origin.y)));
    canvas.scale((scalar(transform.scale()), scalar(transform.scale())));
    canvas.translate((scalar(-viewport.camera.x), scalar(-viewport.camera.y)));

    paint_bridges(canvas, graph, positions, visible, state);
    paint_events(canvas, graph, positions, visible, state);
    canvas.restore();
}

fn visible_world_rect(transform: ViewportTransform, width: f32, height: f32) -> Rect {
    let top_left = transform.screen_to_world(WorldPoint { x: 0.0, y: 0.0 });
    let bottom_right = transform.screen_to_world(WorldPoint {
        x: f64::from(width),
        y: f64::from(height),
    });
    let margin = CULL_MARGIN_PIXELS / transform.scale();
    Rect::new(
        scalar(top_left.x - margin),
        scalar(top_left.y - margin),
        scalar(bottom_right.x + margin),
        scalar(bottom_right.y + margin),
    )
}

fn paint_bridges<S: BuildHasher>(
    canvas: &Canvas,
    graph: &GraphSnapshot,
    positions: &HashMap<EventId, WorldPoint, S>,
    visible: Rect,
    state: SceneState<'_>,
) {
    for bridge in graph.bridges.values() {
        let (Some(from), Some(to)) = (positions.get(&bridge.from), positions.get(&bridge.to))
        else {
            continue;
        };
        if !bridge_bounds(*from, *to).intersects(visible) {
            continue;
        }
        let path = bridge_path(*from, *to);
        let bridge_event = state.bridge_event.or(state.expanded_event);
        let active_progress = bridge_event.map_or(0.0, |active| {
            if active != &bridge.from && active != &bridge.to {
                0.0
            } else if state.expanded_event == Some(active) {
                state.expansion_progress
            } else {
                state.collapse_progress
            }
        });
        if active_progress > 0.01 {
            let mut glow = Paint::default();
            glow.set_anti_alias(true);
            glow.set_style(PaintStyle::Stroke);
            glow.set_stroke_width(lerp(7.0, 12.0, active_progress));
            glow.set_stroke_cap(PaintCap::Round);
            glow.set_color4f(
                color4f_with_alpha(BRIDGE_HIGHLIGHT, 0.12 * active_progress),
                None,
            );
            glow.set_mask_filter(skia_safe::MaskFilter::blur(BlurStyle::Normal, 6.0, None));
            canvas.draw_path(&path, &glow);
        }
        let mut paint = Paint::default();
        paint.set_anti_alias(true);
        paint.set_style(PaintStyle::Stroke);
        paint.set_stroke_width(lerp(2.2, 3.4, active_progress));
        paint.set_stroke_cap(PaintCap::Round);
        let mut line_color = lerp_color(BRIDGE, BRIDGE_HIGHLIGHT, active_progress);
        line_color.a = lerp(0.22, 0.72, active_progress);
        paint.set_color4f(line_color, None);
        draw_dashed_path(canvas, &path, &paint, state.animation.bridge_flow * 140.0);
    }
}

fn paint_events<S: BuildHasher>(
    canvas: &Canvas,
    graph: &GraphSnapshot,
    positions: &HashMap<EventId, WorldPoint, S>,
    visible: Rect,
    state: SceneState<'_>,
) {
    for event in graph.events.values() {
        let Some(position) = positions.get(&event.id) else {
            continue;
        };
        let center = point(*position);
        let progress = if event.artifacts.len() <= 1 {
            0.0
        } else if state
            .expanded_event
            .is_some_and(|active| active == &event.id)
        {
            state.expansion_progress
        } else if state
            .collapsing_event
            .is_some_and(|active| active == &event.id)
        {
            state.collapse_progress
        } else {
            0.0
        };
        let expanded_radius = if progress > 0.0 {
            cached_artifact_metrics(event, |metrics| metrics.radius) * f64::from(progress)
        } else {
            0.0
        };
        let paint_radius = scalar(expanded_radius).max(lerp(EVENT_LABEL_RADIUS, 28.0, progress));
        let paint_bounds = Rect::new(
            center.x - paint_radius,
            center.y - paint_radius,
            center.x + paint_radius,
            center.y + paint_radius,
        );
        if !paint_bounds.intersects(visible) {
            continue;
        }

        canvas.save();
        canvas.translate(center);

        if progress > 0.0 {
            cached_artifact_metrics(event, |metrics| {
                for artifact in &metrics.artifacts {
                    paint_artifact(canvas, event, artifact, progress);
                }
            });
        }

        let node_radius = lerp(EVENT_NODE_RADIUS, 17.0, progress);
        let glow_radius = node_radius * 2.6;
        let glow_colors = [
            flutter_gradient_color4f(event.color, lerp(0.22, 0.34, progress)),
            flutter_gradient_color4f(event.color, 0.0),
        ];
        let glow = Gradient::new(
            Colors::new(&glow_colors, None, TileMode::Clamp, None),
            Interpolation::default(),
        );
        let mut glow_paint = Paint::default();
        glow_paint.set_anti_alias(true);
        glow_paint.set_shader(shaders::radial_gradient(
            ((0.0, 0.0), glow_radius),
            &glow,
            None,
        ));
        canvas.draw_circle((0.0, 0.0), glow_radius, &glow_paint);

        let mut node = Paint::default();
        node.set_anti_alias(true);
        node.set_color(color(event.color));
        canvas.draw_circle((0.0, 0.0), node_radius, &node);

        let mut glint = Paint::default();
        glint.set_anti_alias(true);
        glint.set_color4f(Color4f::new(1.0, 1.0, 1.0, 0.16), None);
        glint.set_mask_filter(skia_safe::MaskFilter::blur(BlurStyle::Normal, 2.0, None));
        canvas.draw_circle(
            (node_radius * -0.3, node_radius * -0.34),
            node_radius * 0.32,
            &glint,
        );

        paint_event_labels(canvas, &event.title, &event.date, progress);

        canvas.restore();
    }
}

fn paint_artifact(
    canvas: &Canvas,
    event: &not_news_domain::ResearchEvent,
    layout: &ArtifactLayout,
    progress: f32,
) {
    let artifact = &event.artifacts[layout.artifact_index];
    let eased = scalar(Motion::ease_out_cubic(f64::from(progress)));
    let offset_x = layout.offset.x * f64::from(eased);
    let offset_y = layout.offset.y * f64::from(eased);
    let offset = Point::new(scalar(offset_x), scalar(offset_y));
    let radius = scalar(layout.radius) * lerp(0.2, 1.0, eased);

    let mut tether = Paint::default();
    tether.set_anti_alias(true);
    tether.set_color4f(color4f_with_alpha(TEXT, 0.18 * eased), None);
    tether.set_stroke_width(1.4);
    canvas.draw_line((0.0, 0.0), offset, &tether);

    if eased > 0.02 {
        let halo_radius = radius + 12.0;
        let halo_colors = [
            flutter_gradient_color4f(event.color, 0.22 * eased),
            flutter_gradient_color4f(event.color, 0.0),
        ];
        let halo_gradient = Gradient::new(
            Colors::new(&halo_colors, None, TileMode::Clamp, None),
            Interpolation::default(),
        );
        let mut halo = Paint::default();
        halo.set_anti_alias(true);
        halo.set_shader(shaders::radial_gradient(
            (offset, halo_radius),
            &halo_gradient,
            None,
        ));
        canvas.draw_circle(offset, radius + 8.0, &halo);
    }

    let mut fill = Paint::default();
    fill.set_anti_alias(true);
    fill.set_color4f(color4f_with_alpha(PANEL_RAISED, 0.96 * eased), None);
    canvas.draw_circle(offset, radius, &fill);

    let mut marker = Paint::default();
    marker.set_anti_alias(true);
    marker.set_style(PaintStyle::Stroke);
    marker.set_stroke_width(1.8);
    marker.set_color4f(color4f_with_alpha(event.color, 0.8 * eased), None);
    canvas.draw_circle(offset, radius, &marker);

    let mut hairline = Paint::default();
    hairline.set_anti_alias(true);
    hairline.set_style(PaintStyle::Stroke);
    hairline.set_stroke_width(1.0);
    hairline.set_color4f(color4f_with_alpha(TEXT, 0.07 * eased), None);
    canvas.draw_circle(offset, radius - 3.0, &hairline);

    if eased > 0.4 && radius > 26.0 {
        paint_provenance_dial(
            canvas,
            &artifact.source,
            offset,
            (offset_x, offset_y),
            radius,
            eased,
        );
    }

    if progress > 0.96 {
        paint_artifact_label(canvas, layout, offset);
    }
}

fn paint_provenance_dial(
    canvas: &Canvas,
    source: &str,
    offset: Point,
    precise_offset: (f64, f64),
    radius: f32,
    eased: f32,
) {
    let source_color = source_color(source);
    let outer_angle = precise_offset.1.atan2(precise_offset.0);
    let rim_radius = radius - 3.0;
    let dial_alpha = ((eased - 0.4) / 0.6).clamp(0.0, 1.0);
    let mut ticks = Paint::default();
    ticks.set_anti_alias(true);
    ticks.set_stroke_width(1.0);
    ticks.set_stroke_cap(PaintCap::Round);
    ticks.set_color4f(color4f_with_alpha(TEXT, 0.13 * dial_alpha), None);
    for index in 0..8 {
        let angle = f64::from(index) * std::f64::consts::FRAC_PI_4;
        let direction = (angle.cos(), angle.sin());
        let inner = Point::new(
            offset.x + scalar(direction.0) * (rim_radius - 2.0),
            offset.y + scalar(direction.1) * (rim_radius - 2.0),
        );
        let outer = Point::new(
            offset.x + scalar(direction.0) * (rim_radius + 1.0),
            offset.y + scalar(direction.1) * (rim_radius + 1.0),
        );
        canvas.draw_line(inner, outer, &ticks);
    }

    let arc_rect = Rect::new(
        offset.x - rim_radius,
        offset.y - rim_radius,
        offset.x + rim_radius,
        offset.y + rim_radius,
    );
    let mut arc = Paint::default();
    arc.set_anti_alias(true);
    arc.set_style(PaintStyle::Stroke);
    arc.set_stroke_width(1.8);
    arc.set_stroke_cap(PaintCap::Round);
    arc.set_color4f(color4f_with_alpha(source_color, 0.85 * dial_alpha), None);
    canvas.draw_arc(
        arc_rect,
        scalar((outer_angle - 1.55 / 2.0).to_degrees()),
        scalar(1.55f64.to_degrees()),
        false,
        &arc,
    );

    let dot_center = Point::new(
        offset.x + scalar(outer_angle.cos()) * rim_radius,
        offset.y + scalar(outer_angle.sin()) * rim_radius,
    );
    let mut dot = Paint::default();
    dot.set_anti_alias(true);
    dot.set_color4f(color4f_with_alpha(source_color, dial_alpha), None);
    canvas.draw_circle(dot_center, 2.0, &dot);
}

fn paint_artifact_label(canvas: &Canvas, layout: &ArtifactLayout, offset: Point) {
    TEXT_RESOURCES.with(|resources| {
        let mut resources = resources.borrow_mut();
        let text = layout.lines.join("\n");
        let paragraph = resources.paragraph(
            &text,
            TextContract {
                family: crate::palette::MONO_FONT,
                color: TEXT,
                alpha: 1.0,
                size: 10.0,
                height: Some(1.12),
                letter_spacing: 0.3,
                max_width: scalar(layout.radius * 1.9),
            },
        );
        paragraph.paint(
            canvas,
            (
                offset.x - paragraph.max_width() / 2.0,
                offset.y - paragraph.height() / 2.0 - 1.0,
            ),
        );
    });
}

fn source_color(source: &str) -> u32 {
    if source.eq_ignore_ascii_case("official") {
        SIGNAL
    } else if source.eq_ignore_ascii_case("report") {
        DATA
    } else if source.eq_ignore_ascii_case("summary") {
        PLUM
    } else {
        TEXT_DIM
    }
}

fn cached_artifact_metrics<R>(
    event: &not_news_domain::ResearchEvent,
    consume: impl FnOnce(&ArtifactMetrics) -> R,
) -> R {
    ARTIFACT_LAYOUTS.with(|layouts| {
        let mut layouts = layouts.borrow_mut();
        let key = artifact_cache_key(event);
        if !layouts.contains_key(&key) && layouts.len() >= 512 {
            layouts.clear();
        }
        let metrics = layouts
            .entry(key)
            .or_insert_with(|| layout_artifacts(event));
        consume(metrics)
    })
}

fn artifact_cache_key(event: &not_news_domain::ResearchEvent) -> String {
    let mut key = format!("{}\0{}", event.id.0, event.artifacts.len());
    for artifact in &event.artifacts {
        key.push('\0');
        key.push_str(&artifact.text);
        key.push('\0');
        key.push_str(&artifact.source);
        key.push('\0');
        key.push_str(&artifact.url);
    }
    key
}

thread_local! {
    static ARTIFACT_LAYOUTS: RefCell<HashMap<String, ArtifactMetrics>> = RefCell::new(HashMap::new());
}

fn paint_event_labels(canvas: &Canvas, title: &str, date: &str, progress: f32) {
    let opacity = 1.0 - progress;
    if opacity <= 0.02 {
        return;
    }
    TEXT_RESOURCES.with(|resources| {
        let mut resources = resources.borrow_mut();
        let title_height = {
            let title = resources.paragraph(
                title,
                TextContract {
                    family: crate::palette::DISPLAY_FONT,
                    color: crate::palette::TEXT,
                    alpha: opacity,
                    size: 15.0,
                    height: Some(1.14),
                    letter_spacing: 0.15,
                    max_width: 156.0,
                },
            );
            title.paint(canvas, (-78.0, 44.0 - 10.0 * progress));
            title.height()
        };

        let date = resources.paragraph(
            date,
            TextContract {
                family: crate::palette::MONO_FONT,
                color: crate::palette::TEXT_DIM,
                alpha: opacity,
                size: 10.0,
                height: None,
                letter_spacing: 1.2,
                max_width: 150.0,
            },
        );
        let date_y = 44.0 - 10.0 * progress + title_height + 13.0 - date.height() / 2.0;
        date.paint(canvas, (-75.0, date_y));
    });
}

#[derive(Clone, Copy)]
struct TextContract {
    family: &'static str,
    color: u32,
    alpha: f32,
    size: f32,
    height: Option<f32>,
    letter_spacing: f32,
    max_width: f32,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct TextKey {
    text: String,
    family: &'static str,
    color: u32,
    alpha: u32,
    size: u32,
    height: Option<u32>,
    letter_spacing: u32,
    max_width: u32,
}

impl TextKey {
    fn new(text: &str, contract: TextContract) -> Self {
        Self {
            text: text.into(),
            family: contract.family,
            color: contract.color,
            alpha: contract.alpha.to_bits(),
            size: contract.size.to_bits(),
            height: contract.height.map(f32::to_bits),
            letter_spacing: contract.letter_spacing.to_bits(),
            max_width: contract.max_width.to_bits(),
        }
    }
}

struct TextResources {
    fonts: FontCollection,
    paragraphs: HashMap<TextKey, skia_safe::textlayout::Paragraph>,
}

impl TextResources {
    const CACHE_LIMIT: usize = 2_048;

    fn new() -> Self {
        Self {
            fonts: bundled_fonts(),
            paragraphs: HashMap::new(),
        }
    }

    fn paragraph(
        &mut self,
        text: &str,
        contract: TextContract,
    ) -> &skia_safe::textlayout::Paragraph {
        let key = TextKey::new(text, contract);
        if !self.paragraphs.contains_key(&key) && self.paragraphs.len() >= Self::CACHE_LIMIT {
            self.paragraphs.clear();
        }
        let fonts = self.fonts.clone();
        self.paragraphs
            .entry(key)
            .or_insert_with(|| build_paragraph(&fonts, text, contract))
    }
}

fn build_paragraph(
    fonts: &FontCollection,
    text: &str,
    contract: TextContract,
) -> skia_safe::textlayout::Paragraph {
    let mut style = TextStyle::new();
    let mut foreground = Paint::default();
    foreground.set_color4f(color4f_with_alpha(contract.color, contract.alpha), None);
    style.set_foreground_paint(&foreground);
    style.set_font_families(&[contract.family]);
    style.set_font_style(FontStyle::bold());
    let weight = [Coordinate {
        axis: Coordinate::wght,
        value: 700.0,
    }];
    let font_arguments = FontArguments::new().set_variation_design_position(VariationPosition {
        coordinates: &weight,
    });
    style.set_font_arguments(&font_arguments);
    style.set_font_size(contract.size);
    style.set_letter_spacing(contract.letter_spacing);
    if let Some(height) = contract.height {
        style.set_height(height);
        style.set_height_override(true);
    }

    let mut paragraph_style = ParagraphStyle::new();
    paragraph_style.set_text_align(TextAlign::Center);
    paragraph_style.set_text_direction(TextDirection::LTR);
    paragraph_style.set_text_style(&style);
    let mut builder = ParagraphBuilder::new(&paragraph_style, fonts.clone());
    builder.push_style(&style);
    builder.add_text(text);
    let mut paragraph = builder.build();
    paragraph.layout(contract.max_width);
    paragraph
}

thread_local! {
    static TEXT_RESOURCES: RefCell<TextResources> = RefCell::new(TextResources::new());
}

fn bundled_fonts() -> FontCollection {
    skia_safe::icu::init();
    let font_manager = FontMgr::new();
    let mut provider = TypefaceFontProvider::new();
    let manrope = font_manager
        .new_from_data(
            include_bytes!("../../../assets/fonts/manrope/Manrope-Regular.ttf"),
            None,
        )
        .expect("bundled Manrope font must decode");
    provider.register_typeface(manrope, Some(crate::palette::DISPLAY_FONT));
    let jetbrains = font_manager
        .new_from_data(
            include_bytes!("../../../assets/fonts/jetbrainsmono/JetBrainsMono-Regular.ttf"),
            None,
        )
        .expect("bundled JetBrains Mono font must decode");
    provider.register_typeface(jetbrains, Some(crate::palette::MONO_FONT));

    let mut collection = FontCollection::new();
    collection.set_asset_font_manager(Some(provider.into()));
    collection
}

fn bridge_bounds(from: WorldPoint, to: WorldPoint) -> Rect {
    let left = from.x.min(to.x) - 216.0;
    let top = from.y.min(to.y) - 216.0;
    let right = from.x.max(to.x) + 216.0;
    let bottom = from.y.max(to.y) + 216.0;
    Rect::new(scalar(left), scalar(top), scalar(right), scalar(bottom))
}

fn bridge_path(from: WorldPoint, to: WorldPoint) -> Path {
    let control = bridge_control(from, to);
    let mut path = PathBuilder::new();
    path.move_to(point(from));
    path.quad_to(point(control), point(to));
    path.detach()
}

fn bridge_control(from: WorldPoint, to: WorldPoint) -> WorldPoint {
    let dx = to.x - from.x;
    let dy = to.y - from.y;
    let measured_length = dx.hypot(dy);
    let length = if measured_length == 0.0 {
        1.0
    } else {
        measured_length
    };
    let bend = 72.0f64.min(length * 0.14);
    WorldPoint {
        x: from.x + dx * 0.5 + (-dy / length) * bend,
        y: from.y + dy * 0.5 + (dx / length) * bend,
    }
}

fn draw_dashed_path(canvas: &Canvas, path: &Path, paint: &Paint, phase: f32) {
    let interval = BRIDGE_DASH + BRIDGE_GAP;
    let mut measure = PathMeasure::new(path, false, None);
    let length = measure.length();
    let mut distance = -(phase % interval);
    while distance < length {
        let start = distance.max(0.0);
        let end = (distance + BRIDGE_DASH).min(length);
        if end > start {
            let mut segment = PathBuilder::new();
            if measure.get_segment(start, end, &mut segment, true) {
                canvas.draw_path(&segment.detach(), paint);
            }
        }
        distance += interval;
    }
}

fn point(value: WorldPoint) -> Point {
    Point::new(scalar(value.x), scalar(value.y))
}

fn lerp(from: f32, to: f32, progress: f32) -> f32 {
    from + (to - from) * progress
}

fn lerp_color(from: u32, to: u32, progress: f32) -> Color4f {
    let from = color4f(from);
    let to = color4f(to);
    Color4f::new(
        lerp(from.r, to.r, progress),
        lerp(from.g, to.g, progress),
        lerp(from.b, to.b, progress),
        lerp(from.a, to.a, progress),
    )
}

#[allow(clippy::cast_possible_truncation)]
fn scalar(value: f64) -> f32 {
    assert!(value.is_finite() && value >= f64::from(f32::MIN) && value <= f64::from(f32::MAX));
    value as f32
}

#[cfg(test)]
mod tests {
    use indexmap::IndexMap;
    use not_news_domain::{
        BridgeId, EventBridge, GraphSnapshot, Provenance, ResearchEvent, SourceArtifact,
    };
    use skia_safe::{Data, Image, ImageInfo, image::CachingHint, surfaces};

    use super::*;
    use crate::paint_grid;

    #[test]
    fn bridge_control_is_the_flutter_normal_displacement() {
        assert_eq!(
            bridge_control(
                WorldPoint { x: 10.0, y: 20.0 },
                WorldPoint { x: 110.0, y: 20.0 }
            ),
            WorldPoint { x: 60.0, y: 34.0 }
        );
        assert_eq!(
            bridge_control(WorldPoint { x: 7.0, y: 9.0 }, WorldPoint { x: 7.0, y: 9.0 }),
            WorldPoint { x: 7.0, y: 9.0 }
        );
    }

    #[test]
    fn unchanged_labels_reuse_shaped_paragraphs_across_frames() {
        let mut surface = surfaces::raster_n32_premul((320, 180)).unwrap();
        TEXT_RESOURCES.with(|resources| resources.borrow_mut().paragraphs.clear());
        paint_event_labels(surface.canvas(), "Stable title", "Jul 14, 2026", 0.0);
        let after_first = TEXT_RESOURCES.with(|resources| resources.borrow().paragraphs.len());
        paint_event_labels(surface.canvas(), "Stable title", "Jul 14, 2026", 0.0);
        let after_second = TEXT_RESOURCES.with(|resources| resources.borrow().paragraphs.len());
        assert_eq!((after_first, after_second), (2, 2));
    }

    #[test]
    fn four_thousand_event_frame_shapes_only_visible_labels_and_reuses_them() {
        let mut graph = GraphSnapshot::default();
        let mut positions = HashMap::new();
        for index in 0..4_096 {
            let id = EventId(format!("event-{index}"));
            graph.events.insert(
                id.clone(),
                ResearchEvent {
                    id: id.clone(),
                    title: format!("Distinct research finding {index}"),
                    date: "Jul 14, 2026".into(),
                    color: 0xff4c_9be8,
                    summary: "Performance evidence.".into(),
                    source_label: "Primary".into(),
                    artifacts: Vec::new(),
                    url: None,
                },
            );
            let point = if index < 8 {
                WorldPoint {
                    x: 220.0 + usize_to_scalar(index) * 130.0,
                    y: 450.0,
                }
            } else {
                WorldPoint {
                    x: 100_000.0 + usize_to_scalar(index) * 300.0,
                    y: 100_000.0,
                }
            };
            positions.insert(id, point);
        }
        TEXT_RESOURCES.with(|resources| resources.borrow_mut().paragraphs.clear());
        let mut surface = surfaces::raster_n32_premul((1_280, 800)).unwrap();
        paint_graph(
            surface.canvas(),
            1_280.0,
            800.0,
            Viewport::default(),
            &graph,
            &positions,
            SceneState::default(),
        );
        let after_first = TEXT_RESOURCES.with(|resources| resources.borrow().paragraphs.len());
        paint_graph(
            surface.canvas(),
            1_280.0,
            800.0,
            Viewport::default(),
            &graph,
            &positions,
            SceneState::default(),
        );
        let after_second = TEXT_RESOURCES.with(|resources| resources.borrow().paragraphs.len());
        assert_eq!(
            after_first, 9,
            "eight visible titles share one identical date paragraph"
        );
        assert_eq!(after_second, after_first, "the next frame reshaped labels");
    }

    #[test]
    fn artifact_paragraph_preserves_flutter_layout_metrics() {
        TEXT_RESOURCES.with(|resources| {
            let mut resources = resources.borrow_mut();
            let paragraph = resources.paragraph(
                "Independent\nreport with\na materially\nlonger\nfinding",
                TextContract {
                    family: crate::palette::MONO_FONT,
                    color: TEXT,
                    alpha: 1.0,
                    size: 10.0,
                    height: Some(1.12),
                    letter_spacing: 0.3,
                    max_width: scalar(63.104_666_435_502_98 * 1.9),
                },
            );
            assert_eq!((paragraph.max_width(), paragraph.height()), (119.0, 55.0));
            assert!((paragraph.max_intrinsic_width() - 75.6).abs() < 1e-4);
            assert!((paragraph.longest_line() - 75.600_006).abs() < 1e-4);
        });
    }

    #[test]
    fn reference_closed_graph_stays_within_flutter_raster_budget() {
        const FLUTTER_PNG: &[u8] =
            include_bytes!("../../../test/goldens/closed-graph-1400x900.png");
        let first_id = EventId("first".into());
        let second_id = EventId("second".into());
        let event = |id: EventId, title: &str, date: &str, color| ResearchEvent {
            id,
            title: title.into(),
            date: date.into(),
            color,
            summary: "Summary".into(),
            source_label: "Source".into(),
            artifacts: vec![],
            url: None,
        };
        let graph = GraphSnapshot {
            events: IndexMap::from([
                (
                    first_id.clone(),
                    event(
                        first_id.clone(),
                        "Orbital model accord",
                        "Jul 14, 2026",
                        0xffe8_a44c,
                    ),
                ),
                (
                    second_id.clone(),
                    event(
                        second_id.clone(),
                        "Compute treaty",
                        "Jul 15, 2026",
                        0xff4c_c9d6,
                    ),
                ),
            ]),
            bridges: IndexMap::from([(
                BridgeId("bridge".into()),
                EventBridge {
                    id: BridgeId("bridge".into()),
                    from: first_id.clone(),
                    to: second_id.clone(),
                    label: "causes".into(),
                    provenance: Provenance::Legacy,
                },
            )]),
            ..GraphSnapshot::default()
        };
        let positions = HashMap::from([
            (first_id, WorldPoint { x: 550.0, y: 450.0 }),
            (second_id, WorldPoint { x: 850.0, y: 450.0 }),
        ]);
        let mut surface = surfaces::raster_n32_premul((1400, 900)).unwrap();
        paint_grid(surface.canvas(), 1400.0, 900.0, Viewport::default());
        paint_closed_graph(
            surface.canvas(),
            1400.0,
            900.0,
            Viewport::default(),
            &graph,
            &positions,
            SceneAnimation::default(),
        );
        let flutter = decode_png(FLUTTER_PNG, 1400, 900);
        let rust = read_image(&surface.image_snapshot(), 1400, 900);
        let (changed_pixels, mean, maximum_delta) = raster_metrics(&flutter, &rust);
        let changed_fraction = ratio_usize(changed_pixels, 1_260_000);
        assert!(
            mean <= 0.07 && changed_fraction <= 0.06,
            "Flutter/Rust reference graph drift: {changed_pixels}/1260000 pixels; \
             mean channel delta {mean:.6}; max {maximum_delta}"
        );
    }

    #[test]
    fn scaled_closed_graph_stays_within_flutter_raster_budget() {
        const FLUTTER_PNG: &[u8] = include_bytes!("../../../test/goldens/closed-graph-480x270.png");
        let first_id = EventId("first".into());
        let second_id = EventId("second".into());
        let event = |id: EventId, title: &str, date: &str, color| ResearchEvent {
            id,
            title: title.into(),
            date: date.into(),
            color,
            summary: "Summary".into(),
            source_label: "Source".into(),
            artifacts: vec![],
            url: None,
        };
        let graph = GraphSnapshot {
            events: IndexMap::from([
                (
                    first_id.clone(),
                    event(
                        first_id.clone(),
                        "Orbital model accord",
                        "Jul 14, 2026",
                        0xffe8_a44c,
                    ),
                ),
                (
                    second_id.clone(),
                    event(
                        second_id.clone(),
                        "Compute treaty",
                        "Jul 15, 2026",
                        0xff4c_c9d6,
                    ),
                ),
            ]),
            bridges: IndexMap::from([(
                BridgeId("bridge".into()),
                EventBridge {
                    id: BridgeId("bridge".into()),
                    from: first_id.clone(),
                    to: second_id.clone(),
                    label: "causes".into(),
                    provenance: Provenance::Legacy,
                },
            )]),
            ..GraphSnapshot::default()
        };
        let positions = HashMap::from([
            (first_id, WorldPoint { x: 550.0, y: 450.0 }),
            (second_id, WorldPoint { x: 850.0, y: 450.0 }),
        ]);
        let mut surface = surfaces::raster_n32_premul((480, 270)).unwrap();
        paint_grid(surface.canvas(), 480.0, 270.0, Viewport::default());
        paint_closed_graph(
            surface.canvas(),
            480.0,
            270.0,
            Viewport::default(),
            &graph,
            &positions,
            SceneAnimation::default(),
        );
        let flutter = decode_png(FLUTTER_PNG, 480, 270);
        let rust = read_image(&surface.image_snapshot(), 480, 270);
        let (changed_pixels, mean, maximum_delta) = raster_metrics(&flutter, &rust);
        let changed_fraction = ratio_usize(changed_pixels, 129_600);
        assert!(
            mean <= 0.22 && changed_fraction <= 0.175,
            "Flutter/Rust scaled graph drift: {changed_pixels}/129600 pixels; \
             mean channel delta {mean:.6}; max {maximum_delta}"
        );
    }

    #[test]
    fn expanded_artifact_graph_stays_within_flutter_raster_budget() {
        const FLUTTER_PNG: &[u8] =
            include_bytes!("../../../test/goldens/artifact-graph-open-1400x900.png");
        assert_artifact_raster_budget(FLUTTER_PNG, 1.0, 0.08, 0.077);
    }

    #[test]
    fn half_expanded_artifact_graph_stays_within_flutter_raster_budget() {
        const FLUTTER_PNG: &[u8] =
            include_bytes!("../../../test/goldens/artifact-graph-half-1400x900.png");
        assert_artifact_raster_budget(FLUTTER_PNG, 0.5, 0.075, 0.073);
    }

    #[test]
    fn expanded_neighbor_and_bridge_stay_within_flutter_raster_budget() {
        const FLUTTER_PNG: &[u8] =
            include_bytes!("../../../test/goldens/artifact-neighbor-open-1400x900.png");
        let active_id = EventId("artifact-oracle".into());
        let neighbor_id = EventId("displaced-neighbor".into());
        let active = artifact_event(active_id.clone());
        let neighbor = ResearchEvent {
            id: neighbor_id.clone(),
            title: "Adjacent finding".into(),
            date: "Jul 15, 2026".into(),
            color: 0xff4c_c9d6,
            summary: "Neighbor displaced by expanded evidence.".into(),
            source_label: "Source".into(),
            artifacts: vec![],
            url: None,
        };
        let bridge_id = BridgeId("bridge".into());
        let graph = GraphSnapshot {
            events: IndexMap::from([(active_id.clone(), active), (neighbor_id.clone(), neighbor)]),
            bridges: IndexMap::from([(
                bridge_id.clone(),
                EventBridge {
                    id: bridge_id,
                    from: active_id.clone(),
                    to: neighbor_id.clone(),
                    label: "informs".into(),
                    provenance: Provenance::Legacy,
                },
            )]),
            ..GraphSnapshot::default()
        };
        let settled = HashMap::from([
            (active_id.clone(), WorldPoint { x: 700.0, y: 450.0 }),
            (neighbor_id, WorldPoint { x: 790.0, y: 450.0 }),
        ]);
        let positions = crate::expanded_positions(&graph, &settled, Some(&active_id));
        let mut surface = surfaces::raster_n32_premul((1400, 900)).unwrap();
        paint_grid(surface.canvas(), 1400.0, 900.0, Viewport::default());
        paint_graph(
            surface.canvas(),
            1400.0,
            900.0,
            Viewport::default(),
            &graph,
            &positions,
            SceneState {
                expanded_event: Some(&active_id),
                expansion_progress: 1.0,
                ..SceneState::default()
            },
        );
        let flutter = decode_png(FLUTTER_PNG, 1400, 900);
        let rust = read_image(&surface.image_snapshot(), 1400, 900);
        let (changed_pixels, mean, maximum_delta) = raster_metrics(&flutter, &rust);
        let changed_fraction = ratio_usize(changed_pixels, 1_260_000);
        assert!(
            mean <= 0.08 && changed_fraction <= 0.085,
            "Flutter/Rust expanded-neighbor drift: {changed_pixels}/1260000 pixels \
             ({changed_fraction:.6}); mean {mean:.6}; max {maximum_delta}"
        );
    }

    #[test]
    fn activation_midpoint_stays_within_flutter_temporal_raster_budget() {
        const FLUTTER_PNG: &[u8] =
            include_bytes!("../../../test/goldens/artifact-neighbor-midpoint-1400x900.png");
        let active_id = EventId("artifact-oracle".into());
        let neighbor_id = EventId("displaced-neighbor".into());
        let active = artifact_event(active_id.clone());
        let neighbor = ResearchEvent {
            id: neighbor_id.clone(),
            title: "Adjacent finding".into(),
            date: "Jul 15, 2026".into(),
            color: 0xff4c_c9d6,
            summary: "Neighbor displaced by expanded evidence.".into(),
            source_label: "Source".into(),
            artifacts: vec![],
            url: None,
        };
        let bridge_id = BridgeId("bridge".into());
        let graph = GraphSnapshot {
            events: IndexMap::from([(active_id.clone(), active), (neighbor_id.clone(), neighbor)]),
            bridges: IndexMap::from([(
                bridge_id.clone(),
                EventBridge {
                    id: bridge_id,
                    from: active_id.clone(),
                    to: neighbor_id.clone(),
                    label: "informs".into(),
                    provenance: Provenance::Legacy,
                },
            )]),
            ..GraphSnapshot::default()
        };
        let settled = HashMap::from([
            (active_id.clone(), WorldPoint { x: 700.0, y: 450.0 }),
            (neighbor_id, WorldPoint { x: 790.0, y: 450.0 }),
        ]);
        let expanded = crate::expanded_positions(&graph, &settled, Some(&active_id));
        let progress = Motion::ease_out_cubic(0.5);
        let positions: HashMap<EventId, WorldPoint> = settled
            .iter()
            .map(|(id, from)| {
                let to = expanded[id];
                (
                    id.clone(),
                    WorldPoint {
                        x: from.x + (to.x - from.x) * progress,
                        y: from.y + (to.y - from.y) * progress,
                    },
                )
            })
            .collect();
        let mut surface = surfaces::raster_n32_premul((1400, 900)).unwrap();
        paint_grid(surface.canvas(), 1400.0, 900.0, Viewport::default());
        paint_graph(
            surface.canvas(),
            1400.0,
            900.0,
            Viewport::default(),
            &graph,
            &positions,
            SceneState {
                bridge_event: Some(&active_id),
                expanded_event: Some(&active_id),
                expansion_progress: scalar(progress),
                ..SceneState::default()
            },
        );
        let flutter = decode_png(FLUTTER_PNG, 1400, 900);
        let rust = read_image(&surface.image_snapshot(), 1400, 900);
        let (changed_pixels, mean, maximum_delta) = raster_metrics(&flutter, &rust);
        let changed_fraction = ratio_usize(changed_pixels, 1_260_000);
        assert!(
            mean <= 0.08 && changed_fraction <= 0.085,
            "Flutter/Rust activation midpoint drift: {changed_pixels}/1260000 pixels \
             ({changed_fraction:.6}); mean {mean:.6}; max {maximum_delta}"
        );
    }

    fn artifact_event(id: EventId) -> ResearchEvent {
        ResearchEvent {
            id,
            title: "Artifact oracle".into(),
            date: "Jul 14, 2026".into(),
            color: 0xffe8_a44c,
            summary: "Expanded evidence.".into(),
            source_label: "Source".into(),
            artifacts: [
                ("Official model release", "Official", "official"),
                (
                    "Independent report with a materially longer finding",
                    "Report",
                    "report",
                ),
                ("Concise synthesis", "Summary", "summary"),
            ]
            .into_iter()
            .map(|(text, source, slug)| SourceArtifact {
                text: text.into(),
                source: source.into(),
                url: format!("https://example.com/{slug}"),
            })
            .collect(),
            url: None,
        }
    }

    fn assert_artifact_raster_budget(
        flutter_png: &[u8],
        progress: f32,
        maximum_mean: f64,
        maximum_changed_fraction: f64,
    ) {
        let event_id = EventId("artifact-oracle".into());
        let event = artifact_event(event_id.clone());
        let graph = GraphSnapshot {
            events: IndexMap::from([(event_id.clone(), event)]),
            ..GraphSnapshot::default()
        };
        let positions = HashMap::from([(event_id.clone(), WorldPoint { x: 700.0, y: 450.0 })]);
        let mut surface = surfaces::raster_n32_premul((1400, 900)).unwrap();
        paint_grid(surface.canvas(), 1400.0, 900.0, Viewport::default());
        paint_graph(
            surface.canvas(),
            1400.0,
            900.0,
            Viewport::default(),
            &graph,
            &positions,
            SceneState {
                expanded_event: Some(&event_id),
                expansion_progress: progress,
                ..SceneState::default()
            },
        );
        let flutter = decode_png(flutter_png, 1400, 900);
        let rust = read_image(&surface.image_snapshot(), 1400, 900);
        let (changed_pixels, mean, maximum_delta) = raster_metrics(&flutter, &rust);
        let changed_fraction = ratio_usize(changed_pixels, 1_260_000);
        assert!(
            mean <= maximum_mean && changed_fraction <= maximum_changed_fraction,
            "Flutter/Rust artifact graph drift at {progress}: {changed_pixels}/1260000 pixels; \
             mean channel delta {mean:.6}; max {maximum_delta}"
        );
    }

    fn raster_metrics(expected: &[u8], actual: &[u8]) -> (usize, f64, u8) {
        assert_eq!(expected.len(), actual.len());
        let mut changed_pixels = 0usize;
        let mut absolute_delta = 0u64;
        let mut maximum_delta = 0u8;
        for (expected, actual) in expected.chunks_exact(4).zip(actual.chunks_exact(4)) {
            let mut changed = false;
            for (&expected, &actual) in expected.iter().zip(actual) {
                let delta = expected.abs_diff(actual);
                changed |= delta != 0;
                absolute_delta += u64::from(delta);
                maximum_delta = maximum_delta.max(delta);
            }
            changed_pixels += usize::from(changed);
        }
        let mean = ratio_u64(absolute_delta, expected.len());
        (changed_pixels, mean, maximum_delta)
    }

    #[allow(clippy::cast_precision_loss)]
    fn usize_to_scalar(value: usize) -> f64 {
        value as f64
    }

    #[allow(clippy::cast_precision_loss)]
    fn ratio_usize(numerator: usize, denominator: usize) -> f64 {
        numerator as f64 / denominator as f64
    }

    #[allow(clippy::cast_precision_loss)]
    fn ratio_u64(numerator: u64, denominator: usize) -> f64 {
        numerator as f64 / denominator as f64
    }

    fn decode_png(png: &[u8], width: i32, height: i32) -> Vec<u8> {
        let image = Image::from_encoded(Data::new_copy(png)).unwrap();
        read_image(&image, width, height)
    }

    fn read_image(image: &Image, width: i32, height: i32) -> Vec<u8> {
        assert_eq!((image.width(), image.height()), (width, height));
        let info = ImageInfo::new_n32_premul((width, height), None);
        let row_bytes = info.min_row_bytes();
        let mut pixels = vec![0; info.compute_byte_size(row_bytes)];
        assert!(image.read_pixels(&info, &mut pixels, row_bytes, (0, 0), CachingHint::Disallow,));
        pixels
    }
}
