//! Search (§5): an authored table per pane (title, section titles, index words; the
//! reference indexes its panes' search terms in a system index whose ranking is not known), matched and
//! ranked locally on every edit, and the geometry of the results popup.
//!
//! Matching: a result matches when every typed word is a prefix of a word of the entry, case- and width-insensitive
//! with diacritics significant (the collation flags of, the word rule approximate). A hyphenated word
//! also counts without its hyphens ("wifi" finds Wi-Fi; approximate). The words of a query may match in different
//! places of a pane, in any order: its title, its section titles and its index words and row titles together ("mouse
//! pointer" finds Mouse, by its title and the word "pointer"). Ranking: exact title, title prefix, a title word with
//! the rest elsewhere in the pane, section title, index word; ties in sidebar order (approximate).

use crate::pane::{Pane, PaneId, PANES};
use crate::sidebar::{self, fold_width, RowSize};
use cupertino_text_field::TextField;

/// Why an entry matched, best first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Rank {
    ExactTitle,
    TitlePrefix,
    /// A word of the query is a word of the title, the others match elsewhere in the pane.
    TitleWord,
    Section,
    Word,
}

/// One search result: the pane, the section it scrolls to (`None`: the pane's top) and why it matched.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hit {
    pub pane: PaneId,
    /// Index into the pane's `sections`.
    pub section: Option<usize>,
    pub rank: Rank,
}

impl Hit {
    /// The subtext of the result row: the section's title.
    pub fn subtitle(&self) -> Option<&'static str> {
        self.section.map(|i| self.pane.pane().sections[i])
    }
}

/// `text` case- and width-folded (diacritics kept).
fn fold(text: &str) -> String {
    text.chars()
        .map(fold_width)
        .flat_map(char::to_lowercase)
        .collect()
}

/// The words an entry is found by: its whitespace-separated words, folded, with each hyphenated word also joined.
fn words(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for w in fold(text).split_whitespace() {
        let w = w.trim_matches(|c: char| !c.is_alphanumeric());
        if w.is_empty() {
            continue;
        }
        out.push(w.to_string());
        if w.contains('-') {
            out.push(w.replace('-', ""));
        }
    }
    out
}

/// Whether every word of `query` is a prefix of a word of `text`.
fn prefixes(query: &[String], text: &str) -> bool {
    let entry = words(text);
    !query.is_empty()
        && query
            .iter()
            .all(|q| entry.iter().any(|w| w.starts_with(q.as_str())))
}

/// The best match of `query` in `pane`: the title, else each section, else an index word.
fn matches(pane: &Pane, query: &[String], joined: &str) -> Vec<Hit> {
    let hit = |section, rank| Hit {
        pane: pane.id,
        section,
        rank,
    };
    let mut out = Vec::new();
    if fold(pane.title) == joined {
        out.push(hit(None, Rank::ExactTitle));
    } else if prefixes(query, pane.title) {
        out.push(hit(None, Rank::TitlePrefix));
    }
    for (i, section) in pane.sections.iter().enumerate() {
        if prefixes(query, section) {
            out.push(hit(Some(i), Rank::Section));
        }
    }
    if out.is_empty() {
        let title = words(pane.title);
        let pane_words: Vec<String> = pane
            .words
            .iter()
            .chain(pane.sections)
            .flat_map(|w| words(w))
            .chain(title.iter().cloned())
            .collect();
        let found = |q: &String| pane_words.iter().any(|w| w.starts_with(q.as_str()));
        if query.iter().all(found) {
            let in_title = query
                .iter()
                .any(|q| title.iter().any(|w| w.starts_with(q.as_str())));
            out.push(hit(
                None,
                if in_title {
                    Rank::TitleWord
                } else {
                    Rank::Word
                },
            ));
        }
    }
    out
}

/// The results of `query` without the `hidden` panes, best first; empty for an empty query.
pub fn search(query: &str, hidden: &[PaneId]) -> Vec<Hit> {
    let q = words(query);
    if q.is_empty() {
        return Vec::new();
    }
    // the typed words as typed (a hyphenated one only once), to tell an exact title
    let joined = fold(&query.split_whitespace().collect::<Vec<_>>().join(" "));
    let order = sidebar::rows(hidden);
    let mut hits: Vec<Hit> = PANES
        .iter()
        .filter(|p| !hidden.contains(&p.id))
        .flat_map(|p| matches(p, &q, &joined))
        .collect();
    hits.sort_by_key(|h| (h.rank, order.iter().position(|p| *p == h.pane), h.section));
    hits
}

