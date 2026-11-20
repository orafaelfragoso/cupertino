//! The source gate: rules every file git would track must follow.
//!
//! Some literals below are assembled with `concat!` so that this file passes its own rules.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fmt::Write as _;
use std::path::{Component, Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use sha2::{Digest, Sha256};

/// File suffixes no file may have, unless it is an allowed binary.
const FORBIDDEN_SUFFIXES: [&str; 3] = [".ttf", ".otf", ".ttc"];

/// Binaries that may be committed, with the sha256 of their exact content.
const ALLOWED_BINARIES: [(&str, &str); 1] = [(
    "crates/cupertino-assets/fonts/InterVariable-subset.ttf",
    "8082ab1587b6b1a0ee14e44287b204f3aae7f2021583ca76beb1f717a093dcea",
)];

const ID: &str = r"\b[A-Z]{2,5}-(?:\d{3}[a-z]?|nnn|xxx)\b";
const CLASS: &str = concat!(
    r"\b(?:VERI",
    "FIED|DOCU",
    "MENTED|INFE",
    "RRED|APPROXI",
    "MATED|UNK",
    r"NOWN)\b"
);
const CACHE_ADDR: &str = r"@?\b0x(?:7ff[0-9a-fA-F]{9}|1[0-9a-fA-F]{8})\b(?:…|\.\.\.)?";
const PROVENANCE: &str = concat!(
    r"(?i)\breco",
    r"vered\b|\bre",
    r"search\b|\b[Tt]he reference\b"
);
/// What may follow the provenance pattern's last alternative without it counting.
const PROVENANCE_EXCUSE: &str =
    r"(?i)^(?:\s|//[/!]?)+(?:implementation|renderer|image|path|output|kernel)";
const PRIVATE_PATH: &str = concat!(
    r"\b(?:re",
    r"search|references|specifi",
    r"cations|extraction)/\S|\b(?:REA|AGENTS|BACKLOG|ARCHITECTURE)\.md\b|\.\./\.\./\.\./|\bspecifi",
    r"cations\b"
);
const CLEANUP: &str = r"\(`\)|\(full source\)|\( \)";

/// Listed terms no file may hold, kept as sha256 digests of their text so that this file does not spell them out.
/// Product names: exact case, one to three words joined by single spaces.
const LISTED_NAMES: [&str; 35] = [
    "08c9188c5d641e944ca3c674f027a1c0186d00f69d1079c8ba1417aa6bfa2d51",
    "1a9a658b5d023f73ec331216cf4069a4eeb9e4e85c1ccc6587e3c4ab5455cb64",
    "23cbcfc3e80a6fe5d4f319b08c0921d2b979f2db8ecb178dec4fbff034485b1c",
    "257c97330f448883b45b1053e0c4d956d12df58cbb2597314356a2ba3e70ac42",
    "29f87af4fb46de9584d7ff40f33d2d4713821c2031520cf08a7652d08c400c45",
    "2d05f5afece7167c0d91c06ab1a4abd7487a303e8f1901ded7bf5ef91f4dfa09",
    "355f5e74848522db142812706d3559c35daf9f66536507b4f5bf7ad3e1f68575",
    "38fdf519314e3151d7e7f6ef456f327b78ddb84bc457bdb0d49bce0b1fc3c959",
    "3a0e2e624aa20ff2159ebbff57a527546a99e427a2d3bad774c6313613253b8a",
    "421dac2ceec30e00569df2509acb0237b0a84e7ddf8e9b52d989f70b6fcbfafa",
    "527859c0ad13e24ca8c9b65edd36de55aaf1b66a5c4a1419865a5e77a9447184",
    "5ab413d93c4a9ab5e8e95974f09e475e48a6dcc8497d1d332afc139b98a9f1a1",
    "5f0330392292a13a29ae269a1efe6f7c2a8210572d884d8c2392a9105663a41b",
    "7a4721a879cb49bf793cbf62a831c2e70ae16da6a6a56fc83fe0dcdccaacee97",
    "7cdcbd87852f9d125c8d7f71559429c15d185d17c9df4eb3b80582d33383bb63",
    "815eca977c7d75f72bd23bccc91bc6f9931a42fbf03451eb0d7db8d2a704d94f",
    "8f5302cbdf238881c06b53a87ef86fee7e536559f174a9972bee01a461e46ec7",
    "a7b5855eb126881212efa2f6e52dc15003f34229f1e9ee3660f119f0f8cab27a",
    "aca3c1364979e0358fdb430f51ca6796d352c139c76ca40858f75fec58016e8f",
    "aed6b7aa2a0511a9bbcaae2a10127a3139fc6494c57769b31b1a694c14483ce7",
    "b05d344f153ca91b245cd82f943f071e51a4529103e0b5d0277f162e89e9d32e",
    "b4bbd684e4c8babe32714f13b91135586575b2947fef65b348ce354df1d69619",
    "b81bbeeb7ed2ce377642c0daa3e315f89f41f96779b080324f9d3a06ea7b9fb5",
    "ba40ff241db36ac39b47c1f9d338a9a48909deb1b4dc2af4eb7ab91522e9fb6a",
    "bf5521de93439c3b5613f4b87d60e9ff4596a660768a241b5685c1673f5a1dd3",
    "c8bdc6fa86fc7a67deaf0fc456e609ada8511f9f598ac893b4c2166356e0fecc",
    "cdd89742cf8b5d378b96dc7cfa363c6cc62334a9c7d52720dd7bad1f0bdfe77d",
    "cff8ef95912352a3146bef4b8f5dd2d81c16393b2dbede6fcbe2eba242d68efe",
    "e42df0232d77711b5578526bb9b952da23ba9a8c046a39309e68628b17058a06",
    "e68bcce7f0f46cd769630b1725c90a41b7c7e3de27012354d2f52401c82dfaa3",
    "ecf4bed079f79dd889594a919c224ca535d491edc7a4f019f842e270f7b2ed46",
    "f06783f44c0c4d216d786235877fa9c174e63a71e7a8c57d0b7c2ae733d8afff",
    "f09f6d1442e8399c655171906e8da403098d305484a085b70d35c62f93dc73ee",
    "fca7aa83d276c8bd69f53641339b55cfd8846130d20011a7eb8ec924cb59e655",
    "fdbdc46d32fef7e79d43fd88a71521d741b74170cce38e296a76825a1ad653bc",
];
/// A product name at the start of an identifier, in any case (lowercased before hashing), unless a lowercase letter
/// follows: it counts in `name_cursor` and `NameCursor`, not inside a longer lowercase word.
const LISTED_NAME_HEADS: [&str; 1] =
    ["3a7bd3e2360a3d29eea436fcfb7e44c735d117c42d1c1835420b6b9942dd4f1b"];
const LISTED_NAME_HEAD_LENGTHS: [usize; 1] = [5];
/// Provenance words, whole words in any case (lowercased before hashing).
const LISTED_WORDS: [&str; 5] = [
    "25eb6591c84eb015825dd839dc9d2916147ddc145f6c4346717b33ec015e0246",
    "4f361b43bd568fc2f1b9974cb3882464277e62865a733d8da86c5637051e7eb4",
    "59a2ea36c20b30a30f3aa251b346188d5afbeddfca2d496e67b0c2d9eb74a345",
    "74371f0025ee1a6211a4c8f277448020c81faa771ca0589a7819d8adc3f70b95",
    "c2179a0a39c15793be3827210219e3fc066df9ea8b89a62c8f686839ee4a507b",
];
/// Provenance word beginnings in any case (lowercased before hashing).
const LISTED_WORD_HEADS: [&str; 2] = [
    "2b3d4ca3d1124eceded88275ec7af1f75486c233e617c48f99297f0ee8f4e6fc",
    "c521018fc36a56f8a64df42af0df52a50a6e3c13e44edc570c3f662f1db3ee35",
];
const LISTED_WORD_HEAD_LENGTHS: [usize; 2] = [8, 10];
/// Analysis phrases: two words in any case joined by a space or a hyphen (lowercased, joined by a space, before
/// hashing).
const LISTED_PHRASES: [&str; 11] = [
    "042ed30a47d9c5cbf634c046ee2e3f4ca0d6332e203ff2566f3f7ccd6568a48d",
    "07207cfadff51b649932bf438c4467e3b6ef6600376dd23bce52d60a16ce2341",
    "290c916689affd6a373c8e16f4b31f781e68a3c20e9420f2a367c64c4a53b51e",
    "2fcd0d1d332dbe5fc8af394e51cb57ccc9a16c50a9672eb1b5e64e97dde80665",
    "7221ba7689f6a52fdcef47830b86f6c78980d34fe8b93f47a573bb3c1be812c7",
    "7d2c5e2ba439501247242285b0039b1c9fa3e141276a68e9aaa1e508d1915ead",
    "9aed1062091007bb3f090bddacc9482d4ee80c0cd3cd6a500f933ec4046e5e8e",
    "ceaecc684b4f92d86881a760a286f5d9ff1a4f231bf645ec27df95bc37bccbd2",
    "d9cbe6ab94ebd22cc69b0d86bc72f90a525253d13b81a69866e7532a9b1fd074",
    "df0cf82b19f44c150203827d039c912c37806c28fefc2e42f3cabf2252b0209e",
    "e84ea07273cd05ac5961e788f723b6a7b9c80a58c5ac6e7d859269a6b7ca6ebc",
];
/// File suffixes (lowercase, with the dot) and first four bytes of files that may not be committed.
const LISTED_SUFFIXES: [&str; 8] = [
    "52d170b579a69cd6c7a742ed7d9c71cbda30eaef1d8dfcb97324e0d67688be1d",
    "62ccfe9faa79e5aaa913d52b93fbae306ad73f91c5f6a3de63365fba5f943d3d",
    "67b38016c0e503110fd437753c683dd74fdaedff2ebcb464d9be6a1fa64d081f",
    "6e7a0da7ec8955b9e470c7121019538d40050af8624c346c8a59a0cea86bad29",
    "7ac82063e983328f696d356278a5553585e08127368f395bc741eea7f36876bd",
    "94644341556428956155e7f5073cf8d95003c54b03fbfe0fdf6cbdde34fd3ea8",
    "9b7c5ca4f780888a81a4933325c025f1b8363c5fbac1db484c96e60be6d69b56",
    "c6192a7fc9fd706744f38e2ab21e37cad9bbe83f77bf0d55872878ba70c41ed1",
];
const LISTED_MAGIC: [&str; 5] = [
    "1c67b14f68145a0e93cb09fdf8c79365f0e33b28ce0c1a58f19b6ac99659a399",
    "3684d4581255ca55e94c2cb89affc7d0dc914b7462c1f496780d7f2214877709",
    "65ab12a8ff3263fbc257e5ddf0aa563c64573d0bab1f1115b9b107834cfa6971",
    "67a524103ffac2fcb99209bc8e3b7c3ee4ad27980975e322967bfc01eb2e6d1f",
    "d2238b6d7c34ad608a5f6caddf95928d741307fea41f0336302e390109311d72",
];

/// Rules a file is excused from, by path.
const GATE_EXEMPT: [(&str, &str); 8] = [
    ("crates/cupertino-ui-files/src/mime.rs", "evidence class"),
    ("crates/cupertino-ui-files/tests/mime.rs", "evidence class"),
    ("crates/cupertino-theme/src/evidence.rs", "evidence class"),
    // The vendored smithay release, unchanged: an EGL error code and comments about Rust references.
    (
        "third-party/smithay/src/backend/egl/ffi.rs",
        "evidence class",
    ),
    (
        "third-party/smithay/src/backend/renderer/element/utils/elements.rs",
        "provenance wording",
    ),
    (
        "third-party/smithay/src/desktop/space/utils.rs",
        "provenance wording",
    ),
    (
        "third-party/smithay/src/input/keyboard/mod.rs",
        "provenance wording",
    ),
    (
        "third-party/smithay/src/wayland/shm/mod.rs",
        "provenance wording",
    ),
];

/// Third-party copyright holders whose names trip a rule; the literal is blanked before matching, the rest of the
/// file is checked.
const GATE_HOLDERS: [(&str, &str); 1] =
    // The name is split so that this file passes the gate itself.
    [(
        "packaging/licenses/THIRD-PARTY-NOTICES",
        concat!("Status Re", "search & Development GmbH"),
    )];

const THIRD_PARTY_EXEMPT: [&str; 2] = ["private document path", "cleanup remnant"];

/// Directories whose Rust sources may hold no test code: tests live in a crate's `tests/` and use its public
/// interface.
const TEST_FREE: [&str; 4] = [
    "crates/*/src/**",
    "crates/*/examples/**",
    "crates/*/benches/**",
    "xtask/src/**",
];
/// Test code markers.
const TEST_MARKERS: &str = concat!(r"#\[te", r"st\]|#\[cfg\(te", r"st\)\]|\bmod\s+te", r"sts\b");

/// Maximum lines permitted in a tracked Rust source file under `crates/*/src/**` or `xtask/src/**`.
/// Enforced by the gate so source files stay modular and readable.
const MAX_FILE_LINES: usize = 1500;
/// Directories subject to [`MAX_FILE_LINES`].
const SIZE_LIMITED: [&str; 2] = ["crates/*/src/**", "xtask/src/**"];
/// Legacy Wave 6–7 (`shell` and `apps`) files still awaiting their wave's refactor, capped at their current line count
/// so they may never grow; entries must be removed once split under [`MAX_FILE_LINES`].
const LARGE_FILE_CAPS: [(&str, usize); 13] = [
    ("crates/cupertino-controlcenter/src/view.rs", 3387),
    ("crates/cupertino-dock/src/dock.rs", 1536),
    ("crates/cupertino-dock/src/kit.rs", 3346),
    ("crates/cupertino-files/src/kit/mod.rs", 3377),
    ("crates/cupertino-launcher/src/kit.rs", 2636),
    ("crates/cupertino-launcher/src/view.rs", 3007),
    ("crates/cupertino-login/src/view.rs", 2293),
    ("crates/cupertino-login/src/view/widgets.rs", 2833),
    ("crates/cupertino-notifications/src/center.rs", 1511),
    ("crates/cupertino-notifications/src/view.rs", 3641),
    ("crates/cupertino-portal/src/dialog.rs", 1999),
    ("crates/cupertino-settings/src/panes.rs", 3251),
    ("crates/cupertino-settings/src/shell/state.rs", 1738),
];

/// Script suffixes no tracked file may have.
const SCRIPT_SUFFIXES: [&str; 5] = [".sh", ".py", ".bash", ".zsh", ".fish"];
/// Files in a format of their own that happens to be a script.
const SCRIPT_ALLOWED: [&str; 1] = ["packaging/iso/profiledef.sh"];

/// The top-level directories that were removed; nothing may point into them.
const REMOVED_DIRECTORIES: [&str; 2] = ["scripts", "ci"];
/// Files that may still name the removed directories: the history of the project.
const REMOVED_DIRECTORIES_ALLOWED: [&str; 1] = ["CHANGELOG.md"];

/// Crates whose lockfile version is held exactly: name, the one version allowed (and why, beside it).
const PINNED_LOCK_VERSIONS: [(&str, &str); 1] = [
    // 1.7.0 keeps an idle pool thread for good, waking it twice a second in every process with a bus connection.
    ("blocking", "1.6.2"),
];

fn regex(pattern: &str) -> Regex {
    Regex::new(pattern).expect("gate patterns are valid")
}

static GATE: LazyLock<Vec<(&'static str, Regex)>> = LazyLock::new(|| {
    vec![
        ("evidence ID", regex(ID)),
        ("evidence class", regex(CLASS)),
        ("absolute address", regex(CACHE_ADDR)),
        ("provenance wording", regex(PROVENANCE)),
        ("private document path", regex(PRIVATE_PATH)),
        ("cleanup remnant", regex(CLEANUP)),
    ]
});
static EXCUSE: LazyLock<Regex> = LazyLock::new(|| regex(PROVENANCE_EXCUSE));
static WORD: LazyLock<Regex> = LazyLock::new(|| regex(r"[^\W_]+"));
static TESTS: LazyLock<Regex> = LazyLock::new(|| regex(TEST_MARKERS));
static SCANNER_XML: LazyLock<Regex> = LazyLock::new(|| {
    regex(r#"\bgenerate_(?:interfaces|client_code|server_code)!\s*\(\s*"([^"]+)""#)
});
static INCLUDED: LazyLock<Regex> = LazyLock::new(|| regex(r#"\binclude_bytes!\s*\(\s*"([^"]+)""#));

pub(crate) fn digest(bytes: &[u8]) -> String {
    let hash = Sha256::digest(bytes);
    let mut text = String::with_capacity(64);
    for byte in hash {
        let _ = write!(text, "{byte:02x}");
    }
    text
}

thread_local! {
    /// Digests of the words seen so far: the same words come back in every file.
    static TERM_DIGESTS: std::cell::RefCell<std::collections::HashMap<String, String>> = Default::default();
}

/// Whether the digest of `term` is in `set`.
fn listed_in(set: &HashSet<&'static str>, term: &str) -> bool {
    TERM_DIGESTS.with_borrow_mut(|cache| {
        if let Some(found) = cache.get(term) {
            return set.contains(found.as_str());
        }
        let found = digest(term.as_bytes());
        let listed = set.contains(found.as_str());
        cache.insert(term.to_owned(), found);
        listed
    })
}

struct Listed {
    names: HashSet<&'static str>,
    name_heads: HashSet<&'static str>,
    words: HashSet<&'static str>,
    word_heads: HashSet<&'static str>,
    phrases: HashSet<&'static str>,
}

static LISTED: LazyLock<Listed> = LazyLock::new(|| Listed {
    names: LISTED_NAMES.into_iter().collect(),
    name_heads: LISTED_NAME_HEADS.into_iter().collect(),
    words: LISTED_WORDS.into_iter().collect(),
    word_heads: LISTED_WORD_HEADS.into_iter().collect(),
    phrases: LISTED_PHRASES.into_iter().collect(),
});

/// The first `length` characters of `text`, if it has that many.
fn head(text: &str, length: usize) -> Option<&str> {
    match text.char_indices().nth(length) {
        Some((end, _)) => Some(&text[..end]),
        None if text.chars().count() == length => Some(text),
        None => None,
    }
}

/// (rule name, byte offset, matched text) of every listed term in `text`.
fn listed_matches(text: &str) -> Vec<(&'static str, usize, String)> {
    let listed = &*LISTED;
    let mut found = Vec::new();
    let words: Vec<regex::Match> = WORD.find_iter(text).collect();
    for (index, word) in words.iter().enumerate() {
        let lower = word.as_str().to_lowercase();
        if listed_in(&listed.words, &lower)
            || LISTED_WORD_HEAD_LENGTHS.iter().any(|&length| {
                head(&lower, length).is_some_and(|head| listed_in(&listed.word_heads, head))
            })
        {
            found.push(("provenance wording", word.start(), word.as_str().to_owned()));
        }
        if let Some(next) = words.get(index + 1) {
            let between = &text[word.end()..next.start()];
            if between == " " || between == "-" {
                let phrase = format!("{lower} {}", next.as_str().to_lowercase());
                if listed_in(&listed.phrases, &phrase) {
                    found.push((
                        "analysis wording",
                        word.start(),
                        text[word.start()..next.end()].to_owned(),
                    ));
                }
            }
        }
        let mut end = index;
        while end < words.len() && end - index < 3 {
            if end > index && &text[words[end - 1].end()..words[end].start()] != " " {
                break;
            }
            let phrase = &text[word.start()..words[end].end()];
            if listed_in(&listed.names, phrase) {
                found.push(("vendor or product name", word.start(), phrase.to_owned()));
                break;
            }
            end += 1;
        }
    }
    // A run of ASCII letters of the listed length, after no ASCII letter or digit and before no lowercase letter;
    // matches do not overlap.
    let bytes = text.as_bytes();
    for length in LISTED_NAME_HEAD_LENGTHS {
        let mut start = 0;
        while start + length <= bytes.len() {
            let before_ok = start == 0 || !bytes[start - 1].is_ascii_alphanumeric();
            let letters = bytes[start..start + length]
                .iter()
                .all(u8::is_ascii_alphabetic);
            let after_ok = bytes
                .get(start + length)
                .is_none_or(|byte| !byte.is_ascii_lowercase());
            if before_ok && letters && after_ok {
                let matched = &text[start..start + length];
                if listed_in(&listed.name_heads, &matched.to_lowercase()) {
                    found.push(("vendor or product name", start, matched.to_owned()));
                }
                start += length;
            } else {
                start += 1;
            }
        }
    }
    found
}

/// The rule's matches of `pattern`, with the provenance pattern's excused endings taken out.
fn rule_matches<'t>(name: &str, pattern: &Regex, text: &'t str) -> Vec<regex::Match<'t>> {
    let mut found = Vec::new();
    let mut at = 0;
    while let Some(found_match) = pattern.find_at(text, at) {
        let excused = name == "provenance wording"
            && found_match.as_str().to_lowercase().starts_with("the ")
            && EXCUSE.is_match(&text[found_match.end()..]);
        // After an excused match the search goes on from its next character, as a lookahead that fails would.
        let next_character = found_match.start()
            + text[found_match.start()..]
                .chars()
                .next()
                .map_or(1, char::len_utf8);
        if excused || found_match.is_empty() {
            at = next_character;
        } else {
            found.push(found_match);
            at = found_match.end();
        }
        if at > text.len() {
            break;
        }
    }
    found
}

/// A quoted rendering of `text`, as the problems show matches.
fn quoted(text: &str) -> String {
    let quote = if text.contains('\'') && !text.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut result = String::new();
    result.push(quote);
    for character in text.chars() {
        match character {
            '\\' => result.push_str("\\\\"),
            '\n' => result.push_str("\\n"),
            '\t' => result.push_str("\\t"),
            '\r' => result.push_str("\\r"),
            c if c == quote => {
                result.push('\\');
                result.push(c);
            }
            c => result.push(c),
        }
    }
    result.push(quote);
    result
}

fn line_of(text: &str, offset: usize) -> usize {
    text.as_bytes()[..offset]
        .iter()
        .filter(|&&byte| byte == b'\n')
        .count()
        + 1
}

/// The suffix of the file name, lowercased, with its dot; empty for none.
fn suffix(rel: &str) -> String {
    let name = rel.rsplit('/').next().unwrap_or(rel);
    match name.rfind('.') {
        Some(dot) if dot > 0 && dot + 1 < name.len() => name[dot..].to_lowercase(),
        _ => String::new(),
    }
}

/// The paths git would track under `root`: tracked plus untracked-not-ignored.
pub(crate) fn visible_paths(root: &Path) -> Result<BTreeSet<String>, String> {
    let text = crate::plan::output(
        root,
        "git",
        &[
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
        ],
    )?;
    Ok(text
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(str::to_owned)
        .collect())
}

/// Lexically normalized components of `path`: `.` dropped, `..` taking out the component before it when there is one.
fn normalize(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(result.components().next_back(), Some(Component::Normal(_))) {
                    result.pop();
                } else {
                    result.push("..");
                }
            }
            other => result.push(other.as_os_str()),
        }
    }
    result
}

