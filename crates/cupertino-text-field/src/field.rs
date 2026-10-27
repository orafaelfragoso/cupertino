//! The field's text, caret, selection, undo, clicks, blink and scroll.

use std::ops::Range;
use std::time::{Duration, Instant};

use cupertino_keyboard::TextAction;
use unicode_segmentation::UnicodeSegmentation;

use crate::keymap::{Command, KeyInput, Keymap};

/// How long the caret shows and hides while it blinks; it shows for a whole period after every edit, move or click.
/// Both are approximate: design values, not measured.
pub const CARET_BLINK_ON: Duration = Duration::from_millis(500);
pub const CARET_BLINK_OFF: Duration = Duration::from_millis(500);
/// Presses closer together than this at the same place count as a double or triple click.
pub const MULTI_CLICK_INTERVAL: Duration = Duration::from_millis(500);

/// What a key or pointer input did; the window redraws for every value but [`Response::Ignored`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Response {
    /// Not a field key (Return, Escape, Tab, a window shortcut): the window handles it.
    Ignored,
    /// The caret or the selection moved; the text is the same.
    Moved,
    /// The text changed.
    Edited,
    /// Put this text on the clipboard; the text is the same.
    Copy(String),
    /// Put this text on the clipboard; it was cut, so the text changed.
    Cut(String),
    /// Read the clipboard and pass its text to [`TextField::paste`].
    Paste,
}

/// Text and caret positions undo returns to.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Snapshot {
    text: String,
    caret: usize,
    anchor: usize,
}

/// Edits that undo as one while they follow each other.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Group {
    Typing,
    Deleting,
    /// The updates of one composition: it undoes as one step of its own.
    Composing,
}

/// What a drag extends: characters, words (after a double click) or nothing (a triple click selects all).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Granularity {
    Character,
    Word,
    All,
}

#[derive(Clone, Copy, Debug)]
struct Press {
    at: Instant,
    index: usize,
    count: u8,
}

#[derive(Clone, Debug, Default)]
pub struct TextField {
    text: String,
    caret: usize,
    anchor: usize,
    composition: Option<Range<usize>>,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    group: Option<Group>,
    kill: String,
    last_press: Option<Press>,
    /// The range the drag started from and how it extends, while the button is held.
    drag: Option<(Range<usize>, Granularity)>,
    /// When the caret last moved; the blink counts from it.
    activity: Option<Instant>,
    scroll: f64,
}