/// The most bytes the search field holds.
pub const MAX_LEN: usize = 1024;

/// The search field's text editor, the results it shows and the highlighted one.
#[derive(Clone, Debug)]
pub struct SearchField {
    field: TextField,
    results: Vec<Hit>,
    highlighted: Option<usize>,
    /// The panes left out of the sidebar, and so of the results.
    hidden: Vec<PaneId>,
}

impl Default for SearchField {
    fn default() -> Self {
        SearchField {
            field: TextField::with_max_len(MAX_LEN),
            results: Vec::new(),
            highlighted: None,
            hidden: Vec::new(),
        }
    }
}

impl SearchField {
    pub fn text(&self) -> &str {
        self.field.text()
    }

    pub fn field(&self) -> &TextField {
        &self.field
    }

    pub fn results(&self) -> &[Hit] {
        &self.results
    }

    pub fn highlighted(&self) -> Option<usize> {
        self.highlighted
    }

    /// Whether the results popup is open: while the field has text (an empty result shows "No results").
    pub fn is_open(&self) -> bool {
        !self.field.is_empty()
    }

    /// Change the field with `f`; when its text changes the search runs again.
    pub fn edit<R>(&mut self, f: impl FnOnce(&mut TextField) -> R) -> R {
        let before = self.field.text().to_string();
        let r = f(&mut self.field);
        if self.field.text() != before {
            self.results = search(self.field.text(), &self.hidden);
            self.highlighted = (!self.results.is_empty()).then_some(0);
        }
        r
    }

    /// Leave `hidden` out of the results from the next search on.
    pub fn set_hidden(&mut self, hidden: &[PaneId]) {
        self.hidden = hidden.to_vec();
    }

    /// Empty the field and close the popup.
    pub fn clear(&mut self) {
        *self = SearchField {
            hidden: std::mem::take(&mut self.hidden),
            ..SearchField::default()
        };
    }

    /// Move the highlight by `step` rows, staying on the results.
    pub fn step(&mut self, step: isize) {
        if let Some(i) = self.highlighted {
            self.highlighted = Some(i.saturating_add_signed(step).min(self.results.len() - 1));
        }
    }

    pub fn highlight(&mut self, index: usize) {
        if index < self.results.len() {
            self.highlighted = Some(index);
        }
    }

    /// Choose the highlighted result: the field empties and the popup closes.
    pub fn choose(&mut self) -> Option<Hit> {
        let hit = self.highlighted.map(|i| self.results[i]);
        if hit.is_some() {
            self.clear();
        }
        hit
    }
}

/// Geometry of the results popup.
pub mod popup {
    use super::RowSize;

    /// Size at creation: `ceil(200 f) × 100`.
    pub fn creation_size(f: f64) -> (f64, f64) {
        ((200.0 * f).ceil(), 100.0)
    }

    /// The popup's corner radius.
    pub const RADIUS: f64 = 8.0;
    /// Around the rows; between rows.
    pub const CONTENT_PADDING: f64 = 5.0;
    pub const ROW_SPACING: f64 = 0.0;
    /// A group title row: 12 pt semibold, padding 10 / 5, 25 high.
    pub const GROUP_TITLE_HEIGHT: f64 = 25.0;
    pub const GROUP_TITLE_FONT: f64 = 12.0;
    pub const GROUP_TITLE_PADDING: (f64, f64) = (10.0, 5.0);
    /// A divider: 10 high with 5 pt vertical padding.
    pub const DIVIDER_HEIGHT: f64 = 10.0;
    /// Rows shown before the popup scrolls. approximate.
    pub const MAX_ROWS: usize = 6;
    /// The subtext size of a result row. approximate.
    pub const SUBTEXT_FONT: f64 = 11.0;

    /// The result row's padding (vertical, horizontal) and height: icon + 2 × vertical.
    pub fn row_padding(size: RowSize) -> (f64, f64) {
        match size {
            RowSize::Small => (3.0, 5.0),
            RowSize::Medium => (4.0, 10.0),
            RowSize::Large => (5.0, 10.0),
        }
    }

    pub fn row_height(size: RowSize) -> f64 {
        size.result_icon() + 2.0 * row_padding(size).0
    }

