//! Search (§5): an authored table per pane (title, section titles, index words; the
//! reference indexes its panes' search terms in a system index whose ranking is not known), matched and
//! ranked locally on every edit.
//!
//! Matching: a result matches when every typed word is a prefix of a word of the entry, case- and width-insensitive
//! with diacritics significant (the collation flags of, the word rule approximate). A hyphenated word
//! also counts without its hyphens ("wifi" finds Wi-Fi; approximate). The words of a query may match in different
//! places of a pane, in any order: its title, its section titles and its index words and row titles together ("mouse
//! pointer" finds Mouse, by its title and the word "pointer"). Ranking: exact title, title prefix, a title word with
//! the rest elsewhere in the pane, section title, index word; ties in sidebar order (approximate).

use crate::pane::{Pane, PaneId, PANES};
use crate::sidebar::{self, fold_width};

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
