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
            !c.is_control()
                && !matches!(c, '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
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

/// How a menu draws a shortcut: the modifier glyphs and key symbols of the Mac layout, or the key names of the PC
/// layout (`Ctrl+Alt+` and `F4`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ShortcutStyle {
    #[default]
    Glyphs,
    Names,
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

    /// Parse a chord in the shortcut file's syntax, such as `Ctrl+Super+Left`: physical Super is the primary
    /// modifier, physical Ctrl stays Ctrl.
    pub fn from_chord(chord: &str) -> Option<Shortcut> {
        Self::from_tokens(&chord.split('+').collect::<Vec<_>>())
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
                ShortcutModifier::Ctrl => MODIFIER_GLYPHS[0],
                ShortcutModifier::Alt => MODIFIER_GLYPHS[1],
                ShortcutModifier::Shift => MODIFIER_GLYPHS[2],
                ShortcutModifier::Primary => MODIFIER_GLYPHS[3],
            });
        }
        let lower = self.key.to_ascii_lowercase();
        let key = match KEY_GLYPHS
            .iter()
            .find(|(names, _)| names.contains(&lower.as_str()))
        {
            Some((_, glyph)) => glyph.to_string(),
            None if self.key.chars().count() == 1 => self.key.to_uppercase(),
            None => key_char(&lower).map_or_else(|| self.key.clone(), String::from),
        };
        ShortcutGlyphs { modifiers, key }
    }

    /// The shortcut as `style` draws it.
    pub fn shown(&self, style: ShortcutStyle) -> ShortcutGlyphs {
        match style {
            ShortcutStyle::Glyphs => self.glyphs(),
            ShortcutStyle::Names => self.names(),
        }
    }

    /// The shortcut as the PC layout names its keys: `Ctrl+`, `Alt+`, `Shift+`, `Super+` in that order and the key by
    /// its usual name (`Enter`, `Esc`, `Left`, `,`).
    pub fn names(&self) -> ShortcutGlyphs {
        let display = self.display();
        let modifiers = display[..display.len() - self.key.len()].to_string();
        let key = match self.key.to_ascii_lowercase().as_str() {
            "return" | "enter" | "kp_enter" => "Enter".to_string(),
            "backspace" => "Backspace".to_string(),
            "delete" => "Del".to_string(),
            "escape" => "Esc".to_string(),
            "space" => "Space".to_string(),
            "page_up" | "prior" => "PgUp".to_string(),
            "page_down" | "next" => "PgDn".to_string(),
            "comma" => ",".to_string(),
            "period" => ".".to_string(),
            "slash" => "/".to_string(),
            "backslash" => "\\".to_string(),
            "grave" => "`".to_string(),
            "minus" => "-".to_string(),
            "plus" => "Plus".to_string(),
            "equal" => "=".to_string(),
            "semicolon" => ";".to_string(),
            "apostrophe" => "'".to_string(),
            "bracketleft" => "[".to_string(),
            "bracketright" => "]".to_string(),
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

/// The modifier glyphs of the glyph style: control, option, shift, command.
const MODIFIER_GLYPHS: [char; 4] = ['\u{2303}', '\u{2325}', '\u{21e7}', '\u{2318}'];

/// The keys the glyph style draws as a symbol or a short word, by their lower-case keysym names. Every symbol here must be
/// in the embedded font (a test checks it): Inter has no keypad Enter symbol, so that key is a word.
const KEY_GLYPHS: &[(&[&str], &str)] = &[
    (&["return", "enter"], "\u{21a9}"),
    (&["kp_enter"], "Enter"),
    (&["backspace"], "\u{232b}"),
    (&["delete"], "\u{2326}"),
    (&["tab"], "\u{21e5}"),
    (&["escape"], "\u{238b}"),
    (&["space"], "Space"),
    (&["page_up", "prior"], "\u{21de}"),
    (&["page_down", "next"], "\u{21df}"),
    (&["home"], "\u{2196}"),
    (&["end"], "\u{2198}"),
    (&["up"], "\u{2191}"),
    (&["down"], "\u{2193}"),
    (&["left"], "\u{2190}"),
    (&["right"], "\u{2192}"),
];

/// The character a keysym name types, for the punctuation names chords use.
pub(crate) fn key_char(name: &str) -> Option<char> {
    let mut chars = name.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        return Some(c);
    }
    Some(match name {
        "comma" => ',',
        "period" => '.',
        "slash" => '/',
        "backslash" => '\\',
        "semicolon" => ';',
        "colon" => ':',
        "apostrophe" => '\'',
        "quotedbl" => '"',
        "grave" => '`',
        "minus" => '-',
        "equal" => '=',
        "plus" => '+',
        "bracketleft" => '[',
        "bracketright" => ']',
        "question" => '?',
        "exclam" => '!',
        _ => return None,
    })
}

/// Whose item a [`MenuItem`] is. An activated item is routed by its origin, never by its id: an exporter chooses its
/// ids freely, so an id says nothing about whose item it is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Origin {
    /// The bar's own item: the system, App and synthetic menus and what the bar adds to an exporter's menus.
    #[default]
    Bar,
    /// An item of a DBusMenu or GMenuModel exporter, set by the reader.
    Exporter,
}

