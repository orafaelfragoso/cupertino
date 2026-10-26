//! `keyboard.conf` (`$CUPERTINO_KEYBOARD_CONFIG` overrides the path): key repeat, the modifier profile, input sources,
//! caps lock, the globe key, decoded layouts, the keyboard accessibility filters, the client chord rewrite and the
//! accent picker. The server reads it and applies it on top of the keyboard model's defaults (`cupertino-keyboard`,
//! where the live); a key the file does not set is `None` here.
//!
//! | Key | Value |
//! |---|---|
//! | `repeat_delay`, `repeat_interval`, `caps_lock_delay`, `mouse_keys_delay` | milliseconds |
//! | `repeat_delay_saved` | milliseconds: the delay the settings application restores when key repeat is turned on again (turning it off writes `repeat_delay = 0`); Original, the server does not read it |
//! | `caps_lock_hold` | milliseconds, 1 to [`CAPS_LOCK_HOLD_MAX_MS`] |
//! | `slow_keys_delay` | milliseconds, at least 1 |
//! | `mouse_keys_max_speed` | points per tick, at least 1 |
//! | `modifier_profile` | `cupertino`, `classic-pc` |
//! | `input_sources` | comma-separated xkb layouts with an optional `(variant)`, or `decoded:<path> |
//! | `fn_key` | `do-nothing`, `change-input-source`, `emoji`, `dictation` |
//! | `layout_keyboard` | `ansi`, `iso`, `jis` |
//! | `layout_level3` | `both-alt`, `right-alt`, `none` |
//! | `caps_lock_switch`, `slow_keys`, `sticky_keys`, `sticky_keys_shortcut`, `mouse_keys`, `mouse_keys_shortcut`, `client_rewrite`, `accent_picker` | `on`, `off` |
//! | `rewrite_terminals`, `rewrite_off`, `accent_keyboards`, `accent_disabled_apps` | comma-separated names |
//! | `accent_table` | a path (`~/ is the home folder) |

use std::io;
use std::path::{Path, PathBuf};

use crate::conf::{self, parse_list, parse_switch, switch_text, Key, Problem};

/// Environment variable that redirects the settings file.
pub const KEYBOARD_CONF_ENV: &str = "CUPERTINO_KEYBOARD_CONFIG";

/// Longest caps-lock press that the setting accepts as the hold duration (accepted 1 to 1500 ms).
pub const CAPS_LOCK_HOLD_MAX_MS: u32 = 1500;

/// `~/` at the start of `path` replaced by `$HOME`.
pub fn expand_home(path: &str) -> PathBuf {
    match (path.strip_prefix("~/"), std::env::var_os("HOME")) {
        (Some(rest), Some(h)) => PathBuf::from(h).join(rest),
        _ => PathBuf::from(path),
    }
}

