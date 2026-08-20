//! Browser renderer for Not News.
//!
//! The JavaScript host gives Emscripten the same WebGL2 context that Three.js
//! uses. Skia wraps the framebuffer backing a `THREE.WebGLRenderTarget`, so a
//! frame is sampled by the physical CRT mesh without crossing the CPU or being
//! copied through a canvas.

use std::{ffi::CString, os::raw::c_char, os::raw::c_void, time::Instant};

use not_news_domain::{GraphSnapshot, Point as WorldPoint};
use not_news_interaction::{CanvasInteraction, InteractionEffect};
use not_news_renderer::{
    RecordOrbState, SceneAnimation, SceneState, paint_background, paint_fixed_chrome, paint_graph,
    paint_grid, resolved_positions,
};
use skia_safe::{
    ColorType, Surface,
    gpu::{self, DirectContext, Protected, SurfaceOrigin, gl::FramebufferInfo},
};

unsafe extern "C" {
    fn emscripten_GetProcAddress(name: *const c_char) -> *const c_void;
    fn emscripten_get_now() -> f64;
}

struct GpuState {
    context: DirectContext,
    framebuffer: FramebufferInfo,
}

pub struct State {
    gpu: GpuState,
    surface: Surface,
    width: i32,
    height: i32,
    graph: GraphSnapshot,
    interaction: CanvasInteraction,
    pending_url: Option<CString>,
    needs_frame: bool,
    last_cpu_ms: f64,
}

fn init_gl() {
    gl::load_with(|symbol| {
        let symbol = CString::new(symbol).expect("OpenGL symbol names contain no NUL bytes");
        unsafe { emscripten_GetProcAddress(symbol.as_ptr()) }
    });
}

fn create_gpu_state() -> GpuState {
    let interface = skia_safe::gpu::gl::Interface::new_native()
        .expect("Emscripten did not expose a Skia-compatible WebGL2 interface");
    let context = skia_safe::gpu::direct_contexts::make_gl(interface, None)
        .expect("Skia could not create its Ganesh WebGL context");
    let mut framebuffer = 0;
    unsafe { gl::GetIntegerv(gl::FRAMEBUFFER_BINDING, &mut framebuffer) };
    assert!(
        framebuffer > 0,
        "Not News must initialize on Three's registered render target"
    );
    GpuState {
        context,
        framebuffer: FramebufferInfo {
            fboid: u32::try_from(framebuffer).expect("framebuffer ID must be positive"),
            format: skia_safe::gpu::gl::Format::RGBA8.into(),
            protected: Protected::No,
        },
    }
}

fn create_surface(gpu: &mut GpuState, width: i32, height: i32) -> Surface {
    assert!(width > 0 && height > 0);
    let target = gpu::backend_render_targets::make_gl((width, height), 1, 8, gpu.framebuffer);
    gpu::surfaces::wrap_backend_render_target(
        &mut gpu.context,
        &target,
        SurfaceOrigin::BottomLeft,
        ColorType::RGBA8888,
        None,
        None,
    )
    .expect("Skia could not wrap Three's Not News render target")
}

fn embedded_graph() -> GraphSnapshot {
    let graph: GraphSnapshot =
        serde_json::from_str(include_str!(concat!(env!("OUT_DIR"), "/graph.json")))
            .expect("the build-time validated graph snapshot must deserialize");
    graph
        .validate()
        .expect("the embedded graph snapshot must remain valid");
    graph
}

impl State {
    fn new(mut gpu: GpuState, width: i32, height: i32) -> Self {
        let graph = embedded_graph();
        let mut interaction = CanvasInteraction::new(resolved_positions(&graph));
        interaction.resize(
            u32::try_from(width).expect("surface width must be positive"),
            u32::try_from(height).expect("surface height must be positive"),
        );
        let surface = create_surface(&mut gpu, width, height);
        Self {
            gpu,
            surface,
            width,
            height,
            graph,
            interaction,
            pending_url: None,
            needs_frame: true,
            last_cpu_ms: 0.0,
        }
    }

    fn paint(&mut self, _now_ms: f64) {
        let started = unsafe { emscripten_get_now() };
        self.gpu.context.reset(None);
        let now = Instant::now();
        let frame = self.interaction.frame(&self.graph, now);
        let viewport = self.interaction.viewport();
        let canvas = self.surface.canvas();
        let width = self.width as f32;
        let height = self.height as f32;
        paint_background(canvas, width, height);
        paint_grid(canvas, width, height, viewport);
        paint_graph(
            canvas,
            width,
            height,
            viewport,
            &self.graph,
            &frame.positions,
            SceneState {
                animation: SceneAnimation {
                    bridge_flow: frame.bridge_flow,
                },
                bridge_event: frame.bridge_event.as_ref(),
                expanded_event: frame.expanded_event.as_ref(),
                expansion_progress: frame.expansion_progress,
                collapsing_event: frame.collapsing_event.as_ref(),
                collapse_progress: frame.collapse_progress,
            },
        );
        paint_fixed_chrome(
            canvas,
            width,
            height,
            1.0,
            viewport.zoom,
            RecordOrbState::Idle,
        );
        self.gpu
            .context
            .flush_and_submit_surface(&mut self.surface, None);
        self.needs_frame = self.interaction.next_deadline(now).is_some();
        self.last_cpu_ms = unsafe { emscripten_get_now() } - started;
    }

