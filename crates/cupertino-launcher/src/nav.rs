//! The launcher's navigation state: which view is up (hidden, search or the application browser), which nested state
//! sits on top of it (the enlarged preview, the browser's view options, an action's parameters), where the keyboard
//! is, and what the next Escape, toggle or dismissal does. It is pure: [`Nav::apply`] takes an [`Intent`] and returns
//! the [`Step`] the model carries out, so every transition is tested as a table without a window.
//!
//! Escape steps back one level at a time, the innermost first:
//!
//! 1. a nested state closes, and the keyboard goes back to what opened it;
//! 2. in search, with the keyboard in the result list, the keyboard goes back to the field;
//! 3. a typed query is cleared;
//! 4. in the browser, a category filter is cleared;
//! 5. the browser folds into the compact search field;
//! 6. the search field closes.
//!
//! The browser toggles: its shortcut (or the dock's request) opens it from hidden, unfolds it from search, and closes it
//! while it is up. A dismissal (a press outside, the keyboard leaving, an activation, the session locking) closes
//! whatever is up, nested states included.

/// The launcher's main views.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum View {
    Hidden,
    /// The search field, with results below it once a query finds some.
    Search,
    /// The application browser: categories over a grid or a list of every application.
    Browse,
    /// The files scope: filters over the recent files and a grid of the files the query finds.
    Files,
}

impl View {
    /// Whether the view is a scope panel under the query row (the browser or the files scope).
    pub fn is_scope(self) -> bool {
        matches!(self, View::Browse | View::Files)
    }
}

/// A state over a view that Escape leaves before anything else.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Nested {
    /// The selected result's preview, enlarged over the results.
    Preview,
    /// The browser's view options (grid or list).
    ViewOptions,
    /// An action's parameters being filled in.
    Parameters,
}

/// Where the keyboard is besides the query field.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Focus {
    /// The query field; in search the top result is highlighted, in the browser nothing is selected.
    #[default]
    Field,
    /// The result list or the browser's applications.
    Content,
    /// The browser's category row.
    Categories,
}

/// What happened, as far as navigation is concerned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Intent {
    /// The launcher shortcut (`Toggle` on the bus).
    Toggle,
    /// The browser's shortcut, the dock's request or Primary+1.
    Browse,
    /// The files scope's shortcut or its mode button.
    Files,
    Escape,
    /// The query is now empty (`false`) or not (`true`).
    Query(bool),
    /// A category filter was turned on (`true`) or off.
    Category(bool),
    /// The keyboard moved.
    Focus(Focus),
    /// Open a nested state over the current view.
    Open(Nested),
    /// The nested state finished (its choice was made): back to what opened it.
    Done,
    /// A result was activated, a press landed outside the panel, the keyboard left it or the session locked.
    Dismiss,
}

/// What the model does for an [`Intent`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// Nothing changes.
    Stay,
    /// The panel appears in this view, with an empty query.
    Show(View),
    /// The panel goes away.
    Hide,
    /// The browser folds into the search field.
    Fold,
    /// The search field unfolds into the browser.
    Unfold,
    /// The search field, or the browser, unfolds into the files scope.
    UnfoldFiles,
    /// The nested state closes; the keyboard returns to `Focus`.
    Leave(Nested, Focus),
    /// A nested state opens.
    Enter(Nested),
    /// The keyboard goes back to the query field.
    FocusField,
    ClearQuery,
    ClearCategory,
    /// Only the state was noted.
    Noted,
}

/// The navigation state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Nav {
    pub view: View,
    pub nested: Option<Nested>,
    pub focus: Focus,
    /// Whether a query is typed.
    pub query: bool,
    /// Whether a category filter is on (browser only).
    pub category: bool,
    /// Where the keyboard was when the nested state opened.
    pub opener: Focus,
}

impl Default for Nav {
    fn default() -> Self {
        Self::HIDDEN
    }
}