    /// The result rows of a popup `width` wide holding `count` results (one row, "No results", for none), inside the
    /// content padding, in popup points.
    pub fn rows(count: usize, width: f64, size: RowSize) -> Vec<crate::Rect> {
        let h = row_height(size);
        (0..count.max(1))
            .map(|i| {
                crate::Rect::new(
                    CONTENT_PADDING,
                    CONTENT_PADDING + i as f64 * (h + ROW_SPACING),
                    (width - 2.0 * CONTENT_PADDING).max(0.0),
                    h,
                )
            })
            .collect()
    }

    /// The popup's size at factor `f` for `count` results: the creation width, as many rows as there are results
    /// (one for none) up to [`MAX_ROWS`].
    pub fn size(f: f64, count: usize, size: RowSize) -> (f64, f64) {
        (
            creation_size(f).0,
            height(&vec![Item::Row; count.max(1)], size),
        )
    }

    /// One line of the popup.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Item {
        Row,
        GroupTitle,
        Divider,
    }

    /// Height of a popup holding `items`: the padding around them and their heights, rows past [`MAX_ROWS`] scrolled.
    pub fn height(items: &[Item], size: RowSize) -> f64 {
        let mut rows = 0;
        let mut h = 2.0 * CONTENT_PADDING;
        for item in items {
            h += match item {
                Item::Row if rows >= MAX_ROWS => continue,
                Item::Row => {
                    rows += 1;
                    row_height(size)
                }
                Item::GroupTitle => GROUP_TITLE_HEIGHT,
                Item::Divider => DIVIDER_HEIGHT,
            };
        }
        h
    }
}

#[cfg(test)]
mod tests {
    use super::popup::Item;
    use super::*;

    fn panes(query: &str) -> Vec<(PaneId, Option<&'static str>, Rank)> {
        search(query, &[])
            .iter()
            .map(|h| (h.pane, h.subtitle(), h.rank))
            .collect()
    }

    #[test]
    fn the_words_of_a_query_match_anywhere_in_a_pane_in_any_order_and_the_title_ranks_first() {
        let both = panes("mouse pointer");
        assert_eq!(
            both.iter().map(|(p, ..)| *p).collect::<Vec<_>>(),
            [PaneId::Mouse, PaneId::Accessibility]
        );
        assert_eq!(both[0].2, Rank::TitleWord);
        assert_eq!(both[1].2, Rank::Word);
        assert_eq!(
            panes("pointer mouse"),
            both,
            "the order of the words does not matter"
        );
        let pointer = panes("pointer");
        assert_eq!(
            pointer.iter().map(|(p, ..)| *p).collect::<Vec<_>>(),
            [PaneId::Accessibility, PaneId::Mouse],
            "a section title before an index word"
        );
        assert_eq!(pointer[0].1, Some("Pointer"));
        assert_eq!(panes("shake locate")[0].0, PaneId::Accessibility);
        assert_eq!(panes("size pointer")[0].0, PaneId::Accessibility);
        assert!(panes("mouse nonsense").is_empty(), "every word must match");
    }

    #[test]
    fn dark_finds_appearance_and_wifi_finds_wi_fi_by_its_title() {
        assert_eq!(panes("dark"), [(PaneId::Appearance, None, Rank::Word)]);
        for query in ["wifi", "wi-fi"] {
            assert_eq!(panes(query)[0].0, PaneId::Wifi, "{query}");
        }
        assert_eq!(panes("wi-fi")[0].2, Rank::ExactTitle);
    }

    #[test]
    fn words_match_by_prefix_without_case_or_width_but_with_diacritics() {
        assert_eq!(
            panes("keyb")[0],
            (PaneId::Keyboard, None, Rank::TitlePrefix)
        );
        assert_eq!(
            panes("KEYBOARD")[0],
            (PaneId::Keyboard, None, Rank::ExactTitle)
        );
        assert_eq!(
            panes("ｋｅｙｂｏａｒｄ")[0],
            (PaneId::Keyboard, None, Rank::ExactTitle),
            "fullwidth letters"
        );
        assert_eq!(
            panes("wifi")[0],
            (PaneId::Wifi, None, Rank::TitlePrefix),
            "a hyphenated title without the hyphen"
        );
        assert_eq!(panes("désktop"), [], "diacritics are significant");
        assert_eq!(
            panes("dock desk")[0],
            (PaneId::DesktopDock, None, Rank::TitlePrefix),
            "every word, in any order"
        );
        assert!(search("", &[]).is_empty() && search("   ", &[]).is_empty());
    }

