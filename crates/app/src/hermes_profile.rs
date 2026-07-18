use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};

use fs2::FileExt;

const CONFIG: &str = include_str!("../../../hermes/ainews/config.yaml");
const SOUL: &str = include_str!("../../../hermes/ainews/SOUL.md");
const USER_MEMORY: &str = include_str!("../../../hermes/ainews/memories/USER.md");
const PROFILE_METADATA: &str = "description: Evidence-grounded research for the Not News living graph.\ndescription_auto: false\n";
const OWNERSHIP_MARKER_V1: &str = "not-news-profile-v1\n";
const OWNERSHIP_MARKER_V2: &str = "not-news-profile-v2\n";
const POLICY_MARKER_V2: &str = "not-news-policy-v2\nterminal.home_mode=profile\n";
const INSTALLING_DIRECTORY: &str = ".not-news.installing";
const INSTALL_LOCK: &str = ".not-news.install.lock";
const POLICY_MARKER: &str = ".not-news-policy-v2";

pub const PROFILE_ID: &str = "not-news";
pub const POLICY_VERSION: u32 = 2;

#[derive(Debug, Eq, PartialEq)]
pub struct InstalledProfile {
    pub root: PathBuf,
    pub home: PathBuf,
    pub policy_version: u32,
}

/// Installs or advances only the Not News-owned profile policy. The lock and
/// same-directory staging path make ownership establishment atomic across
/// processes; an interrupted owned stage is completed on the next launch.
/// Hermes authentication, providers, sessions, memories, logs, `default`, and
/// unrelated profiles are never copied or selected.
pub fn install(
    root: &Path,
    legacy_application_root: Option<&Path>,
) -> io::Result<InstalledProfile> {
    create_private_dir_all(root)?;
    let profiles = root.join("profiles");
    create_private_dir_all(&profiles)?;
    let lock = open_private_file(&profiles.join(INSTALL_LOCK), false)?;
    lock.lock_exclusive()?;

    let home = profiles.join(PROFILE_ID);
    if !home.exists() {
        finish_interrupted_or_create(&profiles, &home, legacy_application_root)?;
    }
    if !home.is_dir() || !profile_is_owned(&home) {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!(
                "Hermes profile '{}' exists without an unambiguous Not News ownership marker",
                home.display()
            ),
        ));
    }
    prepare_owned_profile(&home)?;
    install_policy_marker(&home)?;
    sync_directory(&home)?;
    Ok(InstalledProfile {
        root: root.to_path_buf(),
        home,
        policy_version: POLICY_VERSION,
    })
}

fn finish_interrupted_or_create(
    profiles: &Path,
    home: &Path,
    legacy_application_root: Option<&Path>,
) -> io::Result<()> {
    let stage = profiles.join(INSTALLING_DIRECTORY);
    if stage.exists() {
        if !stage.is_dir() || !profile_is_owned(&stage) {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!(
                    "interrupted profile stage '{}' is not provably owned by Not News",
                    stage.display()
                ),
            ));
        }
    } else if let Some(source) = legacy_profile_source(legacy_application_root) {
        install_once(&source.join(".not-news-profile"), OWNERSHIP_MARKER_V2)?;
        fs::rename(&source, &stage)?;
    } else {
        create_private_dir(&stage)?;
        install_once(&stage.join(".not-news-profile"), OWNERSHIP_MARKER_V2)?;
    }
    prepare_owned_profile(&stage)?;
    install_policy_marker(&stage)?;
    sync_directory(&stage)?;
    fs::rename(&stage, home)?;
    sync_directory(profiles)
}

fn legacy_profile_source(legacy_application_root: Option<&Path>) -> Option<PathBuf> {
    let legacy_root = legacy_application_root?;
    let named = legacy_root.join("profiles").join(PROFILE_ID);
    if named.is_dir() {
        return Some(named);
    }
    let pre_profile = legacy_root.join("ainews");
    pre_profile.is_dir().then_some(pre_profile)
}

fn prepare_owned_profile(home: &Path) -> io::Result<()> {
    create_private_dir_all(&home.join("memories"))?;
    create_private_dir_all(&home.join("sessions"))?;
    create_private_dir_all(&home.join("logs"))?;
    install_once(&home.join("config.yaml"), CONFIG)?;
    install_once(&home.join("SOUL.md"), SOUL)?;
    install_once(&home.join("memories").join("USER.md"), USER_MEMORY)?;
    install_once(&home.join(".no-bundled-skills"), "")?;
    install_once(&home.join("profile.yaml"), PROFILE_METADATA)
}

