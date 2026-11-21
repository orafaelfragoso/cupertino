//! Keyboard settings and the pure parts of the keyboard integration (`cupertino-keyboard`):
//! `keyboard.conf`, the key repeat advertised to clients, the modifier profile, the merge of the
//! verified shortcut assignments into the `cupertino-input` shortcut map, the caps-lock debounce and
//! the input-source list behind the switch.
//!
//! Shortcut precedence (highest first):
//!
//! 1. `compositor.json` `keybindings` (single chords, applied last by `Config::apply_file`),
//! 2. `shortcuts.conf` lines (`Super+x = exec:foot`, `Ctrl+Space = none` ...),
//! 3. the `cupertino-input` default map of the modifier profile (the modifiers are the physical keys; `classic-pc` has its
//!    own table and takes nothing from 4),
//! 4. under `cupertino`, the `cupertino-keyboard` reference assignment table (chords, the enabled subset is
//!    approximate, written for Super as the command key): [`merge_reference_shortcuts`] adds a row only when its chord is still free
//!    and its action has a counterpart here. The merge runs before `shortcuts.conf` is applied so that
//!    `chord = none` removes table rows as well. A chord that is taken by the map is a *conflict*: the map
//!    wins and the row is reported (and listed in the compositor log), never silently doubled.
//!
//! `keyboard.conf` (`$CUPERTINO_KEYBOARD_CONFIG`, else `~/.config/cupertino/keyboard.conf`), `# starts a comment:
//!
//! ```text
//! repeat_delay = 500 # ms before the first repeat; 0 switches key repeat off (default)
//! repeat_interval = 83 # ms between repeats (default)
//! modifier_profile = cupertino # or classic-pc
//! input_sources = us, de, fr(nodeadkeys) # xkb layout[(variant)], menu order; two or more enable the switcher;
//!   # unset, the layouts localed holds (X11Layout, X11Variant), read at login
//! caps_lock_delay = 75 # ms the caps-lock key must be held; 0 = off (default)
//! accent_picker = on # hold a letter key to pick an accented variant
//! accent_table = ~/.config/cupertino/accents.json # JSON of `cupertino_keyboard::AccentTable::from_json`
//! accent_keyboards = default, en
//! accent_disabled_apps = foot, alacritty
//! fn_key = change-input-source # tap action of the globe / fn key: do-nothing, emoji, dictation
//! caps_lock_switch = off # on: a caps-lock tap switches to the next input source, a hold turns caps lock on
//! caps_lock_hold = 500 # ms a caps-lock press must last to count as a hold, 1 to 1500 (default)
//! layout_keyboard = ansi # physical key arrangement of `decoded: input sources: ansi, iso, jis
//! layout_level3 = both-alt # keys that select the alternate symbols (level 3, with Shift level 4): both-alt
//!   # (default), right-alt, none (the layouts' own); a right Alt that is the compose key stays the compose key;
//!   # unset under classic-pc: right-alt for `decoded:` sources, the system layouts' own keys
//! compose_key = right-alt # the compose key of the system layouts: off, right-alt, right-ctrl, menu, caps-lock,
//!   # scroll-lock; unset keeps the compose option of localed's X11Options, which apply except its layout
//!   # switching keys (grp:*: the input sources switch the layouts); right-alt takes AltGr from layouts that use it
//! slow_keys = off # a key counts only once it is held for the delay
//! slow_keys_delay = 250 # ms (default)
//! sticky_keys = off # modifiers latch for the next key, a second press locks them
//! sticky_keys_shortcut = off # five Shift presses within 30 s toggle sticky keys
//! mouse_keys = off # the keypad moves the pointer
//! mouse_keys_shortcut = off # five Alt presses within 30 s toggle mouse keys
//! mouse_keys_delay = 1000 # ms before a held direction key repeats (default)
//! mouse_keys_max_speed = 11 # points per 50 ms tick (default)
//! ```
//!
//! The two shortcut toggles of the accessibility
//! filters default to off and `caps_lock_switch` too (approximate: the design defaults is not known). Under the
//! `cupertino` profile both Alt keys select the alternate symbols by default, as in the design (`classic-pc` keeps
//! Alt for the applications): applications no longer see Alt from them, while the
//! compositor's shortcuts and the client rewrite still count them as Alt. Under the `cupertino` profile the US
//! layout without a variant compiles as `us(mac)`, whose alternate symbols include the dead keys (Alt+E, then E
//! types é).

use std::path::PathBuf;

