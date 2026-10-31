//! The editable text of a field or a multi-line editor: text, caret, selection, undo, clicks, blink and scroll.

use std::ops::Range;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use unicode_segmentation::UnicodeSegmentation;

use crate::{bidi, Affinity, Direction, TextPosition};

/// How long the caret shows and hides while it blinks; it shows for a whole period after every edit, move or click.
/// Both are approximate: design values, not measured.
pub const CARET_BLINK_ON: Duration = Duration::from_millis(500);
pub const CARET_BLINK_OFF: Duration = Duration::from_millis(500);
/// Presses closer together than this at the same place count as a double or triple click.
pub const MULTI_CLICK_INTERVAL: Duration = Duration::from_millis(500);

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

/// `now` as time since one origin for the process, so presses timed by an `Instant` and by an application's clock
/// (with [`TextBuffer::press_at`]) share one scale.
fn since_origin(now: Instant) -> Duration {
    static ORIGIN: OnceLock<Instant> = OnceLock::new();
    now.saturating_duration_since(*ORIGIN.get_or_init(|| now))
}

#[derive(Clone, Copy, Debug)]
struct Press {
    at: Duration,
    index: usize,
    count: u8,
}

/// The editable text of a field, with its selection and undo history.
///
/// Its `Debug` output shows the text, the undo snapshots and the kill buffer, so protected input never enters a
/// `TextBuffer`: a password field uses [`SecureBuffer`](crate::SecureBuffer) only. A field's buffer strips line breaks;
/// a [`TextBuffer::multiline`] one keeps them and tabs.
#[derive(Clone, Debug, Default)]
pub struct TextBuffer {
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
    activity: Option<Duration>,
    scroll: f64,
    /// The most bytes the text holds; `None` for no limit.
    max_len: Option<usize>,
    /// The caret's byte when a visual move put it on the end edge of the grapheme before it, at a direction change.
    upstream: Option<usize>,
    /// The base direction the text is laid out in; `None` takes it from the first strong character.
    direction: Option<Direction>,
    multiline: bool,
    /// The x a run of vertical moves aims for, so a short line in between does not pull the caret to its end.
    goal: Option<f64>,
}

/// The visual lines of a multi-line text as it is laid out, for the caret moves that depend on them.
pub trait LineLayout {
    fn line_count(&self) -> usize;
    /// The visual line `at` is drawn on, and its x in points.
    fn locate(&self, at: TextPosition) -> (usize, f64);
    /// The caret position on visual line `line` nearest to `x`.
    fn position_on(&self, line: usize, x: f64) -> TextPosition;
    /// The caret positions at the logical start and end of visual line `line`.
    fn line_edges(&self, line: usize) -> (TextPosition, TextPosition);
    /// How many visual lines a page up or down moves.
    fn page_lines(&self) -> usize;
}

impl TextBuffer {
    pub fn new() -> Self {
        TextBuffer::default()
    }

    /// The buffer of a multi-line editor: line breaks and tabs stay.
    pub fn multiline() -> Self {
        TextBuffer {
            multiline: true,
            ..TextBuffer::default()
        }
    }