/// `path` relative to `base`, both lexically normalized and absolute.
fn relative(path: &Path, base: &Path) -> String {
    let path: Vec<Component> = path.components().collect();
    let base: Vec<Component> = base.components().collect();
    let common = path.iter().zip(&base).take_while(|(a, b)| a == b).count();
    let mut result = PathBuf::new();
    for _ in common..base.len() {
        result.push("..");
    }
    for component in &path[common..] {
        result.push(component.as_os_str());
    }
    result.to_string_lossy().into_owned()
}

/// Whether the removed directory's name at `offset` in `text` starts a path from the repository's top level: it is
/// not a later segment of another path or the end of a longer name.
fn names_removed_directory(text: &str, offset: usize) -> bool {
    let bytes = text.as_bytes();
    let mut start = offset;
    // Leading `./` and `../` steps still point at the repository's top level.
    loop {
        if start >= 2 && &bytes[start - 2..start] == b"./" {
            if start >= 3 && bytes[start - 3] == b'.' {
                start -= 3;
            } else {
                start -= 2;
            }
        } else {
            break;
        }
    }
    start == 0
        || !matches!(bytes[start - 1], b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'-' | b'.' | b'/')
}

/// The gate's findings on the tree at `root`.
pub struct Outcome {
    pub hits: Vec<(String, usize)>,
    pub problems: Vec<String>,
}

