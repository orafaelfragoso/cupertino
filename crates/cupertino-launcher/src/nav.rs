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
