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
    Viewport, ViewportTransform,
    palette::{BRIDGE, color, color4f_with_alpha, flutter_gradient_color4f},
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
    let transform = ViewportTransform::new(f64::from(width), f64::from(height), viewport);
    let visible = visible_world_rect(transform, width, height);
    let origin = transform.origin();

    canvas.save();
    canvas.translate((scalar(origin.x), scalar(origin.y)));
    canvas.scale((scalar(transform.scale()), scalar(transform.scale())));
    canvas.translate((scalar(-viewport.camera.x), scalar(-viewport.camera.y)));

    paint_bridges(canvas, graph, positions, visible, animation.bridge_flow);
    paint_events(canvas, graph, positions, visible);
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
    bridge_flow: f32,
) {
    let mut paint = Paint::default();
    paint.set_anti_alias(true);
    paint.set_style(PaintStyle::Stroke);
    paint.set_stroke_width(2.2);
    paint.set_stroke_cap(PaintCap::Round);
    paint.set_color4f(color4f_with_alpha(BRIDGE, 0.22), None);

    for bridge in graph.bridges.values() {
        let (Some(from), Some(to)) = (positions.get(&bridge.from), positions.get(&bridge.to))
        else {
            continue;
        };
        if !bridge_bounds(*from, *to).intersects(visible) {
            continue;
        }
        let path = bridge_path(*from, *to);
        draw_dashed_path(canvas, &path, &paint, bridge_flow * 140.0);
    }
}

fn paint_events<S: BuildHasher>(
    canvas: &Canvas,
    graph: &GraphSnapshot,
    positions: &HashMap<EventId, WorldPoint, S>,
    visible: Rect,
) {
    for event in graph.events.values() {
        let Some(position) = positions.get(&event.id) else {
            continue;
        };
        let center = point(*position);
        let paint_bounds = Rect::new(
            center.x - EVENT_LABEL_RADIUS,
            center.y - EVENT_LABEL_RADIUS,
            center.x + EVENT_LABEL_RADIUS,
            center.y + EVENT_LABEL_RADIUS,
        );
        if !paint_bounds.intersects(visible) {
            continue;
        }

        canvas.save();
        canvas.translate(center);

        let glow_radius = EVENT_NODE_RADIUS * 2.6;
        let glow_colors = [
            flutter_gradient_color4f(event.color, 0.22),
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
        canvas.draw_circle((0.0, 0.0), EVENT_NODE_RADIUS, &node);

        let mut glint = Paint::default();
        glint.set_anti_alias(true);
        glint.set_color4f(Color4f::new(1.0, 1.0, 1.0, 0.16), None);
        glint.set_mask_filter(skia_safe::MaskFilter::blur(BlurStyle::Normal, 2.0, None));
        canvas.draw_circle(
            (EVENT_NODE_RADIUS * -0.3, EVENT_NODE_RADIUS * -0.34),
            EVENT_NODE_RADIUS * 0.32,
            &glint,
        );

        paint_closed_labels(canvas, &event.title, &event.date);

        canvas.restore();
    }
}

fn paint_closed_labels(canvas: &Canvas, title: &str, date: &str) {
    TEXT_RESOURCES.with(|resources| {
        let mut resources = resources.borrow_mut();
        let title_height = {
            let title = resources.paragraph(
                title,
                TextContract {
                    family: crate::palette::DISPLAY_FONT,
                    color: crate::palette::TEXT,
                    size: 15.0,
                    height: Some(1.14),
                    letter_spacing: 0.15,
                    max_width: 156.0,
                },
            );
            title.paint(canvas, (-78.0, 44.0));
            title.height()
        };

        let date = resources.paragraph(
            date,
            TextContract {
                family: crate::palette::MONO_FONT,
                color: crate::palette::TEXT_DIM,
                size: 10.0,
                height: None,
                letter_spacing: 1.2,
                max_width: 150.0,
            },
        );
        let date_y = 44.0 + title_height + 13.0 - date.height() / 2.0;
        date.paint(canvas, (-75.0, date_y));
    });
}

#[derive(Clone, Copy)]
struct TextContract {
    family: &'static str,
    color: u32,
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
    style.set_color(color(contract.color));
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

#[allow(clippy::cast_possible_truncation)]
fn scalar(value: f64) -> f32 {
    assert!(value.is_finite() && value >= f64::from(f32::MIN) && value <= f64::from(f32::MAX));
    value as f32
}

#[cfg(test)]
mod tests {
    use indexmap::IndexMap;
    use not_news_domain::{BridgeId, EventBridge, GraphSnapshot, Provenance, ResearchEvent};
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
        paint_closed_labels(surface.canvas(), "Stable title", "Jul 14, 2026");
        let after_first = TEXT_RESOURCES.with(|resources| resources.borrow().paragraphs.len());
        paint_closed_labels(surface.canvas(), "Stable title", "Jul 14, 2026");
        let after_second = TEXT_RESOURCES.with(|resources| resources.borrow().paragraphs.len());
        assert_eq!((after_first, after_second), (2, 2));
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
            mean <= 0.07 && changed_fraction <= 0.06 && maximum_delta <= 5,
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
            mean <= 0.22 && changed_fraction <= 0.175 && maximum_delta <= 4,
            "Flutter/Rust scaled graph drift: {changed_pixels}/129600 pixels; \
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