fn count(hits: &mut Vec<(String, usize)>, name: &str) {
    match hits.iter_mut().find(|(rule, _)| rule == name) {
        Some((_, number)) => *number += 1,
        None => hits.push((name.to_owned(), 1)),
    }
}

fn source_problems(
    root: &Path,
    hits: &mut Vec<(String, usize)>,
    problems: &mut Vec<String>,
) -> Result<(), String> {
    let visible = visible_paths(root)?;
    let canonical_root = root
        .canonicalize()
        .map_err(|error| format!("{}: {error}", root.display()))?;
    let allowed: BTreeMap<&str, &str> = ALLOWED_BINARIES.into_iter().collect();
    let removed: Vec<Regex> = REMOVED_DIRECTORIES
        .iter()
        .map(|directory| regex(&format!(r"\b{}/", regex::escape(directory))))
        .collect();
    for rel in &visible {
        let path = root.join(rel);
        let Ok(metadata) = path.symlink_metadata() else {
            continue;
        };
        if metadata.file_type().is_symlink() {
            let link = std::fs::read_link(&path).map_err(|error| format!("{rel}: {error}"))?;
            match path.canonicalize() {
                Ok(target) if !link.is_absolute() && target.starts_with(&canonical_root) => {}
                Ok(_) => problems.push(format!("source symlink escapes tree: {rel}")),
                Err(_) => problems.push(format!("source symlink is dangling: {rel}")),
            }
            continue;
        }
        let file_suffix = suffix(rel);
        if (FORBIDDEN_SUFFIXES.contains(&file_suffix.as_str())
            || LISTED_SUFFIXES.contains(&digest(file_suffix.as_bytes()).as_str()))
            && !allowed.contains_key(rel.as_str())
        {
            problems.push(format!("forbidden file type: {rel}"));
            continue;
        }
        if !metadata.is_file() {
            continue;
        }
        let data = std::fs::read(&path).map_err(|error| format!("{rel}: {error}"))?;
        if let Some(expected) = allowed.get(rel.as_str()) {
            if digest(&data) != *expected {
                problems.push(format!("allowed binary changed: {rel}"));
            }
            continue;
        }
        if LISTED_MAGIC.contains(&digest(&data[..data.len().min(4)]).as_str()) {
            problems.push(format!("forbidden file type: {rel}"));
            continue;
        }
        let third_party = rel.starts_with("third-party/");
        let file_name = rel.rsplit('/').next().unwrap_or(rel);
        let allowed_script = third_party
            || SCRIPT_ALLOWED.contains(&rel.as_str())
            || file_name == "PKGBUILD"
            || file_name.ends_with(".install")
            || file_name.ends_with(".hook");
        let shebang = data.starts_with(b"#!") && !data.starts_with(b"#![");
        if !allowed_script && (SCRIPT_SUFFIXES.contains(&file_suffix.as_str()) || shebang) {
            count(hits, "script");
            problems.push(format!(
                "{rel}: script file: the repository's tasks are written in Rust, in xtask"
            ));
        }
        let Ok(text) = std::str::from_utf8(&data) else {
            // Matches the source policy; this is not a general binary allowlist.
            continue;
        };
        let mut text = text.to_owned();
        for (holder_path, holder) in GATE_HOLDERS {
            if rel == holder_path {
                text = text.replace(holder, &" ".repeat(holder.len()));
            }
        }
        let mut exempt: BTreeSet<&str> = GATE_EXEMPT
            .iter()
            .filter(|(path, _)| *path == rel)
            .map(|(_, rule)| *rule)
            .collect();
        if third_party {
            exempt.extend(THIRD_PARTY_EXEMPT);
        }
        for (name, pattern) in GATE.iter() {
            if exempt.contains(name) {
                continue;
            }
            for found in rule_matches(name, pattern, &text) {
                count(hits, name);
                problems.push(format!(
                    "{rel}:{}: {name}: {}",
                    line_of(&text, found.start()),
                    quoted(found.as_str())
                ));
            }
        }
        for (name, start, matched) in listed_matches(&text) {
            if exempt.contains(name) {
                continue;
            }
            count(hits, name);
            problems.push(format!(
                "{rel}:{}: {name}: {}",
                line_of(&text, start),
                quoted(&matched)
            ));
        }
        if file_suffix == ".rs"
            && SIZE_LIMITED
                .iter()
                .any(|pattern| crate::glob::matches(pattern, rel))
        {
            let lines = text.lines().count();
            match LARGE_FILE_CAPS.iter().find(|(path, _)| *path == rel) {
                Some((_, cap)) if lines > *cap => {
                    count(hits, "file too large");
                    problems.push(format!(
                        "{rel}: file has {lines} lines, exceeding its grandfathered cap of {cap} (target limit {MAX_FILE_LINES})"
                    ));
                }
                Some(_) if lines <= MAX_FILE_LINES => {
                    count(hits, "stale file-size cap");
                    problems.push(format!(
                        "{rel}: file is now {lines} lines (<= {MAX_FILE_LINES}); remove its entry from LARGE_FILE_CAPS"
                    ));
                }
                Some(_) => {}
                None if lines > MAX_FILE_LINES => {
                    count(hits, "file too large");
                    problems.push(format!(
                        "{rel}: file has {lines} lines, exceeding the {MAX_FILE_LINES}-line limit; split into focused submodules"
                    ));
                }
                None => {}
            }
        }
        if file_suffix == ".rs"
            && TEST_FREE
                .iter()
                .any(|pattern| crate::glob::matches(pattern, rel))
        {
            for found in TESTS.find_iter(&text) {
                count(hits, "test code");
                problems.push(format!(
                    "{rel}:{}: test code outside a crate's tests/: {}",
                    line_of(&text, found.start()),
                    quoted(found.as_str())
                ));
            }
        }
        if !third_party && !REMOVED_DIRECTORIES_ALLOWED.contains(&rel.as_str()) {
            for pattern in &removed {
                for found in pattern.find_iter(&text) {
                    if names_removed_directory(&text, found.start()) {
                        count(hits, "removed path");
                        problems.push(format!(
                            "{rel}:{}: removed path: {}",
                            line_of(&text, found.start()),
                            quoted(found.as_str())
                        ));
                    }
                }
            }
        }
    }
    Ok(())
}

