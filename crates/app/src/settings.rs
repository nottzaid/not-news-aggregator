use std::{fs, io, path::Path};

use keyring::{Entry, Error as KeyringError};
use thiserror::Error;

const CREDENTIAL_SERVICE: &str = "not-news-canvas";
const GROQ_ACCOUNT: &str = "groq-api-key";
const EXA_ACCOUNT: &str = "exa-api-key";
const BROWSERBASE_ACCOUNT: &str = "browserbase-api-key";

/// Removes the retired 0.1.1 backend selector only when the file has exactly
/// that known schema. Unknown future or user-authored settings remain intact.
pub fn retire_backend_selector(data_directory: &Path) -> io::Result<bool> {
    let path = data_directory.join("settings.json");
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
    #[error("the {0} API key is empty")]
    EmptyApiKey(&'static str),
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
    let path = data_directory.join("settings.json");
    let mut settings = match fs::read(&path) {
        Ok(bytes) => serde_json::from_slice::<serde_json::Map<String, serde_json::Value>>(&bytes)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => serde_json::Map::new(),
        Err(error) => return Err(error.into()),
    };
    settings.insert("searxng_url".into(), serde_json::Value::String(url));
    fs::write(path, serde_json::to_vec_pretty(&settings)?)?;
    Ok(())
}

fn saved_searxng_url(data_directory: &Path) -> Result<Option<String>, SettingsError> {
    let path = data_directory.join("settings.json");
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

fn read_credential(account: &str) -> Result<Option<String>, SettingsError> {
    match credential_entry(account).and_then(|entry| entry.get_password()) {
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
    credential_entry(account)
        .and_then(|entry| entry.set_password(secret.trim()))
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
    match credential_entry(account).and_then(|entry| entry.delete_credential()) {
        Ok(()) | Err(KeyringError::NoEntry) => Ok(()),
        Err(error) => Err(SettingsError::Vault(error)),
    }
}

fn credential_state(account: &str) -> CredentialState {
    match credential_entry(account).and_then(|entry| entry.get_password()) {
        Ok(secret) if !secret.trim().is_empty() => CredentialState::Vault,
        Ok(_) | Err(KeyringError::NoEntry) => CredentialState::Missing,
        Err(error) => CredentialState::Unavailable(error.to_string()),
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

#[cfg(target_os = "windows")]
fn prepare_linux_secret_service() {}

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
        assert!(matches!(
            normalize_searxng_url("file:///private"),
            Err(SettingsError::InvalidSearxngUrl)
        ));
    }

    #[test]
    #[ignore = "writes and removes a disposable entry in the live OS credential vault"]
    fn live_os_vault_round_trip_does_not_touch_the_user_key() {
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