fn install_policy_marker(home: &Path) -> io::Result<()> {
    let marker = home.join(POLICY_MARKER);
    match fs::read_to_string(&marker) {
        Ok(contents) if contents == POLICY_MARKER_V2 => return Ok(()),
        Ok(_) => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "owned policy marker '{}' has unknown contents",
                    marker.display()
                ),
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let temporary = home.join(".not-news-policy-v2.pending");
    match fs::remove_file(&temporary) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    install_once(&temporary, POLICY_MARKER_V2)?;
    fs::rename(&temporary, &marker)?;
    sync_directory(home)
}

pub fn profile_is_owned(home: &Path) -> bool {
    fs::read_to_string(home.join(".not-news-profile")).is_ok_and(|contents| {
        matches!(contents.as_str(), OWNERSHIP_MARKER_V1 | OWNERSHIP_MARKER_V2)
    })
}

pub fn owned_profile(root: &Path) -> io::Result<Option<PathBuf>> {
    let home = root.join("profiles").join(PROFILE_ID);
    if !home.exists() {
        return Ok(None);
    }
    if home.is_dir() && profile_is_owned(&home) {
        return Ok(Some(home));
    }
    Err(io::Error::new(
        io::ErrorKind::PermissionDenied,
        format!(
            "refusing to treat '{}' as Not News state without its ownership marker",
            home.display()
        ),
    ))
}

/// Removes only state carrying an exact Not News ownership marker. The same
/// lock as installation excludes a concurrent profile seed or policy update;
/// ambiguous profile and staging directories stop the operation unchanged.
pub fn erase_owned(root: &Path) -> io::Result<bool> {
    let profiles = root.join("profiles");
    if !profiles.exists() {
        return Ok(false);
    }
    let lock_path = profiles.join(INSTALL_LOCK);
    let lock = open_private_file(&lock_path, false)?;
    lock.lock_exclusive()?;
    let home = owned_profile(root)?;
    let stage = profiles.join(INSTALLING_DIRECTORY);
    if stage.exists() && (!stage.is_dir() || !profile_is_owned(&stage)) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "refusing complete erase because '{}' lacks an exact Not News ownership marker",
                stage.display()
            ),
        ));
    }
    if let Some(home) = home.as_ref() {
        fs::remove_dir_all(home)?;
    }
    if stage.exists() {
        fs::remove_dir_all(stage)?;
    }
    sync_directory(&profiles)?;
    FileExt::unlock(&lock)?;
    drop(lock);
    match fs::remove_file(lock_path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    Ok(home.is_some())
}

