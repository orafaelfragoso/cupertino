//! Applications: the applications installed for the session, shown as the virtual folder `applications:///`. Each is
//! listed once under its name, with its own icon, from the desktop entries of `applications/` below every data folder
//! (`$XDG_DATA_HOME` first, then `$XDG_DATA_DIRS`): the first folder that has an entry's id wins, entries that hide
//! themselves (`NoDisplay`, `Hidden`, `OnlyShowIn`, `NotShowIn`, a failing `TryExec`) and command-line programs are
//! left out, as the launcher leaves them out, and so is every file that is not an application's entry (association
//! lists, caches). An item is the entry's file: opening it starts the application ([`crate::open::application_at`]).

use crate::entry::Entry;
use crate::natural::NameKey;
use cupertino_apps::entry::{index_applications, xdg_application_dirs, Locale, Visibility};
use std::path::{Path, PathBuf};

/// Where the Applications place points: not a directory, the browser lists the installed applications there.
pub const APPLICATIONS: &str = "applications:///";

/// The type of an application's item.
pub const APPLICATION_TYPE: &str = "application/x-desktop";

/// [`APPLICATIONS`] as a path.
pub fn location() -> &'static Path {
    Path::new(APPLICATIONS)
}

/// The folders the session's applications are installed in, most important first.
pub fn application_dirs() -> Vec<PathBuf> {
    xdg_application_dirs()
}

/// Whether the session has any applications folder: the place is shown only then.
pub fn any_installed(dirs: &[PathBuf]) -> bool {
    dirs.iter().any(|d| d.is_dir())
}

/// The applications installed in `dirs` (most important first), as items named after their entries, for a person in
/// `locale` on a desktop with `visibility`.
pub fn application_entries(
    dirs: &[PathBuf],
    locale: &Locale,
    visibility: &Visibility,
) -> Vec<Entry> {
    index_applications(dirs, locale, visibility)
        .into_iter()
        .filter_map(|app| {
            let mut entry = Entry::from_path(app.source?).ok()?;
            entry.name_key = NameKey::new(&app.name);
            entry.name = app.name;
            entry.hidden = false;
            entry.mime = Some(APPLICATION_TYPE.into());
            // an application that names no icon draws the generic one
            entry.icon = Some(app.icon.unwrap_or_default());
            Some(entry)
        })
        .collect()
}

/// [`application_entries`] of the session's folders, in its locale and on its desktop.
pub fn session_entries(dirs: &[PathBuf]) -> Vec<Entry> {
    application_entries(dirs, &Locale::from_env(), &Visibility::from_env())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn visibility() -> Visibility {
        Visibility {
            current_desktops: vec!["Cupertino".into()],
            path_dirs: Vec::new(),
            skip_try_exec: true,
        }
    }

    fn app(name: &str, extra: &str) -> String {
        format!("[Desktop Entry]\nType=Application\nName={name}\nExec=prog-{name}\nIcon=icon-{name}\n{extra}")
    }

    fn names(entries: &[Entry]) -> Vec<(String, Option<String>)> {
        let mut out: Vec<_> = entries
            .iter()
            .map(|e| (e.name.clone(), e.icon.clone()))
            .collect();
        out.sort();
        out
    }

    #[test]
    fn applications_are_listed_by_name_with_their_icons_and_nothing_else_is() {
        let d = tempfile::tempdir().unwrap();
        let apps = d.path().join("share/applications");
        std::fs::create_dir_all(apps.join("sub")).unwrap();
        std::fs::write(apps.join("org.example.Mail.desktop"), app("Mail", "")).unwrap();
        std::fs::write(apps.join("sub/viewer.desktop"), app("Viewer", "")).unwrap();
        std::fs::write(apps.join("hidden.desktop"), app("Hidden", "Hidden=true\n")).unwrap();
        std::fs::write(
            apps.join("nodisplay.desktop"),
            app("Helper", "NoDisplay=true\n"),
        )
        .unwrap();
        std::fs::write(
            apps.join("elsewhere.desktop"),
            app("Other", "OnlyShowIn=KDE;\n"),
        )
        .unwrap();
        std::fs::write(apps.join("console.desktop"), app("Top", "Terminal=true\n")).unwrap();
        std::fs::write(
            apps.join("link.desktop"),
            "[Desktop Entry]\nType=Link\nName=Site\nURL=https://example.org\n",
        )
        .unwrap();
        std::fs::write(
            apps.join("cupertino-mimeapps.list"),
            "[Default Applications]\n",
        )
        .unwrap();
        std::fs::write(apps.join("mimeinfo.cache"), "[MIME Cache]\n").unwrap();
        let entries = application_entries(
            std::slice::from_ref(&apps),
            &Locale::default(),
            &visibility(),
        );
        assert_eq!(
            names(&entries),
            [
                ("Mail".to_owned(), Some("icon-Mail".to_owned())),
                ("Viewer".to_owned(), Some("icon-Viewer".to_owned())),
            ]
        );
        let mail = entries.iter().find(|e| e.name == "Mail").unwrap();
        assert_eq!(mail.path, apps.join("org.example.Mail.desktop"));
        assert_eq!(mail.mime.as_deref(), Some(APPLICATION_TYPE));
        assert!(!mail.is_dir);
    }

    #[test]
    fn the_first_folder_with_an_id_wins_and_its_hiding_hides_the_others() {
        let d = tempfile::tempdir().unwrap();
        let (user, system) = (d.path().join("user"), d.path().join("system"));
        std::fs::create_dir_all(&user).unwrap();
        std::fs::create_dir_all(&system).unwrap();
        std::fs::write(user.join("mail.desktop"), app("My Mail", "")).unwrap();
        std::fs::write(system.join("mail.desktop"), app("Mail", "")).unwrap();
        std::fs::write(user.join("editor.desktop"), app("Editor", "Hidden=true\n")).unwrap();
        std::fs::write(system.join("editor.desktop"), app("Editor", "")).unwrap();
        std::fs::write(system.join("player.desktop"), app("Player", "")).unwrap();
        let entries =
            application_entries(&[user.clone(), system], &Locale::default(), &visibility());
        assert_eq!(
            names(&entries)
                .into_iter()
                .map(|(name, _)| name)
                .collect::<Vec<_>>(),
            ["My Mail", "Player"],
            "the user's own entry shadows the system's, and a user's hidden entry removes it"
        );
        let mail = entries.iter().find(|e| e.name == "My Mail").unwrap();
        assert_eq!(mail.path, user.join("mail.desktop"));
    }

    #[test]
    fn a_window_on_applications_is_titled_and_pathed_with_the_places_name() {
        assert_eq!(crate::places::window_title(location(), &[]), "Applications");
        assert_eq!(
            crate::window::path_chain(location(), &[]),
            [(location().to_path_buf(), "Applications".to_owned())]
        );
    }

    #[test]
    fn names_are_localised_and_the_place_needs_a_folder() {
        let d = tempfile::tempdir().unwrap();
        let apps = d.path().join("applications");
        assert!(!any_installed(std::slice::from_ref(&apps)));
        std::fs::create_dir_all(&apps).unwrap();
        assert!(any_installed(&[d.path().join("missing"), apps.clone()]));
        std::fs::write(apps.join("mail.desktop"), app("Mail", "Name[de]=Post\n")).unwrap();
        let entries = application_entries(&[apps], &Locale::parse("de_DE.UTF-8"), &visibility());
        assert_eq!(entries[0].name, "Post");
    }
}
