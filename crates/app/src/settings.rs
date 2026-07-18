use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::Path,
    sync::mpsc,
    thread,
    time::Duration,
};

use fs2::FileExt as _;
use keyring::{Entry, Error as KeyringError};
use thiserror::Error;

const CREDENTIAL_SERVICE: &str = "not-news-canvas";
const GROQ_ACCOUNT: &str = "groq-api-key";
const EXA_ACCOUNT: &str = "exa-api-key";
const BROWSERBASE_ACCOUNT: &str = "browserbase-api-key";
const SETTINGS_FILE: &str = "settings.json";
const SETTINGS_LOCK: &str = ".settings.lock";
const SETTINGS_PENDING: &str = ".settings.pending";
const SETTINGS_PREVIOUS: &str = ".settings.previous";
const VAULT_TIMEOUT: Duration = Duration::from_secs(5);

/// Removes the retired 0.1.1 backend selector only when the file has exactly
/// that known schema. Unknown future or user-authored settings remain intact.
pub fn retire_backend_selector(data_directory: &Path) -> io::Result<bool> {
    fs::create_dir_all(data_directory)?;
    let lock = settings_lock(data_directory)?;
    lock.lock_exclusive()?;
    recover_settings(data_directory)?;
    let path = data_directory.join(SETTINGS_FILE);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    let Ok(serde_json::Value::Object(settings)) = serde_json::from_slice(&bytes) else {
        return Ok(false);
    };
    let retired = settings.len() == 1
        && settings
            .get("research_backend")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|value| matches!(value, "auto" | "opencode" | "hermes"));
    if retired {
        fs::remove_file(path)?;
    }
    Ok(retired)
}

