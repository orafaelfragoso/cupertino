//! Copy generated files into XDG directories. Only used on explicit request (`--install`); the
//! target directories are always explicit parameters, so tests never touch the real home.
//!
//! Mapping (generated path → target):
//!
//! | generated | target |
//! |---|---|
//! | `themes/**` | `<data_home>/themes/**` |
//! | `config/**` | `<config_home>/**` |
//! | `qt/color-schemes/*` | `<data_home>/color-schemes/*` |
//! | `qt/qt5ct/colors/*`, `qt/qt6ct/colors/*` | `<config_home>/qt5ct/colors/*`, `<config_home>/qt6ct/colors/*` |
//! | `fontconfig/conf.d/*` | `<config_home>/fontconfig/conf.d/*` |
//!
//! `README.txt` and `*.snippet` files are never installed. An existing different file is kept
//! unless `force` is set; with `force` it is first renamed to `<name>.bak`.

use crate::generate::GeneratedFile;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Targets {
    pub data_home: PathBuf,
    pub config_home: PathBuf,
}

impl Targets {
    /// `XDG_DATA_HOME` / `XDG_CONFIG_HOME` with the usual `$HOME` fallbacks; `home` overrides `$HOME`.
    pub fn from_env(home: Option<&Path>) -> Option<Targets> {
        let home = home.map(Path::to_path_buf).or_else(|| std::env::var_os("HOME").map(PathBuf::from))?;
        let pick = |var: &str, fallback: &str| match (home_override(home.as_path(), var), std::env::var_os(var)) {
            (true, _) => home.join(fallback),
            (false, Some(v)) if !v.is_empty() => PathBuf::from(v),
            _ => home.join(fallback),
        };
        Some(Targets { data_home: pick("XDG_DATA_HOME", ".local/share"), config_home: pick("XDG_CONFIG_HOME", ".config") })
    }
}