impl Nav {
    pub const HIDDEN: Nav = Nav {
        view: View::Hidden,
        nested: None,
        focus: Focus::Field,
        query: false,
        category: false,
        opener: Focus::Field,
    };

    pub fn is_visible(&self) -> bool {
        self.view != View::Hidden
    }

    /// The state a view opens in: empty query, no filter, the keyboard in the field.
    fn opened(view: View) -> Nav {
        Nav {
            view,
            ..Nav::HIDDEN
        }
    }

    /// What the next Escape would do, without doing it.
    pub fn escape_step(&self) -> Step {
        let mut copy = *self;
        copy.apply(Intent::Escape)
    }

    /// Take `intent` and return what the model has to do.
    pub fn apply(&mut self, intent: Intent) -> Step {
        if self.view == View::Hidden {
            return match intent {
                Intent::Toggle => {
                    *self = Nav::opened(View::Search);
                    Step::Show(View::Search)
                }
                Intent::Browse => {
                    *self = Nav::opened(View::Browse);
                    Step::Show(View::Browse)
                }
                Intent::Files => {
                    *self = Nav::opened(View::Files);
                    Step::Show(View::Files)
                }
                _ => Step::Stay,
            };
        }
        match intent {
            Intent::Toggle | Intent::Dismiss => self.hide(),
            Intent::Browse => match self.view {
                View::Browse => self.hide(),
                _ => {
                    // the query typed so far filters the applications
                    *self = Nav {
                        view: View::Browse,
                        query: self.query,
                        ..Nav::HIDDEN
                    };
                    Step::Unfold
                }
            },
            Intent::Files => match self.view {
                View::Files => self.hide(),
                _ => {
                    // the query typed so far filters the files
                    *self = Nav {
                        view: View::Files,
                        query: self.query,
                        ..Nav::HIDDEN
                    };
                    Step::UnfoldFiles
                }
            },
            Intent::Escape => self.escape(),
            Intent::Query(typed) => {
                self.query = typed;
                if typed && self.view == View::Search {
                    // typing gives the keyboard to the field; the browser keeps its selection
                    self.focus = Focus::Field;
                }
                Step::Noted
            }
            Intent::Category(on) => {
                if !self.view.is_scope() {
                    return Step::Stay;
                }
                self.category = on;
                Step::Noted
            }
            Intent::Focus(focus) => {
                let allowed = match focus {
                    Focus::Field | Focus::Content => true,
                    Focus::Categories => self.view.is_scope(),
                };
                if !allowed || self.nested.is_some() || self.focus == focus {
                    return Step::Stay;
                }
                self.focus = focus;
                Step::Noted
            }
            Intent::Open(nested) => {
                let allowed = match nested {
                    Nested::Preview => self.view == View::Search && self.focus == Focus::Content,
                    Nested::ViewOptions => self.view == View::Browse,
                    Nested::Parameters => self.view == View::Search,
                };
                if !allowed || self.nested.is_some() {
                    return Step::Stay;
                }
                self.nested = Some(nested);
                self.opener = self.focus;
                Step::Enter(nested)
            }
            Intent::Done => match self.nested.take() {
                Some(nested) => {
                    self.focus = self.opener;
                    Step::Leave(nested, self.opener)
                }
                None => Step::Stay,
            },
        }
    }

    fn hide(&mut self) -> Step {
        *self = Nav::HIDDEN;
        Step::Hide
    }

