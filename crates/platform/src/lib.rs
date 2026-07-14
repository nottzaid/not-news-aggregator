//! Windows/Linux window, input, and GPU-surface boundary.

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
compile_error!("not-news-platform supports Windows and Linux only");

mod open_gl;

use std::{env, fs, io, path::PathBuf, process::Command, thread, time::Instant};

use skia_safe::Canvas;
use winit::event::WindowEvent;

pub use open_gl::{
    FrameMeasurement, PlatformError, RendererBackend, RunReport, WindowOptions, run,
};
pub use skia_safe;
pub use winit;

/// Creates and returns the platform-native per-user data directory.
///
/// # Errors
///
/// Rejects unsafe application identifiers, missing platform home data, and
/// filesystem failures.
pub fn application_data_directory(application_id: &str) -> Result<PathBuf, io::Error> {
    if application_id.is_empty()
        || !application_id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "application ID must contain lowercase ASCII letters, digits, or hyphens",
        ));
    }
    #[cfg(target_os = "linux")]
    let directory = linux_data_directory(
        application_id,
        env::var_os("XDG_DATA_HOME"),
        env::var_os("HOME"),
    )?;
    #[cfg(target_os = "windows")]
    let directory = env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "LOCALAPPDATA does not name an absolute directory",
            )
        })?
        .join(application_id);
    fs::create_dir_all(&directory)?;
    Ok(directory)
}

#[cfg(target_os = "linux")]
fn linux_data_directory(
    application_id: &str,
    xdg_data_home: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
) -> Result<PathBuf, io::Error> {
    if let Some(xdg) = xdg_data_home
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
    {
        return Ok(xdg.join(application_id));
    }
    home.map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .map(|path| path.join(".local/share").join(application_id))
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "neither XDG_DATA_HOME nor HOME names an absolute directory",
            )
        })
}

/// Opens a URL through the desktop's registered external handler without
/// blocking the render thread or invoking a shell.
///
/// # Errors
///
/// Rejects non-HTTP(S) sources and returns an error when the reaper thread
/// cannot be created; handler launch failures are reported asynchronously.
pub fn open_external_url(url: &str) -> Result<(), io::Error> {
    let mut command = external_url_command(url)?;
    thread::Builder::new()
        .name("external-url".into())
        .spawn(move || match command.status() {
            Ok(status) if status.success() => {}
            Ok(status) => eprintln!("external URL handler exited with {status}"),
            Err(error) => eprintln!("external URL handler could not start: {error}"),
        })?;
    Ok(())
}

fn external_url_command(url: &str) -> Result<Command, io::Error> {
    let scheme = url.split_once(':').map(|(scheme, _)| scheme);
    if !scheme.is_some_and(|scheme| {
        scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https")
    }) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "source URL must use HTTP or HTTPS",
        ));
    }
    #[cfg(target_os = "linux")]
    let mut command = Command::new("xdg-open");
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = Command::new("rundll32");
        command.arg("url.dll,FileProtocolHandler");
        command
    };
    command.arg(url);
    Ok(command)
}

#[derive(Clone, Copy, Debug)]
pub struct FrameInfo {
    pub physical_width: u32,
    pub physical_height: u32,
    pub scale_factor: f64,
    pub now: Instant,
}

#[derive(Clone, Copy, Debug, Default)]
pub enum FrameSchedule {
    #[default]
    Wait,
    RedrawAt(Instant),
    Exit,
}

/// Safe application side of the native window and GPU boundary.
pub trait PlatformApplication {
    /// Handles an input or lifecycle event and reports whether it changed the
    /// pixels that should be presented.
    fn window_event(&mut self, _event: &WindowEvent) -> bool {
        false
    }

    /// Reports whether the sole window should route native IME composition to
    /// the application. The runner updates this after every input event.
    fn text_input_active(&self) -> bool {
        false
    }

    /// Paints one frame and selects the next animation deadline.
    fn render(&mut self, canvas: &Canvas, frame: FrameInfo) -> FrameSchedule;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn external_sources_are_http_only_and_remain_one_process_argument() {
        let url = "https://example.test/a?literal=$(touch%20forbidden)&x=;rm";
        let command = external_url_command(url).unwrap();
        let args: Vec<_> = command.get_args().collect();
        assert_eq!(args.last().unwrap().to_str(), Some(url));
        assert!(external_url_command("file:///etc/passwd").is_err());
        assert!(external_url_command("$(touch /tmp/forbidden)").is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_data_path_obeys_absolute_xdg_then_home_fallback() {
        assert_eq!(
            linux_data_directory("not-news-canvas", Some("/var/data".into()), None).unwrap(),
            PathBuf::from("/var/data/not-news-canvas")
        );
        assert_eq!(
            linux_data_directory(
                "not-news-canvas",
                Some("relative".into()),
                Some("/home/researcher".into()),
            )
            .unwrap(),
            PathBuf::from("/home/researcher/.local/share/not-news-canvas")
        );
        assert!(linux_data_directory("not-news-canvas", None, None).is_err());
    }
}
