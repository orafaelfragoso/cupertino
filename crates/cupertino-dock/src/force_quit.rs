//! The Force Quit Applications dialog: the running applications with the ones that stopped answering marked, a Force
//! Quit button, and a second step that asks again before anything is ended. The model and the layout; drawing is in
//! `render::paint_force_quit`, the surface is the process's (`app.rs`).

use cupertino_wm::geometry::{Point, Rect};

/// The list step's panel, in points: the content is 338 wide, 20 from the edges to the heading and the list, with the
/// heading's two lines of 17 and 8 between it and the list (measured); the title bar's height is approximate.
pub const WIDTH: f64 = 338.0;
pub const TITLE_BAR_HEIGHT: f64 = 28.0;
pub const PADDING: f64 = 20.0;
pub const HEADING_HEIGHT: f64 = 34.0;
pub const HEADING_GAP: f64 = 8.0;
pub const LIST_WIDTH: f64 = 298.0;
pub const LIST_HEIGHT: f64 = 168.0;
/// A row is 20 high with 2 between rows.
pub const ROW_HEIGHT: f64 = 22.0;
/// The rows of the list that show at once.
pub const VISIBLE_ROWS: usize = 7;
/// From the list's bottom to the panel's bottom.
pub const FOOTER_AREA: f64 = 57.0;
pub const BUTTON_WIDTH: f64 = 104.0;
pub const BUTTON_HEIGHT: f64 = 32.0;
/// The question step's panel: the icon, the text beside it and the buttons under them.
pub const QUESTION_WIDTH: f64 = 415.0;
pub const QUESTION_HEIGHT: f64 = 119.0;
pub const ICON_SIZE: f64 = 64.0;
/// The panel's corner radius (approximate).
pub const RADIUS: f64 = 12.0;

pub const TITLE: &str = "Force Quit Applications";
pub const HEADING: &str =
    "If an application does not respond for a while, select its name and click Force Quit.";
pub const FOOTER: &str = "You can open this window by pressing Super-Alt-Escape.";
pub const NOT_RESPONDING_SUFFIX: &str = " (not responding)";
pub const FORCE_QUIT: &str = "Force Quit";
pub const CANCEL: &str = "Cancel";
pub const NO_APPLICATIONS: &str = "No applications are open.";
pub const UNSAVED_CHANGES: &str = "You will lose any unsaved changes.";

/// One running application.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub app_id: String,
    pub name: String,
    pub unresponsive: bool,
    /// The file manager: it is listed last.
    pub built_in: bool,
}

/// The second step's question for `entries`.
pub fn question(entries: &[&Entry]) -> String {
    match entries {
        [one] => format!("Do you want to force \u{201c}{}\u{201d} to quit?", one.name),
        many => format!(
            "Do you want to force the {} selected apps to quit?",
            many.len()
        ),
    }
}

/// A key of the dialog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Up,
    Down,
    Home,
    End,
    Return,
    Escape,
}

/// What a click on a row does to the selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Extend {
    /// Select just this row.
    Replace,
    /// Add the row to the selection, or take it out.
    Toggle,
    /// Select every row from the one last chosen to this one.
    Range,
}

/// What a key or a click did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Nothing,
    /// The dialog changed: draw it again.
    Redraw,
    /// The dialog is over.
    Close,
    /// The person confirmed: force these applications to quit.
    Force(Vec<String>),
}

/// A button of the dialog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    /// The first step's Force Quit, and the second step's.
    ForceQuit,
    Cancel,
}

/// An open dialog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dialog {
    /// Alphabetical without regard to case, the file manager last.
    entries: Vec<Entry>,
    /// The application ids of the selected rows.
    selected: Vec<String>,
    /// The row a range selection starts from, and the row the arrow keys move from.
    anchor: Option<String>,
    cursor: Option<String>,
    /// The applications the second step asks about; empty on the first step.
    confirming: Vec<String>,
    /// The first row of the list that shows.
    top: usize,
}

