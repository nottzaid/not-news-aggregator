use std::{cell::RefCell, collections::HashMap};

use skia_safe::{
    BlurStyle, Canvas, Color4f, FontArguments, FontMgr, FontStyle, MaskFilter, Paint, PaintStyle,
    Path, PathBuilder, RRect, Rect,
    font_arguments::{VariationPosition, variation_position::Coordinate},
    font_style::{Slant, Weight, Width},
    textlayout::{
        FontCollection, ParagraphBuilder, ParagraphStyle, TextAlign, TextDirection, TextStyle,
        TypefaceFontProvider,
    },
};

use not_news_domain::{Point as WorldPoint, ResearchEvent};

use crate::palette::{
    DISPLAY_FONT, HAIRLINE, HAIRLINE_DIM, INK_0, MONO_FONT, PANEL, PANEL_RAISED, SIGNAL,
    SIGNAL_HOT_DEEP, TEXT, TEXT_DIM, TEXT_FAINT, color, color4f_with_alpha,
};

const RECORD_DIAMETER: f32 = 72.0;
const RECORD_BOTTOM: f32 = 28.0;
const CONTROL_RIGHT: f32 = 18.0;
const CONTROL_BOTTOM: f32 = 28.0;
const CONTROL_HEIGHT: f32 = 40.0;
const CONTROL_WIDTH: f32 = 215.0;
const ACTIVITY_RIGHT: f32 = 14.0;
const ACTIVITY_BUTTON: f32 = 48.0;
const ACTIVITY_GAP: f32 = 8.0;
const METADATA_FOOTER_HEIGHT: f32 = 25.0;
const METADATA_DESKTOP_MAX_HEIGHT: f32 = 360.0;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChromeControl {
    Record,
    ZoomOut,
    ZoomLabel,
    ZoomIn,
    ResetZoom,
    Clear,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum RecordOrbState {
    #[default]
    Idle,
    Busy,
}

/// Resolves fixed-chrome hit regions in the same logical coordinate space used
/// for painting. Non-action label/divider pixels are captured as `ZoomLabel`
/// so canvas gestures cannot pass through the control strip.
pub fn hit_fixed_chrome(
    physical_point: WorldPoint,
    physical_width: f64,
    physical_height: f64,
    scale_factor: f64,
) -> Option<ChromeControl> {
    if !physical_width.is_finite()
        || !physical_height.is_finite()
        || !scale_factor.is_finite()
        || physical_width <= 0.0
        || physical_height <= 0.0
        || scale_factor <= 0.0
    {
        return None;
    }
    let point = WorldPoint {
        x: physical_point.x / scale_factor,
        y: physical_point.y / scale_factor,
    };
    let width = physical_width / scale_factor;
    let height = physical_height / scale_factor;
    let (record_left, record_bottom, record_diameter) = record_geometry_f64(width);
    let record_center = WorldPoint {
        x: record_left + record_diameter / 2.0,
        y: height - record_bottom - record_diameter / 2.0,
    };
    let record_delta = WorldPoint {
        x: point.x - record_center.x,
        y: point.y - record_center.y,
    };
    if record_delta
        .x
        .mul_add(record_delta.x, record_delta.y * record_delta.y)
        <= record_diameter * record_diameter / 4.0
    {
        return Some(ChromeControl::Record);
    }

    let left = width - f64::from(CONTROL_RIGHT + CONTROL_WIDTH);
    let control_bottom = if width > 720.0 {
        f64::from(CONTROL_BOTTOM)
    } else {
        18.0
    };
    let top = height - control_bottom - f64::from(CONTROL_HEIGHT);
    if point.x < left
        || point.x > left + f64::from(CONTROL_WIDTH)
        || point.y < top
        || point.y > top + f64::from(CONTROL_HEIGHT)
    {
        return None;
    }
    let local_x = point.x - left;
    Some(if local_x < 40.0 {
        ChromeControl::ZoomOut
    } else if local_x < 94.0 {
        ChromeControl::ZoomLabel
    } else if local_x < 134.0 {
        ChromeControl::ZoomIn
    } else if local_x < 174.0 {
        ChromeControl::ResetZoom
    } else if local_x < 175.0 {
        ChromeControl::ZoomLabel
    } else {
        ChromeControl::Clear
    })
}

pub fn hit_activity_toggle(
    physical_point: WorldPoint,
    physical_width: f64,
    scale_factor: f64,
    open_progress: f64,
) -> bool {
    if !physical_width.is_finite()
        || !scale_factor.is_finite()
        || scale_factor <= 0.0
        || !(0.0..=1.0).contains(&open_progress)
    {
        return false;
    }
    let width = physical_width / scale_factor;
    let point = WorldPoint {
        x: physical_point.x / scale_factor,
        y: physical_point.y / scale_factor,
    };
    let panel_width = activity_panel_width(width) * open_progress;
    let top = activity_top_f64(width);
    let left = width - f64::from(ACTIVITY_RIGHT + ACTIVITY_BUTTON) - panel_width;
    point.x >= left
        && point.x <= left + f64::from(ACTIVITY_BUTTON)
        && point.y >= top
        && point.y <= top + f64::from(ACTIVITY_BUTTON)
}

pub fn hit_activity_surface(
    physical_point: WorldPoint,
    physical_width: f64,
    physical_height: f64,
    scale_factor: f64,
    open_progress: f64,
) -> bool {
    if hit_activity_toggle(physical_point, physical_width, scale_factor, open_progress) {
        return true;
    }
    if !physical_height.is_finite() || physical_height <= 0.0 || open_progress <= 0.0 {
        return false;
    }
    let width = physical_width / scale_factor;
    let height = physical_height / scale_factor;
    let point = WorldPoint {
        x: physical_point.x / scale_factor,
        y: physical_point.y / scale_factor,
    };
    let open_width = activity_panel_width(width) * open_progress;
    let top = activity_top_f64(width);
    let bottom = if width > 960.0 { 116.0 } else { 164.0 };
    let left = width - f64::from(ACTIVITY_RIGHT) - open_width + f64::from(ACTIVITY_GAP);
    point.x >= left
        && point.x <= width - f64::from(ACTIVITY_RIGHT)
        && point.y >= top
        && point.y <= height - bottom
}

#[derive(Clone, Copy, Debug)]
struct MetadataLayout {
    bounds: Rect,
    summary_top: f32,
    header_stacked: bool,
    footer_height: f32,
    scroll_max: f32,
}

/// Resolves the adaptive metadata overlay so canvas gestures cannot pass through it.
#[allow(clippy::cast_possible_truncation)]
pub fn hit_active_metadata(
    physical_point: WorldPoint,
    physical_width: f64,
    physical_height: f64,
    scale_factor: f64,
    event: &ResearchEvent,
    screen_position: WorldPoint,
) -> bool {
    if !physical_width.is_finite()
        || !physical_height.is_finite()
        || !scale_factor.is_finite()
        || scale_factor <= 0.0
    {
        return false;
    }
    let point = WorldPoint {
        x: physical_point.x / scale_factor,
        y: physical_point.y / scale_factor,
    };
    let Some(layout) = metadata_layout(
        physical_width as f32,
        physical_height as f32,
        scale_factor as f32,
        event,
        screen_position,
    ) else {
        return false;
    };
    point.x >= f64::from(layout.bounds.left)
        && point.x <= f64::from(layout.bounds.right)
        && point.y >= f64::from(layout.bounds.top)
        && point.y <= f64::from(layout.bounds.bottom)
}

/// Returns the logical scroll extent of the adaptive metadata reader.
#[allow(clippy::cast_possible_truncation)]
pub fn active_metadata_scroll_max(
    physical_width: f64,
    physical_height: f64,
    scale_factor: f64,
    event: &ResearchEvent,
) -> f64 {
    if !physical_width.is_finite()
        || !physical_height.is_finite()
        || !scale_factor.is_finite()
        || scale_factor <= 0.0
    {
        return 0.0;
    }
    metadata_layout(
        physical_width as f32,
        physical_height as f32,
        scale_factor as f32,
        event,
        WorldPoint {
            x: physical_width * 0.5,
            y: physical_height * 0.5,
        },
    )
    .map_or(0.0, |layout| f64::from(layout.scroll_max))
}

pub fn paint_activity_drawer(
    canvas: &Canvas,
    physical_width: f32,
    physical_height: f32,
    scale_factor: f32,
    messages: &[String],
    running: bool,
    open_progress: f32,
) {
    let width = physical_width / scale_factor;
    let height = physical_height / scale_factor;
    let progress = open_progress.clamp(0.0, 1.0);
    let open_width = activity_panel_width_f32(width) * progress;
    let top = activity_top(width);
    let bottom = if width > 960.0 { 116.0 } else { 164.0 };
    let button = Rect::from_xywh(
        width - ACTIVITY_RIGHT - open_width - ACTIVITY_BUTTON,
        top,
        ACTIVITY_BUTTON,
        ACTIVITY_BUTTON,
    );
    canvas.save();
    canvas.scale((scale_factor, scale_factor));
    paint_activity_button(canvas, button, progress > 0.5);
    if open_width > ACTIVITY_GAP {
        let panel = Rect::from_xywh(
            button.right + ACTIVITY_GAP,
            top,
            open_width - ACTIVITY_GAP,
            (height - top - bottom).max(80.0),
        );
        canvas.save();
        canvas.clip_rect(panel, skia_safe::ClipOp::Intersect, true);
        paint_activity_panel(canvas, panel, messages, running);
        canvas.restore();
    }
    canvas.restore();
}

fn paint_activity_button(canvas: &Canvas, bounds: Rect, open: bool) {
    paint_metadata_shadow(canvas, bounds, INK_0, 0.45, 18.0, 6.0);
    let rounded = RRect::new_rect_xy(bounds, 24.0, 24.0);
    let mut fill = Paint::default();
    fill.set_anti_alias(true);
    fill.set_color(color(PANEL));
    canvas.draw_rrect(rounded, &fill);
    let mut outline = Paint::default();
    outline.set_anti_alias(true);
    outline.set_style(PaintStyle::Stroke);
    outline.set_stroke_width(1.0);
    outline.set_color(color(HAIRLINE));
    canvas.draw_rrect(rounded, &outline);

    let direction = if open { 1.0 } else { -1.0 };
    let center = bounds.center();
    let mut chevron = PathBuilder::new();
    chevron.move_to((center.x - 3.0 * direction, center.y - 6.0));
    chevron.line_to((center.x + 3.0 * direction, center.y));
    chevron.line_to((center.x - 3.0 * direction, center.y + 6.0));
    let mut paint = Paint::default();
    paint.set_anti_alias(true);
    paint.set_style(PaintStyle::Stroke);
    paint.set_stroke_width(2.0);
    paint.set_stroke_cap(skia_safe::paint::Cap::Round);
    paint.set_stroke_join(skia_safe::paint::Join::Round);
    paint.set_color(color(TEXT_DIM));
    canvas.draw_path(&chevron.detach(), &paint);
}

fn paint_activity_panel(canvas: &Canvas, bounds: Rect, messages: &[String], running: bool) {
    paint_metadata_shadow(canvas, bounds, INK_0, 0.50, 24.0, 10.0);
    let rounded = RRect::new_rect_xy(bounds, 10.0, 10.0);
    let mut fill = Paint::default();
    fill.set_anti_alias(true);
    fill.set_color(color(PANEL));
    canvas.draw_rrect(rounded, &fill);
    let mut outline = Paint::default();
    outline.set_anti_alias(true);
    outline.set_style(PaintStyle::Stroke);
    outline.set_stroke_width(1.0);
    outline.set_color(color(HAIRLINE));
    canvas.draw_rrect(rounded, &outline);

    let dot_color = if running {
        crate::palette::SIGNAL_HOT
    } else {
        crate::palette::DATA
    };
    let dot_center = (bounds.left + 16.5, bounds.top + 18.0);
    let mut glow = Paint::default();
    glow.set_anti_alias(true);
    glow.set_color4f(color4f_with_alpha(dot_color, 0.6), None);
    glow.set_mask_filter(MaskFilter::blur(
        BlurStyle::Normal,
        flutter_blur_sigma(8.0),
        None,
    ));
    canvas.draw_circle(dot_center, 4.5, &glow);
    let mut dot = Paint::default();
    dot.set_anti_alias(true);
    dot.set_color(color(dot_color));
    canvas.draw_circle(dot_center, 4.5, &dot);

    STATUS_TEXT.with(|resources| {
        let mut resources = resources.borrow_mut();
        resources
            .activity_header("HERMES")
            .paint(canvas, (bounds.left + 30.0, bounds.top + 10.0));
        let state = resources.activity_state(if running { "ACTIVE" } else { "IDLE" });
        let state_x = bounds.right - 12.0 - state.max_intrinsic_width();
        state.paint(canvas, (state_x, bounds.top + 11.0));
    });
    paint_activity_messages(canvas, bounds, messages);
}

fn paint_activity_messages(canvas: &Canvas, bounds: Rect, messages: &[String]) {
    let content = Rect::from_ltrb(
        bounds.left + 12.0,
        bounds.top + 34.0,
        bounds.right - 12.0,
        bounds.bottom - 12.0,
    );
    if messages.is_empty() {
        STATUS_TEXT.with(|resources| {
            resources
                .borrow_mut()
                .activity_state("Awaiting research activity…")
                .paint(canvas, (content.left, content.top));
        });
        return;
    }
    let mut cards = Vec::new();
    let mut used = 0.0;
    STATUS_TEXT.with(|resources| {
        let mut resources = resources.borrow_mut();
        for (reverse_index, message) in messages.iter().rev().enumerate() {
            let paragraph = resources.activity_message(message, content.width() - 20.0);
            let height = paragraph.height() + 16.0;
            if used + height > content.height() && !cards.is_empty() {
                break;
            }
            cards.push((messages.len() - 1 - reverse_index, message.clone(), height));
            used += height + 8.0;
        }
    });
    cards.reverse();
    let latest = messages.len() - 1;
    let mut y = content.top;
    for (index, message, height) in cards {
        let bounds = Rect::from_xywh(content.left, y, content.width(), height);
        let mut fill = Paint::default();
        fill.set_color4f(
            color4f_with_alpha(
                if index == latest {
                    SIGNAL
                } else {
                    PANEL_RAISED
                },
                if index == latest { 0.12 } else { 0.5 },
            ),
            None,
        );
        canvas.draw_rrect(RRect::new_rect_xy(bounds, 6.0, 6.0), &fill);
        let mut outline = Paint::default();
        outline.set_anti_alias(true);
        outline.set_style(PaintStyle::Stroke);
        outline.set_stroke_width(1.0);
        if index == latest {
            outline.set_color4f(color4f_with_alpha(SIGNAL, 0.5), None);
        } else {
            outline.set_color(color(HAIRLINE_DIM));
        }
        canvas.draw_rrect(RRect::new_rect_xy(bounds, 6.0, 6.0), &outline);
        STATUS_TEXT.with(|resources| {
            resources
                .borrow_mut()
                .activity_message(&message, bounds.width() - 20.0)
                .paint(canvas, (bounds.left + 10.0, bounds.top + 8.0));
        });
        y += height + 8.0;
    }
}

fn activity_panel_width(width: f64) -> f64 {
    (if width > 960.0 { 380.0 } else { width - 92.0 }).clamp(280.0, 380.0)
}

fn activity_panel_width_f32(width: f32) -> f32 {
    (if width > 960.0 { 380.0 } else { width - 92.0 }).clamp(280.0, 380.0)
}

fn activity_top(width: f32) -> f32 {
    if width > 960.0 { 72.0 } else { 84.0 }
}

fn activity_top_f64(width: f64) -> f64 {
    if width > 960.0 { 72.0 } else { 84.0 }
}

/// Paints Flutter's persistent record orb and zoom/reset/clear control strip.
///
/// # Panics
///
/// Panics when physical dimensions, scale factor, or zoom are non-finite or
/// non-positive.
pub fn paint_fixed_chrome(
    canvas: &Canvas,
    physical_width: f32,
    physical_height: f32,
    scale_factor: f32,
    zoom: f64,
    record_state: RecordOrbState,
) {
    assert!(physical_width.is_finite() && physical_width > 0.0);
    assert!(physical_height.is_finite() && physical_height > 0.0);
    assert!(scale_factor.is_finite() && scale_factor > 0.0);
    assert!(zoom.is_finite() && zoom > 0.0);
    let width = physical_width / scale_factor;
    let height = physical_height / scale_factor;
    canvas.save();
    canvas.scale((scale_factor, scale_factor));
    paint_record_orb(canvas, width, height, record_state);
    paint_zoom_controls(canvas, width, height, zoom);
    canvas.restore();
}

#[allow(clippy::cast_possible_truncation)]
fn metadata_layout(
    physical_width: f32,
    physical_height: f32,
    scale_factor: f32,
    event: &ResearchEvent,
    physical_screen_position: WorldPoint,
) -> Option<MetadataLayout> {
    if !physical_width.is_finite()
        || !physical_height.is_finite()
        || !scale_factor.is_finite()
        || physical_width <= 0.0
        || physical_height <= 0.0
        || scale_factor <= 0.0
    {
        return None;
    }
    let width = physical_width / scale_factor;
    let height = physical_height / scale_factor;
    let screen_position = WorldPoint {
        x: physical_screen_position.x / f64::from(scale_factor),
        y: physical_screen_position.y / f64::from(scale_factor),
    };
    let mobile = width < 720.0;
    let sheet_width = if mobile {
        width - 28.0
    } else {
        desktop_metadata_width(&event.summary)
    };
    if sheet_width <= 80.0 {
        return None;
    }
    let content_width = sheet_width - 32.0;
    let (source_width, date_width, summary_height) = METADATA_TEXT.with(|resources| {
        let mut resources = resources.borrow_mut();
        let source = resources.paragraph(
            &event.source_label.to_uppercase(),
            MetadataStyle::Source,
            content_width,
        );
        let source_width = source.max_intrinsic_width().ceil();
        let date = resources.paragraph(&event.date, MetadataStyle::Date, content_width);
        let date_width = date.max_intrinsic_width().ceil();
        let summary = resources.paragraph(&event.summary, MetadataStyle::Summary, content_width);
        summary.layout(content_width);
        (source_width, date_width, summary.height())
    });
    let header_stacked = source_width + 8.0 + date_width > content_width;
    let summary_top = if header_stacked { 47.0 } else { 33.0 };
    let content_height = summary_top + summary_height + 13.0;
    let maximum_height = if mobile {
        260.0_f32.min(height - 150.0)
    } else {
        METADATA_DESKTOP_MAX_HEIGHT.min(height - 100.0)
    };
    if maximum_height <= 40.0 {
        return None;
    }
    let overflowing = content_height > maximum_height;
    let footer_height = if overflowing {
        METADATA_FOOTER_HEIGHT
    } else {
        0.0
    };
    let sheet_height = content_height.min(maximum_height);
    let scroll_max = (content_height - (sheet_height - footer_height)).max(0.0);
    let left = if mobile {
        14.0
    } else if screen_position.x >= f64::from(width) * 0.5 {
        22.0
    } else {
        width - 22.0 - sheet_width
    };
    let top = if mobile {
        height - 92.0 - sheet_height
    } else if screen_position.y >= f64::from(height) * 0.5 {
        78.0
    } else {
        height - 22.0 - sheet_height
    };
    Some(MetadataLayout {
        bounds: Rect::from_xywh(left, top, sheet_width, sheet_height),
        summary_top,
        header_stacked,
        footer_height,
        scroll_max,
    })
}

/// Paints the adaptive active-event metadata sheet in screen space.
#[allow(clippy::too_many_arguments)]
pub fn paint_active_metadata(
    canvas: &Canvas,
    physical_width: f32,
    physical_height: f32,
    scale_factor: f32,
    event: &ResearchEvent,
    screen_position: WorldPoint,
    scroll_offset: f32,
    focused: bool,
) {
    let Some(layout) = metadata_layout(
        physical_width,
        physical_height,
        scale_factor,
        event,
        screen_position,
    ) else {
        return;
    };
    let bounds = layout.bounds;
    canvas.save();
    canvas.scale((scale_factor, scale_factor));
    paint_metadata_shadow(canvas, bounds, INK_0, 0.62, 36.0, 18.0);
    paint_metadata_shadow(canvas, bounds, event.color, 0.20, 24.0, 6.0);

    let rounded = RRect::new_rect_xy(bounds, 10.0, 10.0);
    let mut panel = Paint::default();
    panel.set_anti_alias(true);
    panel.set_color(color(PANEL));
    canvas.draw_rrect(rounded, &panel);
    let mut outline = Paint::default();
    outline.set_anti_alias(true);
    outline.set_style(PaintStyle::Stroke);
    outline.set_stroke_width(1.0);
    outline.set_color(color(HAIRLINE));
    canvas.draw_rrect(rounded, &outline);

    canvas.save();
    canvas.clip_rrect(rounded, skia_safe::ClipOp::Intersect, true);
    let stripe = Rect::from_xywh(bounds.left, bounds.top, 4.0, bounds.height());
    let mut stripe_glow = Paint::default();
    stripe_glow.set_anti_alias(true);
    stripe_glow.set_color4f(color4f_with_alpha(event.color, 0.50), None);
    stripe_glow.set_mask_filter(MaskFilter::blur(
        BlurStyle::Normal,
        flutter_blur_sigma(18.0),
        None,
    ));
    canvas.draw_rect(stripe, &stripe_glow);
    let mut stripe_paint = Paint::default();
    stripe_paint.set_color(color(event.color));
    canvas.draw_rect(stripe, &stripe_paint);
    canvas.restore();

    canvas.save();
    canvas.clip_rect(
        Rect::from_ltrb(
            bounds.left,
            bounds.top,
            bounds.right,
            bounds.bottom - layout.footer_height,
        ),
        skia_safe::ClipOp::Intersect,
        true,
    );
    canvas.translate((0.0, -scroll_offset.clamp(0.0, layout.scroll_max)));
    paint_metadata_text(canvas, layout, event);
    canvas.restore();
    if layout.footer_height > 0.0 {
        paint_metadata_footer(canvas, layout, focused);
    }
    canvas.restore();
}

/// Paints a non-interactive Flutter-style status panel in screen space.
pub fn paint_status(
    canvas: &Canvas,
    physical_width: f32,
    physical_height: f32,
    scale_factor: f32,
    message: &str,
    running: bool,
) {
    let width = physical_width / scale_factor;
    let height = physical_height / scale_factor;
    let panel_width = 420.0_f32.min(width - 44.0);
    if panel_width <= 80.0 {
        return;
    }
    let message_width = panel_width - 82.0;
    canvas.save();
    canvas.scale((scale_factor, scale_factor));
    let message_height = STATUS_TEXT.with(|resources| {
        let mut resources = resources.borrow_mut();
        resources.paragraph(message, message_width).height()
    });
    let panel_height = (message_height + 22.0).max(38.0);
    let bounds = Rect::from_xywh(
        22.0,
        height - if width > 720.0 { 28.0 } else { 90.0 } - panel_height,
        panel_width,
        panel_height,
    );
    paint_metadata_shadow(canvas, bounds, INK_0, 0.50, 18.0, 8.0);

    let rounded = RRect::new_rect_xy(bounds, 10.0, 10.0);
    let mut panel = Paint::default();
    panel.set_anti_alias(true);
    panel.set_color(color(PANEL));
    canvas.draw_rrect(rounded, &panel);
    let mut border = Paint::default();
    border.set_anti_alias(true);
    border.set_style(PaintStyle::Stroke);
    border.set_stroke_width(1.0);
    border.set_color(color(HAIRLINE));
    canvas.draw_rrect(rounded, &border);

    let accent = if running {
        crate::palette::SIGNAL_HOT
    } else {
        crate::palette::DATA
    };
    let center_y = bounds.center_y();
    let mut glow = Paint::default();
    glow.set_anti_alias(true);
    glow.set_color4f(color4f_with_alpha(accent, 0.60), None);
    glow.set_mask_filter(MaskFilter::blur(
        BlurStyle::Normal,
        flutter_blur_sigma(8.0),
        None,
    ));
    canvas.draw_circle((39.5, center_y), 4.5, &glow);
    let mut dot = Paint::default();
    dot.set_anti_alias(true);
    dot.set_color(color(accent));
    canvas.draw_circle((39.5, center_y), 4.5, &dot);

    STATUS_TEXT.with(|resources| {
        let mut resources = resources.borrow_mut();
        let label = resources.label(if running { "LIVE" } else { "IDLE" }, running);
        label.paint(canvas, (53.0, center_y - label.height() / 2.0));
    });
    STATUS_TEXT.with(|resources| {
        let mut resources = resources.borrow_mut();
        let message_paragraph = resources.paragraph(message, message_width);
        message_paragraph.paint(
            canvas,
            (
                bounds.left + 69.0,
                center_y - message_paragraph.height() / 2.0,
            ),
        );
    });
    canvas.restore();
}

/// Paints the keyboard-driven text-research composer above the Canvas without
/// changing persistent Flutter-matched chrome when the composer is closed.
pub fn paint_research_prompt(
    canvas: &Canvas,
    physical_width: f32,
    physical_height: f32,
    scale_factor: f32,
    prompt: &str,
    preedit: &str,
) {
    paint_text_prompt(
        canvas,
        physical_width,
        physical_height,
        scale_factor,
        "RESEARCH",
        "ENTER SEND  ·  ESC CLOSE  ·  CTRL+V PASTE",
        prompt,
        preedit,
        4_096,
        "Ask a research question…",
    );
}

#[allow(clippy::too_many_arguments)]
pub fn paint_curation_prompt(
    canvas: &Canvas,
    physical_width: f32,
    physical_height: f32,
    scale_factor: f32,
    title: &str,
    instruction: &str,
    input: &str,
    preedit: &str,
) {
    paint_text_prompt(
        canvas,
        physical_width,
        physical_height,
        scale_factor,
        title,
        instruction,
        input,
        preedit,
        96,
        "Type an explicit relationship predicate…",
    );
}

/// Paints a credential editor without borrowing curation language or exposing
/// the secret itself; callers supply masked input and preedit strings.
#[allow(clippy::too_many_arguments)]
pub fn paint_credential_prompt(
    canvas: &Canvas,
    physical_width: f32,
    physical_height: f32,
    scale_factor: f32,
    title: &str,
    input: &str,
    preedit: &str,
    placeholder: &str,
) {
    paint_text_prompt(
        canvas,
        physical_width,
        physical_height,
        scale_factor,
        title,
        "ENTER STORE IN OS VAULT  ·  ESC BACK  ·  CTRL+V PASTE",
        input,
        preedit,
        4_096,
        placeholder,
    );
}

/// Paints a visible, non-secret connection endpoint editor.
pub fn paint_connection_prompt(
    canvas: &Canvas,
    physical_width: f32,
    physical_height: f32,
    scale_factor: f32,
    input: &str,
    preedit: &str,
) {
    paint_text_prompt(
        canvas,
        physical_width,
        physical_height,
        scale_factor,
        "SEARXNG FRONTIER",
        "ENTER SAVE ENDPOINT  ·  ESC BACK  ·  CTRL+V PASTE",
        input,
        preedit,
        2_048,
        "https://your-searxng.example",
    );
}

#[allow(clippy::too_many_arguments)]
fn paint_text_prompt(
    canvas: &Canvas,
    physical_width: f32,
    physical_height: f32,
    scale_factor: f32,
    title: &str,
    instruction: &str,
    input: &str,
    preedit: &str,
    maximum: usize,
    placeholder: &str,
) {
    let width = physical_width / scale_factor;
    let height = physical_height / scale_factor;
    let panel_width = 680.0_f32.min(width - 44.0);
    if panel_width <= 160.0 || height < 180.0 {
        return;
    }
    let bounds = Rect::from_xywh((width - panel_width) / 2.0, 28.0, panel_width, 116.0);
    canvas.save();
    canvas.scale((scale_factor, scale_factor));
    paint_metadata_shadow(canvas, bounds, INK_0, 0.55, 24.0, 10.0);

    let rounded = RRect::new_rect_xy(bounds, 10.0, 10.0);
    let mut panel = Paint::default();
    panel.set_anti_alias(true);
    panel.set_color(color(PANEL));
    canvas.draw_rrect(rounded, &panel);
    let mut border = Paint::default();
    border.set_anti_alias(true);
    border.set_style(PaintStyle::Stroke);
    border.set_stroke_width(1.0);
    border.set_color(color(SIGNAL));
    canvas.draw_rrect(rounded, &border);

    let field = Rect::from_xywh(
        bounds.left + 16.0,
        bounds.top + 57.0,
        panel_width - 32.0,
        42.0,
    );
    let mut field_fill = Paint::default();
    field_fill.set_anti_alias(true);
    field_fill.set_color(color(PANEL_RAISED));
    canvas.draw_rrect(RRect::new_rect_xy(field, 5.0, 5.0), &field_fill);
    let mut field_border = Paint::default();
    field_border.set_anti_alias(true);
    field_border.set_style(PaintStyle::Stroke);
    field_border.set_stroke_width(1.0);
    field_border.set_color(color(HAIRLINE_DIM));
    canvas.draw_rrect(RRect::new_rect_xy(field, 5.0, 5.0), &field_border);

    STATUS_TEXT.with(|resources| {
        let mut resources = resources.borrow_mut();
        resources
            .menu_header(title)
            .paint(canvas, (bounds.left + 16.0, bounds.top + 12.0));
        resources
            .prompt_instruction(instruction, panel_width - 32.0)
            .paint(canvas, (bounds.left + 16.0, bounds.top + 35.0));
        let content = prompt_tail(input, preedit, placeholder);
        let empty = input.is_empty() && preedit.is_empty();
        canvas.save();
        canvas.clip_rect(
            Rect::from_xywh(
                field.left + 10.0,
                field.top + 5.0,
                field.width() - 112.0,
                32.0,
            ),
            skia_safe::ClipOp::Intersect,
            true,
        );
        resources
            .prompt_value(&content, field.width() - 112.0, empty)
            .paint(canvas, (field.left + 10.0, field.top + 9.0));
        canvas.restore();
        resources
            .activity_state(&format!("{} / {maximum}", input.chars().count()))
            .paint(canvas, (field.right - 90.0, field.top + 11.0));
    });
    let mut cursor = Paint::default();
    cursor.set_color(color(crate::palette::SIGNAL_HOT));
    cursor.set_stroke_width(1.8);
    canvas.draw_line(
        (field.left + 1.0, field.bottom - 1.0),
        (field.right - 1.0, field.bottom - 1.0),
        &cursor,
    );
    canvas.restore();
}

fn prompt_tail(prompt: &str, preedit: &str, placeholder: &str) -> String {
    const VISIBLE_CHARACTERS: usize = 480;
    let mut content = if prompt.is_empty() && preedit.is_empty() {
        return placeholder.into();
    } else {
        prompt.to_owned()
    };
    if !preedit.is_empty() {
        content.push_str(" 〈");
        content.push_str(preedit);
        content.push('〉');
    }
    let characters = content.chars().count();
    if characters <= VISIBLE_CHARACTERS {
        return content;
    }
    let start = content
        .char_indices()
        .nth(characters - VISIBLE_CHARACTERS)
        .map_or(0, |(index, _)| index);
    format!("…{}", &content[start..])
}

pub fn hit_curation_menu(
    physical_point: WorldPoint,
    physical_width: f64,
    physical_height: f64,
    scale_factor: f64,
    anchor: WorldPoint,
    items: &[String],
    scroll_offset: f64,
) -> Option<usize> {
    if items.is_empty() || !scale_factor.is_finite() || scale_factor <= 0.0 {
        return None;
    }
    let point = WorldPoint {
        x: physical_point.x / scale_factor,
        y: physical_point.y / scale_factor,
    };
    let anchor = WorldPoint {
        x: anchor.x / scale_factor,
        y: anchor.y / scale_factor,
    };
    let layout = menu_layout(
        physical_width / scale_factor,
        physical_height / scale_factor,
        anchor,
        items,
        scroll_offset,
    )?;
    let content_top = f64::from(layout.bounds.top + 42.0);
    if point.x < f64::from(layout.bounds.left)
        || point.x > f64::from(layout.bounds.right)
        || point.y < content_top
        || point.y > f64::from(layout.bounds.bottom)
    {
        return None;
    }
    layout
        .rows
        .iter()
        .position(|row| point.y >= f64::from(row.top) && point.y < f64::from(row.bottom))
}

pub fn hit_curation_menu_surface(
    physical_point: WorldPoint,
    physical_width: f64,
    physical_height: f64,
    scale_factor: f64,
    anchor: WorldPoint,
    items: &[String],
    scroll_offset: f64,
) -> bool {
    if !scale_factor.is_finite() || scale_factor <= 0.0 {
        return false;
    }
    let point = WorldPoint {
        x: physical_point.x / scale_factor,
        y: physical_point.y / scale_factor,
    };
    let anchor = WorldPoint {
        x: anchor.x / scale_factor,
        y: anchor.y / scale_factor,
    };
    menu_layout(
        physical_width / scale_factor,
        physical_height / scale_factor,
        anchor,
        items,
        scroll_offset,
    )
    .is_some_and(|layout| {
        point.x >= f64::from(layout.bounds.left)
            && point.x <= f64::from(layout.bounds.right)
            && point.y >= f64::from(layout.bounds.top)
            && point.y <= f64::from(layout.bounds.bottom)
    })
}

pub fn curation_menu_scroll_max(
    physical_width: f64,
    physical_height: f64,
    scale_factor: f64,
    anchor: WorldPoint,
    items: &[String],
) -> f64 {
    if !scale_factor.is_finite() || scale_factor <= 0.0 {
        return 0.0;
    }
    menu_layout(
        physical_width / scale_factor,
        physical_height / scale_factor,
        WorldPoint {
            x: anchor.x / scale_factor,
            y: anchor.y / scale_factor,
        },
        items,
        0.0,
    )
    .map_or(0.0, |layout| f64::from(layout.scroll_max))
}

pub fn curation_menu_reveal_offset(
    physical_width: f64,
    physical_height: f64,
    scale_factor: f64,
    anchor: WorldPoint,
    items: &[String],
    row: usize,
    current: f64,
) -> f64 {
    if !scale_factor.is_finite() || scale_factor <= 0.0 {
        return 0.0;
    }
    let anchor = WorldPoint {
        x: anchor.x / scale_factor,
        y: anchor.y / scale_factor,
    };
    let Some(layout) = menu_layout(
        physical_width / scale_factor,
        physical_height / scale_factor,
        anchor,
        items,
        current,
    ) else {
        return 0.0;
    };
    let Some(rect) = layout.rows.get(row) else {
        return current.clamp(0.0, f64::from(layout.scroll_max));
    };
    let viewport_top = layout.bounds.top + 42.0;
    let viewport_bottom = layout.bounds.bottom;
    let next = if rect.top < viewport_top {
        current - f64::from(viewport_top - rect.top)
    } else if rect.bottom > viewport_bottom {
        current + f64::from(rect.bottom - viewport_bottom)
    } else {
        current
    };
    next.clamp(0.0, f64::from(layout.scroll_max))
}

#[derive(Clone, Copy)]
pub struct CurationMenu<'a> {
    pub title: &'a str,
    pub items: &'a [String],
    pub selected: Option<usize>,
    pub scroll_offset: f32,
}