use cupertino_defaults::keyboard::{
    expand_home, ComposeKey, FnKey, KeyboardSettings as KeyboardConf, LayoutKeyboard, Level3,
};
use cupertino_input::{Action as InputAction, KeyCombo, Modifiers, ScreenshotRequest, ShortcutMap};
use cupertino_keyboard::keys::{Key, NamedKey};
use cupertino_keyboard::modifiers::FnUsage;
use cupertino_keyboard::modifiers::{Physical, Profile, RawMods};
use cupertino_keyboard::repeat::{CapsLockDelay, RepeatConfig, RepeatInfo, CAPS_HOLD_DEFAULT_MS};
use cupertino_keyboard::rewrite::ClientClass;
use cupertino_keyboard::shortcuts::{ShortcutSet, SystemAction};
use cupertino_keyboard::switcher::{Step, SwitchEvent, Switcher};
use cupertino_keyboard::xkb::{Keyboard, Level3Keys};
use cupertino_keyboard::{mouse_keys, slow_keys};
use smithay::input::keyboard::{keysyms, xkb, ModifiersState};

use crate::input::Mods;

pub use cupertino_defaults::keyboard::KEYBOARD_CONF_ENV as KEYBOARD_ENV;

pub use cupertino_defaults::keyboard::SourceSpec;

/// Accent picker settings (the candidate table is user data: nothing is embedded).
#[derive(Clone, Debug, PartialEq)]
pub struct AccentSettings {
    pub enabled: bool,
    pub table: Option<PathBuf>,
    pub keyboards: Vec<String>,
    pub disabled_apps: Vec<String>,
}

impl Default for AccentSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            table: None,
            keyboards: vec!["default".into()],
            disabled_apps: Vec::new(),
        }
    }
}

/// The accessibility keyboard filters.
#[derive(Clone, Debug, PartialEq)]
pub struct AccessSettings {
    pub slow_keys: bool,
    /// Acceptance delay of slow keys (default 250 ms).
    pub slow_keys_delay_ms: u32,
    pub sticky_keys: bool,
    /// Five Shift presses within 30 s toggle sticky keys (approximate default off).
    pub sticky_keys_shortcut: bool,
    pub mouse_keys: bool,
    /// Five Alt presses within 30 s toggle mouse keys (approximate default off).
    pub mouse_keys_shortcut: bool,
    /// Delay before a held direction key repeats (default 1 s).
    pub mouse_keys_delay_ms: u64,
    /// Points per tick at full speed (default 11).
    pub mouse_keys_max_speed: u32,
}