    fn resize(&mut self, width: i32, height: i32) {
        self.width = width;
        self.height = height;
        self.interaction.resize(
            u32::try_from(width).expect("surface width must be positive"),
            u32::try_from(height).expect("surface height must be positive"),
        );
        self.surface = create_surface(&mut self.gpu, width, height);
    }

    fn reset_interaction(&mut self) {
        self.interaction = CanvasInteraction::new(resolved_positions(&self.graph));
        self.interaction.resize(
            u32::try_from(self.width).expect("surface width must be positive"),
            u32::try_from(self.height).expect("surface height must be positive"),
        );
        self.needs_frame = true;
    }

    fn benchmark_step(&mut self, frame_index: u32) {
        // This is the same cursor trajectory and initial press used by the
        // native `--performance-check`, so browser and desktop exercise the
        // same hit-testing, interaction, layout, and paint path.
        let cycle = frame_index % 472;
        let offset = if cycle < 236 { cycle } else { 472 - cycle };
        let now = Instant::now();
        self.interaction.cursor_moved(
            WorldPoint {
                x: 40.0 + f64::from(offset) * 5.0,
                y: 400.0,
            },
            &self.graph,
            now,
        );
        if frame_index == 0 {
            self.interaction.pointer_down(&self.graph, now);
        }
        self.needs_frame = true;
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn not_news_init(width: i32, height: i32) -> Box<State> {
    init_gl();
    Box::new(State::new(create_gpu_state(), width, height))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn not_news_render(state: *mut State, now_ms: f64) {
    let state = unsafe { state.as_mut() }.expect("invalid Not News state pointer");
    state.paint(now_ms);
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn not_news_resize(state: *mut State, width: i32, height: i32) {
    let state = unsafe { state.as_mut() }.expect("invalid Not News state pointer");
    state.resize(width, height);
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn not_news_pointer_down(state: *mut State, x: f64, y: f64) {
    let state = unsafe { state.as_mut() }.expect("invalid Not News state pointer");
    let now = Instant::now();
    state
        .interaction
        .cursor_moved(WorldPoint { x, y }, &state.graph, now);
    state.interaction.pointer_down(&state.graph, now);
    state.needs_frame = true;
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn not_news_pointer_move(state: *mut State, x: f64, y: f64) {
    let state = unsafe { state.as_mut() }.expect("invalid Not News state pointer");
    state.needs_frame |=
        state
            .interaction
            .cursor_moved(WorldPoint { x, y }, &state.graph, Instant::now());
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn not_news_pointer_leave(state: *mut State) {
    let state = unsafe { state.as_mut() }.expect("invalid Not News state pointer");
    state.needs_frame |= state.interaction.cursor_left(Instant::now());
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn not_news_pointer_up(state: *mut State) -> *const c_char {
    let state = unsafe { state.as_mut() }.expect("invalid Not News state pointer");
    state.pending_url = None;
    match state.interaction.pointer_up(&state.graph, Instant::now()) {
        InteractionEffect::Move(command) => {
            state
                .graph
                .apply_move(&command)
                .expect("the browser interaction emitted a valid placement move");
            state.interaction.placement_committed(&state.graph);
            state.needs_frame = true;
        }
        InteractionEffect::OpenUrl(url) => {
            state.pending_url = CString::new(url).ok();
        }
        InteractionEffect::PixelsChanged => state.needs_frame = true,
        InteractionEffect::Unchanged => {}
    }
    state
        .pending_url
        .as_ref()
        .map_or(std::ptr::null(), |url| url.as_ptr())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn not_news_wheel(state: *mut State, delta_y: f64) {
    let state = unsafe { state.as_mut() }.expect("invalid Not News state pointer");
    state.needs_frame |= state.interaction.scroll(-delta_y);
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn not_news_needs_frame(state: *const State) -> i32 {
    i32::from(
        unsafe { state.as_ref() }
            .expect("invalid Not News state pointer")
            .needs_frame,
    )
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn not_news_last_cpu_ms(state: *const State) -> f64 {
    unsafe { state.as_ref() }
        .expect("invalid Not News state pointer")
        .last_cpu_ms
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn not_news_benchmark_begin(state: *mut State) {
    let state = unsafe { state.as_mut() }.expect("invalid Not News state pointer");
    state.reset_interaction();
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn not_news_benchmark_step(state: *mut State, frame_index: u32) {
    let state = unsafe { state.as_mut() }.expect("invalid Not News state pointer");
    state.benchmark_step(frame_index);
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn not_news_benchmark_end(state: *mut State) {
    let state = unsafe { state.as_mut() }.expect("invalid Not News state pointer");
    state.reset_interaction();
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn not_news_destroy(state: *mut State) {
    if !state.is_null() {
        drop(unsafe { Box::from_raw(state) });
    }
}

fn main() {}