#[allow(clippy::too_many_lines)]
pub fn paint_curation_menu(
    canvas: &Canvas,
    physical_width: f32,
    physical_height: f32,
    scale_factor: f32,
    anchor: WorldPoint,
    menu: CurationMenu<'_>,
) {
    if menu.items.is_empty() || scale_factor <= 0.0 {
        return;
    }
    let width = physical_width / scale_factor;
    let height = physical_height / scale_factor;
    let anchor = WorldPoint {
        x: anchor.x / f64::from(scale_factor),
        y: anchor.y / f64::from(scale_factor),
    };
    let Some(layout) = menu_layout(
        f64::from(width),
        f64::from(height),
        anchor,
        menu.items,
        f64::from(menu.scroll_offset),
    ) else {
        return;
    };
    let bounds = layout.bounds;
    canvas.save();
    canvas.scale((scale_factor, scale_factor));
    paint_metadata_shadow(canvas, bounds, INK_0, 0.62, 24.0, 10.0);
    let rounded = RRect::new_rect_xy(bounds, 9.0, 9.0);
    let mut panel = Paint::default();
    panel.set_anti_alias(true);
    panel.set_color(color(PANEL));
    canvas.draw_rrect(rounded, &panel);
    let mut border = Paint::default();
    border.set_anti_alias(true);
    border.set_style(PaintStyle::Stroke);
    border.set_stroke_width(1.0);
    border.set_color(color(SIGNAL));
    canvas.draw_rrect(rounded, &border);

    STATUS_TEXT.with(|resources| {
        let mut resources = resources.borrow_mut();
        resources
            .menu_header(menu.title)
            .paint(canvas, (bounds.left + 14.0, bounds.top + 10.0));
        let mut header_rule = Paint::default();
        header_rule.set_color(color(HAIRLINE_DIM));
        canvas.draw_line(
            (bounds.left + 10.0, bounds.top + 41.0),
            (bounds.right - 10.0, bounds.top + 41.0),
            &header_rule,
        );
        canvas.save();
        canvas.clip_rect(
            Rect::from_ltrb(bounds.left, bounds.top + 42.0, bounds.right, bounds.bottom),
            skia_safe::ClipOp::Intersect,
            true,
        );
        for (index, (item, row_slot)) in menu.items.iter().zip(&layout.rows).enumerate() {
            let active = menu.selected == Some(index);
            let row = Rect::from_xywh(
                bounds.left + 6.0,
                row_slot.top + 3.0,
                bounds.width() - 12.0,
                row_slot.height() - 6.0,
            );
            if active {
                let mut selection = Paint::default();
                selection.set_anti_alias(true);
                selection.set_color(color(PANEL_RAISED));
                canvas.draw_rrect(RRect::new_rect_xy(row, 5.0, 5.0), &selection);
                let mut accent = Paint::default();
                accent.set_anti_alias(true);
                accent.set_color(color(crate::palette::SIGNAL_HOT));
                accent.set_stroke_width(2.0);
                canvas.draw_line(
                    (row.left + 1.0, row.top + 7.0),
                    (row.left + 1.0, row.bottom - 7.0),
                    &accent,
                );
                canvas.draw_line(
                    (row.right - 15.0, row.top + 17.0),
                    (row.right - 11.0, row.top + 21.0),
                    &accent,
                );
                canvas.draw_line(
                    (row.right - 11.0, row.top + 21.0),
                    (row.right - 15.0, row.top + 25.0),
                    &accent,
                );
            } else if index > 0 {
                let mut divider = Paint::default();
                divider.set_color(color(HAIRLINE_DIM));
                canvas.draw_line(
                    (bounds.left + 12.0, row_slot.top),
                    (bounds.right - 12.0, row_slot.top),
                    &divider,
                );
            }
            let (primary, detail) = item
                .split_once("  ·  ")
                .map_or((item.as_str(), None), |(primary, detail)| {
                    (primary, Some(detail))
                });
            resources
                .label(&(index + 1).to_string(), active)
                .paint(canvas, (row.left + 10.0, row.top + 10.0));
            let text_width = row.width() - 62.0;
            let primary = resources.menu_item(primary, text_width, active);
            let primary_height = primary.height();
            let primary_y = if detail.is_some() {
                row.top + 6.0
            } else {
                row.top + (row.height() - primary_height) * 0.5
            };
            primary.paint(canvas, (row.left + 38.0, primary_y));
            if let Some(detail) = detail {
                resources
                    .menu_detail(detail, text_width, active)
                    .paint(canvas, (row.left + 38.0, primary_y + primary_height + 3.0));
            }
        }
        canvas.restore();
        if layout.scroll_max > 0.0 {
            let viewport_top = bounds.top + 46.0;
            let viewport_height = (bounds.height() - 50.0).max(1.0);
            let content_height = viewport_height + layout.scroll_max;
            let thumb_height = (viewport_height * viewport_height / content_height).max(24.0);
            let offset = menu.scroll_offset.clamp(0.0, layout.scroll_max);
            let thumb_top =
                viewport_top + offset / layout.scroll_max * (viewport_height - thumb_height);
            let mut track = Paint::default();
            track.set_color(color(HAIRLINE_DIM));
            canvas.draw_round_rect(
                Rect::from_xywh(bounds.right - 5.0, viewport_top, 2.0, viewport_height),
                1.0,
                1.0,
                &track,
            );
            let mut thumb = Paint::default();
            thumb.set_color(color(TEXT_FAINT));
            canvas.draw_round_rect(
                Rect::from_xywh(bounds.right - 5.0, thumb_top, 2.0, thumb_height),
                1.0,
                1.0,
                &thumb,
            );
        }
    });
    canvas.restore();
}