impl Default for AccessSettings {
    fn default() -> Self {
        Self {
            slow_keys: false,
            slow_keys_delay_ms: slow_keys::DEFAULT_DELAY_MS,
            sticky_keys: false,
            sticky_keys_shortcut: false,
            mouse_keys: false,
            mouse_keys_shortcut: false,
            mouse_keys_delay_ms: mouse_keys::DEFAULT_INITIAL_DELAY_MS,
            mouse_keys_max_speed: mouse_keys::DEFAULT_MAX_SPEED,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct KeyboardSettings {
    /// Initial delay and interval (defaults 500 / 83 ms).
    pub repeat: RepeatConfig,
    pub profile: String,
    pub sources: Vec<SourceSpec>,
    pub caps: CapsLockDelay,
    /// A caps-lock tap switches to the previous input source and a hold turns caps lock on.
    pub caps_switch: bool,
    /// How long a caps-lock press lasts to count as a hold (default 500 ms, accepted 1 to 1500).
    pub caps_hold_ms: u32,
    pub accent: AccentSettings,
    pub access: AccessSettings,
    /// Tap action of the globe / fn key; `None` follows the design default for the number of sources.
    pub fn_usage: Option<FnUsage>,
    /// Physical key arrangement of decoded layouts.
    pub layout_keyboard: Keyboard,
    /// Keys that select the alternate symbols; `None` follows the profile: both Alt keys under `cupertino`, and under
    /// `classic-pc` the right Alt of decoded layouts while the system layouts keep their own.
    pub layout_level3: Option<Level3Keys>,
    /// The compose key of the system layouts; `None` keeps the one the system's xkb options set.
    pub compose: Option<ComposeKey>,
    /// The layout localed holds: its layouts are the sources when `keyboard.conf` sets none, its options apply.
    pub system: crate::localed::SystemLayout,
    /// Rewrite Super and Alt chords for third-party clients under the `cupertino` profile
    /// (§10). On by default (approximate).
    pub client_rewrite: bool,
    /// Application ids of terminal emulators, matched exactly and case-insensitively: a Super chord on a letter becomes
    /// the Ctrl+Shift chord of the letter there (copy, paste, new window, new tab), and Super on a text size key its
    /// Ctrl chord (seed list recalled from the upstream application ids, approximate).
    pub rewrite_terminals: Vec<String>,
    /// Application ids that never get a rewrite, matched like `rewrite_terminals`.
    pub rewrite_off: Vec<String>,
}

/// Application ids of the terminal emulators the rewrite knows (approximate, §10.5).
/// `cupertino-terminal` is not listed: a client of the desktop's toolkit is first-party and never rewritten.
const DEFAULT_REWRITE_TERMINALS: [&str; 8] = [
    "foot",
    "kitty",
    "Alacritty",
    "org.wezfurlong.wezterm",
    "com.mitchellh.ghostty",
    "org.gnome.Terminal",
    "org.kde.konsole",
    "xterm",
];

/// Application ids of the known terminals with no tabs and no chord that closes one, matched like the terminals: the
/// command key's W closes their window ([`KeyboardSettings::close_goes_to_app`]).
const TABLESS_TERMINALS: [&str; 3] = ["foot", "Alacritty", "xterm"];

impl Default for KeyboardSettings {
    fn default() -> Self {
        Self {
            repeat: RepeatConfig::default(),
            profile: "cupertino".into(),
            sources: Vec::new(),
            caps: CapsLockDelay::default(),
            caps_switch: false,
            caps_hold_ms: CAPS_HOLD_DEFAULT_MS,
            accent: AccentSettings::default(),
            access: AccessSettings::default(),
            fn_usage: None,
            layout_keyboard: Keyboard::Ansi,
            layout_level3: None,
            compose: None,
            system: crate::localed::SystemLayout::default(),
            client_rewrite: true,
            rewrite_terminals: DEFAULT_REWRITE_TERMINALS
                .iter()
                .map(|id| (*id).to_string())
                .collect(),
            rewrite_off: Vec::new(),
        }
    }
}

impl KeyboardSettings {
    pub fn path() -> Option<PathBuf> {
        KeyboardConf::path()
    }

    /// Defaults with `keyboard.conf` applied, and what the file held; the problems of its lines are logged and their
    /// keys keep the defaults.
    pub fn load() -> (Self, KeyboardConf) {
        let mut s = Self::default();
        let mut file = KeyboardConf::default();
        if let Some(path) = Self::path() {
            match KeyboardConf::load(&path, None) {
                Ok((read, problems)) => {
                    for p in problems {
                        tracing::warn!(path = %path.display(), line = p.line, error = %p.message, "ignoring invalid keyboard setting");
                    }
                    s.apply_file(&read);
                    file = read;
                }
                Err(e) => {
                    tracing::warn!(path = %path.display(), error = %e, "cannot read the keyboard settings: the defaults apply")
                }
            }
        }
        (s, file)
    }

    /// Apply the keys `file` sets on top of these settings.
    pub fn apply_file(&mut self, file: &KeyboardConf) {
        let f = file;
        if let Some(v) = f.repeat_delay_ms {
            self.repeat.initial_delay_ms = v;
        }
        if let Some(v) = f.repeat_interval_ms {
            self.repeat.interval_ms = v;
        }
        if let Some(p) = f.modifier_profile {
            self.profile = p.name().to_string();
        }
        if let Some(v) = &f.input_sources {
            self.sources = v.clone();
        }
        if let Some(v) = f.caps_lock_delay_ms {
            self.caps.delay_ms = v;
        }
        if let Some(v) = f.caps_lock_switch {
            self.caps_switch = v;
        }
        if let Some(v) = f.caps_lock_hold_ms {
            self.caps_hold_ms = v;
        }
        if let Some(k) = f.fn_key {
            self.fn_usage = Some(match k {
                FnKey::DoNothing => FnUsage::DoNothing,
                FnKey::ChangeInputSource => FnUsage::ChangeInputSource,
                FnKey::Emoji => FnUsage::ShowEmojiPicker,
                FnKey::Dictation => FnUsage::StartDictation,
            });
        }
        if let Some(k) = f.layout_keyboard {
            self.layout_keyboard = match k {
                LayoutKeyboard::Ansi => Keyboard::Ansi,
                LayoutKeyboard::Iso => Keyboard::Iso,
                LayoutKeyboard::Jis => Keyboard::Jis,
            };
        }
        if let Some(k) = f.compose_key {
            self.compose = Some(k);
        }
        if let Some(l) = f.layout_level3 {
            self.layout_level3 = Some(match l {
                Level3::BothAlt => Level3Keys::BothAlt,
                Level3::RightAlt => Level3Keys::RightAlt,
                Level3::None => Level3Keys::None,
            });
        }
        let a = &mut self.access;
        for (set, field) in [
            (f.slow_keys, &mut a.slow_keys),
            (f.sticky_keys, &mut a.sticky_keys),
            (f.sticky_keys_shortcut, &mut a.sticky_keys_shortcut),
            (f.mouse_keys, &mut a.mouse_keys),
            (f.mouse_keys_shortcut, &mut a.mouse_keys_shortcut),
        ] {
            if let Some(v) = set {
                *field = v;
            }
        }
        if let Some(v) = f.slow_keys_delay_ms {
            a.slow_keys_delay_ms = v;
        }
        if let Some(v) = f.mouse_keys_delay_ms {
            a.mouse_keys_delay_ms = u64::from(v);
        }
        if let Some(v) = f.mouse_keys_max_speed {
            a.mouse_keys_max_speed = v;
        }
        if let Some(v) = f.client_rewrite {
            self.client_rewrite = v;
        }
        if let Some(v) = &f.rewrite_terminals {
            self.rewrite_terminals = v.clone();
        }
        if let Some(v) = &f.rewrite_off {
            self.rewrite_off = v.clone();
        }
        if let Some(v) = f.accent_picker {
            self.accent.enabled = v;
        }
        if let Some(t) = &f.accent_table {
            self.accent.table = Some(expand_home(t));
        }
        if let Some(v) = &f.accent_keyboards {
            self.accent.keyboards = v.clone();
        }
        if let Some(v) = &f.accent_disabled_apps {
            self.accent.disabled_apps = v.clone();
        }
    }

    /// The key repeat in force: the configured one, or 420 ms delay and interval while slow keys are on (the filter
    /// substitutes them whenever repeat is enabled).
    pub fn effective_repeat(&self) -> RepeatConfig {
        if self.access.slow_keys && self.repeat.is_enabled() {
            RepeatConfig::slow_keys()
        } else {
            self.repeat
        }
    }

    /// `wl_keyboard.repeat_info` payload.
    pub fn repeat_info(&self) -> RepeatInfo {
        self.effective_repeat().repeat_info()
    }

    /// The hold duration of caps-lock hold-to-switch, `None` when it is off. It needs two or more input sources
    /// (the design offers the option only then); with fewer it is reported and left off.
    pub fn caps_switch_hold(&self) -> Option<u32> {
        if !self.caps_switch {
            return None;
        }
        if self.sources.len() < 2 {
            tracing::warn!("caps_lock_switch needs two or more input_sources; caps lock stays an ordinary caps lock");
            return None;
        }
        Some(self.caps_hold_ms)
    }

    /// Tap action of the globe / fn key: the configured one, else change input source with two or more sources and
    /// nothing otherwise (default).
    pub fn fn_action(&self) -> FnUsage {
        self.fn_usage
            .unwrap_or_else(|| FnUsage::default_for(self.sources.len()))
    }

    /// The class of the client with application id `app` for the chord rewrite: first-party when it uses the
    /// desktop's toolkit (`kit`, it bound the toolkit's protocol, whatever its id), else one of the configured terminals
    /// or everything else.
    pub fn client_class(&self, app: &str, kit: bool) -> ClientClass {
        if kit {
            ClientClass::FirstParty
        } else if self.lists(&self.rewrite_terminals, app) {
            ClientClass::Terminal
        } else {
            ClientClass::Other
        }
    }

    /// Whether the command key's W reaches application `app` of class `class` as the chord it closes its front tab or
    /// document with, instead of closing its window: the rewrite gives a third-party application the Ctrl chord (a
    /// terminal the Ctrl+Shift one) of every command-key letter, and that of W closes a tab in a browser, a document in
    /// an editor and a tab in a terminal that has them. Not for a first-party application, which reads the command key
    /// itself and closes what is in front when asked to close, nor where the rewrite is off (the application would get
    /// a Super chord it ignores), nor for a terminal without tabs, whose Ctrl+Shift+W is a control character for the
    /// shell: the compositor closes those windows.
    pub fn close_goes_to_app(&self, app: &str, class: ClientClass) -> bool {
        let rewritten =
            self.modifier_profile().primary() == Physical::Super && !self.rewrite_opted_out(app);
        rewritten
            && match class {
                ClientClass::FirstParty => false,
                ClientClass::Terminal => !TABLESS_TERMINALS
                    .iter()
                    .any(|id| id.eq_ignore_ascii_case(app)),
                ClientClass::Other => true,
            }
    }

    /// True when the chord rewrite is off for the whole compositor or for application id `app`.
    pub fn rewrite_opted_out(&self, app: &str) -> bool {
        !self.client_rewrite || self.lists(&self.rewrite_off, app)
    }

    fn lists(&self, ids: &[String], app: &str) -> bool {
        ids.iter().any(|id| id.eq_ignore_ascii_case(app))
    }

    pub fn modifier_profile(&self) -> Profile {
        Profile::by_name(&self.profile).unwrap_or_else(Profile::cupertino)
    }

    /// Logical modifiers (`logo` = the primary modifier of the profile) of a physical modifier state.
    pub fn translate(&self, m: &ModifiersState) -> Mods {
        let mut raw = 0u8;
        for (on, bit) in [
            (m.shift, RawMods::SHIFT),
            (m.ctrl, RawMods::CTRL),
            (m.alt || m.iso_level3_shift, RawMods::ALT),
            (m.logo, RawMods::SUPER),
        ] {
            if on {
                raw |= bit.0;
            }
        }
        let l = self.modifier_profile().translate(RawMods(raw));
        use cupertino_keyboard::modifiers::Mods as L;
        Mods {
            ctrl: l.contains(L::CTRL),
            alt: l.contains(L::ALT),
            shift: l.contains(L::SHIFT),
            logo: l.contains(L::PRIMARY),
        }
    }

    /// The physical modifier keys of a keyboard state: the keys the system shortcuts are matched on (Super is `logo`).
    /// The level-3 keys count as Alt: with `layout_level3` they set that modifier instead of Alt's.
    pub fn physical(m: &ModifiersState) -> Mods {
        Mods {
            ctrl: m.ctrl,
            alt: m.alt || m.iso_level3_shift,
            shift: m.shift,
            logo: m.logo,
        }
    }

    /// Whether the Alt of [`Self::physical`] comes from a level-3 key alone that is AltGr rather than the Option key:
    /// the right Alt of `layout_level3 = right-alt`, or a layout's own level-3 key under `none` (and `classic-pc`'s
    /// default). The applications' shortcuts do not take it for Alt, so AltGr and a key types its symbol in the focused
    /// app. With `both-alt` both Alt keys are the Option key, and count as Alt.
    pub fn alt_gr(&self, m: &ModifiersState) -> bool {
        m.iso_level3_shift && !m.alt && self.decoded_level3() != Level3Keys::BothAlt
    }

    /// Take the system's layouts as the input sources when there are none.
    pub fn use_system_layout(&mut self) {
        if self.sources.is_empty() {
            self.sources = self.system.sources();
        }
    }

    /// The comma separated xkb options of the system layouts: the system's, with `compose` in place of its compose
    /// option when set, and without its layout switching keys (`grp:`), since the input sources switch the layouts
    /// (a switch inside xkb would leave the menu, the HUD and the per-application source behind). With system layouts
    /// among the sources, the level-3 keys of `layout_level3` replace the system's (`lv3:`), except a right Alt that is
    /// the compose key.
    pub fn xkb_options(&self) -> String {
        let system = || self.system.options.split(',').map(str::trim);
        let compose_on_ralt = match self.compose {
            Some(key) => matches!(key, ComposeKey::RightAlt),
            None => system().any(|option| option == "compose:ralt"),
        };
        let level3 = self
            .level3_option(compose_on_ralt)
            .filter(|_| !self.xkb_names().0.is_empty());
        let compose = self.compose.map(|key| match key {
            ComposeKey::Off => None,
            ComposeKey::RightAlt => Some("compose:ralt"),
            ComposeKey::RightCtrl => Some("compose:rctrl"),
            ComposeKey::Menu => Some("compose:menu"),
            ComposeKey::CapsLock => Some("compose:caps"),
            ComposeKey::ScrollLock => Some("compose:sclk"),
        });
        system()
            .filter(|option| {
                !option.is_empty()
                    && !option.starts_with("grp:")
                    && !(compose.is_some() && option.starts_with("compose:"))
                    && !(level3.is_some() && option.starts_with("lv3:"))
            })
            .chain(level3)
            .chain(compose.flatten())
            .collect::<Vec<_>>()
            .join(",")
    }

    fn system_sources(&self) -> impl Iterator<Item = &SourceSpec> {
        self.sources.iter().filter(|s| s.decoded.is_none())
    }

    /// The xkb variant a system source compiles with. Under the `cupertino` profile the US layout without a variant
    /// is `us(mac)`, which has the alternate symbols and dead keys of the level-3 keys (`layout_level3`).
    fn xkb_variant<'a>(&self, source: &'a SourceSpec) -> &'a str {
        if self.profile != "classic-pc" && source.layout == "us" && source.variant.is_empty() {
            "mac"
        } else {
            &source.variant
        }
    }

    /// The keys that select the alternate symbols of decoded layouts.
    pub fn decoded_level3(&self) -> Level3Keys {
        self.layout_level3
            .unwrap_or(if self.profile == "classic-pc" {
                Level3Keys::RightAlt
            } else {
                Level3Keys::BothAlt
            })
    }

    /// The xkb option of `layout_level3` for the system layouts. A right Alt that is the compose key stays one: xkb
    /// applies `lv3:` options over `compose:` ones whatever their order.
    fn level3_option(&self, compose_on_ralt: bool) -> Option<&'static str> {
        let keys = match self.layout_level3 {
            Some(keys) => keys,
            None if self.profile == "classic-pc" => return None,
            None => Level3Keys::BothAlt,
        };
        match (keys, compose_on_ralt) {
            (Level3Keys::BothAlt, false) => Some("lv3:alt_switch"),
            (Level3Keys::BothAlt, true) => Some("lv3:lalt_switch"),
            (Level3Keys::RightAlt, false) => Some("lv3:ralt_switch"),
            (Level3Keys::RightAlt, true) | (Level3Keys::None, _) => None,
        }
    }