/// Every `.rs` file under `directory`, sorted.
pub(crate) fn rust_files(directory: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    let mut entries: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            rust_files(&path, found);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            found.push(path);
        }
    }
}

/// Library sources whose wayland-scanner XML is not also `include_bytes!`-ed in the crate: the macros read it
/// without telling rustc, so a build cache keyed on rustc's dep-info would keep stale bindings after an edit.
fn protocol_xml_problems(root: &Path) -> Vec<String> {
    let mut problems = Vec::new();
    let Ok(entries) = std::fs::read_dir(root.join("crates")) else {
        return problems;
    };
    let mut crates: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.join("Cargo.toml").is_file())
        .collect();
    crates.sort();
    for krate in crates {
        let src = krate.join("src");
        let mut sources = Vec::new();
        rust_files(&src, &mut sources);
        sources.retain(|path| *path != src.join("main.rs") && !path.starts_with(src.join("bin")));
        let mut named = Vec::new();
        let mut included = BTreeSet::new();
        for path in &sources {
            let Ok(text) = std::fs::read_to_string(path) else {
                continue;
            };
            for found in SCANNER_XML.captures_iter(&text) {
                let whole = found.get(0).expect("a match");
                let xml = normalize(Path::new(&found[1]))
                    .to_string_lossy()
                    .into_owned();
                named.push((path.clone(), line_of(&text, whole.start()), xml));
            }
            for found in INCLUDED.captures_iter(&text) {
                let parent = path.parent().unwrap_or(&src);
                included.insert(relative(
                    &normalize(&parent.join(&found[1])),
                    &normalize(&krate),
                ));
            }
        }
        for (path, line, xml) in named {
            if !included.contains(&xml) {
                let rel = path
                    .strip_prefix(root)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .into_owned();
                problems.push(format!(
                    "{rel}:{line}: protocol XML read by wayland-scanner is not include_bytes!-ed in its crate: {xml}"
                ));
            }
        }
    }
    problems
}