#[derive(Debug)]
struct MenuLayout {
    bounds: Rect,
    rows: Vec<Rect>,
    scroll_max: f32,
}

#[allow(clippy::cast_possible_truncation)]
fn menu_layout(
    width: f64,
    height: f64,
    anchor: WorldPoint,
    items: &[String],
    scroll_offset: f64,
) -> Option<MenuLayout> {
    if items.is_empty() || width < 204.0 || height < 104.0 {
        return None;
    }
    let menu_width = 340.0_f64.min(width - 24.0);
    let text_width = menu_width as f32 - 74.0;
    let row_heights = STATUS_TEXT.with(|resources| {
        let mut resources = resources.borrow_mut();
        items
            .iter()
            .map(|item| {
                let (primary, detail) = item
                    .split_once("  ·  ")
                    .map_or((item.as_str(), None), |(primary, detail)| {
                        (primary, Some(detail))
                    });
                let primary_height = resources.menu_item(primary, text_width, false).height();
                detail.map_or(48.0, |detail| {
                    let detail_height = resources.menu_detail(detail, text_width, false).height();
                    (15.0 + primary_height + detail_height).max(48.0)
                })
            })
            .collect::<Vec<_>>()
    });
    let content_height = row_heights.iter().sum::<f32>();
    let available_height = (height - 24.0) as f32;
    let menu_height = (42.0 + content_height).min(available_height);
    let left = anchor.x.clamp(12.0, (width - menu_width - 12.0).max(12.0));
    let top = anchor
        .y
        .clamp(12.0, (height - f64::from(menu_height) - 12.0).max(12.0));
    let bounds = Rect::from_xywh(left as f32, top as f32, menu_width as f32, menu_height);
    let viewport_height = (menu_height - 42.0).max(0.0);
    let scroll_max = (content_height - viewport_height).max(0.0);
    let scroll_offset = scroll_offset.clamp(0.0, f64::from(scroll_max)) as f32;
    let mut y = bounds.top + 42.0 - scroll_offset;
    let rows = row_heights
        .into_iter()
        .map(|height| {
            let row = Rect::from_xywh(bounds.left, y, bounds.width(), height);
            y += height;
            row
        })
        .collect();
    Some(MenuLayout {
        bounds,
        rows,
        scroll_max,
    })
}

