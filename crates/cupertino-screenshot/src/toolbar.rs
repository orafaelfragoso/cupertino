//! The toolbar of the interactive mode: its items where the design puts them and the Options menu.
//! Positions are logical points in the 628 x 104 toolbar surface.
//!
//! The design gives the items' x relative to the background view (the Capture button then ends exactly at the
//! background's 560 pt width); the mode buttons fill the background's height. The recording items are drawn
//! disabled: this tool does not record the screen.

use cupertino_geometry::{Point, Rect};

use crate::settings::{Location, Mode, Settings};
use crate::values::{TIMER_CHOICES, TOOLBAR_BACKGROUND};

/// A toolbar control.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Item {
    Close,
    Mode(Mode),
    RecordScreen,
    RecordWindow,
    StopRecording,
    RecordSelection,
    Options,
    Capture,
}

impl Item {
    /// Whether the control does something here; the recording controls do not.
    pub fn enabled(self) -> bool {
        !matches!(
            self,
            Item::RecordScreen | Item::RecordWindow | Item::StopRecording | Item::RecordSelection
        )
    }
}

/// The background view, where the material is drawn.
pub fn background() -> Rect {
    let (x, y, w, h) = TOOLBAR_BACKGROUND;
    Rect::new(x, y, w, h)
}

/// A rectangle of the design, `(x, y, width, height)` relative to the background.
fn in_background(x: f64, y: f64, w: f64, h: f64) -> Rect {
    let b = background();
    Rect::new(b.x() + x, b.y() + y, w, h)
}

/// Every control with its rectangle (x and widths).
pub fn items() -> [(Item, Rect); 10] {
    let full = |x: f64, w: f64| in_background(x, 0.0, w, TOOLBAR_BACKGROUND.3);
    [
        (Item::Close, in_background(12.0, 15.0, 15.0, 18.0)),
        (Item::Mode(Mode::Screen), full(31.0, 48.0)),
        (Item::Mode(Mode::Window), full(79.0, 48.0)),
        (Item::Mode(Mode::Selection), full(127.0, 46.0)),
        (Item::RecordScreen, full(189.0, 52.0)),
        (Item::RecordWindow, full(237.0, 54.0)),
        (Item::StopRecording, full(285.0, 48.0)),
        (Item::RecordSelection, full(333.0, 51.5)),
        (Item::Options, full(389.0, 86.0)),
        (Item::Capture, full(474.0, 86.0)),
    ]
}

/// The three separators (5 x 36 at y 6 at 179 and 386, 5 x 48 at 472).
pub fn separators() -> [Rect; 3] {
    [
        in_background(179.0, 6.0, 5.0, 36.0),
        in_background(386.0, 6.0, 5.0, 36.0),
        in_background(472.0, 0.0, 5.0, 48.0),
    ]
}

/// The enabled control under `p`.
pub fn item_at(p: Point) -> Option<Item> {
    items()
        .into_iter()
        .find(|(item, rect)| item.enabled() && rect.contains(p))
        .map(|(item, _)| item)
}

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

/// Menu geometry (approximate: the design draws a standard menu, whose metrics are not part of this analysis).
pub const MENU_WIDTH: f64 = 230.0;
pub const MENU_ROW: f64 = 22.0;
pub const MENU_SEPARATOR: f64 = 11.0;
pub const MENU_PADDING: f64 = 5.0;
pub const MENU_RADIUS: f64 = 8.0;
pub const MENU_FONT_SIZE: f64 = 13.0;

/// The rectangle of every row in a menu surface of [`MENU_WIDTH`], and the menu's height.
pub fn menu_layout(rows: &[MenuRow]) -> (Vec<Rect>, f64) {
    let mut y = MENU_PADDING;
    let rects = rows
        .iter()
        .map(|row| {
            let h = if *row == MenuRow::Separator {
                MENU_SEPARATOR
            } else {
                MENU_ROW
            };
            let r = Rect::new(0.0, y, MENU_WIDTH, h);
            y += h;
            r
        })
        .collect();
    (rects, y + MENU_PADDING)
}

/// The selectable row under `p`, as an index into `rows`.
pub fn menu_row_at(rows: &[MenuRow], p: Point) -> Option<usize> {
    let (rects, _) = menu_layout(rows);
    rects
        .iter()
        .zip(rows)
        .position(|(r, row)| matches!(row, MenuRow::Choice { .. }) && r.contains(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_controls_sit_where_the_nib_puts_them_inside_the_background() {
        let b = background();
        assert_eq!(b, Rect::new(34.0, 28.0, 560.0, 48.0));
        let capture = items()
            .into_iter()
            .find(|(i, _)| *i == Item::Capture)
            .map(|(_, r)| r)
            .expect("a capture button");
        assert_eq!(
            capture.max_x(),
            b.max_x(),
            "the Capture button ends at the background's edge"
        );
        assert_eq!(
            item_at(Point::new(34.0 + 31.0 + 1.0, 50.0)),
            Some(Item::Mode(Mode::Screen))
        );
        assert_eq!(
            item_at(Point::new(34.0 + 130.0, 50.0)),
            Some(Item::Mode(Mode::Selection))
        );
        assert_eq!(
            item_at(Point::new(34.0 + 200.0, 50.0)),
            None,
            "recording is disabled"
        );
        assert_eq!(item_at(Point::new(34.0 + 400.0, 50.0)), Some(Item::Options));
        assert_eq!(
            item_at(Point::new(10.0, 10.0)),
            None,
            "the shadow gutter is not a control"
        );
    }

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
    fn only_choice_rows_are_hit() {
        let rows = menu(&Settings::default());
        // the first row is the "Save to" header, the second Desktop
        assert_eq!(
            menu_row_at(&rows, Point::new(20.0, MENU_PADDING + 1.0)),
            None
        );
        assert_eq!(
            menu_row_at(&rows, Point::new(20.0, MENU_PADDING + MENU_ROW + 1.0)),
            Some(1)
        );
        let (_, height) = menu_layout(&rows);
        assert_eq!(
            height,
            2.0 * MENU_PADDING + 13.0 * MENU_ROW + 2.0 * MENU_SEPARATOR
        );
    }
}