    /// Comma separated xkb layout and variant lists for `XkbConfig` of the system layouts among the sources. Without
    /// system layouts (no `input_sources` and an unset system layout) the keymap is US under the `cupertino` profile,
    /// with its alternate symbols as for a US source, and xkb's default (empty names) under `classic-pc`.
    pub fn xkb_names(&self) -> (String, String) {
        let system: Vec<&SourceSpec> = self.system_sources().collect();
        if system.is_empty() && self.profile != "classic-pc" {
            return ("us".into(), "mac".into());
        }
        let layouts = system
            .iter()
            .map(|s| s.layout.as_str())
            .collect::<Vec<_>>()
            .join(",");
        let variants: Vec<&str> = system.iter().map(|s| self.xkb_variant(s)).collect();
        (
            layouts,
            if variants.iter().all(|v| v.is_empty()) {
                String::new()
            } else {
                variants.join(",")
            },
        )
    }
}

// ---- shortcut table merge ----------------------------------------------------------------------

fn named_keysym(k: NamedKey) -> Option<u32> {
    Some(match k {
        NamedKey::Up => keysyms::KEY_Up,
        NamedKey::Down => keysyms::KEY_Down,
        NamedKey::Left => keysyms::KEY_Left,
        NamedKey::Right => keysyms::KEY_Right,
        NamedKey::Home => keysyms::KEY_Home,
        NamedKey::End => keysyms::KEY_End,
        NamedKey::PageUp => keysyms::KEY_Page_Up,
        NamedKey::PageDown => keysyms::KEY_Page_Down,
        NamedKey::Backspace => keysyms::KEY_BackSpace,
        NamedKey::Delete => keysyms::KEY_Delete,
        NamedKey::Return => keysyms::KEY_Return,
        NamedKey::KpEnter => keysyms::KEY_KP_Enter,
        NamedKey::Tab => keysyms::KEY_Tab,
        NamedKey::Escape => keysyms::KEY_Escape,
        NamedKey::Space => keysyms::KEY_space,
        NamedKey::Clear => keysyms::KEY_Clear,
        NamedKey::F(n) if (1..=35).contains(&n) => keysyms::KEY_F1 + u32::from(n) - 1,
        _ => return None,
    })
}

