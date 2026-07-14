use std::{
    env, fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

use keyring::{Entry, Error as KeyringError};
use not_news_agent::ResearchBackendChoice;
use serde::{Deserialize, Serialize};
use thiserror::Error;

const GROQ_SERVICE: &str = "not-news-canvas";
const GROQ_ACCOUNT: &str = "groq-api-key";

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum StoredResearchBackend {
    #[default]
    Auto,
    OpenCode,
    Hermes,
}

impl StoredResearchBackend {
    pub fn next(self) -> Self {
        match self {
            Self::Auto => Self::OpenCode,
            Self::OpenCode => Self::Hermes,
            Self::Hermes => Self::Auto,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "AUTO",
            Self::OpenCode => "OPENCODE",
            Self::Hermes => "HERMES",
        }
    }
}

impl From<StoredResearchBackend> for ResearchBackendChoice {
    fn from(value: StoredResearchBackend) -> Self {
        match value {
            StoredResearchBackend::Auto => Self::Auto,
            StoredResearchBackend::OpenCode => Self::OpenCode,
            StoredResearchBackend::Hermes => Self::Hermes,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct UserSettings {
    pub research_backend: StoredResearchBackend,
}

#[derive(Clone, Debug)]
pub struct SettingsStore {
    path: PathBuf,
}

#[derive(Debug, Error)]
pub enum SettingsError {
    #[error("settings could not be read: {0}")]
    Read(#[source] io::Error),
    #[error("settings are invalid: {0}")]
    Decode(#[source] serde_json::Error),
    #[error("settings could not be encoded: {0}")]
    Encode(#[source] serde_json::Error),
    #[error("settings could not be written durably: {0}")]
    Write(#[source] io::Error),
    #[error("the operating-system credential vault is unavailable: {0}")]
    Vault(#[source] KeyringError),
    #[error("the Groq API key is empty")]
    EmptyGroqKey,
}

impl SettingsStore {
    pub fn new(data_directory: &Path) -> Self {
        Self {
            path: data_directory.join("settings.json"),
        }
    }

    pub fn load(&self) -> Result<UserSettings, SettingsError> {
        match fs::read(&self.path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(SettingsError::Decode),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(UserSettings::default()),
            Err(error) => Err(SettingsError::Read(error)),
        }
    }

    pub fn save(&self, settings: UserSettings) -> Result<(), SettingsError> {
        let parent = self.path.parent().ok_or_else(|| {
            SettingsError::Write(io::Error::new(
                io::ErrorKind::InvalidInput,
                "settings path has no parent",
            ))
        })?;
        fs::create_dir_all(parent).map_err(SettingsError::Write)?;
        let bytes = serde_json::to_vec_pretty(&settings).map_err(SettingsError::Encode)?;
        let pending = self.path.with_extension("json.pending");
        let mut file = fs::File::create(&pending).map_err(SettingsError::Write)?;
        file.write_all(&bytes).map_err(SettingsError::Write)?;
        file.sync_all().map_err(SettingsError::Write)?;
        replace_file(&pending, &self.path).map_err(SettingsError::Write)?;
        sync_directory(parent).map_err(SettingsError::Write)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GroqCredentialState {
    Environment,
    Vault,
    Missing,
    Unavailable(String),
}

impl GroqCredentialState {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Environment => "ENVIRONMENT OVERRIDE",
            Self::Vault => "OS VAULT",
            Self::Missing => "NOT CONFIGURED",
            Self::Unavailable(_) => "VAULT UNAVAILABLE",
        }
    }
}

pub fn groq_credential_state() -> GroqCredentialState {
    if environment_groq_key().is_some() {
        return GroqCredentialState::Environment;
    }
    match vault_entry().and_then(|entry| entry.get_password()) {
        Ok(secret) if !secret.trim().is_empty() => GroqCredentialState::Vault,
        Ok(_) | Err(KeyringError::NoEntry) => GroqCredentialState::Missing,
        Err(error) => GroqCredentialState::Unavailable(error.to_string()),
    }
}

pub fn groq_api_key() -> Result<Option<String>, SettingsError> {
    if let Some(key) = environment_groq_key() {
        return Ok(Some(key));
    }
    match vault_entry().and_then(|entry| entry.get_password()) {
        Ok(secret) if !secret.trim().is_empty() => Ok(Some(secret)),
        Ok(_) | Err(KeyringError::NoEntry) => Ok(None),
        Err(error) => Err(SettingsError::Vault(error)),
    }
}

pub fn save_groq_api_key(secret: &str) -> Result<(), SettingsError> {
    if secret.trim().is_empty() {
        return Err(SettingsError::EmptyGroqKey);
    }
    vault_entry()
        .and_then(|entry| entry.set_password(secret.trim()))
        .map_err(SettingsError::Vault)
}

pub fn delete_groq_api_key() -> Result<(), SettingsError> {
    match vault_entry().and_then(|entry| entry.delete_credential()) {
        Ok(()) | Err(KeyringError::NoEntry) => Ok(()),
        Err(error) => Err(SettingsError::Vault(error)),
    }
}

fn environment_groq_key() -> Option<String> {
    env::var("GROQ_API_KEY")
        .ok()
        .filter(|value| !value.trim().is_empty())
}

fn vault_entry() -> Result<Entry, KeyringError> {
    vault_entry_for(GROQ_SERVICE, GROQ_ACCOUNT)
}

#[cfg(target_os = "linux")]
fn vault_entry_for(service: &str, account: &str) -> Result<Entry, KeyringError> {
    use std::collections::HashMap;

    prepare_linux_secret_service();
    // Initialize keyring's platform store, then use an application-owned
    // collection. KWallet installations without a global `default` alias can
    // create this collection without changing the user's default wallet.
    let _ = Entry::new(service, account)?;
    let modifiers = HashMap::from([("target", GROQ_SERVICE)]);
    let inner = keyring_core::Entry::new_with_modifiers(service, account, &modifiers)?;
    Ok(Entry { inner })
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

#[cfg(target_os = "linux")]
fn replace_file(source: &Path, destination: &Path) -> io::Result<()> {
    fs::rename(source, destination)
}

#[cfg(target_os = "windows")]
fn replace_file(source: &Path, destination: &Path) -> io::Result<()> {
    if let Err(error) = fs::remove_file(destination)
        && error.kind() != io::ErrorKind::NotFound
    {
        return Err(error);
    }
    fs::rename(source, destination)
}

#[cfg(target_os = "linux")]
fn sync_directory(directory: &Path) -> io::Result<()> {
    fs::File::open(directory)?.sync_all()
}

#[cfg(target_os = "windows")]
fn sync_directory(_directory: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn nonsecret_backend_choice_round_trips_without_credentials() {
        let directory = TempDir::new().unwrap();
        let store = SettingsStore::new(directory.path());
        let settings = UserSettings {
            research_backend: StoredResearchBackend::Hermes,
        };
        store.save(settings).unwrap();
        assert_eq!(store.load().unwrap(), settings);
        let persisted = fs::read_to_string(directory.path().join("settings.json")).unwrap();
        assert!(persisted.contains("hermes"));
        assert!(!persisted.contains("api") && !persisted.contains("key"));
    }

    #[test]
    fn malformed_settings_are_rejected_instead_of_silently_reinterpreted() {
        let directory = TempDir::new().unwrap();
        let store = SettingsStore::new(directory.path());
        fs::write(
            directory.path().join("settings.json"),
            br#"{"research_backend":"other"}"#,
        )
        .unwrap();
        assert!(matches!(store.load(), Err(SettingsError::Decode(_))));
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
