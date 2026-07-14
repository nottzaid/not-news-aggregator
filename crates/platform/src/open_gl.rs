use std::{ffi::CString, num::NonZeroU32, sync::Arc, time::Instant};

use gl::types::GLint;
use glutin::{
    config::{Config, ConfigTemplateBuilder, GlConfig},
    context::{ContextApi, ContextAttributesBuilder, PossiblyCurrentContext},
    display::{GetGlDisplay, GlDisplay},
    error::ErrorKind,
    prelude::{GlSurface, NotCurrentGlContext, PossiblyCurrentGlContext},
    surface::{Surface as GlutinSurface, SurfaceAttributesBuilder, WindowSurface},
};
use glutin_winit::DisplayBuilder;
use raw_window_handle::HasWindowHandle;
use skia_safe::{
    AlphaType, ColorType, ImageInfo, Surface,
    gpu::{self, SurfaceOrigin, backend_render_targets, gl::FramebufferInfo},
};
use thiserror::Error;
use winit::{
    application::ApplicationHandler,
    dpi::LogicalSize,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    window::{Icon, Window, WindowAttributes, WindowId},
};

use crate::{FrameInfo, FrameSchedule, PlatformApplication};

#[derive(Clone, Debug)]
pub struct WindowOptions {
    pub title: String,
    pub logical_width: f64,
    pub logical_height: f64,
    pub visible: bool,
    /// Forces Skia's CPU raster backend; used by compatibility diagnostics.
    pub force_software: bool,
}

impl Default for WindowOptions {
    fn default() -> Self {
        Self {
            title: "Not News".to_owned(),
            logical_width: 1_280.0,
            logical_height: 800.0,
            visible: true,
            force_software: false,
        }
    }
}

#[derive(Debug, Error)]
pub enum PlatformError {
    #[error("could not create the native event loop: {0}")]
    EventLoop(String),
    #[error("could not choose a native OpenGL configuration: {0}")]
    Display(String),
    #[error("the embedded launcher icon is invalid: {0}")]
    Icon(String),
    #[error("the display builder returned no native window")]
    MissingWindow,
    #[error("could not inspect the native window handle: {0}")]
    WindowHandle(String),
    #[error("could not create an OpenGL context: {0}")]
    Context(String),
    #[error("could not create the OpenGL window surface: {0}")]
    WindowSurface(String),
    #[error("could not make the OpenGL context current: {0}")]
    CurrentContext(String),
    #[error("Skia could not load the platform OpenGL interface")]
    SkiaInterface,
    #[error("Skia could not create a GPU direct context")]
    SkiaContext,
    #[error("Skia could not wrap the native framebuffer")]
    SkiaSurface,
    #[error("could not present the native framebuffer: {0}")]
    Present(String),
    #[error(
        "neither native OpenGL nor software presentation could start; OpenGL: {gpu}; software: {software}"
    )]
    RendererUnavailable { gpu: String, software: String },
    #[error("the native event loop failed: {0}")]
    Run(String),
}

/// Runs the application on the current thread until its sole window closes.
///
/// # Errors
///
/// Returns the first native window, GPU-context, surface, presentation, or
/// event-loop error. Zero-area resize events are held without painting.
///
/// # Panics
///
/// Panics only if glutin violates its configuration-iterator contract and
/// supplies no framebuffer configuration to its mandatory chooser callback.
pub fn run<A: PlatformApplication>(
    application: A,
    options: WindowOptions,
) -> Result<(), PlatformError> {
    let event_loop =
        EventLoop::new().map_err(|error| PlatformError::EventLoop(error.to_string()))?;
    let render_on_resume = !options.visible;
    let icon = Icon::from_rgba(
        include_bytes!("../../../assets/icon/not-news-64.rgba").to_vec(),
        64,
        64,
    )
    .map_err(|error| PlatformError::Icon(error.to_string()))?;
    let attributes = WindowAttributes::default()
        .with_title(options.title)
        .with_inner_size(LogicalSize::new(
            options.logical_width,
            options.logical_height,
        ))
        .with_visible(options.visible)
        .with_window_icon(Some(icon));
    #[cfg(target_os = "linux")]
    let attributes = winit::platform::wayland::WindowAttributesExtWayland::with_name(
        attributes, "not-news", "not-news",
    );
    #[cfg(target_os = "linux")]
    let attributes =
        winit::platform::x11::WindowAttributesExtX11::with_name(attributes, "not-news", "not-news");
    let template = ConfigTemplateBuilder::new().with_alpha_size(8);
    let display_builder = DisplayBuilder::new().with_window_attributes(Some(attributes));
    let (window, config) = display_builder
        .build(&event_loop, template, choose_config)
        .map_err(|error| PlatformError::Display(error.to_string()))?;
    let window = window.ok_or(PlatformError::MissingWindow)?;
    let native = NativeSurface::new(window, config, options.force_software)?;
    native.window.request_redraw();

    let mut runner = Runner {
        application,
        native,
        deferred_error: None,
        render_on_resume,
        recovery: RecoveryBudget::default(),
        ime_allowed: false,
    };
    event_loop
        .run_app(&mut runner)
        .map_err(|error| PlatformError::Run(error.to_string()))?;
    if let Some(error) = runner.deferred_error {
        return Err(error);
    }
    Ok(())
}