impl TextField {
    pub fn new() -> Self {
        TextField::default()
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// The caret: the end of the selection that moves.
    pub fn caret(&self) -> usize {
        self.caret
    }

    /// The selected bytes, empty when there is only a caret.
    pub fn selection(&self) -> Range<usize> {
        self.caret.min(self.anchor)..self.caret.max(self.anchor)
    }

    pub fn selected_text(&self) -> &str {
        &self.text[self.selection()]
    }

    /// The text an input method is composing (drawn underlined), if any.
    pub fn composition(&self) -> Option<Range<usize>> {
        self.composition.clone()
    }

    /// How far the text is scrolled left, in points ([`TextField::follow_caret`]).
    pub fn scroll(&self) -> f64 {
        self.scroll
    }

    /// Replace the whole text (not an edit: undo starts over); the caret goes to the end.
    pub fn set_text(&mut self, text: &str) {
        self.text = single_line(text);
        self.caret = self.text.len();
        self.anchor = self.caret;
        self.composition = None;
        self.undo.clear();
        self.redo.clear();
        self.group = None;
        self.scroll = 0.0;
    }

    /// Select `range` (clamped to the text and to grapheme boundaries), the caret at its end.
    pub fn select(&mut self, range: Range<usize>) {
        self.anchor = self.boundary_at_or_before(range.start.min(self.text.len()));
        self.caret = self.boundary_at_or_before(range.end.min(self.text.len()));
        self.group = None;
    }

    pub fn select_all(&mut self) {
        self.select(0..self.text.len());
    }

    /// The field lost the keyboard: the composition is committed, the selection collapses to the caret and a drag
    /// ends.
    pub fn blur(&mut self) {
        self.composition = None;
        self.anchor = self.caret;
        self.drag = None;
        self.group = None;
    }

    /// Byte offsets of the caret positions, the grapheme boundaries from 0 to the text's length.
    pub fn boundaries(&self) -> Vec<usize> {
        self.text
            .grapheme_indices(true)
            .map(|(i, _)| i)
            .chain([self.text.len()])
            .collect()
    }

    /// The caret position nearest to `x`, given the x of each of [`TextField::boundaries`] in the same order.
    pub fn index_at(&self, offsets: &[f64], x: f64) -> usize {
        let boundaries = self.boundaries();
        offsets
            .iter()
            .zip(&boundaries)
            .min_by(|(a, _), (b, _)| (*a - x).abs().total_cmp(&(*b - x).abs()))
            .map_or(0, |(_, i)| *i)
    }

    /// Scroll so that the caret at `caret_x` (unscrolled points from the text's start) is inside a field `width`
    /// points wide, and no further than the text's width `text_width` needs.
    pub fn follow_caret(&mut self, caret_x: f64, text_width: f64, width: f64) {
        if caret_x < self.scroll {
            self.scroll = caret_x;
        } else if caret_x > self.scroll + width {
            self.scroll = caret_x - width;
        }
        self.scroll = self.scroll.min((text_width - width).max(0.0)).max(0.0);
    }

    /// Whether the caret shows at `now`: never while text is selected, else in the blink's on phase.
    pub fn caret_visible(&self, now: Instant) -> bool {
        if !self.selection().is_empty() {
            return false;
        }
        let Some(since) = self.activity else {
            return true;
        };
        let period = CARET_BLINK_ON + CARET_BLINK_OFF;
        let phase = now.saturating_duration_since(since).as_nanos() % period.as_nanos();
        phase < CARET_BLINK_ON.as_nanos()
    }

    /// How long from `now` until the caret shows or hides next; `None` while text is selected (no caret).
    pub fn next_blink(&self, now: Instant) -> Option<Duration> {
        if !self.selection().is_empty() {
            return None;
        }
        let since = self.activity.unwrap_or(now);
        let period = (CARET_BLINK_ON + CARET_BLINK_OFF).as_nanos();
        let phase = now.saturating_duration_since(since).as_nanos() % period;
        let on = CARET_BLINK_ON.as_nanos();
        let left = if phase < on {
            on - phase
        } else {
            period - phase
        };
        Some(Duration::from_nanos(left as u64))
    }

    /// Insert typed text in place of the selection (or the composition); consecutive typing undoes as one.
    pub fn insert(&mut self, s: &str) {
        self.composition = None;
        self.replace_selection(&single_line(s), Some(Group::Typing));
    }

    /// Insert the clipboard's text in place of the selection; line breaks become spaces.
    pub fn paste(&mut self, s: &str) {
        self.composition = None;
        self.replace_selection(&single_line(s), None);
    }

    /// Show `preedit` as the text being composed, in place of the earlier composition or else the selection.
    pub fn set_composition(&mut self, preedit: &str) {
        match self.composition.take() {
            Some(range) => {
                self.anchor = range.start;
                self.caret = range.end;
            }
            // a composition starts an undo step of its own
            None => self.group = None,
        }
        let start = self.selection().start;
        self.replace_selection(&single_line(preedit), Some(Group::Composing));
        self.composition = (!preedit.is_empty()).then_some(start..self.caret);
    }

    /// Keep the composed text as typed text; what is typed next is a new undo step.
    pub fn commit_composition(&mut self) {
        self.composition = None;
        self.group = None;
    }

    /// Remove the composed text: the field is as it was before the composition, with no undo step left for it.
    pub fn cancel_composition(&mut self) {
        if self.composition.take().is_some() {
            if let Some(before) = self.undo.pop() {
                self.text = before.text;
                self.caret = before.caret;
                self.anchor = before.anchor;
            }
        }
        self.group = None;
    }

    pub fn undo(&mut self) -> bool {
        self.step_history(true)
    }

    pub fn redo(&mut self) -> bool {
        self.step_history(false)
    }

    /// A key press at `now`.
    pub fn key(&mut self, keymap: &mut Keymap, key: KeyInput, now: Instant) -> Response {
        let response = match keymap.command(key) {
            Command::Ignored => return Response::Ignored,
            Command::Pending => Response::Moved,
            Command::SelectAll => {
                self.select_all();
                Response::Moved
            }
            Command::Copy if self.selection().is_empty() => Response::Moved,
            Command::Copy => Response::Copy(self.selected_text().to_string()),
            Command::Cut if self.selection().is_empty() => Response::Moved,
            Command::Cut => {
                let cut = self.selected_text().to_string();
                self.replace_selection("", None);
                Response::Cut(cut)
            }
            Command::Paste => Response::Paste,
            Command::Undo => edited(self.undo()),
            Command::Redo => edited(self.redo()),
            Command::Insert(c) => {
                self.insert(c.encode_utf8(&mut [0; 4]));
                Response::Edited
            }
            Command::Actions(actions) => {
                let before = self.text.clone();
                for action in actions {
                    if !self.apply(action) {
                        return Response::Ignored;
                    }
                }
                if self.text == before {
                    Response::Moved
                } else {
                    Response::Edited
                }
            }
        };
        self.activity = Some(now);
        response
    }

    /// The primary button went down at caret position `index` (from [`TextField::index_at`]); `extend` (Shift)
    /// extends the selection to it. A second and third press within [`MULTI_CLICK_INTERVAL`] at the same place
    /// select the word and the whole text.
    pub fn press(&mut self, index: usize, extend: bool, now: Instant) {
        let index = self.boundary_at_or_before(index.min(self.text.len()));
        let count = match self.last_press {
            Some(p)
                if !extend
                    && p.index == index
                    && now.saturating_duration_since(p.at) <= MULTI_CLICK_INTERVAL =>
            {
                p.count % 3 + 1
            }
            _ => 1,
        };
        self.last_press = Some(Press {
            at: now,
            index,
            count,
        });
        self.composition = None;
        self.group = None;
        self.activity = Some(now);
        let granularity = match count {
            1 => Granularity::Character,
            2 => Granularity::Word,
            _ => Granularity::All,
        };
        let origin = match granularity {
            Granularity::Character if extend => self.anchor..self.anchor,
            Granularity::Character => index..index,
            Granularity::Word => self.word_at(index),
            Granularity::All => 0..self.text.len(),
        };
        self.drag = Some((origin, granularity));
        self.drag_to(index);
    }

    /// The pointer moved to caret position `index` with the button held.
    pub fn drag_to(&mut self, index: usize) {
        let Some((origin, granularity)) = self.drag.clone() else {
            return;
        };
        let index = self.boundary_at_or_before(index.min(self.text.len()));
        let reach = match granularity {
            Granularity::Character => index..index,
            Granularity::Word => self.word_at(index),
            Granularity::All => origin.clone(),
        };
        if reach.start < origin.start {
            self.anchor = origin.end;
            self.caret = reach.start;
        } else {
            self.anchor = origin.start;
            self.caret = reach.end.max(origin.end);
        }
    }

    /// The button went up.
    pub fn release(&mut self) {
        self.drag = None;
    }

    /// Whether a press is being dragged.
    pub fn is_dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// Carry out a standard editing action; `false` for an action the window handles (Return, Tab, Escape).
    fn apply(&mut self, action: TextAction) -> bool {
        use TextAction as A;
        let end = self.text.len();
        match action {
            A::InsertNewline
            | A::InsertNewlineIgnoringFieldEditor
            | A::InsertLineBreak
            | A::InsertTab
            | A::InsertTabIgnoringFieldEditor
            | A::InsertBacktab
            | A::CancelOperation
            | A::Complete
            | A::SelectNextKeyView
            | A::SelectPreviousKeyView => return false,
            A::MoveLeft | A::MoveBackward => self.move_char(false, false),
            A::MoveRight | A::MoveForward => self.move_char(true, false),
            A::MoveLeftAndModifySelection | A::MoveBackwardAndModifySelection => {
                self.move_char(false, true)
            }
            A::MoveRightAndModifySelection | A::MoveForwardAndModifySelection => {
                self.move_char(true, true)
            }
            A::MoveWordLeft | A::MoveWordBackward => {
                let to = self.word_left(self.selection().start.min(self.caret));
                self.move_to(to, false)
            }
            A::MoveWordRight | A::MoveWordForward => {
                let to = self.word_right(self.selection().end.max(self.caret));
                self.move_to(to, false)
            }
            A::MoveWordLeftAndModifySelection | A::MoveWordBackwardAndModifySelection => {
                self.move_to(self.word_left(self.caret), true)
            }
            A::MoveWordRightAndModifySelection | A::MoveWordForwardAndModifySelection => {
                self.move_to(self.word_right(self.caret), true)
            }
            A::MoveToLeftEndOfLine
            | A::MoveToBeginningOfParagraph
            | A::MoveToBeginningOfDocument
            | A::MoveUp
            | A::PageUp
            | A::ScrollToBeginningOfDocument => self.move_to(0, false),
            A::MoveToRightEndOfLine
            | A::MoveToEndOfParagraph
            | A::MoveToEndOfDocument
            | A::MoveDown
            | A::PageDown
            | A::ScrollToEndOfDocument => self.move_to(end, false),
            A::MoveToLeftEndOfLineAndModifySelection
            | A::MoveToBeginningOfParagraphAndModifySelection
            | A::MoveToBeginningOfDocumentAndModifySelection
            | A::MoveParagraphBackwardAndModifySelection
            | A::MoveUpAndModifySelection
            | A::PageUpAndModifySelection => self.move_to(0, true),
            A::MoveToRightEndOfLineAndModifySelection
            | A::MoveToEndOfParagraphAndModifySelection
            | A::MoveToEndOfDocumentAndModifySelection
            | A::MoveParagraphForwardAndModifySelection
            | A::MoveDownAndModifySelection
            | A::PageDownAndModifySelection => self.move_to(end, true),
            A::DeleteBackward | A::DeleteBackwardByDecomposingPreviousCharacter => {
                self.delete_toward(|f| f.prev_boundary(f.caret))
            }
            A::DeleteForward => self.delete_toward(|f| f.next_boundary(f.caret)),
            A::DeleteWordBackward => self.delete_toward(|f| f.word_left(f.caret)),
            A::DeleteWordForward => self.delete_toward(|f| f.word_right(f.caret)),
            A::DeleteToBeginningOfLine => self.delete_toward(|_| 0),
            A::Delete => {
                if !self.selection().is_empty() {
                    self.replace_selection("", None);
                }
            }
            A::DeleteToEndOfParagraph => {
                let range = if self.selection().is_empty() {
                    self.caret..end
                } else {
                    self.selection()
                };
                if !range.is_empty() {
                    self.kill = self.text[range.clone()].to_string();
                    self.anchor = range.start;
                    self.caret = range.end;
                    self.replace_selection("", None);
                }
            }
            A::Yank => {
                let kill = self.kill.clone();
                self.replace_selection(&kill, None);
            }
            A::Transpose => self.transpose(),
            A::InsertDoubleQuoteIgnoringSubstitution => self.insert("\""),
            A::InsertSingleQuoteIgnoringSubstitution => self.insert("'"),
            A::Noop
            | A::CenterSelectionInVisibleArea
            | A::CycleToNextInputKeyboardLayout
            | A::CycleToNextInputScript
            | A::InsertRightToLeftSlash
            | A::MakeBaseWritingDirectionLeftToRight
            | A::MakeBaseWritingDirectionNatural
            | A::MakeBaseWritingDirectionRightToLeft
            | A::MakeTextWritingDirectionLeftToRight
            | A::MakeTextWritingDirectionNatural
            | A::MakeTextWritingDirectionRightToLeft
            | A::ScrollPageDown
            | A::ScrollPageUp
            | A::TogglePlatformInputSystem => {}
        }
        true
    }

    fn move_to(&mut self, to: usize, extend: bool) {
        self.caret = to;
        if !extend {
            self.anchor = to;
        }
        self.group = None;
        self.composition = None;
    }

    /// One grapheme left or right; without `extend` a selection collapses to its start or end instead.
    fn move_char(&mut self, forward: bool, extend: bool) {
        let selection = self.selection();
        let to = match (extend || selection.is_empty(), forward) {
            (true, false) => self.prev_boundary(self.caret),
            (true, true) => self.next_boundary(self.caret),
            (false, false) => selection.start,
            (false, true) => selection.end,
        };
        self.move_to(to, extend);
    }

    /// Delete the selection, or from the caret to `target(self)`.
    fn delete_toward(&mut self, target: impl Fn(&Self) -> usize) {
        if self.selection().is_empty() {
            self.anchor = target(self);
        }
        if !self.selection().is_empty() {
            self.replace_selection("", Some(Group::Deleting));
        }
    }

    /// Swap the graphemes around the caret (at the end: the last two) and move past them.
    fn transpose(&mut self) {
        if !self.selection().is_empty() {
            return;
        }
        let at = if self.caret == self.text.len() {
            self.prev_boundary(self.caret)
        } else {
            self.caret
        };
        let start = self.prev_boundary(at);
        let end = self.next_boundary(at);
        if start == at || at == end {
            return;
        }
        let swapped = format!("{}{}", &self.text[at..end], &self.text[start..at]);
        self.anchor = start;
        self.caret = end;
        self.replace_selection(&swapped, None);
    }

    fn replace_selection(&mut self, s: &str, group: Option<Group>) {
        if group.is_none() || group != self.group {
            self.undo.push(self.snapshot());
        }
        self.redo.clear();
        self.group = group;
        let range = self.selection();
        self.text.replace_range(range.clone(), s);
        self.caret = range.start + s.len();
        self.anchor = self.caret;
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            text: self.text.clone(),
            caret: self.caret,
            anchor: self.anchor,
        }
    }