fn sorted(mut entries: Vec<Entry>) -> Vec<Entry> {
    entries.sort_by_cached_key(|e| (e.built_in, e.name.to_lowercase()));
    entries
}

impl Dialog {
    /// A dialog over `entries`; the first unresponsive application, else none, is selected.
    pub fn new(entries: Vec<Entry>) -> Self {
        let entries = sorted(entries);
        let first = entries
            .iter()
            .find(|e| e.unresponsive)
            .map(|e| e.app_id.clone());
        let mut dialog = Dialog {
            entries,
            selected: first.iter().cloned().collect(),
            anchor: first.clone(),
            cursor: first,
            confirming: Vec::new(),
            top: 0,
        };
        dialog.scroll_to_cursor();
        dialog
    }

    /// A dialog that opens on the second step for `app_id` (the dock menu's Force Quit).
    pub fn asking_about(entries: Vec<Entry>, app_id: &str) -> Self {
        let mut dialog = Dialog::new(entries);
        if dialog.index_of(app_id).is_some() {
            dialog.selected = vec![app_id.to_string()];
            dialog.anchor = Some(app_id.to_string());
            dialog.cursor = Some(app_id.to_string());
            dialog.confirming = vec![app_id.to_string()];
            dialog.scroll_to_cursor();
        }
        dialog
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn top(&self) -> usize {
        self.top
    }

    pub fn is_selected(&self, i: usize) -> bool {
        self.entries
            .get(i)
            .is_some_and(|e| self.selected.contains(&e.app_id))
    }

    /// Force Quit is available while at least one row is selected.
    pub fn can_force_quit(&self) -> bool {
        !self.selected.is_empty()
    }

    /// The applications the second step asks about; `None` on the first step.
    pub fn confirming(&self) -> Option<Vec<&Entry>> {
        let asked: Vec<&Entry> = self
            .entries
            .iter()
            .filter(|e| self.confirming.contains(&e.app_id))
            .collect();
        (!asked.is_empty()).then_some(asked)
    }

    fn index_of(&self, app_id: &str) -> Option<usize> {
        self.entries.iter().position(|e| e.app_id == app_id)
    }

    fn cursor_index(&self) -> Option<usize> {
        self.cursor.as_deref().and_then(|id| self.index_of(id))
    }

    /// The applications changed (one started or quit, one stopped answering): the selection stays on the same
    /// applications, and the second step ends when all of its applications are gone.
    pub fn set_entries(&mut self, entries: Vec<Entry>) {
        self.entries = sorted(entries);
        let present = |ids: &mut Vec<String>, entries: &[Entry]| {
            ids.retain(|id| entries.iter().any(|e| &e.app_id == id));
        };
        present(&mut self.selected, &self.entries);
        present(&mut self.confirming, &self.entries);
        for id in [&mut self.anchor, &mut self.cursor] {
            if id
                .as_deref()
                .is_some_and(|id| !self.entries.iter().any(|e| e.app_id == id))
            {
                *id = None;
            }
        }
        self.top = self
            .top
            .min(self.entries.len().saturating_sub(VISIBLE_ROWS));
        self.scroll_to_cursor();
    }

    fn scroll_to_cursor(&mut self) {
        if let Some(i) = self.cursor_index() {
            if i < self.top {
                self.top = i;
            } else if i >= self.top + VISIBLE_ROWS {
                self.top = i + 1 - VISIBLE_ROWS;
            }
        }
    }

    fn select_range(&mut self, to: usize) {
        let from = self
            .anchor
            .as_deref()
            .and_then(|id| self.index_of(id))
            .unwrap_or(to);
        let (low, high) = (from.min(to), from.max(to));
        self.selected = self.entries[low..=high]
            .iter()
            .map(|e| e.app_id.clone())
            .collect();
        self.anchor = Some(self.entries[from].app_id.clone());
        self.cursor = Some(self.entries[to].app_id.clone());
    }

    /// Select row `i` (a click); nothing happens on the second step.
    pub fn select(&mut self, i: usize, how: Extend) -> Outcome {
        if self.confirming().is_some() || i >= self.entries.len() {
            return Outcome::Nothing;
        }
        let before = (self.selected.clone(), self.cursor.clone());
        let id = self.entries[i].app_id.clone();
        match how {
            Extend::Replace => {
                self.selected = vec![id.clone()];
                self.anchor = Some(id.clone());
                self.cursor = Some(id);
            }
            Extend::Toggle => {
                match self.selected.iter().position(|s| *s == id) {
                    Some(at) => {
                        self.selected.remove(at);
                    }
                    None => self.selected.push(id.clone()),
                }
                self.anchor = Some(id.clone());
                self.cursor = Some(id);
            }
            Extend::Range => self.select_range(i),
        }
        self.scroll_to_cursor();
        if before == (self.selected.clone(), self.cursor.clone()) {
            Outcome::Nothing
        } else {
            Outcome::Redraw
        }
    }

    /// Scroll the list by `rows` (the wheel).
    pub fn scroll(&mut self, rows: isize) -> Outcome {
        let last = self.entries.len().saturating_sub(VISIBLE_ROWS);
        let to = self.top.saturating_add_signed(rows).min(last);
        if to == self.top || self.confirming().is_some() {
            return Outcome::Nothing;
        }
        self.top = to;
        Outcome::Redraw
    }

    /// The Force Quit button: ask about the selected applications.
    pub fn force_quit_pressed(&mut self) -> Outcome {
        if self.confirming().is_some() || !self.can_force_quit() {
            return Outcome::Nothing;
        }
        self.confirming = self
            .entries
            .iter()
            .filter(|e| self.selected.contains(&e.app_id))
            .map(|e| e.app_id.clone())
            .collect();
        Outcome::Redraw
    }

    /// The second step's Force Quit button.
    pub fn confirm_pressed(&mut self) -> Outcome {
        if self.confirming().is_none() {
            return Outcome::Nothing;
        }
        Outcome::Force(std::mem::take(&mut self.confirming))
    }

    /// The second step's Cancel button: back to the list.
    pub fn cancel_pressed(&mut self) -> Outcome {
        if self.confirming().is_none() {
            return Outcome::Nothing;
        }
        self.confirming.clear();
        Outcome::Redraw
    }

    /// The result of `key`; `extend` (Shift held) grows the selection from its anchor. In the list the arrows move,
    /// Return asks about the selected applications and Escape closes; in the second step Cancel is the default, so
    /// Return and Escape go back to the list.
    pub fn key(&mut self, key: Key, extend: bool) -> Outcome {
        if self.confirming().is_some() {
            return match key {
                Key::Return | Key::Escape => self.cancel_pressed(),
                _ => Outcome::Nothing,
            };
        }
        let last = self.entries.len().checked_sub(1);
        let cursor = self.cursor_index();
        let to = match (key, cursor, last) {
            (Key::Escape, ..) => return Outcome::Close,
            (Key::Return, ..) => return self.force_quit_pressed(),
            (_, _, None) => return Outcome::Nothing,
            (Key::Up, None, Some(last)) => last,
            (Key::Up, Some(i), _) => i.saturating_sub(1),
            (Key::Down, None, _) => 0,
            (Key::Down, Some(i), Some(last)) => (i + 1).min(last),
            (Key::Home, ..) => 0,
            (Key::End, _, Some(last)) => last,
        };
        self.select(
            to,
            if extend {
                Extend::Range
            } else {
                Extend::Replace
            },
        )
    }
}

/// The list step: where its parts are, in points from the panel's top-left corner.
#[derive(Clone, Debug, PartialEq)]
pub struct ListLayout {
    pub width: f64,
    pub height: f64,
    pub title_bar: Rect,
    pub heading: Rect,
    /// The list's area; its rows start at its top.
    pub list: Rect,
    /// The rows shown, as indexes into the entries, with their rectangles.
    pub rows: Vec<(usize, Rect)>,
    pub footer: Rect,
    pub force_quit: Rect,
}

/// The second step: the icon, the question, what is lost, and the buttons.
#[derive(Clone, Debug, PartialEq)]
pub struct QuestionLayout {
    pub width: f64,
    pub height: f64,
    pub icon: Rect,
    pub question: Rect,
    pub detail: Rect,
    pub cancel: Rect,
    pub confirm: Rect,
}

/// Where the parts of the panel are at the dialog's step.
#[derive(Clone, Debug, PartialEq)]
pub enum Layout {
    List(ListLayout),
    Question(QuestionLayout),
}

impl Layout {
    pub fn for_dialog(dialog: &Dialog) -> Self {
        if dialog.confirming().is_some() {
            Layout::Question(QuestionLayout::new())
        } else {
            Layout::List(ListLayout::new(dialog.entries().len(), dialog.top()))
        }
    }

