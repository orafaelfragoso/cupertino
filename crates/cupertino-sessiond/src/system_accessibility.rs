//! The machine's accessibility defaults (`/etc/cupertino/accessibility.conf`, `cupertino_defaults::accessibility`): what
//! an administrator at the console sets from Settings, through `SetSystemAccessibility` ([`crate::service`]). This module
//! holds the parts that need no bus: the bounds and values a call may carry, how often a user may write, and the write.

use std::collections::HashMap;
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use cupertino_defaults::accessibility::{SystemAccessibility, SYSTEM_KEYS};

/// The most bytes of a key or a value a call may carry; a longer one refuses the call before anything else is done.
pub const MAX_FIELD: usize = 64;
/// How often one user may write the file.
pub const PER_USER: Duration = Duration::from_secs(1);
/// How often anyone may write it.
pub const ANYONE: Duration = Duration::from_millis(200);

/// The settings `options` make of `current`, the machine's file as it is: each key must be one of [`SYSTEM_KEYS`] and each
/// value one the file takes. A call with more entries than there are keys, or a field over [`MAX_FIELD`] bytes, is refused
/// before any key is looked at; any bad entry refuses the whole call.
pub fn apply(
    current: SystemAccessibility,
    options: &HashMap<String, String>,
) -> Result<SystemAccessibility, String> {
    if options.len() > SYSTEM_KEYS.len() {
        return Err(format!("at most {} settings", SYSTEM_KEYS.len()));
    }
    if options
        .iter()
        .any(|(k, v)| k.len() > MAX_FIELD || v.len() > MAX_FIELD)
    {
        return Err(format!("a key or value is longer than {MAX_FIELD} bytes"));
    }
    let mut settings = current;
    for (key, value) in options {
        if !SYSTEM_KEYS.contains(&key.as_str()) {
            return Err(format!("{key} is not a setting of the machine's file"));
        }
        settings.set(key, value)?;
    }
    Ok(settings)
}

/// When each user, and anyone, last wrote the file.
#[derive(Debug, Default)]
pub struct Limiter {
    users: HashMap<u32, Instant>,
    anyone: Option<Instant>,
}

impl Limiter {
    /// Whether `uid` may write at `now`; a write that is let through counts.
    pub fn admit(&mut self, uid: u32, now: Instant) -> bool {
        let since =
            |at: Option<&Instant>, gap: Duration| at.is_none_or(|t| now.duration_since(*t) >= gap);
        if !since(self.users.get(&uid), PER_USER) || !since(self.anyone.as_ref(), ANYONE) {
            return false;
        }
        self.users.retain(|_, t| now.duration_since(*t) < PER_USER);
        self.users.insert(uid, now);
        self.anyone = Some(now);
        true
    }
}

/// The directory of the machine's files: `/etc/cupertino`.
pub const MACHINE_DIR: &str = "/etc/cupertino";

/// Make `dir` (mode 755) when it is missing; a link or anything else that is not a directory there is refused. The
/// manager makes it at its start, before any compositor, so every compositor can watch the machine's file in it.
pub fn ensure_dir(dir: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(dir) {
        Ok(meta) if meta.file_type().is_dir() => Ok(()),
        Ok(_) => Err(std::io::Error::other(format!(
            "{} is not a directory",
            dir.display()
        ))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            std::fs::DirBuilder::new().mode(0o755).create(dir)
        }
        Err(e) => Err(e),
    }
}