fn desktop_metadata_width(summary: &str) -> f32 {
    match summary.encode_utf16().count() {
        0..145 => 268.0,
        145..230 => 312.0,
        _ => 352.0,
    }
}

fn paint_metadata_shadow(
    canvas: &Canvas,
    bounds: Rect,
    argb: u32,
    alpha: f32,
    blur_radius: f32,
    offset_y: f32,
) {
    let mut paint = Paint::default();
    paint.set_anti_alias(true);
    paint.set_color4f(color4f_with_alpha(argb, alpha), None);
    paint.set_mask_filter(MaskFilter::blur(
        BlurStyle::Normal,
        flutter_blur_sigma(blur_radius),
        None,
    ));
    canvas.save();
    canvas.translate((0.0, offset_y));
    canvas.draw_rrect(RRect::new_rect_xy(bounds, 10.0, 10.0), &paint);
    canvas.restore();
}

fn paint_metadata_text(canvas: &Canvas, layout: MetadataLayout, event: &ResearchEvent) {
    let bounds = layout.bounds;
    let content_left = bounds.left + 18.0;
    let content_right = bounds.right - 14.0;
    let content_width = content_right - content_left;
    METADATA_TEXT.with(|resources| {
        let mut resources = resources.borrow_mut();
        let source = resources.paragraph(
            &event.source_label.to_uppercase(),
            MetadataStyle::Source,
            content_width,
        );
        let source_width = source.max_intrinsic_width().ceil();
        source.layout(source_width.min(content_width));
        source.paint(canvas, (content_left, bounds.top + 12.0));

        let date = resources.paragraph(&event.date, MetadataStyle::Date, content_width);
        let date_width = date.max_intrinsic_width().ceil().min(content_width);
        date.layout(date_width);
        date.paint(
            canvas,
            (
                if layout.header_stacked {
                    content_left
                } else {
                    content_left + source_width + 8.0
                },
                bounds.top + if layout.header_stacked { 26.0 } else { 12.0 },
            ),
        );

        let summary = resources.paragraph(&event.summary, MetadataStyle::Summary, content_width);
        summary.layout(content_width);
        summary.paint(canvas, (content_left, bounds.top + layout.summary_top));
    });
}

fn paint_metadata_footer(canvas: &Canvas, layout: MetadataLayout, focused: bool) {
    let bounds = layout.bounds;
    let top = bounds.bottom - layout.footer_height;
    let mut fill = Paint::default();
    fill.set_color(color(PANEL));
    canvas.draw_rect(
        Rect::from_ltrb(bounds.left, top, bounds.right, bounds.bottom),
        &fill,
    );
    let mut rule = Paint::default();
    rule.set_color(color(HAIRLINE_DIM));
    canvas.draw_line((bounds.left + 12.0, top), (bounds.right - 12.0, top), &rule);
    STATUS_TEXT.with(|resources| {
        resources
            .borrow_mut()
            .prompt_instruction(
                if focused {
                    "↑↓ READ  ·  PAGE UP/DOWN  ·  ESC RETURN"
                } else {
                    "TAB FOCUS TO READ"
                },
                bounds.width() - 28.0,
            )
            .paint(canvas, (bounds.left + 16.0, top + 7.0));
    });
}