/// An enum of file values with their spellings.
macro_rules! file_enum {
    ($(#[$m:meta])* $name:ident { $($(#[$vm:meta])* $variant:ident = $text:literal),+ $(,)? }) => {
        $(#[$m])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum $name {
            $($(#[$vm])* $variant),+
        }

        impl $name {
            pub const ALL: &'static [$name] = &[$($name::$variant),+];

            /// How the file writes the value.
            pub fn name(self) -> &'static str {
                match self {
                    $($name::$variant => $text),+
                }
            }

            pub fn from_name(text: &str) -> Result<Self, String> {
                Self::ALL.iter().copied().find(|v| v.name() == text).ok_or_else(|| {
                    let names: Vec<&str> = Self::ALL.iter().map(|v| v.name()).collect();
                    format!("expected {}, got {text:?}", names.join(", "))
                })
            }
        }
    };
}

file_enum!(
    /// The modifier profile (`cupertino_keyboard::ModifierProfile::by_name`).
    ModifierProfile { Cupertino = "cupertino", ClassicPc = "classic-pc" }
);
file_enum!(
    /// What a tap on the globe / fn key does.
    FnKey { DoNothing = "do-nothing", ChangeInputSource = "change-input-source", Emoji = "emoji", Dictation = "dictation" }
);
file_enum!(
    /// Physical key arrangement of decoded layouts.
    LayoutKeyboard { Ansi = "ansi", Iso = "iso", Jis = "jis" }
);
file_enum!(
    /// Keys that select the alternate symbol tables of decoded layouts.
    Level3 { BothAlt = "both-alt", RightAlt = "right-alt", None = "none" }
);

/// One entry of the input-source list: an xkb layout with an optional variant, or a decoded layout file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceSpec {
    /// The text of the entry, also the display and switcher identifier: `us`, `fr(nodeadkeys)`, `decoded:~/layouts/ABC.json`.
    pub id: String,
    /// The xkb layout and variant (empty for a decoded layout).
    pub layout: String,
    pub variant: String,
    /// A decoded layout (`decoded:<path>`): its keymap is generated, not taken from the system XKB data.
    pub decoded: Option<PathBuf>,
}

impl SourceSpec {
    pub fn parse(text: &str) -> Option<SourceSpec> {
        let t = text.trim();
        if t.is_empty() {
            return None;
        }
        if let Some(path) = t.strip_prefix("decoded:") {
            let path = path.trim();
            return (!path.is_empty()).then(|| SourceSpec {
                id: t.to_string(),
                layout: String::new(),
                variant: String::new(),
                decoded: Some(expand_home(path)),
            });
        }
        let (layout, variant) = match t.split_once('(') {
            Some((l, v)) => (l.trim(), v.trim().strip_suffix(')')?.trim()),
            None => (t, ""),
        };
        if layout.is_empty()
            || !layout
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return None;
        }
        Some(SourceSpec {
            id: t.to_string(),
            layout: layout.to_string(),
            variant: variant.to_string(),
            decoded: None,
        })
    }
}

/// The entries of an `input_sources` value: commas inside a variant's parentheses do not split.
pub fn split_sources(value: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0;
    let mut cur = String::new();
    for c in value.chars() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => {
                out.push(std::mem::take(&mut cur));
                continue;
            }
            _ => {}
        }
        cur.push(c);
    }
    out.push(cur);
    out.into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Everything in `keyboard.conf`; `None` is a key the file does not set.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct KeyboardSettings {
    pub repeat_delay_ms: Option<u32>,
    pub repeat_interval_ms: Option<u32>,
    /// The delay before repeat was turned off, for the settings application only.
    pub repeat_delay_saved_ms: Option<u32>,
    pub modifier_profile: Option<ModifierProfile>,
    pub input_sources: Option<Vec<SourceSpec>>,
    pub caps_lock_delay_ms: Option<u32>,
    pub caps_lock_switch: Option<bool>,
    pub caps_lock_hold_ms: Option<u32>,
    pub fn_key: Option<FnKey>,
    pub layout_keyboard: Option<LayoutKeyboard>,
    pub layout_level3: Option<Level3>,
    pub slow_keys: Option<bool>,
    pub slow_keys_delay_ms: Option<u32>,
    pub sticky_keys: Option<bool>,
    pub sticky_keys_shortcut: Option<bool>,
    pub mouse_keys: Option<bool>,
    pub mouse_keys_shortcut: Option<bool>,
    pub mouse_keys_delay_ms: Option<u32>,
    pub mouse_keys_max_speed: Option<u32>,
    pub client_rewrite: Option<bool>,
    pub rewrite_terminals: Option<Vec<String>>,
    pub rewrite_off: Option<Vec<String>>,
    pub accent_picker: Option<bool>,
    /// As written; [`expand_home`] gives the path.
    pub accent_table: Option<String>,
    pub accent_keyboards: Option<Vec<String>>,
    pub accent_disabled_apps: Option<Vec<String>>,
}

