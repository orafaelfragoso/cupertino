//! Fuzzy matching with tiered ranking.
//!
//! A query is split on whitespace into tokens; every token has to match one field of a candidate
//! (name, generic name, keywords, executable, ...). A token matches a field in one of six tiers,
//! best first:
//!
//! | tier | meaning | score |
//! |---|---|---|
//! | `Exact` | the field equals the token | 1000 |
//! | `Prefix` | the field starts with the token | 900 (minus a small length penalty) |
//! | `WordPrefix` | a later word of the field starts with the token | 700 − 20 per preceding word |
//! | `Acronym` | the token spells initials of consecutive-or-skipped words (`vsc` → *Visual Studio Code*, camel-case humps count as words) | 600 − 15 per skipped word |
//! | `Substring` | the token occurs inside a word | 450 − 3 per leading character |
//! | `Fuzzy` | the token is a subsequence spanning at most `len + max(4, len/2) characters, scored by a gap/boundary/consecutive-run dynamic program | 100…280 |
//!
//! All numbers are authored (approximate) and live in [`Weights`]. Comparison is
//! case-insensitive and folds common Latin diacritics. The score of a candidate is the mean of its
//! token scores, raised to the score of the whole query as a prefix of the name when that is
//! higher. Fields carry a weight (the name counts fully, comments least).

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier {
    Fuzzy,
    Substring,
    Acronym,
    WordPrefix,
    Prefix,
    Exact,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Weights {
    pub exact: f64,
    pub prefix: f64,
    pub word_prefix: f64,
    pub acronym: f64,
    pub substring: f64,
    pub fuzzy_floor: f64,
    pub fuzzy_span: f64,
}

impl Default for Weights {
    fn default() -> Self {
        Self { exact: 1000.0, prefix: 900.0, word_prefix: 700.0, acronym: 600.0, substring: 450.0, fuzzy_floor: 100.0, fuzzy_span: 180.0 }
    }
}

/// Lower-case a character and fold common Latin diacritics to their base letter.
pub fn fold_char(c: char) -> char {
    let l = c.to_lowercase().next().unwrap_or(c);
    match l {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' => 'a',
        'ç' | 'ć' | 'č' | 'ĉ' | 'ċ' => 'c',
        'ď' | 'đ' => 'd',
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ė' | 'ę' | 'ě' => 'e',
        'ì' | 'í' | 'î' | 'ï' | 'ī' | 'į' | 'ı' => 'i',
        'ł' => 'l',
        'ñ' | 'ń' | 'ň' => 'n',
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ő' => 'o',
        'ŕ' | 'ř' => 'r',
        'ś' | 'š' | 'ş' | 'ș' => 's',
        'ť' | 'ţ' | 'ț' => 't',
        'ù' | 'ú' | 'û' | 'ü' | 'ū' | 'ů' | 'ű' | 'ų' => 'u',
        'ý' | 'ÿ' => 'y',
        'ź' | 'ż' | 'ž' => 'z',
        other => other,
    }
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric()
}

/// A searchable string with precomputed folded characters and word starts.
#[derive(Clone, Debug)]
pub struct Field {
    pub text: String,
    chars: Vec<char>,
    bonus: Vec<f64>,
    word_starts: Vec<usize>,
}

const BONUS_START: f64 = 0.9;
const BONUS_BOUNDARY: f64 = 0.8;
const BONUS_CAMEL: f64 = 0.7;
const BONUS_DOT: f64 = 0.6;
const CONSECUTIVE: f64 = 1.0;
const GAP_LEADING: f64 = -0.005;
const GAP_TRAILING: f64 = -0.005;
const GAP_INNER: f64 = -0.1;

impl Field {
    pub fn new(text: &str) -> Field {
        let orig: Vec<char> = text.chars().collect();
        let chars: Vec<char> = orig.iter().map(|&c| fold_char(c)).collect();
        let mut bonus = Vec::with_capacity(orig.len());
        let mut word_starts = Vec::new();
        for (i, &c) in orig.iter().enumerate() {
            let prev = if i == 0 { None } else { Some(orig[i - 1]) };
            let boundary = match prev {
                None => true,
                Some(p) => (!is_word_char(p) && is_word_char(c)) || (p.is_lowercase() && c.is_uppercase()) || (p.is_alphabetic() && c.is_numeric()),
            };
            if boundary && is_word_char(c) {
                word_starts.push(i);
            }
            bonus.push(match prev {
                None => BONUS_START,
                Some('.') => BONUS_DOT,
                Some(p) if !is_word_char(p) => BONUS_BOUNDARY,
                Some(p) if p.is_lowercase() && c.is_uppercase() => BONUS_CAMEL,
                _ => 0.0,
            });
        }
        Field { text: text.to_string(), chars, bonus, word_starts }
    }

    pub fn len(&self) -> usize {
        self.chars.len()
    }

    pub fn is_empty(&self) -> bool {
        self.chars.is_empty()
    }
}

/// A parsed query.
#[derive(Clone, Debug, Default)]
pub struct Query {
    pub raw: String,
    pub tokens: Vec<Vec<char>>,
    joined: Vec<char>,
}

impl Query {
    pub fn new(s: &str) -> Query {
        let tokens: Vec<Vec<char>> = s.split_whitespace().map(|t| t.chars().map(fold_char).collect()).collect();
        let joined = s.split_whitespace().collect::<Vec<_>>().join(" ").chars().map(fold_char).collect();
        Query { raw: s.to_string(), tokens, joined }
    }

    pub fn is_empty(&self) -> bool {
        self.tokens.is_empty()
    }

    /// Normalised text of the query (single spaces, folded) used as key for learned associations.
    pub fn normalized(&self) -> String {
        self.joined.iter().collect()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct TokenMatch {
    pub score: f64,
    pub positions: Vec<usize>,
    pub tier: Tier,
}

fn is_subsequence(needle: &[char], hay: &[char]) -> bool {
    let mut it = hay.iter();
    needle.iter().all(|n| it.any(|h| h == n))
}

/// Dynamic-programming subsequence score and positions (higher is better).
fn fuzzy(needle: &[char], f: &Field) -> Option<(f64, Vec<usize>)> {
    let (n, m) = (needle.len(), f.chars.len());
    if n == 0 || m < n || !is_subsequence(needle, &f.chars) {
        return None;
    }
    let neg = f64::NEG_INFINITY;
    let mut d = vec![neg; n * m];
    let mut best = vec![neg; n * m];
    for i in 0..n {
        let gap = if i == n - 1 { GAP_TRAILING } else { GAP_INNER };
        let mut prev = neg;
        for j in 0..m {
            let score = if needle[i] == f.chars[j] {
                if i == 0 {
                    Some(j as f64 * GAP_LEADING + f.bonus[j])
                } else if j > 0 {
                    Some((best[(i - 1) * m + j - 1] + f.bonus[j]).max(d[(i - 1) * m + j - 1] + CONSECUTIVE))
                } else {
                    None
                }
            } else {
                None
            };
            match score {
                Some(s) => {
                    d[i * m + j] = s;
                    prev = s.max(prev + gap);
                }
                None => prev += gap,
            }
            best[i * m + j] = prev;
        }
    }
    let total = best[n * m - 1];
    if !total.is_finite() {
        return None;
    }
    let mut pos = vec![0usize; n];
    let mut required = false;
    let mut j = m as isize - 1;
    for i in (0..n).rev() {
        while j >= 0 {
            let (dj, bj) = (d[i * m + j as usize], best[i * m + j as usize]);
            if dj.is_finite() && (required || dj == bj) {
                required = i > 0 && j > 0 && bj == d[(i - 1) * m + j as usize - 1] + CONSECUTIVE;
                pos[i] = j as usize;
                j -= 1;
                break;
            }
            j -= 1;
        }
    }
    let span = pos[n - 1] + 1 - pos[0];
    if span > n + (n / 2).max(4) {
        return None;
    }
    Some((total, pos))
}

/// Match one token against a field. `allow_fuzzy`: also accept scattered subsequences (tokens of
/// a single character never match fuzzily).
pub fn match_token(token: &[char], f: &Field, allow_fuzzy: bool, w: &Weights) -> Option<TokenMatch> {
    let m = token.len();
    if m == 0 || f.chars.len() < m {
        return None;
    }
    let hay = &f.chars;
    if hay == token {
        return Some(TokenMatch { score: w.exact, positions: (0..m).collect(), tier: Tier::Exact });
    }
    if hay.starts_with(token) {
        let penalty = ((hay.len() - m) as f64).min(50.0);
        return Some(TokenMatch { score: w.prefix - penalty, positions: (0..m).collect(), tier: Tier::Prefix });
    }
    for (wi, &s) in f.word_starts.iter().enumerate().skip(1) {
        if hay[s..].starts_with(token) {
            let score = (w.word_prefix - 20.0 * wi as f64).max(w.acronym + 20.0);
            return Some(TokenMatch { score, positions: (s..s + m).collect(), tier: Tier::WordPrefix });
        }
    }
    if m >= 2 && f.word_starts.len() >= 2 {
        let mut ti = 0;
        let mut pos = Vec::new();
        let mut skipped = 0usize;
        for &s in &f.word_starts {
            if ti < m && hay[s] == token[ti] {
                pos.push(s);
                ti += 1;
            } else if ti < m {
                skipped += 1;
            }
        }
        if ti == m {
            let score = (w.acronym - 15.0 * skipped as f64).max(w.substring + 20.0);
            return Some(TokenMatch { score, positions: pos, tier: Tier::Acronym });
        }
    }
    if let Some(off) = (0..=hay.len() - m).find(|&i| &hay[i..i + m] == token) {
        let score = (w.substring - 3.0 * off as f64).max(w.substring - 90.0);
        return Some(TokenMatch { score, positions: (off..off + m).collect(), tier: Tier::Substring });
    }
    if allow_fuzzy && m >= 2 {
        let (raw, positions) = fuzzy(token, f)?;
        let norm = (raw / (m as f64 * 1.1)).clamp(0.0, 1.0);
        return Some(TokenMatch { score: w.fuzzy_floor + w.fuzzy_span * norm, positions, tier: Tier::Fuzzy });
    }
    None
}

/// A searchable item: field 0 is the display name (highlight positions come from it).
#[derive(Clone, Debug)]
pub struct Candidate {
    pub fields: Vec<CandidateField>,
    /// Index of a field at least one token must match with a non-fuzzy tier (used for desktop
    /// actions, so that typing an application name does not list all its actions).
    pub required_field: Option<usize>,
}

#[derive(Clone, Debug)]
pub struct CandidateField {
    pub field: Field,
    pub weight: f64,
    pub allow_fuzzy: bool,
}

impl CandidateField {
    pub fn new(text: &str, weight: f64, allow_fuzzy: bool) -> CandidateField {
        CandidateField { field: Field::new(text), weight, allow_fuzzy }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Scored {
    pub score: f64,
    /// Matched character indices in field 0.
    pub positions: Vec<usize>,
    /// Tier of the weakest-matching token on its best field.
    pub tier: Tier,
}

pub fn score(q: &Query, c: &Candidate, w: &Weights) -> Option<Scored> {
    if q.is_empty() || c.fields.is_empty() {
        return None;
    }
    let mut total = 0.0;
    let mut positions: Vec<usize> = Vec::new();
    let mut weakest = Tier::Exact;
    let mut required_hit = c.required_field.is_none();
    for tok in &q.tokens {
        let mut best: Option<(f64, usize, TokenMatch)> = None;
        for (i, cf) in c.fields.iter().enumerate() {
            let Some(m) = match_token(tok, &cf.field, cf.allow_fuzzy, w) else { continue };
            let s = m.score * cf.weight;
            if Some(i) == c.required_field && m.tier != Tier::Fuzzy {
                required_hit = true;
            }
            if best.as_ref().is_none_or(|(b, _, _)| s > *b) {
                best = Some((s, i, m));
            }
        }
        let (s, i, m) = best?;
        total += s;
        weakest = weakest.min(m.tier);
        if i == 0 {
            positions.extend(m.positions);
        }
    }
    if !required_hit {
        return None;
    }
    let mut score = total / q.tokens.len() as f64;
    if q.tokens.len() > 1 {
        let name = &c.fields[0].field;
        if name.chars.starts_with(&q.joined) {
            let whole = w.prefix - ((name.chars.len() - q.joined.len()) as f64).min(50.0) + 20.0;
            if whole > score {
                score = whole;
                positions = (0..q.joined.len()).collect();
            }
        }
    }
    positions.sort_unstable();
    positions.dedup();
    Some(Scored { score, positions, tier: weakest })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w() -> Weights {
        Weights::default()
    }

    fn cand(name: &str) -> Candidate {
        Candidate { fields: vec![CandidateField::new(name, 1.0, true)], required_field: None }
    }

    fn sc(q: &str, name: &str) -> Option<Scored> {
        score(&Query::new(q), &cand(name), &w())
    }

    fn tier(q: &str, name: &str) -> Option<Tier> {
        sc(q, name).map(|s| s.tier)
    }

    #[test]
    fn tiers_are_recognised() {
        assert_eq!(tier("firefox", "Firefox"), Some(Tier::Exact));
        assert_eq!(tier("fire", "Firefox"), Some(Tier::Prefix));
        assert_eq!(tier("studio", "Visual Studio Code"), Some(Tier::WordPrefix));
        assert_eq!(tier("vsc", "Visual Studio Code"), Some(Tier::Acronym));
        assert_eq!(tier("lw", "LibreOffice Writer"), Some(Tier::Acronym));
        assert_eq!(tier("ice", "LibreOffice"), Some(Tier::Substring));
        assert_eq!(tier("frfx", "Firefox"), Some(Tier::Fuzzy));
        assert_eq!(tier("zzz", "Firefox"), None);
        assert_eq!(tier("fx", "Calculator"), None);
    }

    #[test]
    fn tier_ordering_drives_scores() {
        let s = |q, n| sc(q, n).unwrap().score;
        let exact = s("code", "Code");
        let prefix = s("code", "Code Editor");
        let word = s("code", "Visual Code");
        let acro = s("vc", "Visual Code");
        let sub = s("ode", "Code");
        let fuzzy = s("cde", "Code");
        assert!(exact > prefix && prefix > word && word > acro && acro > sub && sub > fuzzy, "{exact} {prefix} {word} {acro} {sub} {fuzzy}");
        assert!(fuzzy >= 100.0);
    }

    #[test]
    fn shorter_prefix_matches_rank_higher() {
        assert!(sc("ter", "Terminal").unwrap().score > sc("ter", "Terminal Emulator Pro").unwrap().score);
    }

    #[test]
    fn earlier_words_rank_higher() {
        assert!(sc("edit", "Text Editor").unwrap().score > sc("edit", "Advanced Text Photo Editor").unwrap().score);
    }

    #[test]
    fn case_and_diacritics_are_ignored() {
        assert_eq!(tier("CAFE", "Café"), Some(Tier::Exact));
        assert_eq!(tier("uber", "Über Uns"), Some(Tier::Prefix));
        assert_eq!(tier("nino", "El Niño"), Some(Tier::WordPrefix));
        assert_eq!(fold_char('É'), 'e');
    }

    #[test]
    fn highlight_positions_are_character_indices() {
        let s = sc("vsc", "Visual Studio Code").unwrap();
        assert_eq!(s.positions, vec![0, 7, 14]);
        let s = sc("stu", "Visual Studio Code").unwrap();
        assert_eq!(s.positions, vec![7, 8, 9]);
        let s = sc("frfx", "Firefox").unwrap();
        assert_eq!(s.positions.len(), 4);
        let chars: Vec<char> = "Firefox".chars().collect();
        let picked: String = s.positions.iter().map(|&i| chars[i]).collect();
        assert_eq!(picked.to_lowercase(), "frfx");
    }

    #[test]
    fn fuzzy_prefers_consecutive_runs_and_boundaries() {
        let a = sc("gnt", "GNOME Terminal").unwrap();
        let b = sc("gnt", "Gnat Brigade Tool").unwrap();
        assert!(a.score > 100.0 && b.score > 100.0);
        let consecutive = sc("abc", "xabcx").unwrap().score;
        let scattered = sc("abc", "xaxbxcx").unwrap().score;
        assert!(consecutive > scattered);
    }

    #[test]
    fn multi_token_queries_need_every_token() {
        assert!(sc("libre wr", "LibreOffice Writer").is_some());
        assert!(sc("wr libre", "LibreOffice Writer").is_some());
        assert!(sc("libre xyz", "LibreOffice Writer").is_none());
        let whole = sc("visual studio", "Visual Studio Code").unwrap();
        assert!(whole.score >= 900.0, "whole query is a name prefix: {}", whole.score);
        assert_eq!(whole.positions, (0..13).collect::<Vec<_>>());
    }

    #[test]
    fn fuzzy_matches_must_be_compact() {
        assert_eq!(tier("fire", "LibreOffice Writer"), None, "letters scattered over two words");
        assert_eq!(tier("fire", "Firefox"), Some(Tier::Prefix));
        assert_eq!(tier("frfx", "Firefox"), Some(Tier::Fuzzy));
        assert_eq!(tier("chrmm", "Chromium Browser"), Some(Tier::Fuzzy));
    }

    #[test]
    fn single_character_queries_are_not_fuzzy() {
        assert_eq!(tier("x", "Firefox"), Some(Tier::Substring));
        assert_eq!(tier("q", "Firefox"), None);
        assert_eq!(tier("f", "Firefox"), Some(Tier::Prefix));
    }

    #[test]
    fn other_fields_count_with_their_weight() {
        let c = Candidate {
            fields: vec![CandidateField::new("Web Browser", 1.0, true), CandidateField::new("firefox", 0.6, false)],
            required_field: None,
        };
        let s = score(&Query::new("firefox"), &c, &w()).unwrap();
        assert!((s.score - 600.0).abs() < 1e-9, "{}", s.score);
        assert!(s.positions.is_empty(), "no highlight outside the name");
        let by_name = score(&Query::new("web"), &c, &w()).unwrap();
        assert!(by_name.score > s.score);
    }

    #[test]
    fn required_field_filters_candidates() {
        let c = Candidate {
            fields: vec![CandidateField::new("Web Browser: New Private Window", 0.8, true), CandidateField::new("New Private Window", 1.0, false)],
            required_field: Some(1),
        };
        assert!(score(&Query::new("web"), &c, &w()).is_none());
        assert!(score(&Query::new("private"), &c, &w()).is_some());
        assert!(score(&Query::new("web private"), &c, &w()).is_some());
    }

    #[test]
    fn empty_query_and_field_do_not_match() {
        assert!(sc("", "Firefox").is_none());
        assert!(sc("   ", "Firefox").is_none());
        assert!(sc("a", "").is_none());
        assert!(Field::new("").is_empty());
    }

    #[test]
    fn unicode_does_not_panic() {
        for q in ["日本", "😀", "ß", "İ"] {
            let _ = sc(q, "日本語 Editor 😀 Straße");
        }
        assert!(sc("日本", "日本語 Editor").is_some());
    }

    #[test]
    fn query_normalisation() {
        assert_eq!(Query::new("  Libre   WR ").normalized(), "libre wr");
    }
}