fn open_private_file(path: &Path, create_new: bool) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    if create_new {
        options.create_new(true);
    } else {
        options.create(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    options.open(path)
}

fn install_once(path: &Path, contents: &str) -> io::Result<()> {
    match open_private_file(path, true) {
        Ok(mut file) => {
            file.write_all(contents.as_bytes())?;
            file.sync_all()
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(error),
    }
}

fn create_private_dir_all(path: &Path) -> io::Result<()> {
    if path.is_dir() {
        return Ok(());
    }
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        create_private_dir_all(parent)?;
    }
    create_private_dir(path)
}

fn create_private_dir(path: &Path) -> io::Result<()> {
    match fs::create_dir(path) {
        Ok(()) => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
            }
            Ok(())
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists && path.is_dir() => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> io::Result<()> {
    File::open(path)?.sync_all()
}

#[cfg(windows)]
fn sync_directory(_path: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Barrier};
    use std::thread;

    use tempfile::TempDir;

    use super::*;

    #[test]
    fn profile_install_is_complete_and_never_overwrites_private_configuration() {
        let directory = TempDir::new().unwrap();
        let profile = install(directory.path(), None).unwrap();
        let home = &profile.home;
        assert_eq!(profile.policy_version, POLICY_VERSION);
        assert!(
            fs::read_to_string(home.join("config.yaml"))
                .unwrap()
                .contains("home_mode: profile")
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
    }

    #[test]
    fn version_one_profile_advances_policy_without_rewriting_user_files() {
        let directory = TempDir::new().unwrap();
        let home = directory.path().join("profiles/not-news");
        fs::create_dir_all(home.join("memories")).unwrap();
        fs::write(home.join(".not-news-profile"), OWNERSHIP_MARKER_V1).unwrap();
        fs::write(home.join("config.yaml"), "user-authored-config").unwrap();
        fs::write(home.join("memories/USER.md"), "user-authored-memory").unwrap();

        let installed = install(directory.path(), None).unwrap();

        assert_eq!(installed.policy_version, POLICY_VERSION);
        assert_eq!(
            fs::read_to_string(home.join("config.yaml")).unwrap(),
            "user-authored-config"
        );
        assert_eq!(
            fs::read_to_string(home.join("memories/USER.md")).unwrap(),
            "user-authored-memory"
        );
        assert_eq!(
            fs::read_to_string(home.join(POLICY_MARKER)).unwrap(),
            POLICY_MARKER_V2
        );
    }

    #[test]
    fn complete_erase_requires_ownership_and_preserves_default() {
        let directory = tempfile::TempDir::new().unwrap();
        let root = directory.path().join("hermes");
        let default = root.join("profiles/default");
        fs::create_dir_all(&default).unwrap();
        fs::write(default.join("sentinel"), "keep").unwrap();
        let installed = install(&root, None).unwrap();
        assert!(erase_owned(&root).unwrap());
        assert!(!installed.home.exists());
        assert_eq!(
            fs::read_to_string(default.join("sentinel")).unwrap(),
            "keep"
        );

        let collision = root.join("profiles/not-news");
        fs::create_dir(&collision).unwrap();
        fs::write(collision.join("private"), "unowned").unwrap();
        assert!(erase_owned(&root).is_err());
        assert_eq!(
            fs::read_to_string(collision.join("private")).unwrap(),
            "unowned"
        );
    }

    #[test]
    fn simultaneous_first_launches_observe_one_complete_owned_profile() {
        let directory = TempDir::new().unwrap();
        let root = directory.path().join("hermes");
        let barrier = Arc::new(Barrier::new(16));
        let handles = (0..16)
            .map(|_| {
                let root = root.clone();
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    barrier.wait();
                    install(&root, None)
                })
            })
            .collect::<Vec<_>>();

        for handle in handles {
            assert_eq!(
                handle.join().unwrap().unwrap().policy_version,
                POLICY_VERSION
            );
        }
        let home = root.join("profiles/not-news");
        assert!(profile_is_owned(&home));
        assert!(
            fs::read_to_string(home.join("SOUL.md"))
                .unwrap()
                .contains("living graph")
        );
        assert_eq!(
            fs::read_to_string(home.join(POLICY_MARKER)).unwrap(),
            POLICY_MARKER_V2
        );
        assert!(!root.join("profiles").join(INSTALLING_DIRECTORY).exists());
    }

    #[test]
    fn interrupted_owned_stage_is_completed_without_appropriation() {
        let directory = TempDir::new().unwrap();
        let profiles = directory.path().join("profiles");
        let stage = profiles.join(INSTALLING_DIRECTORY);
        fs::create_dir_all(&stage).unwrap();
        fs::write(stage.join(".not-news-profile"), OWNERSHIP_MARKER_V2).unwrap();

        let installed = install(directory.path(), None).unwrap();

        assert_eq!(installed.home, profiles.join(PROFILE_ID));
        assert!(installed.home.join("config.yaml").is_file());
        assert!(!stage.exists());
    }

    #[test]
    fn unowned_existing_profile_and_stage_are_refused_without_mutation() {
        for relative in ["profiles/not-news", "profiles/.not-news.installing"] {
            let directory = TempDir::new().unwrap();
            let path = directory.path().join(relative);
            fs::create_dir_all(&path).unwrap();
            fs::write(path.join("config.yaml"), "belongs-to-someone-else").unwrap();

            let error = install(directory.path(), None).unwrap_err();

            assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
            assert_eq!(
                fs::read_to_string(path.join("config.yaml")).unwrap(),
                "belongs-to-someone-else"
            );
            assert!(!path.join("SOUL.md").exists());
        }
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

    #[cfg(unix)]
    #[test]
    fn newly_created_profile_state_is_private() {
        use std::os::unix::fs::PermissionsExt as _;

        let directory = TempDir::new().unwrap();
        let home = install(&directory.path().join("hermes"), None)
            .unwrap()
            .home;

        assert_eq!(
            fs::metadata(&home).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(home.join("config.yaml"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}