fn ms(value: &str) -> Result<u32, String> {
    value
        .parse::<u32>()
        .map_err(|_| format!("expected milliseconds, got {value:?}"))
}

fn at_least_one(value: &str, what: &str) -> Result<u32, String> {
    value
        .parse::<u32>()
        .ok()
        .filter(|v| *v > 0)
        .ok_or_else(|| format!("expected {what} of at least 1, got {value:?}"))
}

fn sources(value: &str) -> Result<Vec<SourceSpec>, String> {
    split_sources(value)
        .iter()
        .map(|part| SourceSpec::parse(part).ok_or_else(|| format!("bad layout {part:?}")))
        .collect()
}

fn sources_text(list: &[SourceSpec]) -> String {
    list.iter()
        .map(|s| s.id.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

fn list_text(list: &[String]) -> String {
    list.join(", ")
}

/// A key whose value is the `Option` field `$field`: `$parse` reads the value, `$text` writes it.
macro_rules! field_key {
    ($name:literal, $field:ident, $parse:expr, $text:expr) => {
        Key {
            name: $name,
            read: |s, v| ($parse)(v).map(|x| s.$field = Some(x)),
            write: |s| s.$field.as_ref().map($text),
            keep: |s, g| s.$field = g.$field.clone(),
        }
    };
}

fn switch_ref(on: &bool) -> String {
    switch_text(*on)
}

fn number_ref(n: &u32) -> String {
    n.to_string()
}

const KEYS: &[Key<KeyboardSettings>] = &[
    field_key!("repeat_delay", repeat_delay_ms, ms, number_ref),
    field_key!("repeat_interval", repeat_interval_ms, ms, number_ref),
    field_key!("repeat_delay_saved", repeat_delay_saved_ms, ms, number_ref),
    field_key!(
        "modifier_profile",
        modifier_profile,
        ModifierProfile::from_name,
        |p: &ModifierProfile| p.name().to_string()
    ),
    field_key!("input_sources", input_sources, sources, |l: &Vec<
        SourceSpec,
    >| sources_text(
        l
    )),
    field_key!("caps_lock_delay", caps_lock_delay_ms, ms, number_ref),
    field_key!(
        "caps_lock_switch",
        caps_lock_switch,
        parse_switch,
        switch_ref
    ),
    field_key!(
        "caps_lock_hold",
        caps_lock_hold_ms,
        |v: &str| ms(v).and_then(|n| if (1..=CAPS_LOCK_HOLD_MAX_MS).contains(&n) {
            Ok(n)
        } else {
            Err(format!("expected 1 to {CAPS_LOCK_HOLD_MAX_MS} ms, got {n}"))
        }),
        number_ref
    ),
    field_key!("fn_key", fn_key, FnKey::from_name, |k: &FnKey| k
        .name()
        .to_string()),
    field_key!(
        "layout_keyboard",
        layout_keyboard,
        LayoutKeyboard::from_name,
        |k: &LayoutKeyboard| k.name().to_string()
    ),
    field_key!(
        "layout_level3",
        layout_level3,
        Level3::from_name,
        |k: &Level3| k.name().to_string()
    ),
    field_key!("slow_keys", slow_keys, parse_switch, switch_ref),
    field_key!(
        "slow_keys_delay",
        slow_keys_delay_ms,
        |v: &str| at_least_one(v, "a delay (switch the filter off with slow_keys = off)"),
        number_ref
    ),
    field_key!("sticky_keys", sticky_keys, parse_switch, switch_ref),
    field_key!(
        "sticky_keys_shortcut",
        sticky_keys_shortcut,
        parse_switch,
        switch_ref
    ),
    field_key!("mouse_keys", mouse_keys, parse_switch, switch_ref),
    field_key!(
        "mouse_keys_shortcut",
        mouse_keys_shortcut,
        parse_switch,
        switch_ref
    ),
    field_key!("mouse_keys_delay", mouse_keys_delay_ms, ms, number_ref),
    field_key!(
        "mouse_keys_max_speed",
        mouse_keys_max_speed,
        |v: &str| at_least_one(v, "a speed"),
        number_ref
    ),
    field_key!("client_rewrite", client_rewrite, parse_switch, switch_ref),
    field_key!(
        "rewrite_terminals",
        rewrite_terminals,
        |v: &str| Ok::<_, String>(parse_list(v)),
        |l: &Vec<String>| list_text(l)
    ),
    field_key!(
        "rewrite_off",
        rewrite_off,
        |v: &str| Ok::<_, String>(parse_list(v)),
        |l: &Vec<String>| list_text(l)
    ),
    field_key!("accent_picker", accent_picker, parse_switch, switch_ref),
    field_key!(
        "accent_table",
        accent_table,
        |v: &str| Ok::<_, String>(v.to_string()),
        |t: &String| t.clone()
    ),
    field_key!(
        "accent_keyboards",
        accent_keyboards,
        |v: &str| Ok::<_, String>(parse_list(v)),
        |l: &Vec<String>| list_text(l)
    ),
    field_key!(
        "accent_disabled_apps",
        accent_disabled_apps,
        |v: &str| Ok::<_, String>(parse_list(v)),
        |l: &Vec<String>| list_text(l)
    ),
];

impl KeyboardSettings {
    /// Where the file is: `$CUPERTINO_KEYBOARD_CONFIG`, else `keyboard.conf` in the settings directory.
    pub fn path() -> Option<PathBuf> {
        crate::config_file(KEYBOARD_CONF_ENV, "keyboard.conf")
    }

    /// The settings of `text` and the problems of its lines; with `last_good`, a key whose line is wrong keeps its
    /// last good value.
    pub fn parse(text: &str, last_good: Option<&Self>) -> (Self, Vec<Problem>) {
        conf::read(
            &conf::ConfText::parse(text),
            KEYS,
            &Self::default(),
            last_good,
            None,
        )
    }

    /// [`Self::parse`] of the file at `path`; a missing file sets nothing.
    pub fn load(path: &Path, last_good: Option<&Self>) -> io::Result<(Self, Vec<Problem>)> {
        conf::load(path, KEYS, &Self::default(), last_good, None)
    }

    /// Write the keys whose value differs from the file's into `path` atomically; comments and other lines stay.
    pub fn store(&self, path: &Path) -> io::Result<Vec<Problem>> {
        conf::store(path, KEYS, &Self::default(), None, self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_read_and_bad_lines_reported() {
        let (s, problems) = KeyboardSettings::parse(
            "# comment\nrepeat_delay = 300\nrepeat_interval = 40  # fast\ninput_sources = us, de, fr(nodeadkeys)\nmodifier_profile = classic-pc\n\
             caps_lock_delay = 0\naccent_picker = off\naccent_table = ~/a.json\naccent_keyboards = default, en\nbogus = 1\nrepeat_delay = x\nnonsense\nmodifier_profile = nope\n",
            None,
        );
        assert_eq!(
            problems.iter().map(|p| p.line).collect::<Vec<_>>(),
            vec![10, 11, 12, 13]
        );
        assert_eq!(
            problems[3].message,
            "modifier_profile: expected cupertino, classic-pc, got \"nope\""
        );
        assert_eq!(
            (s.repeat_delay_ms, s.repeat_interval_ms),
            (Some(300), Some(40)),
            "the bad line later does not unset the earlier value"
        );
        assert_eq!(
            s.input_sources
                .as_ref()
                .unwrap()
                .iter()
                .map(|x| x.id.as_str())
                .collect::<Vec<_>>(),
            vec!["us", "de", "fr(nodeadkeys)"]
        );
        assert_eq!(s.modifier_profile, Some(ModifierProfile::ClassicPc));
        assert_eq!(
            (s.caps_lock_delay_ms, s.accent_picker),
            (Some(0), Some(false))
        );
        assert_eq!(s.accent_table.as_deref(), Some("~/a.json"));
        assert_eq!(
            s.accent_keyboards,
            Some(vec!["default".to_string(), "en".to_string()])
        );
    }

    #[test]
    fn ranges_and_choices_are_checked() {
        let (s, problems) = KeyboardSettings::parse(
            "caps_lock_hold = 800\nlayout_keyboard = iso\nlayout_level3 = both-alt\nfn_key = do-nothing\n\
             caps_lock_hold = 0\ncaps_lock_hold = 1501\nslow_keys_delay = 0\nfn_key = sing\nmouse_keys_max_speed = 0\nlayout_keyboard = x\n",
            None,
        );
        assert_eq!(
            problems.iter().map(|p| p.line).collect::<Vec<_>>(),
            vec![5, 6, 7, 8, 9, 10]
        );
        assert_eq!(s.caps_lock_hold_ms, Some(800));
        assert_eq!(
            (s.layout_keyboard, s.layout_level3, s.fn_key),
            (
                Some(LayoutKeyboard::Iso),
                Some(Level3::BothAlt),
                Some(FnKey::DoNothing)
            )
        );
    }

    #[test]
    fn sources_parse() {
        assert_eq!(
            SourceSpec::parse("fr(nodeadkeys)"),
            Some(SourceSpec {
                id: "fr(nodeadkeys)".into(),
                layout: "fr".into(),
                variant: "nodeadkeys".into(),
                decoded: None
            })
        );
        let d = SourceSpec::parse("decoded:/l/ABC.json").unwrap();
        assert_eq!(
            (d.layout.as_str(), d.decoded),
            ("", Some(PathBuf::from("/l/ABC.json")))
        );
        assert!(SourceSpec::parse("decoded:").is_none());
        assert_eq!(SourceSpec::parse(" us ").unwrap().layout, "us");
        assert!(SourceSpec::parse("").is_none());
        assert!(SourceSpec::parse("fr(x").is_none());
        assert!(SourceSpec::parse("a b").is_none());
        assert_eq!(
            split_sources("us, fr(a,b), de"),
            vec!["us", "fr(a,b)", "de"]
        );
    }

    #[test]
    fn a_store_round_trips_lists_sources_and_choices() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("keyboard.conf");
        std::fs::write(
            &path,
            "# typing\nrepeat_delay = 300 # slow\ninput_sources = us, fr(nodeadkeys)\n",
        )
        .unwrap();
        let (mut s, _) = KeyboardSettings::load(&path, None).unwrap();
        s.repeat_interval_ms = Some(33);
        s.fn_key = Some(FnKey::Emoji);
        s.rewrite_off = Some(vec!["Code".into(), "gimp".into()]);
        assert!(s.store(&path).unwrap().is_empty());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "# typing\nrepeat_delay = 300 # slow\ninput_sources = us, fr(nodeadkeys)\nrepeat_interval = 33\nfn_key = emoji\nrewrite_off = Code, gimp\n");
        assert_eq!(KeyboardSettings::load(&path, None).unwrap().0, s);
    }

    #[test]
    fn repeat_off_keeps_the_saved_delay() {
        let (s, problems) =
            KeyboardSettings::parse("repeat_delay = 0\nrepeat_delay_saved = 300\n", None);
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(
            (s.repeat_delay_ms, s.repeat_delay_saved_ms),
            (Some(0), Some(300))
        );
    }

    #[test]
    fn a_bad_value_keeps_the_last_good_one() {
        let good = KeyboardSettings {
            repeat_delay_ms: Some(250),
            ..KeyboardSettings::default()
        };
        let (s, problems) = KeyboardSettings::parse("repeat_delay = soon\n", Some(&good));
        assert_eq!(s.repeat_delay_ms, Some(250));
        assert_eq!(problems.len(), 1);
    }
}