/// What a row of a status panel is: a panel is a menu laid out in its own style (fixed width, taller rows, a switch,
/// discs and signal marks) rather than as a list of text rows.
#[derive(Clone, Debug, PartialEq)]
pub enum PanelRow {
    /// The title row with a switch at its trailing end.
    Header { title: String, on: bool },
    /// A small heading over the rows below it.
    Section { title: String },
    /// The network the machine is joined to: an accent disc with the wireless mark, the name and a lock when secured.
    Current { name: String, secured: bool },
    /// A row that shows or hides the list below it; a click leaves the panel open.
    Disclosure { title: String, expanded: bool },
    /// A network of the list: the name, a lock when secured and the signal strength, 0 to 3 bars.
    Network {
        name: String,
        secured: bool,
        strength: u8,
    },
    /// A line of plain secondary text: a state such as no networks.
    Note { text: String },
    /// A plain text row that opens something.
    Link { title: String },
}

impl PanelRow {
    /// Whether choosing the row leaves the panel open: the switch and the disclosure.
    pub fn keeps_open(&self) -> bool {
        matches!(self, Self::Header { .. } | Self::Disclosure { .. })
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MenuItem {
    pub id: ItemId,
    pub origin: Origin,
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
    /// The GMenuModel action the item activates, with its scope (`app.quit`, `win.close`); `None` from other sources.
    pub action: Option<String>,
    /// The GMenuModel accelerator (`<Primary>q`), shown through the bar's modifier map; `None` from other sources.
    pub accel: Option<String>,
    /// Secondary text drawn dimmed after the label (the menu path of a Help search result); `None` for others.
    pub detail: Option<String>,
    /// The item shown in this one's place while Alt is held, with its own id; only the bar's own items have one.
    pub alternate: Option<Box<MenuItem>>,
    /// The row of a status panel this item is, drawn in the panel's style; `None` for the rows of a plain menu. A menu
    /// whose children have one is laid out as a panel.
    pub panel: Option<PanelRow>,
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

    pub fn with_alternate(mut self, alternate: MenuItem) -> Self {
        self.alternate = Some(Box::new(alternate));
        self
    }

    /// The menu as it shows while Alt is held: each child with an alternate is swapped for it, and the alternate keeps
    /// the child as its own alternate, so the row is measured for both and Alt going up swaps it back.
    pub fn alternates_shown(&self) -> MenuItem {
        let mut menu = self.clone();
        for child in &mut menu.children {
            if let Some(mut alternate) = child.alternate.take() {
                std::mem::swap(child, &mut alternate);
                child.alternate = Some(alternate);
            }
        }
        menu
    }

    /// An item that can be highlighted and activated.
    pub fn is_selectable(&self) -> bool {
        self.visible && !self.separator && self.enabled
    }

    /// A submenu whose children are not loaded yet and need an `AboutToShow` round trip.
    pub fn needs_load(&self) -> bool {
        self.has_submenu && self.children.is_empty()
    }

    /// Mark the item and everything below it as an exporter's.
    pub fn exported(mut self) -> Self {
        self.origin = Origin::Exporter;
        self.children = self.children.into_iter().map(MenuItem::exported).collect();
        self
    }

    pub fn find(&self, id: ItemId) -> Option<&MenuItem> {
        if self.id == id {
            return Some(self);
        }
        if let Some(alternate) = self.alternate.as_deref().filter(|a| a.id == id) {
            return Some(alternate);
        }
        self.children.iter().find_map(|c| c.find(id))
    }

    pub fn find_mut(&mut self, id: ItemId) -> Option<&mut MenuItem> {
        if self.id == id {
            return Some(self);
        }
        if let Some(alternate) = self.alternate.as_deref_mut().filter(|a| a.id == id) {
            return Some(alternate);
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
    split_marked(label, &['_', '&'])
}

/// [`split_mnemonic`] for GMenuModel labels, whose only marker is `_`: `Q&A` stays `Q&A`.
pub fn split_gtk_mnemonic(label: &str) -> (String, Option<char>) {
    split_marked(label, &['_'])
}

fn split_marked(label: &str, markers: &[char]) -> (String, Option<char>) {
    let mut out = String::with_capacity(label.len());
    let mut mnemonic = None;
    let mut chars = label.chars().peekable();
    while let Some(c) = chars.next() {
        if markers.contains(&c) {
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
    fn a_gtk_label_keeps_its_ampersands() {
        assert_eq!(split_gtk_mnemonic("Q&A"), ("Q&A".to_string(), None));
        assert_eq!(
            split_gtk_mnemonic("_Save && Close"),
            ("Save && Close".to_string(), Some('S'))
        );
        assert_eq!(split_gtk_mnemonic("a__b"), ("a_b".to_string(), None));
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
    fn a_chord_of_the_shortcut_file_names_physical_keys() {
        let s = Shortcut::from_chord("Ctrl+Super+Left").unwrap();
        assert_eq!(
            s.modifiers,
            vec![ShortcutModifier::Ctrl, ShortcutModifier::Primary]
        );
        assert_eq!(s.key, "Left");
        assert_eq!(Shortcut::from_chord("Ctrl+q").unwrap().display(), "Ctrl+q");
        assert!(Shortcut::from_chord("").is_none());
        assert!(Shortcut::from_chord("Super+").is_none());
    }

    #[test]
    fn an_app_shortcut_key_is_plain_and_short() {
        let key = |k: &str| Shortcut::from_tokens(&["Control", k]).map(|s| s.key);
        assert_eq!(key("\u{202e}Q\u{7}"), Some("Q".to_string()));
        assert_eq!(key(&"X".repeat(200)), Some("X".repeat(16)));
        assert_eq!(key("\u{202e}\u{7}"), None);
    }

    #[test]
    fn a_file_name_cannot_turn_its_extension_around() {
        assert_eq!(plain_text("photo\u{202e}gpj.sh", 64), "photogpj.sh");
        assert_eq!(plain_text("a\u{061c}\u{2067}b\u{2069}\u{200f}", 64), "ab");
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
    fn a_shortcut_is_drawn_by_key_names_in_the_pc_order_under_the_names_style() {
        let names = |tokens: &[&str]| {
            let s = Shortcut::from_tokens(tokens)
                .unwrap()
                .shown(ShortcutStyle::Names);
            (s.modifiers, s.key)
        };
        let pair = |m: &str, k: &str| (m.to_string(), k.to_string());
        assert_eq!(names(&["Alt", "F4"]), pair("Alt+", "F4"));
        assert_eq!(names(&["Super", "Control", "z"]), pair("Ctrl+Super+", "Z"));
        assert_eq!(names(&["Super", "Down"]), pair("Super+", "Down"));
        assert_eq!(
            names(&["Super", "Shift", "Control", "Alt", "q"]),
            pair("Ctrl+Alt+Shift+Super+", "Q"),
            "whatever order they were given in"
        );
        assert_eq!(names(&["Return"]), pair("", "Enter"));
        assert_eq!(names(&["Super", "comma"]), pair("Super+", ","));
        assert_eq!(
            names(&["Shift", "Super", "slash"]),
            pair("Shift+Super+", "/")
        );
        assert_eq!(names(&["Control", "Escape"]), pair("Ctrl+", "Esc"));
        assert_eq!(
            names(&["Control", "plus"]),
            pair("Ctrl+", "Plus"),
            "not Ctrl++"
        );
        // the glyph style is unchanged
        assert_eq!(
            Shortcut::from_tokens(&["Alt", "F4"])
                .unwrap()
                .shown(ShortcutStyle::Glyphs),
            Shortcut::from_tokens(&["Alt", "F4"]).unwrap().glyphs()
        );
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
        for (key, glyph) in [
            ("comma", ","),
            ("slash", "/"),
            ("grave", "`"),
            ("Escape", "\u{238b}"),
            ("space", "Space"),
            ("Page_Up", "\u{21de}"),
            ("Prior", "\u{21de}"),
            ("Page_Down", "\u{21df}"),
            ("Next", "\u{21df}"),
            ("Home", "\u{2196}"),
            ("End", "\u{2198}"),
            ("KP_Enter", "Enter"),
        ] {
            assert_eq!(glyphs(&["Super", key]).key, glyph, "{key}");
        }
    }

    /// The bar draws with the embedded font when no asset pack is installed, and it has no fallback: a glyph it lacks
    /// is a box.
    #[cfg(feature = "app")]
    #[test]
    fn every_glyph_the_glyph_style_draws_is_in_the_embedded_font() {
        let font = swash::FontRef::from_index(cupertino_assets::BUILTIN_FONT, 0).unwrap();
        let charmap = font.charmap();
        let punctuation = [
            "comma",
            "period",
            "slash",
            "backslash",
            "semicolon",
            "colon",
            "apostrophe",
            "quotedbl",
            "grave",
            "minus",
            "equal",
            "plus",
            "bracketleft",
            "bracketright",
            "question",
            "exclam",
        ]
        .map(|name| key_char(name).unwrap());
        let drawn = MODIFIER_GLYPHS
            .into_iter()
            .chain(KEY_GLYPHS.iter().flat_map(|(_, glyph)| glyph.chars()))
            .chain(punctuation);
        for c in drawn {
            assert_ne!(
                charmap.map(c),
                0,
                "{c:?} (U+{:04X}) is not in the embedded font",
                c as u32
            );
        }
    }
}