    pub fn size(&self) -> (f64, f64) {
        match self {
            Layout::List(l) => (l.width, l.height),
            Layout::Question(q) => (q.width, q.height),
        }
    }

    /// The row under `p`; only the list step has rows.
    pub fn row_at(&self, p: Point) -> Option<usize> {
        match self {
            Layout::List(l) => l
                .rows
                .iter()
                .find(|(_, rect)| rect.contains(p))
                .map(|(i, _)| *i),
            Layout::Question(_) => None,
        }
    }

    /// The button under `p`.
    pub fn button_at(&self, p: Point) -> Option<Button> {
        match self {
            Layout::List(l) => l.force_quit.contains(p).then_some(Button::ForceQuit),
            Layout::Question(q) if q.cancel.contains(p) => Some(Button::Cancel),
            Layout::Question(q) => q.confirm.contains(p).then_some(Button::ForceQuit),
        }
    }
}

impl ListLayout {
    /// The layout for a list of `entries` rows, the first shown being `top`.
    pub fn new(entries: usize, top: usize) -> Self {
        let title_bar = Rect::new(0.0, 0.0, WIDTH, TITLE_BAR_HEIGHT);
        let heading = Rect::new(
            PADDING,
            title_bar.bottom() + PADDING,
            LIST_WIDTH,
            HEADING_HEIGHT,
        );
        let list = Rect::new(
            PADDING,
            heading.bottom() + HEADING_GAP,
            LIST_WIDTH,
            LIST_HEIGHT,
        );
        let rows = (top..entries.min(top + VISIBLE_ROWS))
            .enumerate()
            .map(|(k, i)| {
                (
                    i,
                    Rect::new(list.x, list.y + k as f64 * ROW_HEIGHT, list.w, ROW_HEIGHT),
                )
            })
            .collect();
        let height = list.bottom() + FOOTER_AREA;
        ListLayout {
            width: WIDTH,
            height,
            title_bar,
            heading,
            list,
            rows,
            footer: Rect::new(18.0, height - 49.0, 198.0, 26.0),
            force_quit: Rect::new(220.0, height - 45.0, BUTTON_WIDTH, BUTTON_HEIGHT),
        }
    }
}

impl QuestionLayout {
    pub fn new() -> Self {
        QuestionLayout {
            width: QUESTION_WIDTH,
            height: QUESTION_HEIGHT,
            icon: Rect::new(20.0, 17.0, ICON_SIZE, ICON_SIZE),
            question: Rect::new(101.0, 17.0, 296.0, 17.0),
            detail: Rect::new(101.0, 42.0, 296.0, 14.0),
            cancel: Rect::new(193.0, 74.0, BUTTON_WIDTH, BUTTON_HEIGHT),
            confirm: Rect::new(297.0, 74.0, BUTTON_WIDTH, BUTTON_HEIGHT),
        }
    }
}

impl Default for QuestionLayout {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, name: &str, unresponsive: bool) -> Entry {
        Entry {
            app_id: id.to_string(),
            name: name.to_string(),
            unresponsive,
            built_in: false,
        }
    }