fn choose_config(configs: Box<dyn Iterator<Item = Config> + '_>) -> Config {
    configs
        .reduce(|current, candidate| {
            let gains_transparency = candidate.supports_transparency().unwrap_or(false)
                && !current.supports_transparency().unwrap_or(false);
            if gains_transparency || candidate.num_samples() < current.num_samples() {
                candidate
            } else {
                current
            }
        })
        .expect("glutin supplied an empty configuration iterator")
}

struct NativeSurface {
    backend: Option<NativeBackend>,
    config: Config,
    window: Arc<Window>,
}

enum NativeBackend {
    Gpu(GpuSurface),
    Software(SoftwareSurface),
}

struct GpuSurface {
    skia_surface: Surface,
    gl_surface: GlutinSurface<WindowSurface>,
    skia_context: skia_safe::gpu::DirectContext,
    gl_context: PossiblyCurrentContext,
    framebuffer: FramebufferInfo,
    samples: usize,
    stencil_bits: usize,
}

struct SoftwareSurface {
    skia_surface: Surface,
    presentation: softbuffer::Surface<Arc<Window>, Arc<Window>>,
}

enum PresentFailure {
    Gpu(glutin::error::Error),
    Software(String),
}

impl NativeSurface {
    fn new(window: Window, config: Config, force_software: bool) -> Result<Self, PlatformError> {
        let window = Arc::new(window);
        let gpu = if force_software {
            Err(PlatformError::SkiaInterface)
        } else {
            GpuSurface::new(&window, &config)
        };
        let backend = match gpu {
            Ok(gpu) => NativeBackend::Gpu(gpu),
            Err(gpu) => {
                NativeBackend::Software(SoftwareSurface::new(&window).map_err(|software| {
                    PlatformError::RendererUnavailable {
                        gpu: gpu.to_string(),
                        software,
                    }
                })?)
            }
        };
        Ok(Self {
            backend: Some(backend),
            config,
            window,
        })
    }

    fn backend(&mut self) -> &mut NativeBackend {
        self.backend
            .as_mut()
            .expect("a native surface is never used between teardown and replacement")
    }

    fn canvas(&mut self) -> &skia_safe::Canvas {
        match self.backend() {
            NativeBackend::Gpu(gpu) => gpu.skia_surface.canvas(),
            NativeBackend::Software(software) => software.skia_surface.canvas(),
        }
    }

    fn resize(&mut self, width: u32, height: u32) -> Result<(), PlatformError> {
        let Self {
            backend, window, ..
        } = self;
        match backend
            .as_mut()
            .expect("a native surface is never used between teardown and replacement")
        {
            NativeBackend::Gpu(gpu) => gpu.resize(window, width, height),
            NativeBackend::Software(software) => software.resize(width, height),
        }
    }

    fn present(&mut self) -> Result<(), PresentFailure> {
        match self.backend() {
            NativeBackend::Gpu(gpu) => gpu.present().map_err(PresentFailure::Gpu),
            NativeBackend::Software(software) => {
                software.present().map_err(PresentFailure::Software)
            }
        }
    }

    fn rebuild(&mut self) -> Result<(), PlatformError> {
        drop(self.backend.take());
        self.backend =
            Some(match GpuSurface::new(&self.window, &self.config) {
                Ok(gpu) => NativeBackend::Gpu(gpu),
                Err(gpu) => NativeBackend::Software(SoftwareSurface::new(&self.window).map_err(
                    |software| PlatformError::RendererUnavailable {
                        gpu: gpu.to_string(),
                        software,
                    },
                )?),
            });
        Ok(())
    }
}