fn paint_record_orb(canvas: &Canvas, width: f32, height: f32, state: RecordOrbState) {
    let (left, bottom, diameter) = record_geometry(width);
    let center = (left + diameter / 2.0, height - bottom - diameter / 2.0);
    let orb_color = match state {
        RecordOrbState::Idle => SIGNAL,
        RecordOrbState::Busy => SIGNAL_HOT_DEEP,
    };
    paint_blurred_circle(
        canvas,
        (center.0, center.1 + 18.0),
        36.0,
        diameter / 2.0,
        orb_color,
        0.55,
    );
    paint_blurred_circle(
        canvas,
        (center.0, center.1 + 4.0),
        12.0,
        diameter / 2.0,
        orb_color,
        0.30,
    );

    let mut orb = Paint::default();
    orb.set_anti_alias(true);
    orb.set_color(color(orb_color));
    canvas.draw_circle(center, diameter / 2.0, &orb);
    let mut border = Paint::default();
    border.set_anti_alias(true);
    border.set_style(PaintStyle::Stroke);
    border.set_stroke_width(2.0);
    border.set_color4f(color4f_with_alpha(INK_0, 0.28), None);
    canvas.draw_circle(center, diameter / 2.0 - 1.0, &border);

    let mut core = Paint::default();
    core.set_anti_alias(true);
    core.set_color(color(INK_0));
    match state {
        RecordOrbState::Idle => canvas.draw_circle(center, 8.5, &core),
        RecordOrbState::Busy => canvas.draw_rrect(
            RRect::new_rect_xy(
                Rect::from_xywh(center.0 - 9.0, center.1 - 9.0, 18.0, 18.0),
                4.0,
                4.0,
            ),
            &core,
        ),
    };
}

fn paint_zoom_controls(canvas: &Canvas, width: f32, height: f32, zoom: f64) {
    let bottom = if width > 720.0 { CONTROL_BOTTOM } else { 18.0 };
    let bounds = Rect::from_xywh(
        width - CONTROL_RIGHT - CONTROL_WIDTH,
        height - bottom - CONTROL_HEIGHT,
        CONTROL_WIDTH,
        CONTROL_HEIGHT,
    );
    let rounded = RRect::new_rect_xy(bounds, 10.0, 10.0);
    let mut shadow = Paint::default();
    shadow.set_anti_alias(true);
    shadow.set_color4f(Color4f::new(0.0, 0.0, 0.0, 130.0 / 255.0), None);
    shadow.set_mask_filter(MaskFilter::blur(
        BlurStyle::Normal,
        flutter_blur_sigma(24.0),
        None,
    ));
    canvas.save();
    canvas.translate((0.0, 10.0));
    canvas.draw_rrect(rounded, &shadow);
    canvas.restore();

    let mut panel = Paint::default();
    panel.set_anti_alias(true);
    panel.set_color(color(PANEL));
    canvas.draw_rrect(rounded, &panel);
    let mut outline = Paint::default();
    outline.set_anti_alias(true);
    outline.set_style(PaintStyle::Stroke);
    outline.set_stroke_width(1.0);
    outline.set_color(color(HAIRLINE));
    let mut outline_bounds = bounds;
    outline_bounds.inset((0.5, 0.5));
    canvas.draw_rrect(RRect::new_rect_xy(outline_bounds, 9.5, 9.5), &outline);

    let left = bounds.left;
    paint_material_icon(canvas, MaterialIcon::Remove, left + 20.0, bounds.center_y());
    paint_zoom_label(canvas, left + 40.0, bounds.top, zoom);
    paint_material_icon(canvas, MaterialIcon::Add, left + 114.0, bounds.center_y());
    paint_material_icon(
        canvas,
        MaterialIcon::CenterFocusStrong,
        left + 154.0,
        bounds.center_y(),
    );

    let divider_x = left + 174.0;
    let mut divider = Paint::default();
    divider.set_color(color(HAIRLINE));
    divider.set_stroke_width(1.0);
    canvas.draw_line(
        (divider_x + 0.5, bounds.center_y() - 14.0),
        (divider_x + 0.5, bounds.center_y() + 14.0),
        &divider,
    );
    paint_material_icon(
        canvas,
        MaterialIcon::DeleteOutline,
        left + 195.0,
        bounds.center_y(),
    );
}

fn paint_zoom_label(canvas: &Canvas, left: f32, top: f32, zoom: f64) {
    let percentage = format!("{:.0}%", zoom * 100.0);
    CHROME_TEXT.with(|resources| {
        let mut resources = resources.borrow_mut();
        let paragraph = resources.paragraph(&percentage);
        let y = top + (CONTROL_HEIGHT - paragraph.height()) / 2.0;
        paragraph.paint(canvas, (left, y));
    });
}

fn paint_blurred_circle(
    canvas: &Canvas,
    center: (f32, f32),
    blur_radius: f32,
    circle_radius: f32,
    argb: u32,
    alpha: f32,
) {
    let mut paint = Paint::default();
    paint.set_anti_alias(true);
    paint.set_color4f(color4f_with_alpha(argb, alpha), None);
    paint.set_mask_filter(MaskFilter::blur(
        BlurStyle::Normal,
        flutter_blur_sigma(blur_radius),
        None,
    ));
    canvas.draw_circle(center, circle_radius, &paint);
}

fn record_geometry(width: f32) -> (f32, f32, f32) {
    let diameter = if width > 720.0 { RECORD_DIAMETER } else { 64.0 };
    let bottom = if width > 720.0 { RECORD_BOTTOM } else { 18.0 };
    (width / 2.0 - 36.0, bottom, diameter)
}

fn record_geometry_f64(width: f64) -> (f64, f64, f64) {
    let diameter = if width > 720.0 {
        f64::from(RECORD_DIAMETER)
    } else {
        64.0
    };
    let bottom = if width > 720.0 {
        f64::from(RECORD_BOTTOM)
    } else {
        18.0
    };
    (width / 2.0 - 36.0, bottom, diameter)
}

fn flutter_blur_sigma(radius: f32) -> f32 {
    radius * 0.577_35 + 0.5
}

#[derive(Clone, Copy)]
enum MaterialIcon {
    Add,
    Remove,
    CenterFocusStrong,
    DeleteOutline,
}

fn paint_material_icon(canvas: &Canvas, icon: MaterialIcon, center_x: f32, center_y: f32) {
    let path = material_icon_path(icon);
    let mut paint = Paint::default();
    paint.set_anti_alias(true);
    paint.set_color(color(TEXT_DIM));
    canvas.save();
    canvas.translate((center_x - 9.0, center_y - 9.0));
    canvas.scale((0.75, 0.75));
    canvas.draw_path(&path, &paint);
    canvas.restore();
}

fn material_icon_path(icon: MaterialIcon) -> Path {
    match icon {
        MaterialIcon::Add => polygon_path(&[
            (19.0, 13.0),
            (13.0, 13.0),
            (13.0, 19.0),
            (11.0, 19.0),
            (11.0, 13.0),
            (5.0, 13.0),
            (5.0, 11.0),
            (11.0, 11.0),
            (11.0, 5.0),
            (13.0, 5.0),
            (13.0, 11.0),
            (19.0, 11.0),
        ]),
        MaterialIcon::Remove => {
            polygon_path(&[(19.0, 13.0), (5.0, 13.0), (5.0, 11.0), (19.0, 11.0)])
        }
        MaterialIcon::CenterFocusStrong => center_focus_path(),
        MaterialIcon::DeleteOutline => delete_outline_path(),
    }
}

fn center_focus_path() -> Path {
    let mut builder = PathBuilder::new();
    for points in [
        &[
            (9.0, 3.0),
            (5.0, 3.0),
            (3.0, 5.0),
            (3.0, 9.0),
            (5.0, 9.0),
            (5.0, 5.0),
            (9.0, 5.0),
        ][..],
        &[
            (15.0, 3.0),
            (19.0, 3.0),
            (21.0, 5.0),
            (21.0, 9.0),
            (19.0, 9.0),
            (19.0, 5.0),
            (15.0, 5.0),
        ][..],
        &[
            (21.0, 15.0),
            (21.0, 19.0),
            (19.0, 21.0),
            (15.0, 21.0),
            (15.0, 19.0),
            (19.0, 19.0),
            (19.0, 15.0),
        ][..],
        &[
            (9.0, 21.0),
            (5.0, 21.0),
            (3.0, 19.0),
            (3.0, 15.0),
            (5.0, 15.0),
            (5.0, 19.0),
            (9.0, 19.0),
        ][..],
    ] {
        add_polygon(&mut builder, points);
    }
    builder.add_circle((12.0, 12.0), 4.0, None);
    builder.detach()
}

fn delete_outline_path() -> Path {
    let mut builder = PathBuilder::new();
    add_polygon(
        &mut builder,
        &[
            (6.0, 7.0),
            (18.0, 7.0),
            (18.0, 19.0),
            (16.0, 21.0),
            (8.0, 21.0),
            (6.0, 19.0),
        ],
    );
    builder.add_rect(Rect::from_xywh(8.0, 9.0, 8.0, 10.0), None, 0);
    add_polygon(
        &mut builder,
        &[
            (5.0, 4.0),
            (8.5, 4.0),
            (9.5, 3.0),
            (14.5, 3.0),
            (15.5, 4.0),
            (19.0, 4.0),
            (19.0, 6.0),
            (5.0, 6.0),
        ],
    );
    let mut path = builder.detach();
    path.set_fill_type(skia_safe::PathFillType::EvenOdd);
    path
}

fn polygon_path(points: &[(f32, f32)]) -> Path {
    let mut builder = PathBuilder::new();
    add_polygon(&mut builder, points);
    builder.detach()
}

fn add_polygon(builder: &mut PathBuilder, points: &[(f32, f32)]) {
    let Some(&(first_x, first_y)) = points.first() else {
        return;
    };
    builder.move_to((first_x, first_y));
    for &(x, y) in &points[1..] {
        builder.line_to((x, y));
    }
    builder.close();
}

struct ChromeText {
    fonts: FontCollection,
    paragraphs: HashMap<String, skia_safe::textlayout::Paragraph>,
}

impl ChromeText {
    fn new() -> Self {
        let fonts = ui_fonts();
        Self {
            fonts,
            paragraphs: HashMap::new(),
        }
    }

    fn paragraph(&mut self, text: &str) -> &skia_safe::textlayout::Paragraph {
        let fonts = self.fonts.clone();
        self.paragraphs.entry(text.to_owned()).or_insert_with(|| {
            let mut style = TextStyle::new();
            let mut foreground = Paint::default();
            foreground.set_color(color(TEXT));
            style.set_foreground_paint(&foreground);
            style.set_font_families(&[MONO_FONT]);
            style.set_font_style(FontStyle::bold());
            style.set_font_arguments(&FontArguments::new().set_variation_design_position(
                VariationPosition {
                    coordinates: &[Coordinate {
                        axis: Coordinate::wght,
                        value: 700.0,
                    }],
                },
            ));
            style.set_font_size(11.0);
            style.set_letter_spacing(0.4);
            let mut paragraph_style = ParagraphStyle::new();
            paragraph_style.set_text_align(TextAlign::Center);
            paragraph_style.set_text_direction(TextDirection::LTR);
            paragraph_style.set_text_style(&style);
            let mut builder = ParagraphBuilder::new(&paragraph_style, fonts);
            builder.push_style(&style);
            builder.add_text(text);
            let mut paragraph = builder.build();
            paragraph.layout(54.0);
            paragraph
        })
    }
}