/// Write `settings` as `accessibility.conf` in `dir` (`/etc/cupertino`), replacing it whole: the directory is made
/// ([`ensure_dir`]) when it is missing; the text goes to a new file made beside it
/// (exclusively, never through a link, mode 644), is flushed, and is renamed over the old one, and the directory is flushed
/// too, so a reader sees the old file or the new one and a crash leaves one of them. Run as root, both are root's.
pub fn write(dir: &Path, settings: &SystemAccessibility) -> std::io::Result<PathBuf> {
    ensure_dir(dir)?;
    let target = dir.join(cupertino_defaults::accessibility::ACCESSIBILITY_FILE);
    let temporary = dir.join(format!(".accessibility.conf.{}", std::process::id()));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o644)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&temporary)?;
        // the mode the creation asked for is narrowed by the umask
        file.set_permissions(std::fs::Permissions::from_mode(0o644))?;
        file.write_all(settings.to_text().as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&temporary, &target)?;
        std::fs::File::open(dir)?.sync_all()
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result.map(|()| target)
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::MetadataExt;

    use super::*;

    fn options(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn only_allowed_keys_with_good_values_and_within_bounds_change_the_settings() {
        let on = apply(
            SystemAccessibility::default(),
            &options(&[("reduce_motion", "on")]),
        )
        .unwrap();
        assert!(on.settings.display.reduce_motion);
        let reader = apply(on, &options(&[("screen_reader_at_login", "on")])).unwrap();
        assert!(reader.screen_reader_at_login && reader.settings.display.reduce_motion);
        let off = apply(on, &options(&[("reduce_motion", "off")])).unwrap();
        assert!(!off.settings.display.reduce_motion);
        assert_eq!(
            apply(on, &HashMap::new()),
            Ok(on),
            "nothing asked, nothing changed"
        );
        for bad in [
            options(&[("invert_colors", "on")]),
            options(&[("reduce_motion", "maybe")]),
            options(&[("reduce_motion", "on"), ("invert_colors", "on")]),
            options(&[(&"x".repeat(MAX_FIELD + 1), "on")]),
            options(&[("reduce_motion", &"o".repeat(MAX_FIELD + 1))]),
        ] {
            assert!(
                apply(SystemAccessibility::default(), &bad).is_err(),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn a_user_writes_at_most_once_a_second_and_anyone_at_most_five_times() {
        let mut limiter = Limiter::default();
        let start = Instant::now();
        assert!(limiter.admit(1000, start));
        assert!(!limiter.admit(1000, start + Duration::from_millis(900)));
        assert!(
            !limiter.admit(1001, start + Duration::from_millis(100)),
            "anyone's gap"
        );
        assert!(limiter.admit(1001, start + ANYONE));
        assert!(limiter.admit(1000, start + PER_USER));
    }

    #[test]
    fn the_file_is_replaced_whole_with_mode_644_and_only_the_machines_keys() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("cupertino");
        let mut settings = SystemAccessibility::default();
        settings.settings.display.reduce_motion = true;
        settings.settings.invert_colors = true;
        let path = write(&dir, &settings).unwrap();
        assert_eq!(
            std::fs::metadata(&dir).unwrap().mode() & 0o777,
            0o755,
            "made"
        );
        assert_eq!(std::fs::metadata(&path).unwrap().mode() & 0o777, 0o644);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "reduce_motion = on\nscreen_reader_at_login = off\n"
        );
        settings.settings.display.reduce_motion = false;
        write(&dir, &settings).unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "reduce_motion = off\nscreen_reader_at_login = off\n"
        );
        assert_eq!(
            std::fs::read_dir(&dir).unwrap().count(),
            1,
            "no temporary file is left"
        );
    }

    #[test]
    fn the_directory_is_made_when_missing_and_left_as_it_is_otherwise() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("cupertino");
        ensure_dir(&dir).unwrap();
        assert_eq!(std::fs::metadata(&dir).unwrap().mode() & 0o777, 0o755);
        std::fs::write(dir.join("kept"), "").unwrap();
        ensure_dir(&dir).unwrap();
        assert!(dir.join("kept").exists());
        let file = root.path().join("file");
        std::fs::write(&file, "").unwrap();
        assert!(ensure_dir(&file).is_err());
        let link = root.path().join("link");
        std::os::unix::fs::symlink(&dir, &link).unwrap();
        assert!(
            ensure_dir(&link).is_err(),
            "a link is never taken for the directory"
        );
    }

    #[test]
    fn a_link_in_place_of_the_directory_or_the_temporary_file_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let elsewhere = root.path().join("elsewhere");
        std::fs::create_dir(&elsewhere).unwrap();
        let dir = root.path().join("cupertino");
        std::os::unix::fs::symlink(&elsewhere, &dir).unwrap();
        assert!(write(&dir, &SystemAccessibility::default()).is_err());
        assert_eq!(std::fs::read_dir(&elsewhere).unwrap().count(), 0);

        let real = root.path().join("real");
        std::fs::create_dir(&real).unwrap();
        let planted = real.join(format!(".accessibility.conf.{}", std::process::id()));
        std::os::unix::fs::symlink(root.path().join("target"), &planted).unwrap();
        assert!(write(&real, &SystemAccessibility::default()).is_err());
        assert!(
            !root.path().join("target").exists(),
            "the link was not followed"
        );
    }
}
