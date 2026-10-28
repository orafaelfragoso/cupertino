//! Keys to field commands: physical modifiers through the user's modifier profile, then the standard editing chords.

use cupertino_defaults::keyboard::KeyboardSettings;
use cupertino_keyboard::{BindingTable, Chord, Key, NamedKey, Resolution, Resolver, TextAction};
use cupertino_modifiers::{Mods, Profile, RawMods};

/// Evdev codes of the named keys a field reads (`linux/input-event-codes.h`).
const NAMED_KEYS: [(u32, NamedKey); 15] = [
    (1, NamedKey::Escape),
    (14, NamedKey::Backspace),
    (15, NamedKey::Tab),
    (28, NamedKey::Return),
    (57, NamedKey::Space),
    (96, NamedKey::KpEnter),
    (102, NamedKey::Home),
    (103, NamedKey::Up),
    (104, NamedKey::PageUp),
    (105, NamedKey::Left),
    (106, NamedKey::Right),
    (107, NamedKey::End),
    (108, NamedKey::Down),
    (109, NamedKey::PageDown),
    (111, NamedKey::Delete),
];

/// The Latin letters of the letter keys by evdev code (`KEY_Q` = 16 … `KEY_M` = 50), for chords on a layout whose
/// letters are not Latin.
const LATIN_LETTERS: [(u32, char); 26] = [
    (16, 'q'),
    (17, 'w'),
    (18, 'e'),
    (19, 'r'),
    (20, 't'),
    (21, 'y'),
    (22, 'u'),
    (23, 'i'),
    (24, 'o'),
    (25, 'p'),
    (30, 'a'),
    (31, 's'),
    (32, 'd'),
    (33, 'f'),
    (34, 'g'),
    (35, 'h'),
    (36, 'j'),
    (37, 'k'),
    (38, 'l'),
    (44, 'z'),
    (45, 'x'),
    (46, 'c'),
    (47, 'v'),
    (48, 'b'),
    (49, 'n'),
    (50, 'm'),
];

/// A key press as the client reports it: the evdev code, the physical modifiers and the character it types.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyInput {
    pub code: u32,
    pub mods: RawMods,
    pub character: Option<char>,
}

impl KeyInput {
    pub fn new(
        code: u32,
        shift: bool,
        ctrl: bool,
        alt: bool,
        logo: bool,
        character: Option<char>,
    ) -> Self {
        let mut mods = RawMods::default();
        for (held, m) in [
            (shift, RawMods::SHIFT),
            (ctrl, RawMods::CTRL),
            (alt, RawMods::ALT),
            (logo, RawMods::SUPER),
        ] {
            if held {
                mods = mods | m;
            }
        }
        KeyInput {
            code,
            mods,
            character,
        }
    }
}

/// What a key asks the field to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Command {
    SelectAll,
    Copy,
    Cut,
    Paste,
    Undo,
    Redo,
    Actions(Vec<TextAction>),
    /// The first chord of a longer binding: swallowed.
    Pending,
    Insert(char),
    /// Not a field key: the window handles it.
    Ignored,
}

/// The user's modifier profile and editing chords.
#[derive(Clone, Debug)]
pub struct Keymap {
    profile: Profile,
    resolver: Resolver,
}

impl Keymap {
    pub fn new(profile: Profile, bindings: BindingTable) -> Self {
        Keymap {
            profile,
            resolver: Resolver::new(bindings),
        }
    }

    /// The standard chords under `profile`.
    pub fn standard_for(profile: Profile) -> Self {
        Keymap::new(profile, BindingTable::standard())
    }

    /// The design's profile (Super is the primary modifier) with the standard chords.
    pub fn standard() -> Self {
        Keymap::new(Profile::cupertino(), BindingTable::standard())
    }

    /// The profile of `keyboard.conf` and the standard chords with the user's overrides; what cannot be read keeps the
    /// standard value.
    pub fn load() -> Self {
        let profile = KeyboardSettings::path()
            .and_then(|path| KeyboardSettings::load(&path, None).ok())
            .and_then(|(settings, _)| settings.modifier_profile)
            .and_then(|p| Profile::by_name(p.name()))
            .unwrap_or_else(Profile::cupertino);
        Keymap::new(profile, BindingTable::load_default().0)
    }

    /// The logical modifiers the user's profile makes of the physical ones held: for the chords of a window, which
    /// name the command key by its role.
    pub fn logical(&self, shift: bool, ctrl: bool, alt: bool, logo: bool) -> Mods {
        let mut raw = RawMods::default();
        for (held, key) in [
            (shift, RawMods::SHIFT),
            (ctrl, RawMods::CTRL),
            (alt, RawMods::ALT),
            (logo, RawMods::SUPER),
        ] {
            if held {
                raw = raw | key;
            }
        }
        self.profile.translate(raw)
    }

    pub(crate) fn command(&mut self, key: KeyInput) -> Command {
        let mods = self.profile.translate(key.mods);
        let named = NAMED_KEYS
            .iter()
            .find(|(code, _)| *code == key.code)
            .map(|(_, k)| *k);
        let chorded = key.mods.contains(RawMods::CTRL) || key.mods.contains(RawMods::SUPER);
        let latin = LATIN_LETTERS
            .iter()
            .find(|(code, _)| *code == key.code)
            .map(|(_, c)| *c);
        let chord_key = match (named, key.character) {
            (Some(n), _) => Key::Named(n),
            // a chord names its letter by the key's Latin letter when the layout's is not Latin (Super+A on a
            // Cyrillic layout types 'ф')
            (None, Some(c)) if chorded && !c.is_ascii() && latin.is_some() => {
                Key::Char(latin.expect("checked by the guard"))
            }
            (None, Some(c)) => Key::Char(c.to_ascii_lowercase()),
            (None, None) => return Command::Ignored,
        };
        if mods.contains(Mods::PRIMARY) && !mods.contains(Mods::CTRL) {
            let shift = mods.contains(Mods::SHIFT);
            match chord_key {
                Key::Char('a') if !shift => return Command::SelectAll,
                Key::Char('c') if !shift => return Command::Copy,
                Key::Char('x') if !shift => return Command::Cut,
                Key::Char('v') if !shift => return Command::Paste,
                Key::Char('z') if shift => return Command::Redo,
                Key::Char('z') => return Command::Undo,
                _ => {}
            }
        }
        match self.resolver.feed(Chord::new(mods, chord_key)) {
            Resolution::Actions(actions) => Command::Actions(actions),
            Resolution::Pending => Command::Pending,
            Resolution::NoMatch => match key.character {
                Some(c)
                    if !c.is_control()
                        && !key.mods.contains(RawMods::CTRL)
                        && !key.mods.contains(RawMods::SUPER) =>
                {
                    Command::Insert(c)
                }
                _ => Command::Ignored,
            },
        }
    }
}