/// `Cargo.lock` entries of a pinned crate at any version other than the pinned one.
fn lockfile_problems(root: &Path) -> Vec<String> {
    let mut problems = Vec::new();
    let Ok(text) = std::fs::read_to_string(root.join("Cargo.lock")) else {
        return problems;
    };
    let mut name: Option<&str> = None;
    for (index, line) in text.lines().enumerate() {
        if line == "[[package]]" {
            name = None;
        } else if let Some(value) = line.strip_prefix("name = ") {
            name = Some(value.trim_matches('"'));
        } else if let Some(value) = line.strip_prefix("version = ") {
            let version = value.trim_matches('"');
            if let Some((crate_name, pinned)) = PINNED_LOCK_VERSIONS
                .iter()
                .find(|(pinned, _)| Some(*pinned) == name)
            {
                if version != *pinned {
                    problems.push(format!(
                        "Cargo.lock:{}: {crate_name} {version} is locked; only {pinned} is allowed",
                        index + 1
                    ));
                }
            }
        }
    }
    problems
}

/// Checks the tree at `root`.
pub fn run(root: &Path) -> Result<Outcome, String> {
    let mut hits = Vec::new();
    let mut problems = Vec::new();
    source_problems(root, &mut hits, &mut problems)?;
    problems.extend(protocol_xml_problems(root));
    problems.extend(lockfile_problems(root));
    problems.extend(crate::licences::problems(root));
    Ok(Outcome { hits, problems })
}

/// The summary line, with the number of hits of each rule.
pub fn summary(outcome: &Outcome) -> String {
    let hits: Vec<String> = outcome
        .hits
        .iter()
        .map(|(rule, number)| format!("{}: {number}", quoted(rule)))
        .collect();
    format!(
        "gate: {} problems ({{{}}})",
        outcome.problems.len(),
        hits.join(", ")
    )
}