thread_local! {
    static CHROME_TEXT: RefCell<ChromeText> = RefCell::new(ChromeText::new());
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum MetadataStyle {
    Source,
    Date,
    Summary,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct MetadataKey {
    text: String,
    style: MetadataStyle,
    width: u32,
}

struct MetadataText {
    fonts: FontCollection,
    paragraphs: HashMap<MetadataKey, skia_safe::textlayout::Paragraph>,
}

impl MetadataText {
    fn new() -> Self {
        Self {
            fonts: ui_fonts(),
            paragraphs: HashMap::new(),
        }
    }

    fn paragraph(
        &mut self,
        text: &str,
        metadata_style: MetadataStyle,
        width: f32,
    ) -> &mut skia_safe::textlayout::Paragraph {
        let key = MetadataKey {
            text: text.to_owned(),
            style: metadata_style,
            width: width.to_bits(),
        };
        if !self.paragraphs.contains_key(&key) && self.paragraphs.len() >= 1_024 {
            self.paragraphs.clear();
        }
        let fonts = self.fonts.clone();
        self.paragraphs.entry(key).or_insert_with(|| {
            let (family, color_value, size, weight, font_weight, spacing, height) =
                match metadata_style {
                    MetadataStyle::Source => {
                        (MONO_FONT, TEXT_DIM, 9.5, 700.0, Weight::BOLD, 1.4, None)
                    }
                    MetadataStyle::Date => (
                        MONO_FONT,
                        TEXT_FAINT,
                        9.5,
                        600.0,
                        Weight::SEMI_BOLD,
                        0.8,
                        None,
                    ),
                    MetadataStyle::Summary => (
                        DISPLAY_FONT,
                        TEXT,
                        13.5,
                        500.0,
                        Weight::MEDIUM,
                        0.0,
                        Some(1.5),
                    ),
                };
            let mut style = TextStyle::new();
            let mut foreground = Paint::default();
            foreground.set_color(color(color_value));
            style.set_foreground_paint(&foreground);
            style.set_font_families(&[family]);
            style.set_font_style(FontStyle::new(font_weight, Width::NORMAL, Slant::Upright));
            style.set_font_arguments(&FontArguments::new().set_variation_design_position(
                VariationPosition {
                    coordinates: &[Coordinate {
                        axis: Coordinate::wght,
                        value: weight,
                    }],
                },
            ));
            style.set_font_size(size);
            style.set_letter_spacing(spacing);
            if let Some(height) = height {
                style.set_height(height);
                style.set_height_override(true);
            }
            let mut paragraph_style = ParagraphStyle::new();
            paragraph_style.set_text_align(TextAlign::Left);
            paragraph_style.set_text_direction(TextDirection::LTR);
            paragraph_style.set_text_style(&style);
            if matches!(metadata_style, MetadataStyle::Source | MetadataStyle::Date) {
                paragraph_style.set_max_lines(1);
                paragraph_style.set_ellipsis("…");
            }
            let mut builder = ParagraphBuilder::new(&paragraph_style, fonts);
            builder.push_style(&style);
            builder.add_text(text);
            let mut paragraph = builder.build();
            paragraph.layout(width);
            paragraph
        })
    }
}

fn ui_fonts() -> FontCollection {
    skia_safe::icu::init();
    let manager = FontMgr::new();
    let mut provider = TypefaceFontProvider::new();
    let mono = manager
        .new_from_data(
            include_bytes!("../../../assets/fonts/jetbrainsmono/JetBrainsMono-Regular.ttf"),
            None,
        )
        .expect("bundled JetBrains Mono font must decode");
    provider.register_typeface(mono, Some(MONO_FONT));
    let display = manager
        .new_from_data(
            include_bytes!("../../../assets/fonts/manrope/Manrope-Regular.ttf"),
            None,
        )
        .expect("bundled Manrope font must decode");
    provider.register_typeface(display, Some(DISPLAY_FONT));
    let mut fonts = FontCollection::new();
    fonts.set_asset_font_manager(Some(provider.into()));
    fonts
}

thread_local! {
    static METADATA_TEXT: RefCell<MetadataText> = RefCell::new(MetadataText::new());
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum StatusStyle {
    Idle,
    Live,
    Message,
    ActivityHeader,
    ActivityState,
    ActivityMessage,
    MenuItem,
    MenuItemActive,
    MenuDetailActive,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct StatusKey {
    text: String,
    style: StatusStyle,
    width: u32,
}

struct StatusText {
    fonts: FontCollection,
    paragraphs: HashMap<StatusKey, skia_safe::textlayout::Paragraph>,
}

impl StatusText {
    fn new() -> Self {
        Self {
            fonts: ui_fonts(),
            paragraphs: HashMap::new(),
        }
    }

    fn label(&mut self, text: &str, running: bool) -> &skia_safe::textlayout::Paragraph {
        self.text(
            text,
            if running {
                StatusStyle::Live
            } else {
                StatusStyle::Idle
            },
            38.0,
        )
    }

    fn paragraph(&mut self, text: &str, width: f32) -> &skia_safe::textlayout::Paragraph {
        self.text(text, StatusStyle::Message, width)
    }

    fn activity_header(&mut self, text: &str) -> &skia_safe::textlayout::Paragraph {
        self.text(text, StatusStyle::ActivityHeader, 90.0)
    }

    fn menu_header(&mut self, text: &str) -> &skia_safe::textlayout::Paragraph {
        self.text(text, StatusStyle::Live, 280.0)
    }

    fn activity_state(&mut self, text: &str) -> &skia_safe::textlayout::Paragraph {
        self.text(text, StatusStyle::ActivityState, 220.0)
    }

    fn prompt_instruction(&mut self, text: &str, width: f32) -> &skia_safe::textlayout::Paragraph {
        self.text(text, StatusStyle::ActivityState, width)
    }

    fn prompt_value(
        &mut self,
        text: &str,
        width: f32,
        placeholder: bool,
    ) -> &skia_safe::textlayout::Paragraph {
        self.text(
            text,
            if placeholder {
                StatusStyle::ActivityState
            } else {
                StatusStyle::ActivityMessage
            },
            width,
        )
    }

    fn menu_item(
        &mut self,
        text: &str,
        width: f32,
        active: bool,
    ) -> &skia_safe::textlayout::Paragraph {
        self.text(
            text,
            if active {
                StatusStyle::MenuItemActive
            } else {
                StatusStyle::MenuItem
            },
            width,
        )
    }

    fn menu_detail(
        &mut self,
        text: &str,
        width: f32,
        active: bool,
    ) -> &skia_safe::textlayout::Paragraph {
        self.text(
            text,
            if active {
                StatusStyle::MenuDetailActive
            } else {
                StatusStyle::ActivityState
            },
            width,
        )
    }

    fn activity_message(&mut self, text: &str, width: f32) -> &skia_safe::textlayout::Paragraph {
        self.text(text, StatusStyle::ActivityMessage, width)
    }

    fn text(
        &mut self,
        text: &str,
        status_style: StatusStyle,
        width: f32,
    ) -> &skia_safe::textlayout::Paragraph {
        let key = StatusKey {
            text: text.to_owned(),
            style: status_style,
            width: width.to_bits(),
        };
        if !self.paragraphs.contains_key(&key) && self.paragraphs.len() >= 128 {
            self.paragraphs.clear();
        }
        let fonts = self.fonts.clone();
        self.paragraphs.entry(key).or_insert_with(|| {
            let (color_value, size, weight, spacing, height) = match status_style {
                StatusStyle::Idle => (crate::palette::DATA, 9.5, 700.0, 1.4, None),
                StatusStyle::Live => (crate::palette::SIGNAL_HOT, 9.5, 700.0, 1.4, None),
                StatusStyle::Message => (TEXT, 11.0, 600.0, 0.0, Some(1.3)),
                StatusStyle::ActivityHeader => (TEXT, 11.0, 700.0, 1.6, None),
                StatusStyle::ActivityState => (TEXT_FAINT, 9.5, 700.0, 1.4, None),
                StatusStyle::ActivityMessage => (TEXT, 11.0, 600.0, 0.0, Some(1.35)),
                StatusStyle::MenuItem => (TEXT_DIM, 10.0, 650.0, 1.0, None),
                StatusStyle::MenuItemActive => (TEXT, 10.0, 700.0, 1.0, None),
                StatusStyle::MenuDetailActive => {
                    (crate::palette::SIGNAL_HOT, 9.0, 700.0, 1.15, None)
                }
            };
            let mut style = TextStyle::new();
            let mut foreground = Paint::default();
            foreground.set_color(color(color_value));
            style.set_foreground_paint(&foreground);
            style.set_font_families(&[MONO_FONT]);
            style.set_font_arguments(&FontArguments::new().set_variation_design_position(
                VariationPosition {
                    coordinates: &[Coordinate {
                        axis: Coordinate::wght,
                        value: weight,
                    }],
                },
            ));
            style.set_font_size(size);
            style.set_letter_spacing(spacing);
            if let Some(height) = height {
                style.set_height(height);
                style.set_height_override(true);
            }
            let mut paragraph_style = ParagraphStyle::new();
            paragraph_style.set_text_align(TextAlign::Left);
            paragraph_style.set_text_direction(TextDirection::LTR);
            paragraph_style.set_text_style(&style);
            if matches!(status_style, StatusStyle::Message) {
                paragraph_style.set_max_lines(6);
                paragraph_style.set_ellipsis("…");
            } else if matches!(status_style, StatusStyle::ActivityMessage) {
                paragraph_style.set_max_lines(3);
                paragraph_style.set_ellipsis("…");
            }
            let mut builder = ParagraphBuilder::new(&paragraph_style, fonts);
            builder.push_style(&style);
            builder.add_text(text);
            let mut paragraph = builder.build();
            paragraph.layout(width);
            paragraph
        })
    }
}

thread_local! {
    static STATUS_TEXT: RefCell<StatusText> = RefCell::new(StatusText::new());
}

#[cfg(test)]
mod tests {
    use not_news_domain::EventId;
    use skia_safe::{Data, Image, ImageInfo, image::CachingHint, surfaces};

    use super::*;

    #[test]
    fn fixed_chrome_stays_within_flutter_crop_budgets() {
        const FLUTTER: &[u8] =
            include_bytes!("../../../fixtures/reference-raster/full-screen-closed-1280x800.png");
        const FLUTTER_BASE: &[u8] =
            include_bytes!("../../../fixtures/reference-raster/full-screen-base-1280x800.png");
        const FLUTTER_BUSY: &[u8] = include_bytes!(
            "../../../fixtures/reference-raster/full-screen-record-busy-1280x800.png"
        );
        let flutter_chrome = read_image(&Image::from_encoded(Data::new_copy(FLUTTER)).unwrap());
        let flutter_base = read_image(&Image::from_encoded(Data::new_copy(FLUTTER_BASE)).unwrap());
        let flutter_busy = read_image(&Image::from_encoded(Data::new_copy(FLUTTER_BUSY)).unwrap());
        let rust_chrome = paint_over(&flutter_base, |canvas| {
            paint_fixed_chrome(canvas, 1280.0, 800.0, 1.0, 1.0, RecordOrbState::Idle);
        });
        let (_, mean, _) = residual_crop_metrics(
            &flutter_base,
            &flutter_chrome,
            &flutter_base,
            &rust_chrome,
            &[(560, 660, 160, 140), (1010, 690, 270, 110)],
        );
        let record = residual_crop_metrics(
            &flutter_base,
            &flutter_chrome,
            &flutter_base,
            &rust_chrome,
            &[(560, 660, 160, 140)],
        );
        let controls = residual_crop_metrics(
            &flutter_base,
            &flutter_chrome,
            &flutter_base,
            &rust_chrome,
            &[(1010, 690, 270, 110)],
        );
        let rust_busy = paint_over(&flutter_base, |canvas| {
            paint_fixed_chrome(canvas, 1280.0, 800.0, 1.0, 1.0, RecordOrbState::Busy);
        });
        let busy = residual_crop_metrics(
            &flutter_base,
            &flutter_busy,
            &flutter_base,
            &rust_busy,
            &[(560, 660, 160, 140)],
        );
        assert!(
            mean <= 0.60,
            "Flutter/Rust fixed chrome mean drift {mean:.6}"
        );
        assert!(
            record.1 <= 0.60 && record.2 <= 5,
            "Flutter/Rust record-orb drift {record:?}"
        );
        assert!(
            busy.1 <= 0.60 && busy.2 <= 5,
            "Flutter/Rust busy record-orb drift {busy:?}"
        );
        assert!(
            controls.1 <= 0.60,
            "Flutter/Rust control-strip drift {controls:?}"
        );
    }

    #[test]
    fn narrow_chrome_and_status_follow_flutter_breakpoints() {
        const FLUTTER_BASE: &[u8] =
            include_bytes!("../../../fixtures/reference-raster/narrow-active-base-640x720.png");
        const FLUTTER_STATUS: &[u8] =
            include_bytes!("../../../fixtures/reference-raster/narrow-status-640x720.png");
        let flutter_base = read_image_size(
            &Image::from_encoded(Data::new_copy(FLUTTER_BASE)).unwrap(),
            (640, 720),
        );
        let flutter_status = read_image_size(
            &Image::from_encoded(Data::new_copy(FLUTTER_STATUS)).unwrap(),
            (640, 720),
        );
        let rust_status = paint_over_size(&flutter_base, (640, 720), |canvas| {
            paint_status(
                canvas,
                640.0,
                720.0,
                1.0,
                "Graph unavailable; no research is shown or writable.",
                false,
            );
            paint_fixed_chrome(canvas, 640.0, 720.0, 1.0, 1.0, RecordOrbState::Idle);
        });
        let chrome = residual_crop_metrics_width(
            &flutter_base,
            &flutter_status,
            &flutter_base,
            &rust_status,
            &[(260, 680, 120, 40), (400, 680, 240, 40)],
            640,
        );
        let status = residual_crop_metrics_width(
            &flutter_base,
            &flutter_status,
            &flutter_base,
            &rust_status,
            &[(0, 540, 480, 95)],
            640,
        );
        assert!(
            chrome.1 <= 0.8 && chrome.2 <= 130,
            "Flutter/Rust narrow chrome drift {chrome:?}"
        );
        assert!(
            status.1 <= 4.0 && status.2 <= 230,
            "Flutter/Rust narrow status drift {status:?}"
        );
    }

    #[test]
    fn narrow_fixed_surfaces_own_their_source_derived_hit_regions() {
        let event = ResearchEvent {
            id: EventId("active".into()),
            title: "Active".into(),
            date: "Jul 14, 2026".into(),
            color: 0xffb8_5534,
            summary: "Visible mobile metadata".into(),
            source_label: "Primary".into(),
            artifacts: Vec::new(),
            url: None,
        };
        assert_eq!(
            hit_fixed_chrome(WorldPoint { x: 316.0, y: 670.0 }, 640.0, 720.0, 1.0),
            Some(ChromeControl::Record)
        );
        assert_eq!(
            hit_fixed_chrome(WorldPoint { x: 602.0, y: 682.0 }, 640.0, 720.0, 1.0),
            Some(ChromeControl::Clear)
        );
        assert!(hit_active_metadata(
            WorldPoint { x: 20.0, y: 580.0 },
            640.0,
            720.0,
            1.0,
            &event,
            WorldPoint { x: 800.0, y: 500.0 },
        ));
        assert!(!hit_active_metadata(
            WorldPoint { x: 10.0, y: 580.0 },
            640.0,
            720.0,
            1.0,
            &event,
            WorldPoint { x: 800.0, y: 500.0 },
        ));
    }

    #[test]
    fn narrow_metadata_scroll_extent_tracks_real_laid_out_content() {
        let mut event = ResearchEvent {
            id: EventId("active".into()),
            title: "Active".into(),
            date: "Jul 14, 2026".into(),
            color: 0xffb8_5534,
            summary: "Short metadata remains stationary.".into(),
            source_label: "Primary".into(),
            artifacts: Vec::new(),
            url: None,
        };
        assert!(active_metadata_scroll_max(640.0, 720.0, 1.0, &event).abs() < f64::EPSILON);
        event.summary = "A bounded metadata viewport must derive its scroll extent from the same shaped paragraph that is painted. ".repeat(40);
        let extent = active_metadata_scroll_max(640.0, 360.0, 1.0, &event);
        assert!(extent > 700.0, "unexpected long-content extent {extent}");
        assert!(active_metadata_scroll_max(f64::NAN, 360.0, 1.0, &event).abs() < f64::EPSILON);
    }

    #[test]
    fn active_metadata_uses_natural_height_then_a_focused_scroll_viewport() {
        let mut event = ResearchEvent {
            id: EventId("spacex".into()),
            title: "SpaceX compute partnership".into(),
            date: "May 6, 2026".into(),
            color: 0xffb8_5534,
            summary: "Short saved finding.".into(),
            source_label: "Anthropic".into(),
            artifacts: Vec::new(),
            url: None,
        };
        let short = metadata_layout(
            1_280.0,
            800.0,
            1.0,
            &event,
            WorldPoint { x: 900.0, y: 500.0 },
        )
        .unwrap();
        assert!(short.bounds.height() < 120.0, "{:?}", short.bounds);
        assert!(short.footer_height.abs() < f32::EPSILON);
        assert!(short.scroll_max.abs() < f32::EPSILON);
        assert!((short.bounds.left - 22.0).abs() < f32::EPSILON);
        assert!((short.bounds.top - 78.0).abs() < f32::EPSILON);
        assert!(900.0 > short.bounds.right || 500.0 > short.bounds.bottom);

        event.summary = "A long saved finding remains available without network services and must be readable through an explicit focused viewport. ".repeat(80);
        let long = metadata_layout(
            1_280.0,
            800.0,
            1.0,
            &event,
            WorldPoint { x: 900.0, y: 500.0 },
        )
        .unwrap();
        assert!((long.bounds.height() - METADATA_DESKTOP_MAX_HEIGHT).abs() < f32::EPSILON);
        assert!((long.footer_height - METADATA_FOOTER_HEIGHT).abs() < f32::EPSILON);
        assert!(long.scroll_max > 1_000.0);
    }

    fn paint_over(base: &[u8], paint: impl FnOnce(&Canvas)) -> Vec<u8> {
        paint_over_size(base, (1280, 800), paint)
    }

    fn paint_over_size(base: &[u8], size: (i32, i32), paint: impl FnOnce(&Canvas)) -> Vec<u8> {
        let info = ImageInfo::new_n32_premul(size, None);
        let row_bytes = info.min_row_bytes();
        let mut pixels = base.to_vec();
        {
            let mut surface = surfaces::wrap_pixels(&info, &mut pixels, row_bytes, None).unwrap();
            paint(surface.canvas());
        }
        pixels
    }

    #[test]
    fn chrome_hit_regions_match_paint_geometry_at_fractional_dpr() {
        let dpr = 1.5;
        let physical = |x: f64, y: f64| WorldPoint {
            x: x * dpr,
            y: y * dpr,
        };
        let hit = |point| hit_fixed_chrome(point, 1_920.0, 1_200.0, dpr);
        assert_eq!(hit(physical(640.0, 736.0)), Some(ChromeControl::Record));
        assert_eq!(hit(physical(1_067.0, 752.0)), Some(ChromeControl::ZoomOut));
        assert_eq!(
            hit(physical(1_107.0, 752.0)),
            Some(ChromeControl::ZoomLabel)
        );
        assert_eq!(hit(physical(1_161.0, 752.0)), Some(ChromeControl::ZoomIn));
        assert_eq!(
            hit(physical(1_201.0, 752.0)),
            Some(ChromeControl::ResetZoom)
        );
        assert_eq!(hit(physical(1_242.0, 752.0)), Some(ChromeControl::Clear));
        assert_eq!(hit(physical(1_000.0, 752.0)), None);
    }

    #[test]
    fn metadata_summary_preserves_flutter_line_contract() {
        let summary = "Anthropic announces access to SpaceX's Colossus 1 capacity. Claude usage-limit changes and orbital compute interest live inside this event graph as claims, not separate Canvas points.";
        METADATA_TEXT.with(|resources| {
            let mut resources = resources.borrow_mut();
            let paragraph = resources.paragraph(summary, MetadataStyle::Summary, 280.0);
            paragraph.layout(280.0);
            let lines = paragraph.get_line_metrics();
            let ranges: Vec<_> = lines
                .iter()
                .map(|line| (line.start_index, line.end_excluding_whitespaces))
                .collect();
            assert_eq!(
                ranges,
                [(0, 38), (39, 78), (79, 120), (121, 159), (160, 183)]
            );
            assert!(lines.iter().all(|line| line.width < 280.0));
            assert!(
                lines
                    .windows(2)
                    .all(|pair| { ((pair[1].baseline - pair[0].baseline) - 20.0).abs() < 0.001 })
            );
        });
    }

    #[test]
    fn metadata_width_counts_utf16_like_dart() {
        let summary = format!("{}🧪", "a".repeat(142));
        assert_eq!(summary.encode_utf16().count(), 144);
        assert!((desktop_metadata_width(&summary) - 268.0).abs() < f32::EPSILON);
    }

    #[test]
    fn status_failure_is_visible_but_bounded_to_the_lower_left() {
        let mut surface = surfaces::raster_n32_premul((1280, 800)).unwrap();
        surface.canvas().clear(color(INK_0));
        paint_status(
            surface.canvas(),
            1280.0,
            800.0,
            1.0,
            "Graph unavailable; no research is shown or writable.",
            false,
        );
        let pixels = read_image(&surface.image_snapshot());
        let background = &pixels[0..4];
        let changed = pixels
            .chunks_exact(4)
            .filter(|pixel| *pixel != background)
            .count();
        assert!((1_000..60_000).contains(&changed));
        assert!(
            pixels[..1280 * 500 * 4]
                .chunks_exact(4)
                .all(|pixel| pixel == background)
        );
    }

    #[test]
    fn desktop_status_preserves_the_complete_searxng_reachability_diagnostic() {
        let message = "Research did not start; the offline canvas remains available. SearXNG endpoint is unreachable: error sending request for url (http://127.0.0.1:8889/search?q=not-news%20connectivity&format=json)";
        let message_width = 420.0 - 82.0;
        STATUS_TEXT.with(|resources| {
            let mut resources = resources.borrow_mut();
            let paragraph = resources.paragraph(message, message_width);
            let lines = paragraph.get_line_metrics();
            assert!(lines.len() <= 6, "status exceeded its bounded line budget");
            assert_eq!(
                lines.last().unwrap().end_excluding_whitespaces,
                message.len(),
                "ordinary readiness evidence was silently ellipsized"
            );
        });
    }

    #[test]
    fn rewrite_only_research_composer_is_bounded_and_exposes_preedit() {
        let mut surface = surfaces::raster_n32_premul((1_280, 800)).unwrap();
        surface.canvas().clear(color(INK_0));
        paint_research_prompt(
            surface.canvas(),
            1_280.0,
            800.0,
            1.0,
            "Compare primary-source claims",
            "研究",
        );
        let pixels = read_image(&surface.image_snapshot());
        let background = &pixels[0..4];
        assert!(
            pixels[1_280 * 240 * 4..]
                .chunks_exact(4)
                .all(|pixel| pixel == background)
        );
        assert!(
            pixels[1_280 * 28 * 4..1_280 * 146 * 4]
                .chunks_exact(4)
                .any(|pixel| pixel != background)
        );

        let long = "evidence ".repeat(100);
        let tail = prompt_tail(&long, "候補", "Ask a research question…");
        assert!(tail.starts_with('…'));
        assert!(tail.ends_with("〈候補〉"));
        assert!(tail.chars().count() <= 481);
    }

    #[test]
    fn credential_composer_has_its_own_language_and_single_line_header() {
        let mut surface = surfaces::raster_n32_premul((1_280, 800)).unwrap();
        surface.canvas().clear(color(INK_0));
        paint_credential_prompt(
            surface.canvas(),
            1_280.0,
            800.0,
            1.0,
            "GROQ TRANSCRIPTION",
            "••••",
            "••",
            "Paste or type the Groq API key…",
        );
        let pixels = read_image(&surface.image_snapshot());
        let background = &pixels[0..4];
        assert!(
            pixels[1_280 * 28 * 4..1_280 * 170 * 4]
                .chunks_exact(4)
                .any(|pixel| pixel != background)
        );
        assert!(
            pixels[1_280 * 240 * 4..]
                .chunks_exact(4)
                .all(|pixel| pixel == background)
        );
        assert_eq!(
            prompt_tail("", "", "Paste or type the Groq API key…"),
            "Paste or type the Groq API key…"
        );
        STATUS_TEXT.with(|resources| {
            let mut resources = resources.borrow_mut();
            assert!(resources.menu_header("GROQ TRANSCRIPTION").height() < 20.0);
            assert!(
                resources
                    .prompt_instruction(
                        "ENTER STORE IN OS VAULT  ·  ESC BACK  ·  CTRL+V PASTE",
                        648.0,
                    )
                    .height()
                    < 20.0
            );
        });
    }

    #[test]
    fn curation_menu_hits_exact_rows_without_leaking_into_its_heading() {
        let anchor = WorldPoint { x: 120.0, y: 100.0 };
        let items = vec!["ONE".into(), "TWO".into(), "THREE".into()];
        let hit = |x, y| {
            hit_curation_menu(
                WorldPoint { x, y },
                1_280.0,
                800.0,
                1.0,
                anchor,
                &items,
                0.0,
            )
        };
        assert_eq!(hit(140.0, 130.0), None);
        assert_eq!(hit(140.0, 150.0), Some(0));
        assert_eq!(hit(140.0, 198.0), Some(1));
        assert_eq!(hit(140.0, 246.0), Some(2));
        assert_eq!(hit(500.0, 198.0), None);
        STATUS_TEXT.with(|resources| {
            assert!(
                resources
                    .borrow_mut()
                    .menu_header("DETACH EXACT RELATIONSHIP")
                    .height()
                    < 20.0,
                "curation headings must remain a single line"
            );
        });
    }

    #[test]
    fn measured_menu_rows_scroll_and_reveal_without_crossing_the_viewport() {
        let anchor = WorldPoint { x: 120.0, y: 72.0 };
        let items = vec![
            "HERMES  ·  EXECUTABLE PRESENT  ·  ACP CHECK ON RESEARCH".into(),
            "EXA DISCOVERY  ·  OS VAULT".into(),
            "SEARXNG FRONTIER  ·  APP SETTINGS".into(),
            "BROWSE  ·  EXECUTABLE PRESENT  ·  BROWSER UNVERIFIED".into(),
            "CURL  ·  EXECUTABLE PRESENT  ·  VERSION CHECK ON RESEARCH".into(),
            "COMPLETE ERASE  ·  GRAPH, SETTINGS, VAULT, OWNED PROFILE".into(),
        ];
        let initial = menu_layout(640.0, 300.0, anchor, &items, 0.0).unwrap();
        assert!(initial.rows[0].height() > 48.0);
        assert!(initial.scroll_max > 0.0);
        assert!(initial.bounds.bottom <= 288.0);

        let revealed =
            curation_menu_reveal_offset(640.0, 300.0, 1.0, anchor, &items, items.len() - 1, 0.0);
        assert!(revealed > 0.0);
        let layout = menu_layout(640.0, 300.0, anchor, &items, revealed).unwrap();
        let last = layout.rows.last().unwrap();
        let point = WorldPoint {
            x: f64::from(last.center_x()),
            y: f64::from(last.center_y()),
        };
        assert_eq!(
            hit_curation_menu(point, 640.0, 300.0, 1.0, anchor, &items, revealed),
            Some(items.len() - 1)
        );
    }

    #[test]
    fn menu_selection_moves_a_visible_affordance_between_exact_rows() {
        let items = vec![
            "HERMES INFERENCE  ·  OPEN SETTINGS".to_owned(),
            "GROQ VOICE  ·  NOT CONFIGURED".to_owned(),
        ];
        let render = |selected| {
            let mut surface = surfaces::raster_n32_premul((640, 420)).unwrap();
            surface.canvas().clear(color(INK_0));
            paint_curation_menu(
                surface.canvas(),
                640.0,
                420.0,
                1.0,
                WorldPoint { x: 120.0, y: 80.0 },
                CurationMenu {
                    title: "CONNECTIONS  ·  CTRL+,",
                    items: &items,
                    selected: Some(selected),
                    scroll_offset: 0.0,
                },
            );
            read_image_size(&surface.image_snapshot(), (640, 420))
        };
        let first = render(0);
        let second = render(1);
        let row = |pixels: &[u8], top: usize| {
            (top..top + 42)
                .flat_map(|y| {
                    let start = (y * 640 + 126) * 4;
                    let end = (y * 640 + 454) * 4;
                    &pixels[start..end]
                })
                .copied()
                .collect::<Vec<_>>()
        };
        assert_ne!(row(&first, 125), row(&second, 125));
        assert_ne!(row(&first, 173), row(&second, 173));
    }

    #[test]
    fn status_panel_stays_within_flutter_crop_budget() {
        const FLUTTER: &[u8] =
            include_bytes!("../../../fixtures/reference-raster/full-screen-status-1280x800.png");
        const FLUTTER_BASE: &[u8] =
            include_bytes!("../../../fixtures/reference-raster/full-screen-closed-1280x800.png");
        let flutter_status = read_image(&Image::from_encoded(Data::new_copy(FLUTTER)).unwrap());
        let flutter_base = read_image(&Image::from_encoded(Data::new_copy(FLUTTER_BASE)).unwrap());
        let rust_status = paint_over(&flutter_base, |canvas| {
            paint_status(
                canvas,
                1280.0,
                800.0,
                1.0,
                "Graph unavailable; no research is shown or writable.",
                false,
            );
        });
        let metrics = residual_crop_metrics(
            &flutter_base,
            &flutter_status,
            &flutter_base,
            &rust_status,
            &[(0, 680, 500, 120)],
        );
        let fill = residual_crop_metrics(
            &flutter_base,
            &flutter_status,
            &flutter_base,
            &rust_status,
            &[(200, 760, 230, 8)],
        );
        let shadow = residual_crop_metrics(
            &flutter_base,
            &flutter_status,
            &flutter_base,
            &rust_status,
            &[
                (0, 690, 22, 110),
                (442, 690, 58, 110),
                (22, 690, 420, 32),
                (22, 772, 420, 28),
            ],
        );
        let text = residual_crop_metrics(
            &flutter_base,
            &flutter_status,
            &flutter_base,
            &rust_status,
            &[(30, 730, 390, 35)],
        );
        assert!(metrics.1 <= 3.20, "Flutter/Rust status drift {metrics:?}");
        assert_eq!((fill.0, fill.2), (0, 0));
        assert!(fill.1.abs() < f64::EPSILON);
        assert!(
            shadow.1 <= 1.05 && shadow.2 <= 30,
            "Flutter/Rust status geometry/shadow drift {shadow:?}"
        );
        assert!(
            text.1 <= 10.5,
            "Flutter/Rust localized status text drift {text:?}"
        );
        STATUS_TEXT.with(|resources| {
            let mut resources = resources.borrow_mut();
            let paragraph = resources.paragraph(
                "Graph unavailable; no research is shown or writable.",
                338.0,
            );
            let ranges: Vec<_> = paragraph
                .get_line_metrics()
                .iter()
                .map(|line| (line.start_index, line.end_excluding_whitespaces))
                .collect();
            assert_eq!(ranges, [(0, 42), (43, 52)]);
        });
    }

    #[test]
    fn activity_drawer_matches_flutter_geometry_and_raster_budget() {
        const FLUTTER: &[u8] =
            include_bytes!("../../../fixtures/reference-raster/full-screen-activity-1280x800.png");
        const FLUTTER_BASE: &[u8] =
            include_bytes!("../../../fixtures/reference-raster/full-screen-closed-1280x800.png");
        let flutter = read_image(&Image::from_encoded(Data::new_copy(FLUTTER)).unwrap());
        let base = read_image(&Image::from_encoded(Data::new_copy(FLUTTER_BASE)).unwrap());
        let messages = vec![
            "Hermes research started.".to_owned(),
            "Searching official primary sources.".to_owned(),
            "Accepted finding: Rust language release".to_owned(),
        ];
        let rust = paint_over(&base, |canvas| {
            paint_activity_drawer(canvas, 1280.0, 800.0, 1.0, &messages, true, 1.0);
        });
        let metrics = residual_crop_metrics(&base, &flutter, &base, &rust, &[(810, 50, 470, 650)]);
        assert!(
            metrics.0 <= 26_000 && metrics.1 <= 1.60 && metrics.2 <= 230,
            "Flutter/Rust activity-drawer drift {metrics:?}"
        );
    }

    #[test]
    fn activity_hit_regions_follow_the_animated_button_at_fractional_dpr() {
        let dpr = 1.5;
        assert!(hit_activity_toggle(
            WorldPoint {
                x: 1242.0 * dpr,
                y: 96.0 * dpr,
            },
            1280.0 * dpr,
            dpr,
            0.0,
        ));
        assert!(hit_activity_toggle(
            WorldPoint {
                x: 862.0 * dpr,
                y: 96.0 * dpr,
            },
            1280.0 * dpr,
            dpr,
            1.0,
        ));
        assert!(hit_activity_surface(
            WorldPoint {
                x: 1100.0 * dpr,
                y: 300.0 * dpr,
            },
            1280.0 * dpr,
            800.0 * dpr,
            dpr,
            1.0,
        ));
        assert!(!hit_activity_surface(
            WorldPoint {
                x: 600.0 * dpr,
                y: 300.0 * dpr,
            },
            1280.0 * dpr,
            800.0 * dpr,
            dpr,
            1.0,
        ));
    }

    fn residual_crop_metrics(
        expected_base: &[u8],
        expected_chrome: &[u8],
        actual_base: &[u8],
        actual_chrome: &[u8],
        regions: &[(usize, usize, usize, usize)],
    ) -> (usize, f64, u16) {
        residual_crop_metrics_width(
            expected_base,
            expected_chrome,
            actual_base,
            actual_chrome,
            regions,
            1280,
        )
    }

    fn residual_crop_metrics_width(
        expected_base: &[u8],
        expected_chrome: &[u8],
        actual_base: &[u8],
        actual_chrome: &[u8],
        regions: &[(usize, usize, usize, usize)],
        image_width: usize,
    ) -> (usize, f64, u16) {
        let mut changed = 0usize;
        let mut delta = 0u64;
        let mut maximum = 0u16;
        let mut channels = 0usize;
        for &(left, top, width, height) in regions {
            for y in top..top + height {
                for x in left..left + width {
                    let offset = (y * image_width + x) * 4;
                    let mut pixel_changed = false;
                    for channel in 0..4 {
                        let expected = i16::from(expected_chrome[offset + channel])
                            - i16::from(expected_base[offset + channel]);
                        let actual = i16::from(actual_chrome[offset + channel])
                            - i16::from(actual_base[offset + channel]);
                        let difference = expected.abs_diff(actual);
                        pixel_changed |= difference != 0;
                        delta += u64::from(difference);
                        maximum = maximum.max(difference);
                        channels += 1;
                    }
                    changed += usize::from(pixel_changed);
                }
            }
        }
        (changed, ratio(delta, channels), maximum)
    }

    #[allow(clippy::cast_precision_loss)]
    fn ratio(numerator: u64, denominator: usize) -> f64 {
        numerator as f64 / denominator as f64
    }

    fn read_image(image: &Image) -> Vec<u8> {
        read_image_size(image, (1280, 800))
    }

    fn read_image_size(image: &Image, size: (i32, i32)) -> Vec<u8> {
        assert_eq!((image.width(), image.height()), size);
        let info = ImageInfo::new_n32_premul(size, None);
        let row_bytes = info.min_row_bytes();
        let mut pixels = vec![0; info.compute_byte_size(row_bytes)];
        assert!(image.read_pixels(&info, &mut pixels, row_bytes, (0, 0), CachingHint::Disallow,));
        pixels
    }
}
