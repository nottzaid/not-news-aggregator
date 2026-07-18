use std::{
    collections::HashSet,
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Mutex, OnceLock, mpsc},
    thread,
    time::{Duration, Instant},
};

use sha2::{Digest as _, Sha256};
use thiserror::Error;

use crate::runner::{find_on_path, isolated_runtime_environment, prepare_isolated_home};

const CHECK_TIMEOUT: Duration = Duration::from_secs(8);
const MAX_DIAGNOSTIC_BYTES: usize = 4_096;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HermesCompatibility {
    pub executable: PathBuf,
    pub evidence: &'static str,
    pub cached: bool,
}

#[derive(Debug, Error)]
pub enum HermesCompatibilityError {
    #[error("Hermes executable is absent or not executable on PATH")]
    Missing,
    #[error("Hermes profile-local runtime directories are unavailable: {0}")]
    ProfileHome(#[source] io::Error),
    #[error("Hermes ACP compatibility check could not start: {0}")]
    Spawn(#[source] io::Error),
    #[error("Hermes ACP compatibility check could not be observed: {0}")]
    Observe(#[source] io::Error),
    #[error("Hermes ACP compatibility check exceeded eight seconds")]
    Timeout,
    #[error("Hermes ACP compatibility check failed{code}: {detail}")]
    Failed { code: String, detail: String },
}

#[derive(Debug, Error)]
pub enum ToolCapabilityError {
    #[error("{tool} executable is absent or not executable on PATH")]
    Missing { tool: String },
    #[error("{tool} capability probe could not start: {source}")]
    Spawn { tool: String, source: io::Error },
    #[error("{tool} capability probe exceeded three seconds")]
    Timeout { tool: String },
    #[error("{tool} capability probe exited unsuccessfully{code}")]
    Failed { tool: String, code: String },
    #[error("{tool} capability probe could not be observed: {source}")]
    Observe { tool: String, source: io::Error },
}

/// Verifies executable permission and the tool's side-effect-free version
/// command. This proves CLI presence, not browser/service readiness.
///
/// # Errors
///
/// Returns a typed missing, spawn, timeout, observation, or nonzero-exit error.
pub fn check_tool_capability(
    tool: &str,
    arguments: &[&str],
) -> Result<PathBuf, ToolCapabilityError> {
    let executable = find_on_path(tool).ok_or_else(|| ToolCapabilityError::Missing {
        tool: tool.to_owned(),
    })?;
    let mut child = Command::new(&executable)
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|source| ToolCapabilityError::Spawn {
            tool: tool.to_owned(),
            source,
        })?;
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match child
            .try_wait()
            .map_err(|source| ToolCapabilityError::Observe {
                tool: tool.to_owned(),
                source,
            })? {
            Some(status) if status.success() => return Ok(executable),
            Some(status) => {
                return Err(ToolCapabilityError::Failed {
                    tool: tool.to_owned(),
                    code: status
                        .code()
                        .map_or_else(String::new, |code| format!(" with exit code {code}")),
                });
            }
            None if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
            None => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(ToolCapabilityError::Timeout {
                    tool: tool.to_owned(),
                });
            }
        }
    }
}

/// Verifies the exact profile-selection and ACP self-check command consumed by
/// Not News. A pass establishes executable, CLI, profile routing, and ACP
/// installation compatibility; it does not authenticate a model provider or
/// prove streamed research semantics. Successful evidence is cached only for
/// the executable, owned policy version, and profile config bytes observed.
///
/// # Errors
///
/// Returns a typed error when Hermes is absent, profile-local runtime state
/// cannot be prepared, or the exact ACP check cannot be observed successfully.
pub fn check_hermes_compatibility(
    hermes_root: &Path,
    profile_id: &str,
    policy_version: u32,
) -> Result<HermesCompatibility, HermesCompatibilityError> {
    let executable = find_on_path("hermes").ok_or(HermesCompatibilityError::Missing)?;
    check_hermes_executable(executable, hermes_root, profile_id, policy_version)
}