    fn escape(&mut self) -> Step {
        if let Some(nested) = self.nested.take() {
            self.focus = self.opener;
            return Step::Leave(nested, self.opener);
        }
        if self.view == View::Search && self.focus == Focus::Content {
            self.focus = Focus::Field;
            return Step::FocusField;
        }
        if self.query {
            self.query = false;
            if self.view == View::Search {
                self.focus = Focus::Field;
            }
            return Step::ClearQuery;
        }
        if self.view.is_scope() && self.category {
            self.category = false;
            return Step::ClearCategory;
        }
        if self.view.is_scope() {
            *self = Nav::opened(View::Search);
            Step::Fold
        } else {
            self.hide()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nav(view: View) -> Nav {
        Nav::opened(view)
    }

    fn with(view: View, f: impl FnOnce(&mut Nav)) -> Nav {
        let mut n = nav(view);
        f(&mut n);
        n
    }

    /// Every row: a state, an intent, the step and the state after it.
    #[test]
    fn transitions_follow_the_table() {
        use Focus::*;
        use Intent as I;
        use Step as S;
        let search = nav(View::Search);
        let browse = nav(View::Browse);
        let typed = with(View::Search, |n| n.query = true);
        let in_list = with(View::Search, |n| {
            n.query = true;
            n.focus = Content;
        });
        let preview = with(View::Search, |n| {
            n.query = true;
            n.focus = Content;
            n.nested = Some(Nested::Preview);
            n.opener = Content;
        });
        let parameters = with(View::Search, |n| {
            n.query = true;
            n.nested = Some(Nested::Parameters);
        });
        let filtered = with(View::Browse, |n| n.category = true);
        let browse_typed = with(View::Browse, |n| {
            n.query = true;
            n.category = true;
            n.focus = Content;
        });
        let options = with(View::Browse, |n| {
            n.focus = Categories;
            n.nested = Some(Nested::ViewOptions);
            n.opener = Categories;
        });
        let rows: Vec<(Nav, Intent, Step, Nav)> = vec![
            // opening
            (Nav::HIDDEN, I::Toggle, S::Show(View::Search), search),
            (Nav::HIDDEN, I::Browse, S::Show(View::Browse), browse),
            (Nav::HIDDEN, I::Escape, S::Stay, Nav::HIDDEN),
            (Nav::HIDDEN, I::Dismiss, S::Stay, Nav::HIDDEN),
            (Nav::HIDDEN, I::Query(true), S::Stay, Nav::HIDDEN),
            // Escape: the empty search field closes
            (search, I::Escape, S::Hide, Nav::HIDDEN),
            // Escape: a typed query is cleared first, then the field closes
            (typed, I::Escape, S::ClearQuery, search),
            // Escape: from the list the keyboard returns to the field, the query kept
            (in_list, I::Escape, S::FocusField, typed),
            // Escape: a nested state closes first, the keyboard back where it was
            (
                preview,
                I::Escape,
                S::Leave(Nested::Preview, Content),
                in_list,
            ),
            (
                parameters,
                I::Escape,
                S::Leave(Nested::Parameters, Field),
                typed,
            ),
            (
                options,
                I::Escape,
                S::Leave(Nested::ViewOptions, Categories),
                with(View::Browse, |n| {
                    n.focus = Categories;
                    n.opener = Categories;
                }),
            ),
            // Escape in the browser: query, then category, then fold into search, then close
            (
                browse_typed,
                I::Escape,
                S::ClearQuery,
                with(View::Browse, |n| {
                    n.category = true;
                    n.focus = Content;
                }),
            ),
            (filtered, I::Escape, S::ClearCategory, browse),
            (browse, I::Escape, S::Fold, search),
            // the browser's shortcut toggles it and unfolds the search field, keeping the query
            (browse, I::Browse, S::Hide, Nav::HIDDEN),
            (search, I::Browse, S::Unfold, browse),
            (
                typed,
                I::Browse,
                S::Unfold,
                with(View::Browse, |n| n.query = true),
            ),
            (
                in_list,
                I::Browse,
                S::Unfold,
                with(View::Browse, |n| n.query = true),
            ),
            // the launcher shortcut and a dismissal close whatever is up
            (search, I::Toggle, S::Hide, Nav::HIDDEN),
            (browse, I::Toggle, S::Hide, Nav::HIDDEN),
            (preview, I::Dismiss, S::Hide, Nav::HIDDEN),
            (options, I::Dismiss, S::Hide, Nav::HIDDEN),
            // typing in search gives the keyboard to the field
            (
                with(View::Search, |n| n.focus = Content),
                I::Query(true),
                S::Noted,
                typed,
            ),
            // categories exist only in the browser
            (search, I::Category(true), S::Stay, search),
            (search, I::Focus(Categories), S::Stay, search),
            (
                browse,
                I::Focus(Categories),
                S::Noted,
                with(View::Browse, |n| n.focus = Categories),
            ),
            // nested states open only where they belong, one at a time, and keep the keyboard's place
            (typed, I::Open(Nested::Preview), S::Stay, typed),
            (
                in_list,
                I::Open(Nested::Preview),
                S::Enter(Nested::Preview),
                preview,
            ),
            (preview, I::Open(Nested::Parameters), S::Stay, preview),
            (browse, I::Open(Nested::Preview), S::Stay, browse),
            (search, I::Open(Nested::ViewOptions), S::Stay, search),
            (
                with(View::Browse, |n| n.focus = Categories),
                I::Open(Nested::ViewOptions),
                S::Enter(Nested::ViewOptions),
                options,
            ),
            // the keyboard does not move under a nested state
            (preview, I::Focus(Field), S::Stay, preview),
            // a nested state that finishes returns the keyboard to its opener
            (
                options,
                I::Done,
                S::Leave(Nested::ViewOptions, Categories),
                with(View::Browse, |n| {
                    n.focus = Categories;
                    n.opener = Categories;
                }),
            ),
            (search, I::Done, S::Stay, search),
        ];
        for (i, (before, intent, step, after)) in rows.into_iter().enumerate() {
            let mut state = before;
            let got = state.apply(intent);
            assert_eq!(got, step, "row {i}: {before:?} + {intent:?}");
            // the opener is bookkeeping: compare it only while a nested state is open
            let (mut s, mut a) = (state, after);
            if s.nested.is_none() && a.nested.is_none() {
                s.opener = Focus::Field;
                a.opener = Focus::Field;
            }
            assert_eq!(s, a, "row {i}: {before:?} + {intent:?}");
        }
    }

    /// From any state reached by any sequence of intents, Escape pressed often enough closes the launcher, one level
    /// at a time, and never more than six presses.
    #[test]
    fn escape_always_reaches_hidden_within_six_presses() {
        let intents = [
            Intent::Toggle,
            Intent::Browse,
            Intent::Query(true),
            Intent::Query(false),
            Intent::Category(true),
            Intent::Focus(Focus::Content),
            Intent::Focus(Focus::Categories),
            Intent::Open(Nested::Preview),
            Intent::Open(Nested::ViewOptions),
            Intent::Open(Nested::Parameters),
            Intent::Done,
        ];
        // every sequence of up to four intents
        let mut states = vec![Nav::HIDDEN];
        for _ in 0..4 {
            let mut next = Vec::new();
            for s in &states {
                for i in intents {
                    let mut t = *s;
                    t.apply(i);
                    next.push(t);
                }
            }
            states.extend(next);
            states.sort_by_key(|s| format!("{s:?}"));
            states.dedup();
        }
        for start in states {
            let mut s = start;
            let mut presses = 0;
            while s.is_visible() {
                let step = s.apply(Intent::Escape);
                assert_ne!(
                    step,
                    Step::Stay,
                    "{start:?}: Escape must always do something"
                );
                presses += 1;
                assert!(presses <= 6, "{start:?} took more than six presses");
            }
        }
    }

    #[test]
    fn the_escape_step_is_a_preview_that_changes_nothing() {
        let n = with(View::Browse, |n| n.query = true);
        assert_eq!(n.escape_step(), Step::ClearQuery);
        assert!(n.query, "unchanged");
    }
}
