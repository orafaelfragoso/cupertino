//! What the window tells assistive technology without moving the keyboard: a polite live region ([`Announcer`]) that
//! reads the latest of a transfer starting and ending, a search's result count and the selection's count once it holds
//! more than one item. A single item selected is read by the focus that comes to it, and an error by its notification.
//! A search that ends or a selection down to one item takes its own message away, so the region never holds what no
//! longer holds; a transfer's message stays until another replaces it.
//!
//! A screen reader speaks a live region's name when its node appears and whenever the name changes, so each message is
//! a node of its own ([`Announcement::serial`] keys it): the same words said twice, such as a second copy of the same
//! file, are heard twice. What does not change is not said again: a new search with as many results, or another
//! selection of as many items. A rubber band says its count once it is let go, not at each item it crosses, and a
//! transfer says when it starts and when it ends, never its progress.

use cupertino_kit::core::{
    Change, Event, EventCx, Live, MeasureCx, PaintCx, Propagation, Role, SemanticsCx, Widget,
};
use cupertino_kit::geometry::Size;
use cupertino_kit::layout::{Constraints, ContainerKind, LayoutSpec, Measured, SizeSpec};

use super::search::NO_RESULTS;
use super::Message;

/// What a window's state announces when it changes: the search's result count once the folder is read, and how many
/// items are selected.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Tally {
    pub results: Option<usize>,
    pub selected: usize,
    /// A rubber band is being drawn.
    pub banding: bool,
}

/// What a message of the live region is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum About {
    Transfer,
    Results,
    Selection,
}

/// The live region's message, what it is about, and how many messages came before it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Announcement {
    pub text: String,
    pub about: Option<About>,
    /// Counts the messages said, so each one keys a node of its own.
    pub serial: u64,
}

impl Announcement {
    pub fn say(&mut self, about: About, text: String) {
        *self = Announcement {
            text,
            about: Some(about),
            serial: self.serial + 1,
        };
    }

    /// Empty the region if its message is about `about`.
    fn clear(&mut self, about: About) {
        if self.about == Some(about) {
            self.text.clear();
            self.about = None;
        }
    }

    /// Follow the change of the window's state from `before` to `after`: a new result count first, else a new
    /// selection of more than one item, held while a rubber band is drawn and said once it is let go. A search that
    /// ended, or a selection down to one item, takes its message away.
    pub fn follow(&mut self, before: Tally, after: Tally) {
        if after.results != before.results {
            match after.results {
                Some(count) => self.say(About::Results, results(count)),
                None => self.clear(About::Results),
            }
        } else if after.banding {
            // the band's count is said once, when it is let go
        } else if after.selected != before.selected || before.banding {
            match after.selected {
                n if n > 1 => self.say(About::Selection, format!("{n} items selected")),
                _ => self.clear(About::Selection),
            }
        }
    }
}

/// A search's result count, as "No Results", "1 result" or "3 results".
fn results(count: usize) -> String {
    match count {
        0 => NO_RESULTS.to_owned(),
        1 => "1 result".to_owned(),
        n => format!("{n} results"),
    }
}

/// The window's live region: no size, nothing drawn, never a Tab stop, read only while it holds text.
pub(super) struct Announcer {
    pub text: String,
}

impl Widget<Message> for Announcer {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(0.0),
            height: SizeSpec::Fixed(0.0),
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.text != old.text {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::ZERO,
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, Message>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        if self.text.is_empty() {
            return;
        }
        cx.role(Role::StaticText);
        cx.name(self.text.clone());
        cx.live(Live::Polite);
    }
}