/// Keysym of a table key.
pub fn key_to_keysym(k: Key) -> Option<u32> {
    match k {
        Key::Named(n) => named_keysym(n),
        Key::Char(c) => {
            let s = xkb::utf32_to_keysym(c as u32).raw();
            (s != keysyms::KEY_NoSymbol).then_some(s)
        }
    }
}

/// The action of the shortcut map that stands for a table action, `None` without a counterpart.
pub fn system_action_to_input(a: SystemAction) -> Option<InputAction> {
    use SystemAction as S;
    Some(match a {
        S::AppSwitcherNext => InputAction::SwitchApp,
        S::AppSwitcherPrevious => InputAction::SwitchAppReverse,
        S::WindowCycleNext => InputAction::CycleWindows,
        S::WindowCyclePrevious => InputAction::CycleWindowsReverse,
        // the five hot keys the design registers; the clipboard form of the toolbar chord is in the table
        // but not registered there
        S::ScreenshotScreenToFile => InputAction::Screenshot(ScreenshotRequest::Screen),
        S::ScreenshotScreenToClipboard => {
            InputAction::Screenshot(ScreenshotRequest::ScreenToClipboard)
        }
        S::ScreenshotSelectionToFile => InputAction::Screenshot(ScreenshotRequest::Selection),
        S::ScreenshotSelectionToClipboard => {
            InputAction::Screenshot(ScreenshotRequest::SelectionToClipboard)
        }
        S::ScreenshotOptions => InputAction::Screenshot(ScreenshotRequest::Toolbar),
        S::OverviewAllWindows => InputAction::Overview,
        S::OverviewAppWindows => InputAction::AppOverview,
        S::ShowDesktop => InputAction::ShowDesktop,
        S::WorkspacePrevious => InputAction::WorkspacePrevious,
        S::WorkspaceNext => InputAction::WorkspaceNext,
        S::LauncherToggle | S::LauncherFileSearch => InputAction::Launcher,
        S::InputSourcePrevious => InputAction::InputSourcePrevious,
        S::InputSourceNext => InputAction::InputSourceNext,
        S::FullscreenToggle => InputAction::ToggleFullscreen,
        S::WindowMinimize => InputAction::MinimizeWindow,
        S::DockAutoHideToggle => InputAction::ToggleDockAutoHide,
        S::FocusMenuBar => InputAction::FocusMenuBar,
        S::HelpSearch => InputAction::HelpSearch,
        S::FocusDock => InputAction::FocusDock,
        S::WorkspaceGoTo1 => InputAction::Workspace(1),
        S::WorkspaceGoTo2 => InputAction::Workspace(2),
        S::WorkspaceGoTo3 => InputAction::Workspace(3),
        S::WorkspaceGoTo4 => InputAction::Workspace(4),
        S::WorkspaceGoTo5 => InputAction::Workspace(5),
        S::WorkspaceGoTo6 => InputAction::Workspace(6),
        S::WorkspaceGoTo7 => InputAction::Workspace(7),
        S::WorkspaceGoTo8 => InputAction::Workspace(8),
        S::WorkspaceGoTo9 => InputAction::Workspace(9),
        _ => return None,
    })
}

