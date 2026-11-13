//! Refining a search from the query itself: a kind token (`/files`, `/apps` ..) keeps the results of one kind, and a
//! kind's name followed by Tab turns into its token, so the search is scoped to that kind.
//!
//! A token is a word of its own that starts with `/` and names a kind; it may stand anywhere in the query. Any other
//! word starting with `/` (a path, say) is searched as typed.

use crate::engine::SectionKind;

/// A kind of result a token keeps.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Applications,
    Files,
    Folders,
    Calculator,
    Web,
}

impl Kind {
    pub const ALL: [Kind; 5] = [
        Kind::Applications,
        Kind::Files,
        Kind::Folders,
        Kind::Calculator,
        Kind::Web,
    ];

    /// The token's word after the slash, and the other words that name the kind.
    fn words(self) -> &'static [&'static str] {
        match self {
            Kind::Applications => &["apps", "app", "applications", "application"],
            Kind::Files => &["files", "file", "documents", "document", "docs"],
            Kind::Folders => &["folders", "folder", "directories", "directory"],
            Kind::Calculator => &["calculator", "calc", "math", "conversion", "convert"],
            Kind::Web => &["web"],
        }
    }

    /// The token that keeps this kind, as the query shows it.
    pub fn token(self) -> String {
        format!("/{}", self.words()[0])
    }

    /// The kind `word` names (any case), if any.
    pub fn named(word: &str) -> Option<Kind> {
        let word = word.to_lowercase();
        Kind::ALL
            .into_iter()
            .find(|k| k.words().contains(&word.as_str()))
    }

    /// Whether a section of `section`'s kind holds results of this kind. The top hit is decided after the filter.
    pub fn keeps(self, section: SectionKind) -> bool {
        matches!(
            (self, section),
            (
                Kind::Applications,
                SectionKind::Applications | SectionKind::Suggestions
            ) | (Kind::Files, SectionKind::Files)
                | (Kind::Folders, SectionKind::Folders)
                | (
                    Kind::Calculator,
                    SectionKind::Calculator | SectionKind::Conversion
                )
                | (Kind::Web, SectionKind::Web)
        )
    }
}

/// A query with its kind tokens taken out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refined {
    /// The kinds the tokens keep; empty: every kind.
    pub kinds: Vec<Kind>,
    /// What is searched: the query without its tokens, its words joined by single spaces.
    pub text: String,
}

impl Refined {
    /// Whether a section of `section`'s kind is shown.
    pub fn keeps(&self, section: SectionKind) -> bool {
        self.kinds.is_empty() || self.kinds.iter().any(|k| k.keeps(section))
    }
}

/// `query` split into its kind tokens and the text searched. A query without a token is searched as typed.
pub fn refine(query: &str) -> Refined {
    let mut kinds = Vec::new();
    let mut words = Vec::new();
    for word in query.split_whitespace() {
        match word.strip_prefix('/').and_then(Kind::named) {
            Some(kind) => {
                if !kinds.contains(&kind) {
                    kinds.push(kind);
                }
            }
            None => words.push(word),
        }
    }
    if kinds.is_empty() {
        return Refined {
            kinds,
            text: query.to_owned(),
        };
    }
    Refined {
        kinds,
        text: words.join(" "),
    }
}

/// What Tab makes of `query` when it is a kind's name and nothing else: the kind's token and a space, ready for the
/// words to search for. `None` when the query names no kind.
pub fn scope_by_name(query: &str) -> Option<String> {
    let name = query.trim();
    if name.is_empty() || name.contains(char::is_whitespace) || name.starts_with('/') {
        return None;
    }
    Kind::named(name).map(|k| format!("{} ", k.token()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_kind_token_anywhere_keeps_its_kind_and_leaves_the_words_searched() {
        let r = refine("/files report");
        assert_eq!(r.kinds, vec![Kind::Files]);
        assert_eq!(r.text, "report");
        let r = refine("tax  /FOLDERS  2024");
        assert_eq!(r.kinds, vec![Kind::Folders]);
        assert_eq!(r.text, "tax 2024");
        assert!(r.keeps(SectionKind::Folders));
        assert!(!r.keeps(SectionKind::Files));
        assert!(!r.keeps(SectionKind::Web));
        let r = refine("/apps /calc");
        assert_eq!(r.kinds, vec![Kind::Applications, Kind::Calculator]);
        assert!(r.keeps(SectionKind::Applications) && r.keeps(SectionKind::Calculator));
    }

    #[test]
    fn a_slash_word_that_names_no_kind_is_searched_as_typed() {
        let r = refine("/usr/share notes");
        assert!(r.kinds.is_empty());
        assert_eq!(r.text, "/usr/share notes");
        assert!(r.keeps(SectionKind::Files) && r.keeps(SectionKind::Web));
        // a query without a token keeps its spacing
        assert_eq!(refine(" 2 + 2 ").text, " 2 + 2 ");
    }

    #[test]
    fn tab_after_a_kind_s_name_turns_it_into_its_token() {
        assert_eq!(scope_by_name("Files").as_deref(), Some("/files "));
        assert_eq!(scope_by_name("apps").as_deref(), Some("/apps "));
        assert_eq!(scope_by_name("Folder ").as_deref(), Some("/folders "));
        assert_eq!(scope_by_name("firefox"), None);
        assert_eq!(scope_by_name("my files"), None);
        assert_eq!(scope_by_name("/files"), None);
        assert_eq!(scope_by_name(""), None);
    }
}