    fn apps(n: usize) -> Vec<Entry> {
        (0..n)
            .map(|i| entry(&format!("app{i:02}"), &format!("App {i:02}"), false))
            .collect()
    }

    fn selected_ids(d: &Dialog) -> Vec<&str> {
        (0..d.entries().len())
            .filter(|&i| d.is_selected(i))
            .map(|i| d.entries()[i].app_id.as_str())
            .collect()
    }

    #[test]
    fn rows_are_alphabetical_without_regard_to_case_with_the_file_manager_last() {
        let mut files = entry("files", "Files", false);
        files.built_in = true;
        let d = Dialog::new(vec![
            files,
            entry("z", "zed", false),
            entry("b", "Beta", false),
            entry("a", "alpha", false),
        ]);
        let names: Vec<&str> = d.entries().iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["alpha", "Beta", "zed", "Files"]);
    }

    #[test]
    fn the_first_application_that_does_not_answer_is_selected_and_the_arrows_move_without_wrapping()
    {
        let mut d = Dialog::new(vec![
            entry("a", "A", false),
            entry("b", "B", true),
            entry("c", "C", true),
        ]);
        assert_eq!(selected_ids(&d), ["b"]);
        assert_eq!(d.key(Key::Down, false), Outcome::Redraw);
        assert_eq!(selected_ids(&d), ["c"]);
        assert_eq!(d.key(Key::Down, false), Outcome::Nothing);
        assert_eq!(d.key(Key::Home, false), Outcome::Redraw);
        assert_eq!(selected_ids(&d), ["a"]);
        assert_eq!(d.key(Key::Up, false), Outcome::Nothing);
    }

    #[test]
    fn force_quit_needs_a_selection() {
        let mut d = Dialog::new(apps(2));
        assert!(!d.can_force_quit());
        assert_eq!(d.force_quit_pressed(), Outcome::Nothing);
        assert_eq!(d.key(Key::Return, false), Outcome::Nothing);
        d.select(1, Extend::Replace);
        assert!(d.can_force_quit());
    }

    #[test]
    fn a_click_replaces_the_selection_control_toggles_and_shift_takes_a_range() {
        let mut d = Dialog::new(apps(6));
        d.select(1, Extend::Replace);
        d.select(3, Extend::Toggle);
        assert_eq!(selected_ids(&d), ["app01", "app03"]);
        d.select(1, Extend::Toggle);
        assert_eq!(selected_ids(&d), ["app03"]);
        d.select(5, Extend::Range);
        assert_eq!(
            selected_ids(&d),
            ["app01", "app02", "app03", "app04", "app05"]
        );
        d.select(0, Extend::Range);
        assert_eq!(selected_ids(&d), ["app00", "app01"]);
        d.select(2, Extend::Replace);
        assert_eq!(selected_ids(&d), ["app02"]);
    }

    #[test]
    fn shift_and_the_arrows_grow_the_selection_from_its_anchor() {
        let mut d = Dialog::new(apps(5));
        d.select(1, Extend::Replace);
        d.key(Key::Down, true);
        d.key(Key::Down, true);
        assert_eq!(selected_ids(&d), ["app01", "app02", "app03"]);
        d.key(Key::Up, true);
        assert_eq!(selected_ids(&d), ["app01", "app02"]);
        d.key(Key::Down, false);
        assert_eq!(selected_ids(&d), ["app03"]);
    }

    #[test]
    fn return_asks_about_the_selected_applications_and_only_the_second_step_forces_them() {
        let mut d = Dialog::new(apps(4));
        d.select(1, Extend::Replace);
        d.select(2, Extend::Toggle);
        assert_eq!(d.key(Key::Return, false), Outcome::Redraw);
        let asked = d.confirming().expect("the second step");
        assert_eq!(
            question(&asked),
            "Do you want to force the 2 selected apps to quit?"
        );
        assert_eq!(
            d.confirm_pressed(),
            Outcome::Force(vec!["app01".into(), "app02".into()])
        );
        assert!(d.confirming().is_none());
    }

    #[test]
    fn the_question_names_a_single_application() {
        let mut d = Dialog::new(vec![entry("a", "Notes", true)]);
        d.force_quit_pressed();
        assert_eq!(
            question(&d.confirming().unwrap()),
            "Do you want to force \u{201c}Notes\u{201d} to quit?"
        );
    }

    #[test]
    fn cancel_is_the_default_of_the_second_step_so_return_and_escape_go_back_and_escape_in_the_list_closes(
    ) {
        let mut d = Dialog::new(vec![entry("a", "A", true)]);
        d.force_quit_pressed();
        assert_eq!(d.key(Key::Return, false), Outcome::Redraw);
        assert!(d.confirming().is_none());
        d.force_quit_pressed();
        assert_eq!(d.key(Key::Escape, false), Outcome::Redraw);
        assert_eq!(d.key(Key::Escape, false), Outcome::Close);
    }

    #[test]
    fn a_dialog_can_open_on_the_second_step_for_one_application() {
        let d = Dialog::asking_about(apps(3), "app02");
        let asked = d.confirming().expect("the second step");
        assert_eq!(asked[0].app_id, "app02");
        assert_eq!(selected_ids(&d), ["app02"]);
    }

    #[test]
    fn a_new_list_keeps_the_selection_on_its_applications_and_ends_a_question_about_ones_that_left()
    {
        let mut d = Dialog::new(vec![entry("a", "A", false), entry("b", "B", false)]);
        d.select(1, Extend::Replace);
        d.set_entries(vec![
            entry("c", "C", false),
            entry("a", "A", false),
            entry("b", "B", true),
        ]);
        assert_eq!(selected_ids(&d), ["b"]);
        d.force_quit_pressed();
        d.set_entries(vec![entry("a", "A", false)]);
        assert!(d.confirming().is_none());
        assert!(!d.can_force_quit());
    }

    #[test]
    fn a_long_list_scrolls_to_keep_the_cursor_in_view_and_with_the_wheel() {
        let mut d = Dialog::new(apps(12));
        d.select(0, Extend::Replace);
        for _ in 0..8 {
            d.key(Key::Down, false);
        }
        assert_eq!(d.top(), 2);
        assert_eq!(d.scroll(10), Outcome::Redraw);
        assert_eq!(d.top(), 5);
        assert_eq!(d.scroll(1), Outcome::Nothing);
    }

    #[test]
    fn the_list_step_stacks_heading_list_footer_and_button_and_finds_the_row_under_a_point() {
        let layout = Layout::List(ListLayout::new(3, 0));
        let Layout::List(l) = &layout else {
            unreachable!()
        };
        assert_eq!(l.width, 338.0);
        assert_eq!(l.list, Rect::new(20.0, 90.0, 298.0, 168.0));
        assert_eq!(l.force_quit, Rect::new(220.0, 270.0, 104.0, 32.0));
        assert_eq!(l.height, 315.0);
        assert_eq!(layout.row_at(Point::new(30.0, 90.0 + 22.0 + 5.0)), Some(1));
        assert_eq!(
            layout.row_at(Point::new(30.0, 90.0 + 3.0 * 22.0 + 1.0)),
            None
        );
        assert_eq!(
            layout.button_at(Point::new(230.0, 280.0)),
            Some(Button::ForceQuit)
        );
    }

    #[test]
    fn the_question_step_has_the_icon_the_text_and_two_buttons_in_a_panel_of_its_own() {
        let layout = Layout::Question(QuestionLayout::new());
        assert_eq!(layout.size(), (415.0, 119.0));
        assert_eq!(
            layout.button_at(Point::new(200.0, 80.0)),
            Some(Button::Cancel)
        );
        assert_eq!(
            layout.button_at(Point::new(300.0, 80.0)),
            Some(Button::ForceQuit)
        );
        assert_eq!(layout.button_at(Point::new(100.0, 80.0)), None);
    }
}
