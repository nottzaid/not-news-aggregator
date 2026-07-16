use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};

const CONFIG: &str = include_str!("../../../hermes/ainews/config.yaml");
const SOUL: &str = include_str!("../../../hermes/ainews/SOUL.md");
const USER_MEMORY: &str = include_str!("../../../hermes/ainews/memories/USER.md");
const PROFILE_METADATA: &str = "description: Evidence-grounded research for the Not News living graph.\ndescription_auto: false\n";
const OWNERSHIP_MARKER: &str = "not-news-profile-v1\n";

pub const PROFILE_ID: &str = "not-news";

#[derive(Debug, Eq, PartialEq)]
pub struct InstalledProfile {
    pub root: PathBuf,
    pub home: PathBuf,
}

/// Installs the public, application-owned Hermes profile policy without ever
/// replacing the private configuration or state Hermes creates beside it.
pub fn install(
    root: &Path,
    legacy_application_root: Option<&Path>,
) -> io::Result<InstalledProfile> {
    let profiles = root.join("profiles");
    let home = profiles.join(PROFILE_ID);
    fs::create_dir_all(&profiles)?;
    let mut migrated = false;

    if let Some(legacy_root) = legacy_application_root {
        let named = legacy_root.join("profiles").join(PROFILE_ID);
        let pre_profile = legacy_root.join("ainews");
        let source = if named.is_dir() {
            Some(named)
        } else if pre_profile.is_dir() {
            Some(pre_profile)
        } else {
            None
        };
        if let Some(source) = source {
            if home.exists() && profile_is_pristine(&home) {
                fs::remove_dir_all(&home)?;
            }
            if !home.exists() {
                fs::rename(source, &home)?;
                migrated = true;
            }
        }
    }

    if home.exists() && !migrated && !profile_is_owned(&home) && !profile_is_pristine(&home) {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!(
                "Hermes profile '{}' already exists but is not owned by Not News",
                home.display()
            ),
        ));
    }
    fs::create_dir_all(home.join("memories"))?;
    fs::create_dir_all(home.join("sessions"))?;
    fs::create_dir_all(home.join("logs"))?;
    let config = home.join("config.yaml");
    install_once(&config, CONFIG)?;
    extend_terminal_passthrough(&config)?;
    install_once(&home.join("SOUL.md"), SOUL)?;
    install_once(&home.join("memories").join("USER.md"), USER_MEMORY)?;
    install_once(&home.join(".no-bundled-skills"), "")?;
    install_once(&home.join("profile.yaml"), PROFILE_METADATA)?;
    install_once(&home.join(".not-news-profile"), OWNERSHIP_MARKER)?;
    Ok(InstalledProfile {
        root: root.to_path_buf(),
        home,
    })
}

fn extend_terminal_passthrough(path: &Path) -> io::Result<()> {
    const REQUIRED: [&str; 6] = [
        "EXA_API_KEY",
        "SEARXNG_URL",
        "AI_NEWS_SEARXNG_URL",
        "AI_NEWS_SEARXNG_SEARCH_URL",
        "BROWSERBASE_API_KEY",
        "BROWSE_CLI",
    ];
    let contents = fs::read_to_string(path)?;
    let mut lines = contents.lines().map(str::to_owned).collect::<Vec<_>>();
    let Some(header) = lines.iter().position(|line| line == "  env_passthrough:") else {
        return Ok(());
    };
    let mut end = header + 1;
    while end < lines.len() && lines[end].starts_with("    - ") {
        end += 1;
    }
    let mut changed = false;
    for name in REQUIRED {
        if lines
            .iter()
            .any(|line| line.strip_prefix("    - ") == Some(name))
        {
            continue;
        }
        lines.insert(end, format!("    - {name}"));
        end += 1;
        changed = true;
    }
    if !changed {
        return Ok(());
    }
    let mut updated = lines.join("\n");
    updated.push('\n');
    let mut file = OpenOptions::new().write(true).truncate(true).open(path)?;
    file.write_all(updated.as_bytes())?;
    file.sync_all()
}

fn profile_is_pristine(home: &Path) -> bool {
    ![
        "config.yaml",
        "SOUL.md",
        "auth.json",
        "state.db",
        ".env",
        "memories/USER.md",
    ]
    .iter()
    .any(|path| home.join(path).exists())
}

fn profile_is_owned(home: &Path) -> bool {
    fs::read_to_string(home.join(".not-news-profile"))
        .is_ok_and(|contents| contents == OWNERSHIP_MARKER)
        || fs::read_to_string(home.join("profile.yaml"))
            .is_ok_and(|contents| contents == PROFILE_METADATA)
}

