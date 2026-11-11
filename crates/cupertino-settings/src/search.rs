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

#[cfg(test)]
mod tests {
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
    fn the_apps_that_control_or_capture_the_screen_are_found_and_open_their_pages() {
        use crate::pages::{of_hit, SubPage};
        for (query, sub) in [
            ("desktop control", SubPage::DesktopControl),
            ("screen capture", SubPage::ScreenCapture),
        ] {
            let hits = search(query, &[]);
            assert_eq!(
                (hits[0].pane, hits[0].subtitle(), hits[0].rank),
                (PaneId::Privacy, Some(sub.title()), Rank::Section),
                "{query}"
            );
            assert_eq!(of_hit(&hits[0]).sub, Some(sub), "{query} opens its page");
        }
        for query in ["automation", "screenshot", "remote control"] {
            assert_eq!(panes(query)[0].0, PaneId::Privacy, "{query}");
        }
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
}