impl SoftwareSurface {
    fn new(window: &Arc<Window>) -> Result<Self, String> {
        let context =
            softbuffer::Context::new(window.clone()).map_err(|error| error.to_string())?;
        let mut presentation = softbuffer::Surface::new(&context, window.clone())
            .map_err(|error| error.to_string())?;
        let size = window.inner_size();
        let width = i32::try_from(size.width)
            .map_err(|_| "software surface width exceeds Skia limits".to_owned())?;
        let height = i32::try_from(size.height)
            .map_err(|_| "software surface height exceeds Skia limits".to_owned())?;
        presentation
            .resize(nonzero_extent(size.width), nonzero_extent(size.height))
            .map_err(|error| error.to_string())?;
        let skia_surface = skia_safe::surfaces::raster_n32_premul((width, height))
            .ok_or_else(|| "Skia could not create a raster fallback surface".to_owned())?;
        Ok(Self {
            skia_surface,
            presentation,
        })
    }

    fn resize(&mut self, width: u32, height: u32) -> Result<(), PlatformError> {
        self.presentation
            .resize(nonzero_extent(width), nonzero_extent(height))
            .map_err(|error| PlatformError::WindowSurface(error.to_string()))?;
        if width > 0 && height > 0 {
            let skia_width = i32::try_from(width).map_err(|_| PlatformError::SkiaSurface)?;
            let skia_height = i32::try_from(height).map_err(|_| PlatformError::SkiaSurface)?;
            self.skia_surface = skia_safe::surfaces::raster_n32_premul((skia_width, skia_height))
                .ok_or(PlatformError::SkiaSurface)?;
        }
        Ok(())
    }

    fn present(&mut self) -> Result<(), String> {
        let width = self.skia_surface.width();
        let height = self.skia_surface.height();
        let info = ImageInfo::new(
            (width, height),
            ColorType::RGBA8888,
            AlphaType::Premul,
            None,
        );
        let row_bytes = info.min_row_bytes();
        let mut pixels = vec![0_u8; info.compute_byte_size(row_bytes)];
        if !self.skia_surface.image_snapshot().read_pixels(
            &info,
            &mut pixels,
            row_bytes,
            (0, 0),
            skia_safe::image::CachingHint::Disallow,
        ) {
            return Err("Skia could not read its raster fallback frame".into());
        }
        let mut buffer = self
            .presentation
            .buffer_mut()
            .map_err(|error| error.to_string())?;
        for (destination, rgba) in buffer.iter_mut().zip(pixels.chunks_exact(4)) {
            *destination = rgba_to_xrgb(rgba);
        }
        buffer.present().map_err(|error| error.to_string())
    }
}

fn rgba_to_xrgb(rgba: &[u8]) -> u32 {
    u32::from(rgba[0]) << 16 | u32::from(rgba[1]) << 8 | u32::from(rgba[2])
}

impl GpuSurface {
    fn new(window: &Window, config: &Config) -> Result<Self, PlatformError> {
        let raw_window_handle = window
            .window_handle()
            .map_err(|error| PlatformError::WindowHandle(error.to_string()))?
            .as_raw();
        let context_attributes = ContextAttributesBuilder::new().build(Some(raw_window_handle));
        let fallback_attributes = ContextAttributesBuilder::new()
            .with_context_api(ContextApi::Gles(None))
            .build(Some(raw_window_handle));
        let not_current = create_context(config, &context_attributes, &fallback_attributes)?;

        let size = window.inner_size();
        let attributes = SurfaceAttributesBuilder::<WindowSurface>::new().build(
            raw_window_handle,
            nonzero_extent(size.width),
            nonzero_extent(size.height),
        );
        let gl_surface = create_window_surface(config, &attributes)?;
        let gl_context = not_current
            .make_current(&gl_surface)
            .map_err(|error| PlatformError::CurrentContext(error.to_string()))?;

        gl::load_with(|name| load_symbol(config, name));
        let interface = skia_safe::gpu::gl::Interface::new_load_with(|name| {
            if name == "eglGetCurrentDisplay" {
                return std::ptr::null();
            }
            load_symbol(config, name)
        })
        .ok_or(PlatformError::SkiaInterface)?;
        let mut skia_context = skia_safe::gpu::direct_contexts::make_gl(interface, None)
            .ok_or(PlatformError::SkiaContext)?;
        let framebuffer = framebuffer_info();
        let samples = usize::from(config.num_samples());
        let stencil_bits = usize::from(config.stencil_size());
        let skia_surface = wrap_surface(
            window,
            framebuffer,
            &mut skia_context,
            samples,
            stencil_bits,
        )?;

        Ok(Self {
            skia_surface,
            gl_surface,
            skia_context,
            gl_context,
            framebuffer,
            samples,
            stencil_bits,
        })
    }