    fn step_history(&mut self, back: bool) -> bool {
        let (from, to) = if back {
            (&mut self.undo, &mut self.redo)
        } else {
            (&mut self.redo, &mut self.undo)
        };
        let Some(state) = from.pop() else {
            return false;
        };
        to.push(Snapshot {
            text: std::mem::take(&mut self.text),
            caret: self.caret,
            anchor: self.anchor,
        });
        self.text = state.text;
        self.caret = state.caret;
        self.anchor = state.anchor;
        self.group = None;
        self.composition = None;
        true
    }

    fn prev_boundary(&self, at: usize) -> usize {
        self.text[..at]
            .grapheme_indices(true)
            .next_back()
            .map_or(0, |(i, _)| i)
    }

    fn next_boundary(&self, at: usize) -> usize {
        self.text[at..]
            .graphemes(true)
            .next()
            .map_or(at, |g| at + g.len())
    }

    fn boundary_at_or_before(&self, at: usize) -> usize {
        self.boundaries()
            .into_iter()
            .rev()
            .find(|b| *b <= at)
            .unwrap_or(0)
    }

    /// The word segments of the text (runs that contain a letter or a digit).
    fn words(&self) -> impl Iterator<Item = Range<usize>> + '_ {
        self.text
            .split_word_bound_indices()
            .filter(|(_, w)| w.chars().any(char::is_alphanumeric))
            .map(|(i, w)| i..i + w.len())
    }

    /// The start of the word before `at` (or of the word `at` is inside).
    fn word_left(&self, at: usize) -> usize {
        self.words()
            .filter(|w| w.start < at)
            .last()
            .map_or(0, |w| w.start)
    }

    /// The end of the word after `at` (or of the word `at` is inside).
    fn word_right(&self, at: usize) -> usize {
        self.words()
            .find(|w| w.end > at)
            .map_or(self.text.len(), |w| w.end)
    }

    /// The segment a double click at `at` selects: the word, or the run of spaces or the punctuation, under it.
    fn word_at(&self, at: usize) -> Range<usize> {
        let mut last = 0..0;
        for (i, w) in self.text.split_word_bound_indices() {
            let range = i..i + w.len();
            if range.contains(&at) {
                return range;
            }
            last = range;
        }
        last
    }
}

fn edited(changed: bool) -> Response {
    if changed {
        Response::Edited
    } else {
        Response::Moved
    }
}

/// `s` as typing could have produced it: line breaks and tabs become spaces; other control characters and the bidi
/// embedding, override and isolate controls are dropped.
fn single_line(s: &str) -> String {
    s.replace("\r\n", " ")
        .chars()
        .filter_map(|c| match c {
            '\n' | '\r' | '\t' => Some(' '),
            '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' => None,
            c if c.is_control() => None,
            c => Some(c),
        })
        .collect()
}