fn check_hermes_executable(
    executable: PathBuf,
    hermes_root: &Path,
    profile_id: &str,
    policy_version: u32,
) -> Result<HermesCompatibility, HermesCompatibilityError> {
    let profile = hermes_root.join("profiles").join(profile_id);
    let home = profile.join("home");
    prepare_isolated_home(&home).map_err(HermesCompatibilityError::ProfileHome)?;
    let identity =
        compatibility_identity(&executable, &profile.join("config.yaml"), policy_version)
            .map_err(HermesCompatibilityError::Observe)?;
    if compatibility_cache()
        .lock()
        .expect("compatibility cache poisoned")
        .contains(&identity)
    {
        return Ok(HermesCompatibility {
            executable,
            evidence: "cached executable/profile ACP self-check",
            cached: true,
        });
    }

    let mut command = Command::new(&executable);
    command
        .args(["-p", profile_id, "acp", "--check"])
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (name, value) in isolated_runtime_environment(&home) {
        command.env(name, value);
    }
    command
        .env("HERMES_HOME", hermes_root)
        .env("TERMINAL_HOME_MODE", "profile");
    let mut child = command.spawn().map_err(HermesCompatibilityError::Spawn)?;
    let stderr = child.stderr.take().map(spawn_bounded_reader);
    let stdout = child.stdout.take().map(spawn_bounded_reader);
    let deadline = Instant::now() + CHECK_TIMEOUT;
    let status = loop {
        match child
            .try_wait()
            .map_err(HermesCompatibilityError::Observe)?
        {
            Some(status) => break status,
            None if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
            None => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(HermesCompatibilityError::Timeout);
            }
        }
    };
    let detail = bounded_child_output(stderr, stdout);
    if !status.success() {
        return Err(HermesCompatibilityError::Failed {
            code: status
                .code()
                .map_or_else(String::new, |code| format!(" with exit code {code}")),
            detail: if detail.is_empty() {
                "no diagnostic was emitted".into()
            } else {
                detail
            },
        });
    }
    compatibility_cache()
        .lock()
        .expect("compatibility cache poisoned")
        .insert(identity);
    Ok(HermesCompatibility {
        executable,
        evidence: "executable/profile ACP self-check",
        cached: false,
    })
}

fn compatibility_identity(
    executable: &Path,
    profile_config: &Path,
    policy_version: u32,
) -> io::Result<String> {
    let mut hash = Sha256::new();
    hash.update(fs::read(executable)?);
    hash.update([0]);
    hash.update(fs::read(profile_config)?);
    hash.update(policy_version.to_le_bytes());
    Ok(format!("{:x}", hash.finalize()))
}

fn compatibility_cache() -> &'static Mutex<HashSet<String>> {
    static CACHE: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashSet::new()))
}

fn spawn_bounded_reader(reader: impl Read + Send + 'static) -> mpsc::Receiver<Vec<u8>> {
    let (sender, receiver) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = reader
            .take(u64::try_from(MAX_DIAGNOSTIC_BYTES).unwrap_or(u64::MAX))
            .read_to_end(&mut bytes);
        let _ = sender.send(bytes);
    });
    receiver
}

fn bounded_child_output(
    stderr: Option<mpsc::Receiver<Vec<u8>>>,
    stdout: Option<mpsc::Receiver<Vec<u8>>>,
) -> String {
    let receive = |receiver: Option<mpsc::Receiver<Vec<u8>>>| {
        receiver
            .and_then(|receiver| receiver.recv_timeout(Duration::from_millis(100)).ok())
            .unwrap_or_default()
    };
    let mut bytes = receive(stderr);
    if bytes.is_empty() {
        bytes = receive(stdout);
    }
    String::from_utf8_lossy(&bytes).trim().to_owned()
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use tempfile::TempDir;

    use super::*;

    #[cfg(unix)]
    fn fake_hermes(directory: &Path, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt as _;

        let path = directory.join("hermes");
        let mut file = fs::File::create(&path).unwrap();
        writeln!(file, "#!/bin/sh\n{body}").unwrap();
        let mut permissions = file.metadata().unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&path, permissions).unwrap();
        path
    }

    #[cfg(unix)]
    #[test]
    fn exact_acp_check_passes_caches_and_rejects_an_incompatible_peer() {
        let directory = TempDir::new().unwrap();
        let bin = directory.path().join("bin");
        let root = directory.path().join("hermes-root");
        fs::create_dir_all(&bin).unwrap();
        fs::create_dir_all(root.join("profiles/not-news")).unwrap();
        fs::write(root.join("profiles/not-news/config.yaml"), "terminal: {}\n").unwrap();
        fake_hermes(
            &bin,
            r#"test "$1" = "-p" && test "$2" = "not-news" && test "$3" = "acp" && test "$4" = "--check" && test "$TERMINAL_HOME_MODE" = "profile""#,
        );
        let executable = bin.join("hermes");
        let first = check_hermes_executable(executable.clone(), &root, "not-news", 2).unwrap();
        let second = check_hermes_executable(executable.clone(), &root, "not-news", 2).unwrap();
        assert!(!first.cached);
        assert!(second.cached);

        fake_hermes(&bin, "echo incompatible >&2\nexit 9");
        let error = check_hermes_executable(executable, &root, "not-news", 2).unwrap_err();
        assert!(error.to_string().contains("incompatible"));
    }
}