/// Result of [`merge_reference_shortcuts`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MergeReport {
    /// Rows added to the map: chord text and action name.
    pub added: Vec<(String, String)>,
    /// Rows whose chord and action were already in the map.
    pub identical: usize,
    /// Rows dropped because the chord is bound to a different action: chord, table action, kept action.
    pub conflicts: Vec<(String, String, String)>,
    /// Enabled rows without a counterpart in this compositor.
    pub unsupported: Vec<SystemAction>,
}

/// Add the enabled rows of `set` to `map` (see the module documentation for the precedence rule).
pub fn merge_reference_shortcuts(map: &mut ShortcutMap, set: &ShortcutSet) -> MergeReport {
    let mut report = MergeReport::default();
    for a in SystemAction::ALL {
        if !set.is_enabled(*a) {
            continue;
        }
        let Some(chord) = set.chord_for(*a) else {
            continue;
        };
        let Some(action) = system_action_to_input(*a) else {
            report.unsupported.push(*a);
            continue;
        };
        let Some(sym) = key_to_keysym(chord.key) else {
            report.unsupported.push(*a);
            continue;
        };
        use cupertino_keyboard::modifiers::Mods as L;
        // the table's logical primary modifier is Super in this compositor (`Profile` translates the rest)
        let mods = Modifiers::new(
            chord.mods.contains(L::SHIFT),
            chord.mods.contains(L::CTRL),
            chord.mods.contains(L::ALT),
            chord.mods.contains(L::PRIMARY),
        );
        let combo = KeyCombo::new(mods, sym);
        match map.lookup(combo.modifiers, combo.keysym) {
            Some(existing) if *existing == action => report.identical += 1,
            Some(existing) => {
                report
                    .conflicts
                    .push((combo.to_string(), action.name(), existing.name()))
            }
            None => {
                map.bind(combo, action.clone());
                report.added.push((combo.to_string(), action.name()));
            }
        }
    }
    report
}