fn home_override(home: &Path, _var: &str) -> bool {
    std::env::var_os("HOME").map(PathBuf::from).as_deref() != Some(home)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InstallOptions {
    pub force: bool,
    pub dry_run: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Written(PathBuf),
    Unchanged(PathBuf),
    SkippedExisting(PathBuf),
    BackedUpAndWritten { path: PathBuf, backup: PathBuf },
}

/// Target path for a generated file, `None` for files that are not installed.
pub fn target_for(rel: &Path, t: &Targets) -> Option<PathBuf> {
    let s = rel.to_string_lossy();
    if s == "README.txt" || s.ends_with(".snippet") {
        return None;
    }
    let strip = |prefix: &str| rel.strip_prefix(prefix).ok().map(Path::to_path_buf);
    if let Some(r) = strip("themes") {
        return Some(t.data_home.join("themes").join(r));
    }
    if let Some(r) = strip("config") {
        return Some(t.config_home.join(r));
    }
    if let Some(r) = strip("qt/color-schemes") {
        return Some(t.data_home.join("color-schemes").join(r));
    }
    for tool in ["qt5ct", "qt6ct"] {
        if let Some(r) = strip(&format!("qt/{tool}/colors")) {
            return Some(t.config_home.join(tool).join("colors").join(r));
        }
    }
    strip("fontconfig/conf.d").map(|r| t.config_home.join("fontconfig/conf.d").join(r))
}

pub fn install(files: &[GeneratedFile], t: &Targets, o: InstallOptions) -> std::io::Result<Vec<Action>> {
    let mut actions = Vec::new();
    for f in files {
        let Some(dest) = target_for(&f.path, t) else { continue };
        let existing = std::fs::read_to_string(&dest).ok();
        if existing.as_deref() == Some(f.contents.as_str()) {
            actions.push(Action::Unchanged(dest));
            continue;
        }
        if existing.is_some() || dest.exists() {
            if !o.force {
                actions.push(Action::SkippedExisting(dest));
                continue;
            }
            let backup = PathBuf::from(format!("{}.bak", dest.display()));
            if !o.dry_run {
                std::fs::rename(&dest, &backup)?;
            }
            actions.push(Action::BackedUpAndWritten { path: dest.clone(), backup });
        } else {
            actions.push(Action::Written(dest.clone()));
        }
        if !o.dry_run {
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&dest, &f.contents)?;
        }
    }
    Ok(actions)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn targets(root: &Path) -> Targets {
        Targets { data_home: root.join("data"), config_home: root.join("config") }
    }

    fn f(path: &str, c: &str) -> GeneratedFile {
        GeneratedFile { path: path.into(), contents: c.into() }
    }

    #[test]
    fn target_mapping() {
        let t = targets(Path::new("/x"));
        let m = |p: &str| target_for(Path::new(p), &t).map(|p| p.display().to_string());
        assert_eq!(m("themes/Cupertino/gtk-3.0/gtk.css").as_deref(), Some("/x/data/themes/Cupertino/gtk-3.0/gtk.css"));
        assert_eq!(m("config/gtk-4.0/gtk.css").as_deref(), Some("/x/config/gtk-4.0/gtk.css"));
        assert_eq!(m("qt/color-schemes/CupertinoLight.colors").as_deref(), Some("/x/data/color-schemes/CupertinoLight.colors"));
        assert_eq!(m("qt/qt6ct/colors/CupertinoLight.conf").as_deref(), Some("/x/config/qt6ct/colors/CupertinoLight.conf"));
        assert_eq!(m("qt/qt5ct/colors/CupertinoDark.conf").as_deref(), Some("/x/config/qt5ct/colors/CupertinoDark.conf"));
        assert_eq!(m("fontconfig/conf.d/60.conf").as_deref(), Some("/x/config/fontconfig/conf.d/60.conf"));
        assert_eq!(m("README.txt"), None);
        assert_eq!(m("qt/qt6ct/qt6ct.conf.snippet"), None);
        assert_eq!(m("other/file"), None);
    }

    #[test]
    fn installs_without_overwriting() {
        let d = tempfile::tempdir().unwrap();
        let t = targets(d.path());
        let files = vec![f("config/gtk-4.0/gtk.css", "new"), f("README.txt", "doc")];
        let a = install(&files, &t, InstallOptions::default()).unwrap();
        assert_eq!(a.len(), 1);
        assert!(matches!(a[0], Action::Written(_)));
        assert_eq!(std::fs::read_to_string(d.path().join("config/gtk-4.0/gtk.css")).unwrap(), "new");
        assert!(!d.path().join("README.txt").exists());
        let again = install(&files, &t, InstallOptions::default()).unwrap();
        assert!(matches!(again[0], Action::Unchanged(_)));
        let changed = vec![f("config/gtk-4.0/gtk.css", "newer")];
        let skipped = install(&changed, &t, InstallOptions::default()).unwrap();
        assert!(matches!(skipped[0], Action::SkippedExisting(_)));
        assert_eq!(std::fs::read_to_string(d.path().join("config/gtk-4.0/gtk.css")).unwrap(), "new");
    }

    #[test]
    fn force_backs_up_first() {
        let d = tempfile::tempdir().unwrap();
        let t = targets(d.path());
        install(&[f("config/a.css", "old")], &t, InstallOptions::default()).unwrap();
        let a = install(&[f("config/a.css", "fresh")], &t, InstallOptions { force: true, dry_run: false }).unwrap();
        assert!(matches!(a[0], Action::BackedUpAndWritten { .. }));
        assert_eq!(std::fs::read_to_string(d.path().join("config/a.css")).unwrap(), "fresh");
        assert_eq!(std::fs::read_to_string(d.path().join("config/a.css.bak")).unwrap(), "old");
    }

    #[test]
    fn dry_run_touches_nothing() {
        let d = tempfile::tempdir().unwrap();
        let t = targets(d.path());
        let a = install(&[f("themes/X/index.theme", "x")], &t, InstallOptions { force: false, dry_run: true }).unwrap();
        assert!(matches!(a[0], Action::Written(_)));
        assert!(!d.path().join("data").exists());
    }

    #[test]
    fn explicit_home_is_used_for_defaults() {
        let t = Targets::from_env(Some(Path::new("/nonexistent/home"))).unwrap();
        assert_eq!(t.config_home, Path::new("/nonexistent/home/.config"));
        assert_eq!(t.data_home, Path::new("/nonexistent/home/.local/share"));
    }
}
