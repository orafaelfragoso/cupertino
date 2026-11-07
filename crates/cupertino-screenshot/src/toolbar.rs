//! The toolbar's Options menu as a model: its rows for the current settings and what choosing a row changes. The
//! toolbar and the menu are drawn by CupertinoKit (`view`).

use crate::settings::{Location, Settings};
use crate::values::TIMER_CHOICES;

/// What a row of the Options menu changes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuAction {
    Location(Location),
    Timer(u32),
    ShowThumbnail,
    RememberSelection,
    ShowPointer,
}

/// A row of the Options menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuRow {
    /// A section title, not selectable.
    Header(&'static str),
    Choice {
        label: String,
        action: MenuAction,
        checked: bool,
    },
    Separator,
}

/// The Options menu for `settings`, in the design's order (labels and order). Of the save
/// destinations, Desktop, Documents and Clipboard are the design's; Pictures is this platform's picture directory;
/// a directory set in the file is listed by its path. The mail, messaging, preview and microphone entries have no
/// counterpart here and are left out, as is Show Mouse Clicks (it applies to recordings).
pub fn menu(settings: &Settings) -> Vec<MenuRow> {
    let choice = |label: String, action: MenuAction, checked: bool| MenuRow::Choice {
        label,
        action,
        checked,
    };
    let mut rows = vec![MenuRow::Header("Save to")];
    let mut locations = vec![
        ("Desktop".to_string(), Location::Desktop),
        ("Documents".into(), Location::Documents),
        ("Pictures".into(), Location::Pictures),
        ("Clipboard".into(), Location::Clipboard),
    ];
    if let Location::Directory(path) = &settings.location {
        locations.push((path.display().to_string(), settings.location.clone()));
    }
    for (label, location) in locations {
        let checked = location == settings.location;
        rows.push(choice(label, MenuAction::Location(location), checked));
    }
    rows.push(MenuRow::Separator);
    rows.push(MenuRow::Header("Timer"));
    for seconds in TIMER_CHOICES {
        let label = if seconds == 0 {
            "None".to_string()
        } else {
            format!("{seconds} Seconds")
        };
        rows.push(choice(
            label,
            MenuAction::Timer(seconds),
            settings.timer == seconds,
        ));
    }
    rows.push(MenuRow::Separator);
    rows.push(MenuRow::Header("Options"));
    rows.push(choice(
        "Show Floating Thumbnail".into(),
        MenuAction::ShowThumbnail,
        settings.show_thumbnail,
    ));
    rows.push(choice(
        "Remember Last Selection".into(),
        MenuAction::RememberSelection,
        settings.remember_selection,
    ));
    rows.push(choice(
        "Show Mouse Pointer".into(),
        MenuAction::ShowPointer,
        settings.show_pointer,
    ));
    rows
}

/// `settings` with `action` applied: a destination or timer is chosen, a switch flips.
pub fn apply(settings: &mut Settings, action: &MenuAction) {
    match action {
        MenuAction::Location(location) => settings.location = location.clone(),
        MenuAction::Timer(seconds) => settings.timer = *seconds,
        MenuAction::ShowThumbnail => settings.show_thumbnail = !settings.show_thumbnail,
        MenuAction::RememberSelection => settings.remember_selection = !settings.remember_selection,
        MenuAction::ShowPointer => settings.show_pointer = !settings.show_pointer,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_menu_checks_the_current_choices_and_applying_a_row_changes_the_settings() {
        let mut s = Settings::default();
        let rows = menu(&s);
        let checked: Vec<&str> = rows
            .iter()
            .filter_map(|r| match r {
                MenuRow::Choice {
                    label,
                    checked: true,
                    ..
                } => Some(label.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            checked,
            [
                "Desktop",
                "None",
                "Show Floating Thumbnail",
                "Remember Last Selection"
            ]
        );
        apply(&mut s, &MenuAction::Timer(10));
        apply(&mut s, &MenuAction::ShowPointer);
        apply(&mut s, &MenuAction::Location(Location::Clipboard));
        assert_eq!(
            (s.timer, s.show_pointer, s.location),
            (10, true, Location::Clipboard)
        );
    }

    #[test]
    fn the_menu_has_three_sections_and_lists_a_directory_of_the_file() {
        let rows = menu(&Settings::default());
        let headers: Vec<&str> = rows
            .iter()
            .filter_map(|r| match r {
                MenuRow::Header(h) => Some(*h),
                _ => None,
            })
            .collect();
        assert_eq!(headers, ["Save to", "Timer", "Options"]);
        assert_eq!(rows.iter().filter(|r| **r == MenuRow::Separator).count(), 2);
        let settings = Settings {
            location: Location::Directory("/srv/shots".into()),
            ..Settings::default()
        };
        assert!(menu(&settings).contains(&MenuRow::Choice {
            label: "/srv/shots".into(),
            action: MenuAction::Location(Location::Directory("/srv/shots".into())),
            checked: true,
        }));
    }
}