/// Default shortcut map of the modifier profile `profile`: under `cupertino` the `cupertino-input` defaults plus the free
/// rows of the design table (its chords are written for the Super key as the command key), under `classic-pc` its own
/// table alone.
pub fn default_shortcut_map(profile: &str) -> (ShortcutMap, MergeReport) {
    let mut map = ShortcutMap::defaults_for(profile);
    if profile == "classic-pc" {
        return (map, MergeReport::default());
    }
    let report = merge_reference_shortcuts(&mut map, &ShortcutSet::reference_defaults());
    (map, report)
}

// ---- caps lock debounce ------------------------------------------------------------------------

/// What to do with a caps-lock press or release.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapsStep {
    /// Pass the event on to the keyboard state.
    Deliver,
    /// Hold the event back (the press is delivered later by [`CapsDebounce::due`] if the key stays down).
    Hold,
    /// Drop the event (a press that was released before the delay, and that release).
    Drop,
    /// Drop the event and switch to the previous input source (a tap with `caps_lock_switch`).
    SwitchSource,
}

/// Caps-lock activation delay: the press reaches the keyboard state only when the key is
/// still down after `delay_ms` (default 75 ms), so brushing the key does not toggle the lock.
///
/// With hold-to-switch the hold duration replaces the delay: a press released earlier is a tap that
/// switches the input source, a press held that long turns caps lock on (default 500 ms).
#[derive(Clone, Debug)]
pub struct CapsDebounce {
    pub config: CapsLockDelay,
    /// Hold duration of hold-to-switch, `None` when caps lock does not switch sources.
    pub switch_hold_ms: Option<u32>,
    /// Press time (ms) and whether the press was delivered already.
    pending: Option<(u64, bool)>,
}

impl CapsDebounce {
    pub fn new(config: CapsLockDelay, switch_hold_ms: Option<u32>) -> Self {
        Self {
            config,
            switch_hold_ms,
            pending: None,
        }
    }

    /// Whether the debounce holds presses back at all.
    pub fn active(&self) -> bool {
        self.switch_hold_ms.is_some()
            || self.config.delay_ms > 0
            || self.config.override_ms.is_some_and(|v| v > 0)
    }

    /// The press has lasted long enough to count.
    fn counts(&self, held_ms: u64) -> bool {
        match self.switch_hold_ms {
            Some(hold) => held_ms >= u64::from(hold),
            None => self
                .config
                .press_counts(held_ms.min(u64::from(u32::MAX)) as u32),
        }
    }

    pub fn press(&mut self, now_ms: u64) -> CapsStep {
        if !self.active() {
            return CapsStep::Deliver;
        }
        self.pending = Some((now_ms, false));
        CapsStep::Hold
    }

