use std::{ffi::CString, num::NonZeroU32, time::Instant};

use gl::types::GLint;
use glutin::{
    config::{Config, ConfigTemplateBuilder, GlConfig},
    context::{ContextApi, ContextAttributesBuilder, PossiblyCurrentContext},
    display::{GetGlDisplay, GlDisplay},
    prelude::{GlSurface, NotCurrentGlContext},
    surface::{Surface as GlutinSurface, SurfaceAttributesBuilder, WindowSurface},
};
use glutin_winit::DisplayBuilder;
use raw_window_handle::HasWindowHandle;
use skia_safe::{
    ColorType, Surface,
    gpu::{self, SurfaceOrigin, backend_render_targets, gl::FramebufferInfo},
};
use thiserror::Error;
use winit::{
    application::ApplicationHandler,
    dpi::LogicalSize,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    window::{Window, WindowAttributes, WindowId},
};

use crate::{FrameInfo, FrameSchedule, PlatformApplication};

#[derive(Clone, Debug)]
pub struct WindowOptions {
    pub title: String,
    pub logical_width: f64,
    pub logical_height: f64,
    pub visible: bool,
}

impl Default for WindowOptions {
    fn default() -> Self {
        Self {
            title: "Not News Aggregator".to_owned(),
            logical_width: 1_280.0,
            logical_height: 800.0,
            visible: true,
        }
    }
}

#[derive(Debug, Error)]
pub enum PlatformError {
    #[error("could not create the native event loop: {0}")]
    EventLoop(String),
    #[error("could not choose a native OpenGL configuration: {0}")]
    Display(String),
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
    let attributes = WindowAttributes::default()
        .with_title(options.title)
        .with_inner_size(LogicalSize::new(
            options.logical_width,
            options.logical_height,
        ))
        .with_visible(options.visible);
    let template = ConfigTemplateBuilder::new().with_alpha_size(8);
    let display_builder = DisplayBuilder::new().with_window_attributes(Some(attributes));
    let (window, config) = display_builder
        .build(&event_loop, template, choose_config)
        .map_err(|error| PlatformError::Display(error.to_string()))?;
    let window = window.ok_or(PlatformError::MissingWindow)?;
    let native = NativeSurface::new(window, &config)?;
    native.window.request_redraw();

    let mut runner = Runner {
        application,
        native,
        deferred_error: None,
        render_on_resume,
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
    skia_surface: Surface,
    gl_surface: GlutinSurface<WindowSurface>,
    skia_context: skia_safe::gpu::DirectContext,
    gl_context: PossiblyCurrentContext,
    framebuffer: FramebufferInfo,
    samples: usize,
    stencil_bits: usize,
    window: Window,
}

impl NativeSurface {
    fn new(window: Window, config: &Config) -> Result<Self, PlatformError> {
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
            &window,
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
            window,
        })
    }

    fn resize(&mut self, width: u32, height: u32) -> Result<(), PlatformError> {
        self.gl_surface.resize(
            &self.gl_context,
            nonzero_extent(width),
            nonzero_extent(height),
        );
        if width > 0 && height > 0 {
            self.skia_surface = wrap_surface(
                &self.window,
                self.framebuffer,
                &mut self.skia_context,
                self.samples,
                self.stencil_bits,
            )?;
        }
        Ok(())
    }

    fn present(&mut self) -> Result<(), PlatformError> {
        self.skia_context.flush_and_submit();
        self.gl_surface
            .swap_buffers(&self.gl_context)
            .map_err(|error| PlatformError::Present(error.to_string()))
    }
}

impl Drop for NativeSurface {
    fn drop(&mut self) {
        self.skia_context.release_resources_and_abandon();
    }
}

struct Runner<A> {
    application: A,
    native: NativeSurface,
    deferred_error: Option<PlatformError>,
    render_on_resume: bool,
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
            WindowEvent::Resized(size) => self.native.resize(size.width, size.height),
            WindowEvent::RedrawRequested => self.redraw(event_loop),
            _ => {
                if self.application.window_event(&event) {
                    self.native.window.request_redraw();
                }
                Ok(())
            }
        };
        if let Err(error) = result {
            self.deferred_error = Some(error);
            event_loop.exit();
        }
    }
}

impl<A: PlatformApplication> Runner<A> {
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
        let schedule = self
            .application
            .render(self.native.skia_surface.canvas(), frame);
        self.native.present()?;
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