fn install_once(path: &Path, contents: &str) -> io::Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(path) {
        Ok(mut file) => {
            file.write_all(contents.as_bytes())?;
            file.sync_all()
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn profile_install_is_complete_and_never_overwrites_private_configuration() {
        let directory = TempDir::new().unwrap();
        let profile = install(directory.path(), None).unwrap();
        let home = &profile.home;
        assert!(
            fs::read_to_string(home.join("SOUL.md"))
                .unwrap()
                .contains("living graph")
        );
        assert!(
            fs::read_to_string(home.join("memories/USER.md"))
                .unwrap()
                .contains("evidence-grounded")
        );
        fs::write(
            home.join("config.yaml"),
            "model:\n  provider: user-choice\n",
        )
        .unwrap();
        fs::write(home.join("auth.json"), "private-provider-credentials").unwrap();

        assert_eq!(install(directory.path(), None).unwrap(), profile);
        assert_eq!(
            fs::read_to_string(home.join("config.yaml")).unwrap(),
            "model:\n  provider: user-choice\n"
        );
        assert_eq!(
            fs::read_to_string(home.join("auth.json")).unwrap(),
            "private-provider-credentials"
        );
        assert_eq!(home, &directory.path().join("profiles/not-news"));
    }

    #[test]
    fn profile_upgrade_only_extends_the_owned_terminal_allowlist() {
        let directory = TempDir::new().unwrap();
        let home = install(directory.path(), None).unwrap().home;
        fs::write(
            home.join("config.yaml"),
            "terminal:\n  env_passthrough:\n    - BROWSE_CLI\nmodel:\n  provider: user-choice\n",
        )
        .unwrap();

        install(directory.path(), None).unwrap();

        let config = fs::read_to_string(home.join("config.yaml")).unwrap();
        assert!(config.contains("    - EXA_API_KEY\n"));
        assert!(config.contains("    - BROWSERBASE_API_KEY\n"));
        assert!(config.contains("model:\n  provider: user-choice\n"));
    }

    #[test]
    fn legacy_application_home_becomes_a_named_profile_without_losing_state() {
        let directory = TempDir::new().unwrap();
        let root = directory.path().join("global-hermes");
        let legacy_root = directory.path().join("application-hermes");
        let legacy = legacy_root.join("ainews");
        fs::create_dir_all(legacy.join("memories")).unwrap();
        fs::write(legacy.join("auth.json"), "private-provider-credentials").unwrap();
        fs::write(legacy.join("state.db"), "durable-session-history").unwrap();

        let home = install(&root, Some(&legacy_root)).unwrap().home;

        assert!(!legacy.exists());
        assert_eq!(
            fs::read_to_string(home.join("auth.json")).unwrap(),
            "private-provider-credentials"
        );
        assert_eq!(
            fs::read_to_string(home.join("state.db")).unwrap(),
            "durable-session-history"
        );
    }

    #[test]
    fn hermes_generated_empty_named_skeleton_does_not_block_legacy_migration() {
        let directory = TempDir::new().unwrap();
        let root = directory.path().join("global-hermes");
        let legacy_root = directory.path().join("application-hermes");
        let legacy = legacy_root.join("ainews");
        let skeleton = root.join("profiles/not-news/logs");
        fs::create_dir_all(&legacy).unwrap();
        fs::create_dir_all(&skeleton).unwrap();
        fs::write(skeleton.join("errors.log"), "early diagnostic").unwrap();
        fs::write(legacy.join("auth.json"), "private-provider-credentials").unwrap();

        let home = install(&root, Some(&legacy_root)).unwrap().home;

        assert!(!legacy.exists());
        assert_eq!(
            fs::read_to_string(home.join("auth.json")).unwrap(),
            "private-provider-credentials"
        );
    }

    #[test]
    fn install_neither_reads_nor_changes_the_default_profile() {
        let directory = TempDir::new().unwrap();
        let root = directory.path().join("hermes");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("config.yaml"), "discord-and-signal-default").unwrap();
        fs::write(root.join("state.db"), "131-session-default").unwrap();
        fs::write(root.join("active_profile"), "default\n").unwrap();

        install(&root, None).unwrap();

        assert_eq!(
            fs::read_to_string(root.join("config.yaml")).unwrap(),
            "discord-and-signal-default"
        );
        assert_eq!(
            fs::read_to_string(root.join("state.db")).unwrap(),
            "131-session-default"
        );
        assert_eq!(
            fs::read_to_string(root.join("active_profile")).unwrap(),
            "default\n"
        );
    }

    #[test]
    fn unrelated_existing_profile_is_refused_without_mutation() {
        let directory = TempDir::new().unwrap();
        let home = directory.path().join("profiles/not-news");
        fs::create_dir_all(&home).unwrap();
        fs::write(home.join("config.yaml"), "belongs-to-someone-else").unwrap();

        let error = install(directory.path(), None).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(
            fs::read_to_string(home.join("config.yaml")).unwrap(),
            "belongs-to-someone-else"
        );
        assert!(!home.join("SOUL.md").exists());
    }
}