    pub fn release(&mut self, now_ms: u64) -> CapsStep {
        match self.pending.take() {
            None => CapsStep::Deliver,
            Some((_, true)) => CapsStep::Deliver,
            // released at the exact moment of the delay counts as held (`press_counts`)
            Some((t, false)) => {
                if self.counts(now_ms.saturating_sub(t)) {
                    CapsStep::Deliver
                } else if self.switch_hold_ms.is_some() {
                    CapsStep::SwitchSource
                } else {
                    CapsStep::Drop
                }
            }
        }
    }

    /// A held-back press whose delay has elapsed: returns its press time to deliver it now.
    pub fn due(&mut self, now_ms: u64) -> Option<u64> {
        let (t, delivered) = self.pending?;
        if delivered || !self.counts(now_ms.saturating_sub(t)) {
            return None;
        }
        self.pending = Some((t, true));
        Some(t)
    }

    /// Forget a held-back press, so its release is delivered: for a caller that delivered the
    /// press itself.
    pub fn cancel(&mut self) {
        self.pending = None;
    }

    pub fn delay_ms(&self) -> u64 {
        u64::from(self.switch_hold_ms.unwrap_or_else(|| {
            self.config
                .override_ms
                .filter(|v| *v > 0)
                .unwrap_or(self.config.delay_ms)
        }))
    }
}

// ---- input sources and the switch under way ----------------------------------------------------

/// A switch under way: what the switcher's panel, which the on-screen displays draw, is sent.
#[derive(Clone, Debug, PartialEq)]
pub struct OpenSwitch {
    /// Source ids, historical order (most recently used first).
    pub order: Vec<String>,
    /// The name each source is shown with, in the same order.
    pub labels: Vec<String>,
    pub selection: usize,
}

/// The generated keymap of a decoded layout and its display name.
#[derive(Clone, Debug, PartialEq)]
pub struct DecodedSource {
    pub keymap: String,
    pub label: String,
}

/// Generate the keymap of a `decoded:` source (`cupertino-keyboard::xkb`, validated on the 232 decoded layouts).
pub fn generate_decoded(
    path: &std::path::Path,
    keyboard: Keyboard,
    level3: Level3Keys,
) -> Result<DecodedSource, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let layout = cupertino_keyboard::Layout::from_json(&text)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let out = cupertino_keyboard::generate(
        &layout,
        &cupertino_keyboard::GenOptions {
            keyboard,
            set_index: 0,
            level3,
        },
    )
    .map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(DecodedSource {
        keymap: out.keymap,
        label: layout.name,
    })
}

/// Input-source state: the MRU switcher plus the switch under way.
#[derive(Clone, Debug, Default)]
pub struct InputSources {
    pub specs: Vec<SourceSpec>,
    pub switcher: Switcher,
    /// The switch under way, from its first step to its commit or cancel.
    pub open: Option<OpenSwitch>,
    /// Generated keymaps of the `decoded:` sources that loaded, by source id.
    pub decoded: std::collections::HashMap<String, DecodedSource>,
    /// The seat keymap is a decoded one (the system keymap has to come back for a system source).
    pub decoded_active: bool,
}

impl InputSources {
    pub fn new(specs: Vec<SourceSpec>) -> Self {
        let mut s = Self {
            specs,
            ..Self::default()
        };
        let ids: Vec<String> = s.specs.iter().map(|x| x.id.clone()).collect();
        s.switcher.set_sources(ids.clone());
        if let Some(first) = ids.first() {
            s.switcher.set_current(first);
        }
        s
    }

    pub fn enabled(&self) -> bool {
        self.specs.len() >= 2
    }

    /// Index of a system source in the xkb layout list (`None` for a decoded source).
    pub fn group_of(&self, id: &str) -> Option<usize> {
        self.specs
            .iter()
            .filter(|s| s.decoded.is_none())
            .position(|s| s.id == id)
    }

    pub fn current(&self) -> Option<&str> {
        self.switcher.current()
    }

    /// Run a switcher event. Returns the source to activate now, if any. `label` names a source for the panel.
    pub fn apply(&mut self, ev: SwitchEvent, label: impl Fn(&str) -> String) -> Option<String> {
        match ev {
            SwitchEvent::None => None,
            SwitchEvent::ShowHud { order, selection } => {
                let labels = order.iter().map(|id| label(id)).collect();
                self.open = Some(OpenSwitch {
                    order,
                    labels,
                    selection,
                });
                None
            }
            SwitchEvent::MoveSelection { selection } => {
                if let Some(open) = self.open.as_mut() {
                    open.selection = selection;
                }
                None
            }
            SwitchEvent::Commit { id } => {
                self.open = None;
                Some(id)
            }
            SwitchEvent::Cancel => {
                self.open = None;
                None
            }
        }
    }

    /// Whether a switch is under way: its keys are the switch's, and its panel shows.
    pub fn switch_open(&self) -> bool {
        self.open.is_some()
    }

    pub fn press(&mut self, menu: bool, step: Step, held: bool) -> SwitchEvent {
        if menu {
            self.switcher.press_menu(step, held)
        } else {
            self.switcher.press(step, held)
        }
    }
}
