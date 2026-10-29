//! Keys to field commands: physical modifiers through the user's modifier profile, then the standard editing chords.

use cupertino_key_bindings::{
    BindingTable, Chord, Key, NamedKey, Resolution, Resolver, TextAction,
};
use cupertino_modifiers::{Mods, Physical, Profile, RawMods};

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
    pub fn new(profile: Profile, mut bindings: BindingTable) -> Self {
        // where the Ctrl role is not on the Ctrl key (`classic-pc`) the system keeps Super+Tab for the overview, so
        // the standard Ctrl+Tab of the key views is out of reach: they are on the command key's Tab, which is the
        // physical Ctrl+Tab. A binding of the user's on that chord stays.
        if profile.primary() != Physical::Super {
            for (mods, action) in [
                (Mods::PRIMARY, TextAction::SelectNextKeyView),
                (
                    Mods::PRIMARY | Mods::SHIFT,
                    TextAction::SelectPreviousKeyView,
                ),
            ] {
                let chord = Chord::new(mods, Key::Named(NamedKey::Tab));
                if bindings.get(&[chord]).is_none() {
                    bindings.bind(vec![chord], vec![action]);
                }
            }
        }
        Keymap {
            profile,
            resolver: Resolver::new(bindings),
        }
    }

    /// The user's modifier profile.
    pub fn profile(&self) -> &Profile {
        &self.profile
    }

    /// The standard chords under `profile`.
    pub fn standard_for(profile: Profile) -> Self {
        Keymap::new(profile, BindingTable::standard())
    }

    /// The design's profile (Super is the primary modifier) with the standard chords.
    pub fn standard() -> Self {
        Keymap::new(Profile::cupertino(), BindingTable::standard())
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
        let chorded = key.mods.contains(RawMods::CTRL) || key.mods.contains(RawMods::SUPER);
        self.resolve(
            key.code,
            self.profile.translate(key.mods),
            chorded,
            key.character,
        )
    }

    /// A key whose modifiers the window system already gave their roles, as CupertinoKit's platform reports them.
    pub(crate) fn logical_command(
        &mut self,
        code: u32,
        mods: Mods,
        character: Option<char>,
    ) -> Command {
        let chorded = mods.contains(Mods::CTRL) || mods.contains(Mods::PRIMARY);
        self.resolve(code, mods, chorded, character)
    }

    fn resolve(
        &mut self,
        code: u32,
        mods: Mods,
        chorded: bool,
        character: Option<char>,
    ) -> Command {
        let named = NAMED_KEYS.iter().find(|(c, _)| *c == code).map(|(_, k)| *k);
        let latin = LATIN_LETTERS
            .iter()
            .find(|(c, _)| *c == code)
            .map(|(_, c)| *c);
        let chord_key = match (named, character) {
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
            Resolution::NoMatch => match character {
                Some(c) if !c.is_control() && !chorded => Command::Insert(c),
                _ => Command::Ignored,
            },
        }
    }
}