    /// A field whose text holds at most `max_len` bytes: typing, a paste or a composition past it is cut at a grapheme
    /// boundary.
    pub fn with_max_len(max_len: usize) -> Self {
        TextBuffer {
            max_len: Some(max_len),
            ..TextBuffer::default()
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// Sets the base direction the field lays its text out in, the `ParagraphStyle::direction` it paints with, so Left
    /// and Right walk the carets as they are drawn.
    pub fn set_direction(&mut self, direction: Option<Direction>) {
        self.direction = direction;
    }

    /// The caret with the side of a direction change it is drawn on, for `ParagraphLayout::caret_rect`.
    pub fn caret_position(&self) -> TextPosition {
        TextPosition {
            byte: self.caret,
            affinity: if self.upstream == Some(self.caret) {
                Affinity::Upstream
            } else {
                Affinity::Downstream
            },
        }
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

    /// How far the text is scrolled left, in points ([`TextBuffer::follow_caret`]).
    pub fn scroll(&self) -> f64 {
        self.scroll
    }

    /// Replace the whole text (not an edit: undo starts over); the caret goes to the end.
    pub fn set_text(&mut self, text: &str) {
        self.text = self.cleaned(text);
        self.set_caret(self.text.len());
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
        self.set_caret(self.boundary_at_or_before(range.end.min(self.text.len())));
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

    /// The caret position nearest to `x`, given the x of each of [`TextBuffer::boundaries`] in the same order.
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
        self.scroll = self.scroll_for(caret_x, text_width, width);
    }

    /// The scroll [`TextBuffer::follow_caret`] would set, without setting it (for drawing before the field is updated).
    pub fn scroll_for(&self, caret_x: f64, text_width: f64, width: f64) -> f64 {
        let scroll = if caret_x < self.scroll {
            caret_x
        } else if caret_x > self.scroll + width {
            caret_x - width
        } else {
            self.scroll
        };
        scroll.min((text_width - width).max(0.0)).max(0.0)
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
        let phase = since_origin(now).saturating_sub(since).as_nanos() % period.as_nanos();
        phase < CARET_BLINK_ON.as_nanos()
    }

    /// How long from `now` until the caret shows or hides next; `None` while text is selected (no caret).
    pub fn next_blink(&self, now: Instant) -> Option<Duration> {
        if !self.selection().is_empty() {
            return None;
        }
        let now = since_origin(now);
        let since = self.activity.unwrap_or(now);
        let period = (CARET_BLINK_ON + CARET_BLINK_OFF).as_nanos();
        let phase = now.saturating_sub(since).as_nanos() % period;
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
        self.replace_selection(&self.clean(s), Some(Group::Typing));
    }

    /// Insert the clipboard's text in place of the selection; in a field line breaks become spaces.
    pub fn paste(&mut self, s: &str) {
        self.composition = None;
        self.replace_selection(&self.clean(s), None);
    }

    /// Show `preedit` as the text being composed, in place of the earlier composition or else the selection.
    pub fn set_composition(&mut self, preedit: &str) {
        match self.composition.take() {
            Some(range) => {
                self.anchor = range.start;
                self.set_caret(range.end);
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
                self.set_caret(before.caret);
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

    /// The primary button went down at caret position `index` (from [`TextBuffer::index_at`]); `extend` (Shift)
    /// extends the selection to it. A second and third press within [`MULTI_CLICK_INTERVAL`] at the same place
    /// select the word and the whole text.
    pub fn press(&mut self, index: usize, extend: bool, now: Instant) {
        self.press_at(index, extend, since_origin(now));
    }

    /// [`TextBuffer::press`] at `now`, a time on the caller's own clock; every press of one buffer must use one clock.
    pub fn press_at(&mut self, index: usize, extend: bool, now: Duration) {
        let index = self.boundary_at_or_before(index.min(self.text.len()));
        let count = match self.last_press {
            Some(p)
                if !extend
                    && p.index == index
                    && now.saturating_sub(p.at) <= MULTI_CLICK_INTERVAL =>
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
            self.set_caret(reach.start);
        } else {
            self.anchor = origin.start;
            self.set_caret(reach.end.max(origin.end));
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

    /// Remember that the caret moved at `now`: the blink counts from it.
    pub fn touch(&mut self, now: Instant) {
        self.activity = Some(since_origin(now));
    }

    /// One word left or right. Without `extend` a selection collapses to the word boundary beyond its start or end.
    pub fn move_word(&mut self, forward: bool, extend: bool) {
        let to = match (forward, extend) {
            (false, false) => self.word_left(self.selection().start.min(self.caret)),
            (true, false) => self.word_right(self.selection().end.max(self.caret)),
            (false, true) => self.word_left(self.caret),
            (true, true) => self.word_right(self.caret),
        };
        self.move_to(to, extend);
    }

    pub fn move_to_start(&mut self, extend: bool) {
        self.move_to(0, extend);
    }

    pub fn move_to_end(&mut self, extend: bool) {
        self.move_to(self.text.len(), extend);
    }

    /// Delete the selection, or the grapheme before the caret.
    pub fn delete_backward(&mut self) {
        self.delete_toward(|f| f.prev_boundary(f.caret));
    }

    /// Delete the selection, or the grapheme after the caret.
    pub fn delete_forward(&mut self) {
        self.delete_toward(|f| f.next_boundary(f.caret));
    }

    pub fn delete_word_backward(&mut self) {
        self.delete_toward(|f| f.word_left(f.caret));
    }

    pub fn delete_word_forward(&mut self) {
        self.delete_toward(|f| f.word_right(f.caret));
    }

    pub fn delete_to_line_start(&mut self) {
        self.delete_toward(|f| f.line_start(f.caret));
    }

    /// Delete the selection; with only a caret nothing happens.
    pub fn delete_selection(&mut self) {
        if !self.selection().is_empty() {
            self.replace_selection("", None);
        }
    }

    /// Delete the selection and return it, for the clipboard.
    pub fn cut(&mut self) -> String {
        let cut = self.selected_text().to_string();
        self.replace_selection("", None);
        cut
    }

    /// Delete the selection, or from the caret to the end of its line (at the end, the line break), and keep it in the
    /// kill buffer.
    pub fn kill_to_end(&mut self) {
        let range = if self.selection().is_empty() {
            let end = self.line_end(self.caret);
            self.caret..if end == self.caret {
                self.next_boundary(end)
            } else {
                end
            }
        } else {
            self.selection()
        };
        if !range.is_empty() {
            self.kill = self.text[range.clone()].to_string();
            self.anchor = range.start;
            self.set_caret(range.end);
            self.replace_selection("", None);
        }
    }

    /// Insert the kill buffer in place of the selection.
    pub fn yank(&mut self) {
        let kill = self.kill.clone();
        self.replace_selection(&kill, None);
    }

    /// To the start of the caret's line: in a multi-line buffer the line after the last line break before it.
    pub fn move_to_line_start(&mut self, extend: bool) {
        self.move_to(self.line_start(self.caret), extend);
    }

    pub fn move_to_line_end(&mut self, extend: bool) {
        self.move_to(self.line_end(self.caret), extend);
    }

    /// `lines` visual lines down (up when negative), toward the x the run of vertical moves started at; past the first
    /// or last line the caret goes to the text's start or end. Without `extend` it moves from the selection's start
    /// going up and its end going down.
    pub fn move_vertical(&mut self, layout: &impl LineLayout, lines: isize, extend: bool) {
        let selection = self.selection();
        let from = match (extend || selection.is_empty(), lines < 0) {
            (true, _) => self.caret_position(),
            (false, true) => downstream(selection.start),
            (false, false) => downstream(selection.end),
        };
        let (line, x) = layout.locate(from);
        let goal = self.goal.unwrap_or(x);
        let to = match line.checked_add_signed(lines) {
            None => downstream(0),
            Some(line) if line >= layout.line_count() => downstream(self.text.len()),
            Some(line) => layout.position_on(line, goal),
        };
        self.move_to_position(to, extend);
        self.goal = Some(goal);
    }

    /// One page of visual lines down or up, as [`TextBuffer::move_vertical`].
    pub fn move_page(&mut self, layout: &impl LineLayout, down: bool, extend: bool) {
        let lines = isize::try_from(layout.page_lines().max(1)).unwrap_or(isize::MAX);
        self.move_vertical(layout, if down { lines } else { -lines }, extend);
    }

    /// To the start or end of the caret's visual line.
    pub fn move_to_visual_line_edge(&mut self, layout: &impl LineLayout, end: bool, extend: bool) {
        let (line, _) = layout.locate(self.caret_position());
        let (start, stop) = layout.line_edges(line);
        self.move_to_position(if end { stop } else { start }, extend);
    }

    fn move_to_position(&mut self, to: TextPosition, extend: bool) {
        self.move_to(to.byte.min(self.text.len()), extend);
        self.upstream = (to.affinity == Affinity::Upstream).then_some(self.caret);
    }

    fn set_caret(&mut self, to: usize) {
        self.caret = to;
        self.upstream = None;
        self.goal = None;
    }

    fn move_to(&mut self, to: usize, extend: bool) {
        self.set_caret(to);
        if !extend {
            self.anchor = to;
        }
        self.group = None;
        self.composition = None;
    }

    /// One grapheme left or right; without `extend` a selection collapses to its start or end instead.
    pub fn move_char(&mut self, forward: bool, extend: bool) {
        let selection = self.selection();
        let to = match (extend || selection.is_empty(), forward) {
            (true, false) => self.prev_boundary(self.caret),
            (true, true) => self.next_boundary(self.caret),
            (false, false) => selection.start,
            (false, true) => selection.end,
        };
        self.move_to(to, extend);
    }

    /// One caret stop left or right on screen, through text of either direction; without `extend` a selection
    /// collapses to its side in that direction instead.
    pub fn move_visual(&mut self, rightward: bool, extend: bool) {
        let lines = bidi::caret_lines(&self.text, self.direction);
        let selection = self.selection();
        if extend || selection.is_empty() {
            let to = bidi::visual_neighbour_in(&lines, self.caret_position(), rightward)
                .unwrap_or_else(|| self.caret_position());
            self.move_to(to.byte, extend);
            self.upstream = (to.affinity == Affinity::Upstream).then_some(to.byte);
        } else {
            let place = |byte| {
                bidi::place_of(
                    &lines,
                    TextPosition {
                        byte,
                        affinity: Affinity::Downstream,
                    },
                )
            };
            let to = if (place(selection.end) > place(selection.start)) == rightward {
                selection.end
            } else {
                selection.start
            };
            self.move_to(to, extend);
        }
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
    pub fn transpose(&mut self) {
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
        self.set_caret(end);
        self.replace_selection(&swapped, None);
    }

    /// The longest grapheme prefix of `s` that fits with `kept` bytes of the text staying.
    fn fit<'a>(&self, s: &'a str, kept: usize) -> &'a str {
        let Some(room) = self.max_len.map(|max| max.saturating_sub(kept)) else {
            return s;
        };
        if s.len() <= room {
            return s;
        }
        let end = s
            .grapheme_indices(true)
            .map(|(i, g)| i + g.len())
            .take_while(|end| *end <= room)
            .last()
            .unwrap_or(0);
        &s[..end]
    }

    fn replace_selection(&mut self, s: &str, group: Option<Group>) {
        let s = self.fit(s, self.text.len() - self.selection().len());
        if s.is_empty() && self.selection().is_empty() {
            return;
        }
        if group.is_none() || group != self.group {
            self.undo.push(self.snapshot());
        }
        self.redo.clear();
        self.group = group;
        let range = self.selection();
        self.text.replace_range(range.clone(), s);
        self.set_caret(range.start + s.len());
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
        self.set_caret(state.caret);
        self.anchor = state.anchor;
        self.group = None;
        self.composition = None;
        true
    }

    /// `text` as [`TextBuffer::set_text`] keeps it: without the characters this buffer drops or converts, cut to its
    /// length limit. A model value that differs from the buffer's text only in those is the same text.
    pub fn cleaned(&self, text: &str) -> String {
        self.fit(&self.clean(text), 0).to_string()
    }

    fn clean(&self, s: &str) -> String {
        if self.multiline {
            multi_line(s)
        } else {
            single_line(s)
        }
    }

    fn line_start(&self, at: usize) -> usize {
        self.text[..at].rfind('\n').map_or(0, |i| i + 1)
    }

    fn line_end(&self, at: usize) -> usize {
        self.text[at..]
            .find('\n')
            .map_or(self.text.len(), |i| at + i)
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

fn downstream(byte: usize) -> TextPosition {
    TextPosition {
        byte,
        affinity: Affinity::Downstream,
    }
}

/// `s` as typing could have produced it in an editor: line breaks become `\n`, tabs stay; other control characters and
/// the bidi embedding, override and isolate controls are dropped.
fn multi_line(s: &str) -> String {
    s.replace("\r\n", "\n")
        .chars()
        .filter_map(|c| match c {
            '\r' | '\u{2028}' | '\u{2029}' => Some('\n'),
            '\n' | '\t' => Some(c),
            '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' => None,
            c if c.is_control() => None,
            c => Some(c),
        })
        .collect()
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

#[cfg(test)]
mod tests {
    use super::*;

    fn buffer(text: &str) -> TextBuffer {
        let mut b = TextBuffer::new();
        b.set_text(text);
        b
    }

    #[test]
    fn words_move_by_word_and_a_plain_move_collapses_the_selection_beyond_its_side() {
        let mut b = buffer("one two three");
        b.move_to_start(false);
        b.move_word(true, false);
        assert_eq!(b.caret(), 3);
        b.move_word(true, true);
        assert_eq!(b.selection(), 3..7);
        b.move_word(false, false);
        assert_eq!(b.caret(), 0, "left of the selection's start");
        b.move_to_end(false);
        b.move_word(false, true);
        assert_eq!(b.selection(), 8..13);
    }

    #[test]
    fn deleting_removes_a_grapheme_a_word_or_the_line_start() {
        let mut b = buffer("ab cd\u{301} ef");
        b.delete_backward();
        b.delete_word_backward();
        assert_eq!(b.text(), "ab cd\u{301} ");
        b.move_to_start(false);
        b.delete_forward();
        b.delete_word_forward();
        assert_eq!(b.text(), " cd\u{301} ");
        b.move_to_end(false);
        b.delete_to_line_start();
        assert_eq!(b.text(), "");
    }

    #[test]
    fn delete_selection_does_nothing_for_a_caret_and_cut_returns_the_selection() {
        let mut b = buffer("hello");
        b.delete_selection();
        assert_eq!(b.text(), "hello");
        b.select(1..3);
        assert_eq!(b.cut(), "el");
        assert_eq!(b.text(), "hlo");
        assert!(b.undo());
        assert_eq!(b.text(), "hello");
    }

    #[test]
    fn kill_to_end_fills_the_kill_buffer_and_yank_inserts_it() {
        let mut b = buffer("abc def");
        b.select(3..3);
        b.kill_to_end();
        assert_eq!(b.text(), "abc");
        b.move_to_start(false);
        b.yank();
        assert_eq!(b.text(), " defabc");
    }

    #[test]
    fn transpose_swaps_the_graphemes_around_the_caret() {
        let mut b = buffer("ae\u{301}");
        b.transpose();
        assert_eq!(b.text(), "e\u{301}a");
        assert_eq!(b.caret(), b.text().len());
    }

    #[test]
    fn the_blink_counts_from_the_last_touch() {
        let mut b = buffer("x");
        let t0 = Instant::now();
        b.touch(t0);
        assert!(b.caret_visible(t0 + CARET_BLINK_ON / 2));
        assert!(!b.caret_visible(t0 + CARET_BLINK_ON + CARET_BLINK_OFF / 2));
        b.touch(t0 + CARET_BLINK_ON + CARET_BLINK_OFF / 2);
        assert!(b.caret_visible(t0 + CARET_BLINK_ON + CARET_BLINK_OFF / 2));
    }

    #[test]
    fn a_multiline_buffer_keeps_line_breaks_and_tabs_and_line_edits_stop_at_them() {
        let mut b = TextBuffer::multiline();
        b.set_text("one\r\ntwo\tthree\rfour");
        assert_eq!(b.text(), "one\ntwo\tthree\nfour");
        b.select(6..6);
        b.delete_to_line_start();
        assert_eq!(b.text(), "one\no\tthree\nfour");
        b.kill_to_end();
        assert_eq!(b.text(), "one\n\nfour");
        b.kill_to_end();
        assert_eq!(b.text(), "one\nfour", "at a line's end the line break goes");
        b.move_to_line_end(true);
        assert_eq!(b.selected_text(), "four");
    }
}