    fn resize(&mut self, window: &Window, width: u32, height: u32) -> Result<(), PlatformError> {
        self.gl_surface.resize(
            &self.gl_context,
            nonzero_extent(width),
            nonzero_extent(height),
        );
        if width > 0 && height > 0 {
            self.skia_surface = wrap_surface(
                window,
                self.framebuffer,
                &mut self.skia_context,
                self.samples,
                self.stencil_bits,
            )?;
        }
        Ok(())
    }

    fn present(&mut self) -> Result<(), glutin::error::Error> {
        self.skia_context.flush_and_submit();
        self.gl_surface.swap_buffers(&self.gl_context)
    }
}

impl Drop for GpuSurface {
    fn drop(&mut self) {
        self.skia_context.release_resources_and_abandon();
        let _ = self.gl_context.make_not_current_in_place();
    }
}

#[derive(Default)]
struct RecoveryBudget {
    consecutive: u8,
}

impl RecoveryBudget {
    const LIMIT: u8 = 2;

    fn presented(&mut self) {
        self.consecutive = 0;
    }

    fn permits(&mut self, kind: ErrorKind) -> bool {
        if !is_recoverable_surface_error(kind) || self.consecutive >= Self::LIMIT {
            return false;
        }
        self.consecutive += 1;
        true
    }
}

fn is_recoverable_surface_error(kind: ErrorKind) -> bool {
    matches!(
        kind,
        ErrorKind::BadContext
            | ErrorKind::BadContextState
            | ErrorKind::BadCurrentSurface
            | ErrorKind::BadSurface
            | ErrorKind::ContextLost
    )
}

struct Runner<A> {
    application: A,
    native: NativeSurface,
    deferred_error: Option<PlatformError>,
    render_on_resume: bool,
    recovery: RecoveryBudget,
    ime_allowed: bool,
}

impl<A: PlatformApplication> ApplicationHandler for Runner<A> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.render_on_resume {
            self.render_on_resume = false;
            if let Err(error) = self.redraw(event_loop) {
                self.deferred_error = Some(error);
                event_loop.exit();
            }
        } else {
            self.native.window.request_redraw();
        }
    }

    fn new_events(&mut self, _event_loop: &ActiveEventLoop, cause: winit::event::StartCause) {
        if matches!(cause, winit::event::StartCause::ResumeTimeReached { .. }) {
            self.native.window.request_redraw();
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        if window_id != self.native.window.id() {
            return;
        }
        let result = match &event {
            WindowEvent::CloseRequested => {
                event_loop.exit();
                return;
            }
            WindowEvent::Resized(size) => {
                let resized = self.native.resize(size.width, size.height);
                if resized.is_ok() && self.application.window_event(&event) {
                    self.native.window.request_redraw();
                }
                resized
            }
            WindowEvent::RedrawRequested => self.redraw(event_loop),
            _ => {
                if self.application.window_event(&event) {
                    self.native.window.request_redraw();
                }
                Ok(())
            }
        };
        self.sync_text_input();
        if let Err(error) = result {
            self.deferred_error = Some(error);
            event_loop.exit();
        }
    }
}

impl<A: PlatformApplication> Runner<A> {
    fn sync_text_input(&mut self) {
        let allowed = self.application.text_input_active();
        if allowed == self.ime_allowed {
            return;
        }
        self.ime_allowed = allowed;
        self.native.window.set_ime_allowed(allowed);
        if allowed {
            let size = self.native.window.inner_size();
            self.native.window.set_ime_cursor_area(
                winit::dpi::PhysicalPosition::new(size.width / 2, 112),
                winit::dpi::PhysicalSize::new(1, 24),
            );
        }
    }