    #[test]
    fn titles_rank_before_sections_before_index_words_and_ties_follow_the_sidebar() {
        // "Display" is a title prefix of Displays and a section title of Accessibility; Mouse and Trackpad share "Tracking"
        assert_eq!(
            panes("display")[..2],
            [
                (PaneId::Displays, None, Rank::TitlePrefix),
                (PaneId::Accessibility, Some("Display"), Rank::Section)
            ]
        );
        assert_eq!(
            panes("tracking"),
            [
                (PaneId::Mouse, Some("Tracking"), Rank::Section),
                (PaneId::Trackpad, Some("Tracking"), Rank::Section)
            ]
        );
        assert_eq!(panes("volume"), [(PaneId::Sound, None, Rank::Word)]);
        assert_eq!(panes("sound")[0].2, Rank::ExactTitle);
    }

    #[test]
    fn every_section_of_every_pane_is_found_by_its_title() {
        for p in &PANES {
            for (i, section) in p.sections.iter().enumerate() {
                let hits = search(section, &[]);
                assert!(
                    hits.iter()
                        .any(|h| h.pane == p.id
                            && (h.section == Some(i) || h.rank <= Rank::TitlePrefix)),
                    "{} › {section}: {hits:?}",
                    p.title
                );
            }
        }
    }

    #[test]
    fn the_field_highlights_the_first_result_and_choosing_clears_it() {
        let mut f = SearchField::default();
        f.edit(|t| t.insert("track"));
        assert!(f.is_open());
        assert_eq!(f.highlighted(), Some(0));
        f.step(5);
        assert_eq!(
            f.highlighted(),
            Some(f.results().len() - 1),
            "the highlight stays on the results"
        );
        f.step(-10);
        assert_eq!(f.highlighted(), Some(0));
        let hit = f.choose().unwrap();
        assert_eq!(hit.pane, PaneId::Trackpad);
        assert_eq!((f.text(), f.is_open(), f.results().len()), ("", false, 0));
        f.edit(|t| t.insert("z"));
        assert!(
            f.is_open() && f.results().is_empty() && f.choose().is_none(),
            "no results: nothing to choose, the popup stays"
        );
    }

    #[test]
    fn the_search_runs_again_only_when_the_text_changes() {
        let mut f = SearchField::default();
        f.edit(|t| t.insert("s"));
        assert!(f.results().len() > 1);
        f.step(1);
        assert_eq!(f.highlighted(), Some(1));
        f.edit(TextField::select_all);
        assert_eq!(
            f.highlighted(),
            Some(1),
            "selecting keeps the results and the highlight"
        );
        f.edit(|t| t.insert("Mouse"));
        assert_eq!(f.text(), "Mouse");
        assert_eq!(f.results()[0].pane, PaneId::Mouse);
        assert_eq!(f.highlighted(), Some(0));
    }

    #[test]
    fn the_popup_measures_padding_rows_titles_and_dividers() {
        assert_eq!(popup::creation_size(1.0), (200.0, 100.0));
        assert_eq!(popup::creation_size(1.1), (221.0, 100.0));
        let rows: Vec<f64> = RowSize::ALL.iter().map(|s| popup::row_height(*s)).collect();
        assert_eq!(rows, [22.0, 32.0, 42.0]);
        assert_eq!(popup::row_padding(RowSize::Medium), (4.0, 10.0));
        let items = [
            Item::GroupTitle,
            Item::Row,
            Item::Row,
            Item::Divider,
            Item::GroupTitle,
            Item::Row,
        ];
        assert_eq!(
            popup::height(&items, RowSize::Medium),
            5.0 + 25.0 + 32.0 + 32.0 + 10.0 + 25.0 + 32.0 + 5.0
        );
        let many = [Item::Row; 9];
        assert_eq!(
            popup::height(&many, RowSize::Medium),
            10.0 + 6.0 * 32.0,
            "rows past six scroll"
        );
        assert_eq!(
            popup::size(1.0, 0, RowSize::Medium),
            (200.0, 42.0),
            "the No results row"
        );
        assert_eq!(popup::size(1.0, 2, RowSize::Medium), (200.0, 74.0));
        assert_eq!(
            popup::rows(2, 200.0, RowSize::Medium)[1],
            crate::Rect::new(5.0, 37.0, 190.0, 32.0)
        );
    }
}
