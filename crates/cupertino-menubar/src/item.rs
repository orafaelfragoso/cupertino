//! Menu item model shared by every menu source (synthetic, DBusMenu, GMenuModel, status items).

/// Identifier of an item inside one menu tree. DBusMenu ids are used as they are (the root is 0);
/// other sources number their items.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ItemId(pub i32);

/// The most characters of a shortcut key name a menu shows (the name comes from the app).
const MAX_KEY_CHARS: usize = 16;

/// Text from outside as menu text: without control characters or the characters that turn the text around, at most
/// `max_chars` characters, trimmed.
pub fn plain_text(text: &str, max_chars: usize) -> String {
    text.chars()
        .filter(|c| {
            !c.is_control() && !matches!(c, '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        })
        .take(max_chars)
        .collect::<String>()
        .trim()
        .to_string()
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Toggle {
    #[default]
    None,
    /// A check mark; the flag is the checked state.
    Check(bool),
    /// A radio mark; the flag is the selected state.
    Radio(bool),
    /// A tri-state check that is neither on nor off.
    Indeterminate,
}

impl Toggle {
    pub fn is_marked(self) -> bool {
        matches!(
            self,
            Toggle::Check(true) | Toggle::Radio(true) | Toggle::Indeterminate
        )
    }
}

/// A keyboard shortcut as listed in a menu: modifier names and a key name (xkbcommon naming).
/// A shortcut split for drawing: the modifier glyphs and the key.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ShortcutGlyphs {
    pub modifiers: String,
    pub key: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Shortcut {
    pub modifiers: Vec<ShortcutModifier>,
    pub key: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ShortcutModifier {
    Ctrl,
    Alt,
    Shift,
    /// The primary shortcut modifier of the compositor.
    Primary,
}

impl Shortcut {
    /// Parse the DBusMenu `shortcut` representation: one inner list of tokens such as
    /// `["Control", "Shift", "Q"]`.
    pub fn from_tokens<S: AsRef<str>>(tokens: &[S]) -> Option<Shortcut> {
        let (key, mods) = tokens.split_last()?;
        let mut modifiers = Vec::new();
        for m in mods {
            let m = match m.as_ref().to_ascii_lowercase().as_str() {
                "control" | "ctrl" => ShortcutModifier::Ctrl,
                "alt" => ShortcutModifier::Alt,
                "shift" => ShortcutModifier::Shift,
                "super" | "meta" | "primary" => ShortcutModifier::Primary,
                _ => return None,
            };
            if !modifiers.contains(&m) {
                modifiers.push(m);
            }
        }
        modifiers.sort();
        let key = plain_text(key.as_ref(), MAX_KEY_CHARS);
        (!key.is_empty()).then_some(Shortcut { modifiers, key })
    }

    /// The shortcut as a menu draws it: the modifier glyphs in the order control, option, shift, command (the primary
    /// modifier), and the key, with the names of the common keys as symbols (`Return` as the return arrow). A menu
    /// shows the modifiers right-aligned and the keys in a column of their own.
    pub fn glyphs(&self) -> ShortcutGlyphs {
        let mut modifiers = String::new();
        let mut order = self.modifiers.clone();
        order.sort_by_key(|m| match m {
            ShortcutModifier::Ctrl => 0,
            ShortcutModifier::Alt => 1,
            ShortcutModifier::Shift => 2,
            ShortcutModifier::Primary => 3,
        });
        for m in order {
            modifiers.push(match m {
                ShortcutModifier::Ctrl => '\u{2303}',
                ShortcutModifier::Alt => '\u{2325}',
                ShortcutModifier::Shift => '\u{21e7}',
                ShortcutModifier::Primary => '\u{2318}',
            });
        }
        let key = match self.key.to_ascii_lowercase().as_str() {
            "return" | "enter" => "\u{21a9}".to_string(),
            "backspace" => "\u{232b}".to_string(),
            "delete" => "\u{2326}".to_string(),
            "tab" => "\u{21e5}".to_string(),
            "up" => "\u{2191}".to_string(),
            "down" => "\u{2193}".to_string(),
            "left" => "\u{2190}".to_string(),
            "right" => "\u{2192}".to_string(),
            _ if self.key.chars().count() == 1 => self.key.to_uppercase(),
            _ => self.key.clone(),
        };
        ShortcutGlyphs { modifiers, key }
    }

    /// Display text such as `Ctrl+Shift+Q`.
    pub fn display(&self) -> String {
        let mut s = String::new();
        for m in &self.modifiers {
            s.push_str(match m {
                ShortcutModifier::Ctrl => "Ctrl+",
                ShortcutModifier::Alt => "Alt+",
                ShortcutModifier::Shift => "Shift+",
                ShortcutModifier::Primary => "Super+",
            });
        }
        s.push_str(&self.key);
        s
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MenuItem {
    pub id: ItemId,
    pub label: String,
    /// Character after the mnemonic marker (`_` or `&`), if the source used one. Mnemonics are
    /// stripped from `label`.
    pub mnemonic: Option<char>,
    pub separator: bool,
    pub enabled: bool,
    pub visible: bool,
    pub toggle: Toggle,
    pub shortcut: Option<Shortcut>,
    /// Freedesktop icon name.
    pub icon_name: Option<String>,
    /// Encoded image bytes (PNG) when the source sent the icon inline.
    pub icon_data: Option<Vec<u8>>,
    /// The item opens a submenu (even if `children` has not been loaded yet).
    pub has_submenu: bool,
    pub children: Vec<MenuItem>,
}

impl MenuItem {
    pub fn new(id: i32, label: &str) -> Self {
        let (label, mnemonic) = split_mnemonic(label);
        Self {
            id: ItemId(id),
            label,
            mnemonic,
            enabled: true,
            visible: true,
            ..Default::default()
        }
    }

    pub fn separator(id: i32) -> Self {
        Self {
            id: ItemId(id),
            separator: true,
            enabled: false,
            visible: true,
            ..Default::default()
        }
    }

    pub fn submenu(id: i32, label: &str, children: Vec<MenuItem>) -> Self {
        Self {
            has_submenu: true,
            children,
            ..Self::new(id, label)
        }
    }

    pub fn with_shortcut(mut self, s: Shortcut) -> Self {
        self.shortcut = Some(s);
        self
    }

    pub fn with_toggle(mut self, t: Toggle) -> Self {
        self.toggle = t;
        self
    }

    pub fn disabled(mut self) -> Self {
        self.enabled = false;
        self
    }

    /// An item that can be highlighted and activated.
    pub fn is_selectable(&self) -> bool {
        self.visible && !self.separator && self.enabled
    }

    /// A submenu whose children are not loaded yet and need an `AboutToShow` round trip.
    pub fn needs_load(&self) -> bool {
        self.has_submenu && self.children.is_empty()
    }

    pub fn find(&self, id: ItemId) -> Option<&MenuItem> {
        if self.id == id {
            return Some(self);
        }
        self.children.iter().find_map(|c| c.find(id))
    }

    pub fn find_mut(&mut self, id: ItemId) -> Option<&mut MenuItem> {
        if self.id == id {
            return Some(self);
        }
        self.children.iter_mut().find_map(|c| c.find_mut(id))
    }

    /// Visible children, with leading, trailing and doubled separators removed.
    pub fn visible_children(&self) -> Vec<&MenuItem> {
        let mut out: Vec<&MenuItem> = Vec::new();
        for c in self.children.iter().filter(|c| c.visible) {
            if c.separator && out.last().is_none_or(|l| l.separator) {
                continue;
            }
            out.push(c);
        }
        while out.last().is_some_and(|l| l.separator) {
            out.pop();
        }
        out
    }
}

/// Split a label with a mnemonic marker: `_File` → (`File`, `F`), `Save &As` → (`Save As`, `A`),
/// `a__b` / `a&&b` keep a literal marker character.
pub fn split_mnemonic(label: &str) -> (String, Option<char>) {
    let mut out = String::with_capacity(label.len());
    let mut mnemonic = None;
    let mut chars = label.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '_' || c == '&' {
            match chars.peek() {
                Some(&n) if n == c => {
                    out.push(c);
                    chars.next();
                }
                Some(&n) if mnemonic.is_none() => {
                    mnemonic = Some(n);
                }
                _ => {}
            }
        } else {
            out.push(c);
        }
    }
    (out, mnemonic)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mnemonics_are_stripped() {
        assert_eq!(split_mnemonic("_File"), ("File".to_string(), Some('F')));
        assert_eq!(
            split_mnemonic("Save &As"),
            ("Save As".to_string(), Some('A'))
        );
        assert_eq!(split_mnemonic("a__b"), ("a_b".to_string(), None));
        assert_eq!(
            split_mnemonic("Tom && Jerry"),
            ("Tom & Jerry".to_string(), None)
        );
        assert_eq!(split_mnemonic("plain"), ("plain".to_string(), None));
        assert_eq!(split_mnemonic("trailing_"), ("trailing".to_string(), None));
    }

    #[test]
    fn shortcut_tokens() {
        let s = Shortcut::from_tokens(&["Control", "Shift", "Q"]).unwrap();
        assert_eq!(
            s.modifiers,
            vec![ShortcutModifier::Ctrl, ShortcutModifier::Shift]
        );
        assert_eq!(s.display(), "Ctrl+Shift+Q");
        assert_eq!(
            Shortcut::from_tokens(&["Super", "space"])
                .unwrap()
                .display(),
            "Super+space"
        );
        assert!(Shortcut::from_tokens(&["Hyper", "x"]).is_none());
        assert!(Shortcut::from_tokens::<&str>(&[]).is_none());
        assert_eq!(Shortcut::from_tokens(&["F5"]).unwrap().modifiers, vec![]);
    }

    #[test]
    fn an_app_shortcut_key_is_plain_and_short() {
        let key = |k: &str| Shortcut::from_tokens(&["Control", k]).map(|s| s.key);
        assert_eq!(key("\u{202e}Q\u{7}"), Some("Q".to_string()));
        assert_eq!(key(&"X".repeat(200)), Some("X".repeat(16)));
        assert_eq!(key("\u{202e}\u{7}"), None);
    }

    #[test]
    fn item_flags() {
        let i = MenuItem::new(1, "_Open");
        assert!(i.is_selectable() && i.mnemonic == Some('O'));
        assert!(!i.clone().disabled().is_selectable());
        assert!(!MenuItem::separator(2).is_selectable());
        let mut hidden = MenuItem::new(3, "x");
        hidden.visible = false;
        assert!(!hidden.is_selectable());
        let sub = MenuItem::submenu(4, "More", vec![]);
        assert!(sub.needs_load());
        assert!(!MenuItem::submenu(5, "More", vec![MenuItem::new(6, "a")]).needs_load());
        assert!(
            Toggle::Check(true).is_marked()
                && !Toggle::Check(false).is_marked()
                && Toggle::Indeterminate.is_marked()
        );
    }

    #[test]
    fn visible_children_trim_separators() {
        let root = MenuItem::submenu(
            0,
            "root",
            vec![
                MenuItem::separator(1),
                MenuItem::new(2, "a"),
                MenuItem::separator(3),
                MenuItem::separator(4),
                MenuItem {
                    visible: false,
                    ..MenuItem::new(5, "hidden")
                },
                MenuItem::new(6, "b"),
                MenuItem::separator(7),
            ],
        );
        let ids: Vec<i32> = root.visible_children().iter().map(|c| c.id.0).collect();
        assert_eq!(ids, vec![2, 3, 6]);
    }

    #[test]
    fn find_walks_the_tree() {
        let mut root = MenuItem::submenu(
            0,
            "r",
            vec![MenuItem::submenu(1, "a", vec![MenuItem::new(2, "deep")])],
        );
        assert_eq!(root.find(ItemId(2)).unwrap().label, "deep");
        assert!(root.find(ItemId(9)).is_none());
        root.find_mut(ItemId(2)).unwrap().enabled = false;
        assert!(!root.find(ItemId(2)).unwrap().enabled);
    }

    #[test]
    fn a_shortcut_is_drawn_as_modifier_glyphs_in_a_fixed_order_and_a_key() {
        let glyphs = |tokens: &[&str]| Shortcut::from_tokens(tokens).unwrap().glyphs();
        assert_eq!(
            glyphs(&["Super", "Shift", "Control", "Alt", "q"]),
            ShortcutGlyphs {
                modifiers: "\u{2303}\u{2325}\u{21e7}\u{2318}".into(),
                key: "Q".into()
            },
            "control, option, shift, command, whatever order they were given in"
        );
        assert_eq!(
            glyphs(&["Super", "Control", "q"]).modifiers,
            "\u{2303}\u{2318}"
        );
        assert_eq!(
            glyphs(&["Shift", "Super", "q"]).modifiers,
            "\u{21e7}\u{2318}"
        );
        assert_eq!(glyphs(&["Return"]).key, "\u{21a9}");
        assert_eq!(glyphs(&["Super", "BackSpace"]).key, "\u{232b}");
        assert_eq!(glyphs(&["Super", "Left"]).key, "\u{2190}");
        assert_eq!(
            glyphs(&["Super", "F5"]).key,
            "F5",
            "a key without a symbol keeps its name"
        );
        assert_eq!(glyphs(&["Ctrl", "n"]).modifiers, "\u{2303}");
    }
}