#[derive(Debug, Error)]
pub enum SettingsError {
    #[error("the operating-system credential vault is unavailable: {0}")]
    Vault(#[source] KeyringError),
    #[error("the operating-system credential vault did not respond within five seconds")]
    VaultTimeout,
    #[error("the {0} API key is empty")]
    EmptyApiKey(&'static str),
    #[error("the {0} API key is shorter than eight bytes")]
    ShortApiKey(&'static str),
    #[error("the SearXNG endpoint must be an http:// or https:// URL without whitespace")]
    InvalidSearxngUrl,
    #[error("application settings could not be read or written: {0}")]
    Io(#[from] io::Error),
    #[error("application settings are not valid JSON: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EndpointState {
    Saved,
    Missing,
    Unavailable(String),
}

impl EndpointState {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Saved => "APP SETTINGS",
            Self::Missing => "NOT CONFIGURED",
            Self::Unavailable(_) => "SETTINGS UNAVAILABLE",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CredentialState {
    Vault,
    Missing,
    Unavailable(String),
}

impl CredentialState {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Vault => "OS VAULT",
            Self::Missing => "NOT CONFIGURED",
            Self::Unavailable(_) => "VAULT UNAVAILABLE",
        }
    }
}

pub fn groq_credential_state() -> CredentialState {
    credential_state(GROQ_ACCOUNT)
}

pub fn exa_credential_state() -> CredentialState {
    credential_state(EXA_ACCOUNT)
}

pub fn browserbase_credential_state() -> CredentialState {
    credential_state(BROWSERBASE_ACCOUNT)
}

pub fn groq_api_key() -> Result<Option<String>, SettingsError> {
    read_credential(GROQ_ACCOUNT)
}

pub fn exa_api_key() -> Result<Option<String>, SettingsError> {
    read_credential(EXA_ACCOUNT)
}

pub fn browserbase_api_key() -> Result<Option<String>, SettingsError> {
    read_credential(BROWSERBASE_ACCOUNT)
}

pub fn searxng_endpoint_state(data_directory: &Path) -> EndpointState {
    match saved_searxng_url(data_directory) {
        Ok(Some(_)) => EndpointState::Saved,
        Ok(None) => EndpointState::Missing,
        Err(error) => EndpointState::Unavailable(error.to_string()),
    }
}

pub fn searxng_url(data_directory: &Path) -> Result<Option<String>, SettingsError> {
    saved_searxng_url(data_directory)
}

pub fn save_searxng_url(data_directory: &Path, url: &str) -> Result<(), SettingsError> {
    let url = normalize_searxng_url(url)?;
    fs::create_dir_all(data_directory)?;
    let lock = settings_lock(data_directory)?;
    lock.lock_exclusive()?;
    recover_settings(data_directory)?;
    let path = data_directory.join(SETTINGS_FILE);
    let mut settings = match fs::read(&path) {
        Ok(bytes) => serde_json::from_slice::<serde_json::Map<String, serde_json::Value>>(&bytes)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => serde_json::Map::new(),
        Err(error) => return Err(error.into()),
    };
    settings.insert("searxng_url".into(), serde_json::Value::String(url));
    replace_settings(data_directory, &serde_json::to_vec_pretty(&settings)?)?;
    Ok(())
}

fn saved_searxng_url(data_directory: &Path) -> Result<Option<String>, SettingsError> {
    fs::create_dir_all(data_directory)?;
    let lock = settings_lock(data_directory)?;
    lock.lock_exclusive()?;
    recover_settings(data_directory)?;
    let path = data_directory.join(SETTINGS_FILE);
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let settings = serde_json::from_slice::<serde_json::Map<String, serde_json::Value>>(&bytes)?;
    settings
        .get("searxng_url")
        .and_then(serde_json::Value::as_str)
        .map(normalize_searxng_url)
        .transpose()
}

pub fn delete_searxng_url(data_directory: &Path) -> Result<(), SettingsError> {
    fs::create_dir_all(data_directory)?;
    let lock = settings_lock(data_directory)?;
    lock.lock_exclusive()?;
    recover_settings(data_directory)?;
    let path = data_directory.join(SETTINGS_FILE);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    let mut settings =
        serde_json::from_slice::<serde_json::Map<String, serde_json::Value>>(&bytes)?;
    settings.remove("searxng_url");
    if settings.is_empty() {
        fs::remove_file(path)?;
    } else {
        replace_settings(data_directory, &serde_json::to_vec_pretty(&settings)?)?;
    }
    Ok(())
}

fn settings_lock(data_directory: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    options.open(data_directory.join(SETTINGS_LOCK))
}

fn recover_settings(data_directory: &Path) -> io::Result<()> {
    let settings = data_directory.join(SETTINGS_FILE);
    let pending = data_directory.join(SETTINGS_PENDING);
    let previous = data_directory.join(SETTINGS_PREVIOUS);
    if !settings.exists() {
        if pending.is_file() {
            fs::rename(&pending, &settings)?;
        } else if previous.is_file() {
            fs::rename(&previous, &settings)?;
        }
    }
    for stale in [pending, previous] {
        match fs::remove_file(stale) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn replace_settings(data_directory: &Path, bytes: &[u8]) -> io::Result<()> {
    let settings = data_directory.join(SETTINGS_FILE);
    let pending = data_directory.join(SETTINGS_PENDING);
    let previous = data_directory.join(SETTINGS_PREVIOUS);
    match fs::remove_file(&pending) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options.open(&pending)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    if settings.exists() {
        match fs::remove_file(&previous) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        fs::rename(&settings, &previous)?;
    }
    if let Err(error) = fs::rename(&pending, &settings) {
        if previous.exists() && !settings.exists() {
            let _ = fs::rename(&previous, &settings);
        }
        return Err(error);
    }
    match fs::remove_file(previous) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    sync_directory(data_directory)
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> io::Result<()> {
    File::open(path)?.sync_all()
}

#[cfg(windows)]
fn sync_directory(_path: &Path) -> io::Result<()> {
    Ok(())
}

fn normalize_searxng_url(url: &str) -> Result<String, SettingsError> {
    let url = url.trim().trim_end_matches('/');
    if url.is_empty()
        || url.chars().any(char::is_whitespace)
        || !matches!(url.strip_prefix("http://").or_else(|| url.strip_prefix("https://")), Some(authority) if !authority.is_empty())
    {
        return Err(SettingsError::InvalidSearxngUrl);
    }
    Ok(url.to_owned())
}

fn read_credential(account: &'static str) -> Result<Option<String>, SettingsError> {
    match vault_operation(move || credential_entry(account)?.get_password())? {
        Ok(secret) if !secret.trim().is_empty() => Ok(Some(secret)),
        Ok(_) | Err(KeyringError::NoEntry) => Ok(None),
        Err(error) => Err(SettingsError::Vault(error)),
    }
}

pub fn save_groq_api_key(secret: &str) -> Result<(), SettingsError> {
    save_api_key(GROQ_ACCOUNT, "Groq", secret)
}

pub fn save_exa_api_key(secret: &str) -> Result<(), SettingsError> {
    save_api_key(EXA_ACCOUNT, "Exa", secret)
}

pub fn save_browserbase_api_key(secret: &str) -> Result<(), SettingsError> {
    save_api_key(BROWSERBASE_ACCOUNT, "Browserbase", secret)
}

fn save_api_key(account: &str, label: &'static str, secret: &str) -> Result<(), SettingsError> {
    if secret.trim().is_empty() {
        return Err(SettingsError::EmptyApiKey(label));
    }
    if secret.trim().len() < 8 {
        return Err(SettingsError::ShortApiKey(label));
    }
    let account = account.to_owned();
    let secret = secret.trim().to_owned();
    vault_operation(move || credential_entry(&account)?.set_password(&secret))?
        .map_err(SettingsError::Vault)
}

pub fn delete_groq_api_key() -> Result<(), SettingsError> {
    delete_credential(GROQ_ACCOUNT)
}

pub fn delete_exa_api_key() -> Result<(), SettingsError> {
    delete_credential(EXA_ACCOUNT)
}

pub fn delete_browserbase_api_key() -> Result<(), SettingsError> {
    delete_credential(BROWSERBASE_ACCOUNT)
}

fn delete_credential(account: &str) -> Result<(), SettingsError> {
    let account = account.to_owned();
    match vault_operation(move || credential_entry(&account)?.delete_credential())? {
        Ok(()) | Err(KeyringError::NoEntry) => Ok(()),
        Err(error) => Err(SettingsError::Vault(error)),
    }
}

fn credential_state(account: &str) -> CredentialState {
    let account = account.to_owned();
    match vault_operation(move || credential_entry(&account)?.get_password()) {
        Err(error) => CredentialState::Unavailable(error.to_string()),
        Ok(Ok(secret)) if !secret.trim().is_empty() => CredentialState::Vault,
        Ok(Ok(_) | Err(KeyringError::NoEntry)) => CredentialState::Missing,
        Ok(Err(error)) => CredentialState::Unavailable(error.to_string()),
    }
}

fn vault_operation<T: Send + 'static>(
    operation: impl FnOnce() -> Result<T, KeyringError> + Send + 'static,
) -> Result<Result<T, KeyringError>, SettingsError> {
    let (sender, receiver) = mpsc::sync_channel(1);
    thread::Builder::new()
        .name("credential-vault-operation".into())
        .spawn(move || {
            let _ = sender.send(operation());
        })?;
    match receiver.recv_timeout(VAULT_TIMEOUT) {
        Ok(result) => Ok(result),
        Err(mpsc::RecvTimeoutError::Timeout) => Err(SettingsError::VaultTimeout),
        Err(mpsc::RecvTimeoutError::Disconnected) => Err(SettingsError::Io(io::Error::new(
            io::ErrorKind::BrokenPipe,
            "credential-vault worker stopped without a result",
        ))),
    }
}

fn credential_entry(account: &str) -> Result<Entry, KeyringError> {
    vault_entry_for(CREDENTIAL_SERVICE, account)
}

#[cfg(target_os = "linux")]
fn vault_entry_for(service: &str, account: &str) -> Result<Entry, KeyringError> {
    prepare_linux_secret_service();
    Entry::new(service, account)
}

#[cfg(target_os = "windows")]
fn vault_entry_for(service: &str, account: &str) -> Result<Entry, KeyringError> {
    Entry::new(service, account)
}

#[cfg(target_os = "linux")]
fn prepare_linux_secret_service() {
    let Ok(connection) = zbus::blocking::Connection::session() else {
        return;
    };
    if connection
        .call_method(
            Some("org.freedesktop.secrets"),
            "/",
            Some("org.freedesktop.DBus.Peer"),
            "Ping",
            &(),
        )
        .is_ok()
    {
        return;
    }
    // KWallet exposes the standard Secret Service after its compatibility
    // name is activated. Unknown names fail harmlessly on other desktops.
    let _ = connection.call_method(
        Some("org.kde.secretservicecompat"),
        "/",
        Some("org.freedesktop.DBus.Peer"),
        "Ping",
        &(),
    );
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn retired_backend_selector_is_removed_without_touching_unknown_settings() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("settings.json");
        fs::write(&path, br#"{"research_backend":"opencode"}"#).unwrap();
        assert!(retire_backend_selector(directory.path()).unwrap());
        assert!(!path.exists());

        fs::write(&path, br#"{"research_backend":"opencode","future":true}"#).unwrap();
        assert!(!retire_backend_selector(directory.path()).unwrap());
        assert!(path.exists());
    }

    #[test]
    fn searxng_endpoint_is_normalized_without_erasing_future_settings() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("settings.json");
        fs::write(&path, br#"{"future":true}"#).unwrap();

        save_searxng_url(directory.path(), "  http://127.0.0.1:8889/  ").unwrap();

        assert_eq!(
            saved_searxng_url(directory.path()).unwrap().as_deref(),
            Some("http://127.0.0.1:8889")
        );
        let value: serde_json::Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert_eq!(value["future"], true);
        delete_searxng_url(directory.path()).unwrap();
        let value: serde_json::Value =
            serde_json::from_slice(&fs::read(directory.path().join(SETTINGS_FILE)).unwrap())
                .unwrap();
        assert_eq!(value, serde_json::json!({"future": true}));
        assert!(matches!(
            normalize_searxng_url("file:///private"),
            Err(SettingsError::InvalidSearxngUrl)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn settings_replacement_is_private_and_recovers_an_interrupted_commit() {
        use std::os::unix::fs::PermissionsExt as _;

        let directory = TempDir::new().unwrap();
        save_searxng_url(directory.path(), "https://search.example").unwrap();
        let path = directory.path().join(SETTINGS_FILE);
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );

        fs::rename(&path, directory.path().join(SETTINGS_PREVIOUS)).unwrap();
        assert_eq!(
            saved_searxng_url(directory.path()).unwrap().as_deref(),
            Some("https://search.example")
        );
        assert!(path.is_file());
        assert!(!directory.path().join(SETTINGS_PREVIOUS).exists());
    }

    #[test]
    #[ignore = "writes and removes a disposable entry in the live OS credential vault"]
    fn live_os_vault_round_trip_does_not_touch_the_user_key() {
        #[cfg(target_os = "linux")]
        prepare_linux_secret_service();
        let account = format!("process-{}", std::process::id());
        let entry = vault_entry_for("not-news-canvas-self-test", &account).unwrap();
        let secret = "disposable-not-a-real-key";
        entry.set_password(secret).unwrap();
        assert_eq!(entry.get_password().unwrap(), secret);
        entry.delete_credential().unwrap();
        assert!(matches!(entry.get_password(), Err(KeyringError::NoEntry)));
    }
}