    fn redraw(&mut self, event_loop: &ActiveEventLoop) -> Result<(), PlatformError> {
        let size = self.native.window.inner_size();
        if size.width == 0 || size.height == 0 {
            event_loop.set_control_flow(ControlFlow::Wait);
            return Ok(());
        }
        let frame = FrameInfo {
            physical_width: size.width,
            physical_height: size.height,
            scale_factor: self.native.window.scale_factor(),
            now: Instant::now(),
        };
        let schedule = self.application.render(self.native.canvas(), frame);
        match self.native.present() {
            Ok(()) => self.recovery.presented(),
            Err(PresentFailure::Gpu(error)) if self.recovery.permits(error.error_kind()) => {
                self.native.rebuild()?;
                self.native.window.request_redraw();
                event_loop.set_control_flow(ControlFlow::Wait);
                return Ok(());
            }
            Err(PresentFailure::Gpu(error)) => {
                return Err(PlatformError::Present(error.to_string()));
            }
            Err(PresentFailure::Software(error)) => return Err(PlatformError::Present(error)),
        }
        match schedule {
            FrameSchedule::Wait => event_loop.set_control_flow(ControlFlow::Wait),
            FrameSchedule::RedrawAt(deadline) => {
                event_loop.set_control_flow(ControlFlow::WaitUntil(deadline));
            }
            FrameSchedule::Exit => event_loop.exit(),
        }
        Ok(())
    }
}

fn nonzero_extent(value: u32) -> NonZeroU32 {
    NonZeroU32::new(value.max(1)).expect("one is nonzero")
}

fn load_symbol(config: &Config, name: &str) -> *const std::ffi::c_void {
    let Ok(name) = CString::new(name) else {
        return std::ptr::null();
    };
    config.display().get_proc_address(name.as_c_str())
}

#[allow(unsafe_code)]
fn create_context(
    config: &Config,
    primary: &glutin::context::ContextAttributes,
    fallback: &glutin::context::ContextAttributes,
) -> Result<glutin::context::NotCurrentContext, PlatformError> {
    unsafe {
        config
            .display()
            .create_context(config, primary)
            .or_else(|_| config.display().create_context(config, fallback))
            .map_err(|error| PlatformError::Context(error.to_string()))
    }
}

#[allow(unsafe_code)]
fn create_window_surface(
    config: &Config,
    attributes: &glutin::surface::SurfaceAttributes<WindowSurface>,
) -> Result<GlutinSurface<WindowSurface>, PlatformError> {
    unsafe {
        config
            .display()
            .create_window_surface(config, attributes)
            .map_err(|error| PlatformError::WindowSurface(error.to_string()))
    }
}

#[allow(unsafe_code)]
fn framebuffer_info() -> FramebufferInfo {
    let mut framebuffer_id: GLint = 0;
    unsafe { gl::GetIntegerv(gl::FRAMEBUFFER_BINDING, &raw mut framebuffer_id) };
    FramebufferInfo {
        fboid: u32::try_from(framebuffer_id).expect("OpenGL framebuffer identifiers are unsigned"),
        format: skia_safe::gpu::gl::Format::RGBA8.into(),
        ..FramebufferInfo::default()
    }
}

fn wrap_surface(
    window: &Window,
    framebuffer: FramebufferInfo,
    context: &mut skia_safe::gpu::DirectContext,
    samples: usize,
    stencil_bits: usize,
) -> Result<Surface, PlatformError> {
    let size = window.inner_size();
    let dimensions = (
        i32::try_from(size.width).map_err(|_| PlatformError::SkiaSurface)?,
        i32::try_from(size.height).map_err(|_| PlatformError::SkiaSurface)?,
    );
    let target = backend_render_targets::make_gl(dimensions, samples, stencil_bits, framebuffer);
    gpu::surfaces::wrap_backend_render_target(
        context,
        &target,
        SurfaceOrigin::BottomLeft,
        ColorType::RGBA8888,
        None,
        None,
    )
    .ok_or(PlatformError::SkiaSurface)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_and_surface_loss_rebuild_twice_without_masking_fatal_errors() {
        let mut budget = RecoveryBudget::default();
        assert!(budget.permits(ErrorKind::ContextLost));
        assert!(budget.permits(ErrorKind::BadSurface));
        assert!(!budget.permits(ErrorKind::BadCurrentSurface));
        budget.presented();
        assert!(budget.permits(ErrorKind::BadContextState));
        assert!(!budget.permits(ErrorKind::OutOfMemory));
        assert!(!budget.permits(ErrorKind::BadDisplay));
    }

    #[test]
    fn software_presenter_converts_skia_rgba_to_softbuffer_xrgb() {
        assert_eq!(rgba_to_xrgb(&[0x12, 0x34, 0x56, 0xff]), 0x0012_3456);
        assert_eq!(rgba_to_xrgb(&[0xff, 0x00, 0x00, 0x40]), 0x00ff_0000);
    }
}
