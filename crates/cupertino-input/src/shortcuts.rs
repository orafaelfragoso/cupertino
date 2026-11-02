//! Configurable shortcut map: key combinations → compositor [`Action`]s.
//!
//! Combination strings look like `Super+Tab`, `Ctrl+Alt+Left`, `Super+Shift+grave` or the GTK style
//! `<Super>Tab`; the key is an xkbcommon keysym name (see [`crate::keysym`]). The modifiers are the physical keys
//! (`Logo`, `Win` and `Mod4` are other names of `Super`); `Primary` is the command key of the profile, which is Super
//! under `cupertino` and Ctrl under `classic-pc`.
//!
//! A shortcut file may have sections, `[cupertino]` and `[classic-pc]`: the lines before the first section apply to
//! both profiles, the lines of a section only to its profile.
//!
//! Matching is exact on the four modifiers (lock modifiers are not part of [`Modifiers`]).
//! `ISO_Left_Tab` (what Shift+Tab produces) is folded into `Shift+Tab`, and ASCII letters are
//! matched case-insensitively. Feed [`ShortcutMap::lookup`] the keysym of the key at its base
//! (unshifted) level so that `Super+Shift+3` matches regardless of the layout's shifted symbol.

use std::collections::HashMap;
use std::fmt;
use std::str::FromStr;

use cupertino_modifiers::{Modifier, Physical, Profile};

use crate::geom::Modifiers;
use crate::keysym;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    // windows and apps
    CloseWindow,
    MinimizeWindow,
    /// Restore a tiled, filled or zoomed window, else minimize it (the Windows habit of Super+Down).
    MinimizeOrRestoreWindow,
    ZoomWindow,
    ToggleFullscreen,
    HideApp,
    HideOtherApps,
    QuitApp,
    SwitchApp,
    SwitchAppReverse,
    CycleWindows,
    CycleWindowsReverse,
    TileLeft,
    TileRight,
    TileFill,
    MoveWindowToNextOutput,
    // overview
    Overview,
    AppOverview,
    ShowDesktop,
    AppGrid,
    ForceQuit,
    // workspaces (1-based indices)
    WorkspacePrevious,
    WorkspaceNext,
    Workspace(u32),
    MoveWindowToWorkspacePrevious,
    MoveWindowToWorkspaceNext,
    MoveWindowToWorkspace(u32),
    // launcher and shell
    Launcher,
    ControlPanel,
    NotificationCenter,
    FocusMenuBar,
    /// Open the Help menu of the menu bar with its search field focused.
    HelpSearch,
    FocusDock,
    ToggleDockAutoHide,
    Lock,
    /// Switch the session between the light and the dark appearance (no default chord).
    ToggleAppearance,
    /// Turn the stage on or off (the window manager process decides; no default chord).
    ToggleStage,
    /// The power key: its press and release go to the login process, which times the hold.
    PowerKey,
    /// Ask the login process whether to log out (confirmation panel, countdown, applications quit first).
    Logout,
    /// Open the power panel of the login process: Restart, Sleep, Cancel, Shut Down.
    PowerDialog,
    /// Give the keys and the pointer back to the system while an application holds them (see `is_reserved` in the
    /// server): the one chord an application cannot take away.
    ReleaseShortcuts,
    /// Step to the previously used input source (HUD while the modifiers are held).
    InputSourcePrevious,
    /// Step to the next input source in menu order.
    InputSourceNext,
    /// Start the screenshot process (the design's five screenshot hot keys).
    Screenshot(ScreenshotRequest),
    Exec(String),
}

/// What a screenshot shortcut asks for: the design spawns its capture tool with one flag string per hot key
/// the whole screen or an interactive selection, each to a file or to the clipboard, or the
/// toolbar, whose options choose the destination.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ScreenshotRequest {
    /// Every output at once, to files.
    Screen,
    ScreenToClipboard,
    /// The crosshair: drag a rectangle (Space switches to picking a window), to a file.
    Selection,
    SelectionToClipboard,
    /// The toolbar with the capture modes and the options.
    Toolbar,
}

impl ScreenshotRequest {
    pub const ALL: [ScreenshotRequest; 5] = [
        Self::Screen,
        Self::ScreenToClipboard,
        Self::Selection,
        Self::SelectionToClipboard,
        Self::Toolbar,
    ];

    /// The action name in shortcut files.
    pub fn name(self) -> &'static str {
        match self {
            Self::Screen => "screenshot",
            Self::ScreenToClipboard => "screenshot-clipboard",
            Self::Selection => "screenshot-area",
            Self::SelectionToClipboard => "screenshot-area-clipboard",
            Self::Toolbar => "screenshot-toolbar",
        }
    }
}

impl Action {
    pub fn name(&self) -> String {
        match self {
            Action::CloseWindow => "close-window".into(),
            Action::MinimizeWindow => "minimize-window".into(),
            Action::MinimizeOrRestoreWindow => "minimize-or-restore-window".into(),
            Action::ZoomWindow => "zoom-window".into(),
            Action::ToggleFullscreen => "toggle-fullscreen".into(),
            Action::HideApp => "hide-app".into(),
            Action::HideOtherApps => "hide-other-apps".into(),
            Action::QuitApp => "quit-app".into(),
            Action::SwitchApp => "switch-app".into(),
            Action::SwitchAppReverse => "switch-app-reverse".into(),
            Action::CycleWindows => "cycle-windows".into(),
            Action::CycleWindowsReverse => "cycle-windows-reverse".into(),
            Action::TileLeft => "tile-left".into(),
            Action::TileRight => "tile-right".into(),
            Action::TileFill => "tile-fill".into(),
            Action::MoveWindowToNextOutput => "move-window-to-next-output".into(),
            Action::Overview => "overview".into(),
            Action::AppOverview => "app-overview".into(),
            Action::ShowDesktop => "show-desktop".into(),
            Action::AppGrid => "app-grid".into(),
            Action::ForceQuit => "force-quit".into(),
            Action::WorkspacePrevious => "workspace-previous".into(),
            Action::WorkspaceNext => "workspace-next".into(),
            Action::Workspace(n) => format!("workspace:{n}"),
            Action::MoveWindowToWorkspacePrevious => "move-window-to-workspace-previous".into(),
            Action::MoveWindowToWorkspaceNext => "move-window-to-workspace-next".into(),
            Action::MoveWindowToWorkspace(n) => format!("move-window-to-workspace:{n}"),
            Action::Launcher => "launcher".into(),
            Action::ControlPanel => "control-panel".into(),
            Action::NotificationCenter => "notification-center".into(),
            Action::FocusMenuBar => "focus-menu-bar".into(),
            Action::HelpSearch => "help-search".into(),
            Action::FocusDock => "focus-dock".into(),
            Action::ToggleDockAutoHide => "toggle-dock-auto-hide".into(),
            Action::Lock => "lock".into(),
            Action::ToggleAppearance => "toggle-appearance".into(),
            Action::ToggleStage => "toggle-stage".into(),
            Action::PowerKey => "power-key".into(),
            Action::Logout => "logout".into(),
            Action::PowerDialog => "power-dialog".into(),
            Action::ReleaseShortcuts => "release-shortcuts".into(),
            Action::InputSourcePrevious => "input-source-previous".into(),
            Action::InputSourceNext => "input-source-next".into(),
            Action::Screenshot(r) => r.name().into(),
            Action::Exec(cmd) => format!("exec:{cmd}"),
        }
    }
}

impl fmt::Display for Action {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParseError {
    Empty,
    UnknownModifier(String),
    UnknownKey(String),
    MissingKey,
    UnknownAction(String),
    BadNumber(String),
    MissingSeparator(String),
    UnknownSection(String),
    /// A modifier alone names a tap, and only the Super key can be tapped.
    TapNotSuper(String),
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseError::Empty => write!(f, "empty input"),
            ParseError::UnknownModifier(m) => write!(f, "unknown modifier `{m}`"),
            ParseError::UnknownKey(k) => write!(f, "unknown key `{k}`"),
            ParseError::MissingKey => write!(f, "no key in combination"),
            ParseError::UnknownAction(a) => write!(f, "unknown action `{a}`"),
            ParseError::BadNumber(n) => write!(f, "bad number `{n}`"),
            ParseError::UnknownSection(s) => write!(f, "unknown section `[{s}]`"),
            ParseError::TapNotSuper(m) => {
                write!(f, "`{m}` alone: only the Super key can be tapped")
            }
            ParseError::MissingSeparator(l) => {
                write!(f, "expected `combination = action` in `{l}`")
            }
        }
    }
}

impl std::error::Error for ParseError {}

impl FromStr for Action {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, ParseError> {
        let s = s.trim();
        if s.is_empty() {
            return Err(ParseError::Empty);
        }
        if let Some(cmd) = s.strip_prefix("exec:") {
            return Ok(Action::Exec(cmd.trim().to_string()));
        }
        let num = |n: &str| {
            n.trim()
                .parse::<u32>()
                .map_err(|_| ParseError::BadNumber(n.to_string()))
        };
        if let Some(n) = s.strip_prefix("workspace:") {
            return Ok(Action::Workspace(num(n)?));
        }
        if let Some(n) = s.strip_prefix("move-window-to-workspace:") {
            return Ok(Action::MoveWindowToWorkspace(num(n)?));
        }
        let a = match s {
            "close-window" => Action::CloseWindow,
            "minimize-window" => Action::MinimizeWindow,
            "minimize-or-restore-window" => Action::MinimizeOrRestoreWindow,
            "zoom-window" => Action::ZoomWindow,
            "toggle-fullscreen" => Action::ToggleFullscreen,
            "hide-app" => Action::HideApp,
            "hide-other-apps" => Action::HideOtherApps,
            "quit-app" => Action::QuitApp,
            "switch-app" => Action::SwitchApp,
            "switch-app-reverse" => Action::SwitchAppReverse,
            "cycle-windows" => Action::CycleWindows,
            "cycle-windows-reverse" => Action::CycleWindowsReverse,
            "tile-left" => Action::TileLeft,
            "tile-right" => Action::TileRight,
            "tile-fill" => Action::TileFill,
            "move-window-to-next-output" => Action::MoveWindowToNextOutput,
            "overview" => Action::Overview,
            "app-overview" => Action::AppOverview,
            "show-desktop" => Action::ShowDesktop,
            "app-grid" => Action::AppGrid,
            "force-quit" => Action::ForceQuit,
            "workspace-previous" => Action::WorkspacePrevious,
            "workspace-next" => Action::WorkspaceNext,
            "move-window-to-workspace-previous" => Action::MoveWindowToWorkspacePrevious,
            "move-window-to-workspace-next" => Action::MoveWindowToWorkspaceNext,
            "launcher" => Action::Launcher,
            "control-panel" => Action::ControlPanel,
            "notification-center" => Action::NotificationCenter,
            "focus-menu-bar" => Action::FocusMenuBar,
            "help-search" => Action::HelpSearch,
            "focus-dock" => Action::FocusDock,
            "toggle-dock-auto-hide" => Action::ToggleDockAutoHide,
            "lock" => Action::Lock,
            "toggle-appearance" => Action::ToggleAppearance,
            "toggle-stage" => Action::ToggleStage,
            "power-key" => Action::PowerKey,
            "logout" => Action::Logout,
            "power-dialog" => Action::PowerDialog,
            "release-shortcuts" => Action::ReleaseShortcuts,
            "input-source-previous" => Action::InputSourcePrevious,
            "input-source-next" => Action::InputSourceNext,
            other => match ScreenshotRequest::ALL
                .into_iter()
                .find(|r| r.name() == other)
            {
                Some(r) => Action::Screenshot(r),
                None => return Err(ParseError::UnknownAction(other.to_string())),
            },
        };
        Ok(a)
    }
}

/// A modifier set plus a (case-folded) keysym.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct KeyCombo {
    pub modifiers: Modifiers,
    pub keysym: u32,
}

impl KeyCombo {
    pub fn new(modifiers: Modifiers, keysym: u32) -> Self {
        Self { modifiers, keysym }.normalized()
    }

    fn normalized(mut self) -> Self {
        if self.keysym == keysym::ISO_LEFT_TAB {
            self.keysym = keysym::TAB;
            self.modifiers.shift = true;
        }
        self.keysym = keysym::fold_case(self.keysym);
        self
    }
}

/// The names of the Super key in a combination.
const SUPER_NAMES: [&str; 5] = ["super", "logo", "win", "windows", "mod4"];

/// The physical key `Primary` stands for.
type Primary = Physical;

fn modifier(name: &str, m: &mut Modifiers, primary: Primary) -> bool {
    let name = name.to_ascii_lowercase();
    let key = match name.as_str() {
        "shift" => Physical::Shift,
        "ctrl" | "control" | "ctl" => Physical::Ctrl,
        "alt" | "option" | "opt" | "mod1" => Physical::Alt,
        "primary" => primary,
        n if SUPER_NAMES.contains(&n) => Physical::Super,
        _ => return false,
    };
    match key {
        Physical::Shift => m.shift = true,
        Physical::Ctrl => m.ctrl = true,
        Physical::Alt => m.alt = true,
        Physical::Super => m.super_key = true,
        Physical::Fn => return false,
    }
    true
}

impl KeyCombo {
    /// Parse a combination whose `Primary` is the physical key `primary`.
    pub fn parse_with(s: &str, primary: Physical) -> Result<KeyCombo, ParseError> {
        let mut rest = s.trim();
        if rest.is_empty() {
            return Err(ParseError::Empty);
        }
        let mut mods = Modifiers::NONE;
        while let Some(r) = rest.strip_prefix('<') {
            let end = r
                .find('>')
                .ok_or_else(|| ParseError::UnknownModifier(rest.to_string()))?;
            if !modifier(&r[..end], &mut mods, primary) {
                return Err(ParseError::UnknownModifier(r[..end].to_string()));
            }
            rest = &r[end + 1..];
        }
        let mut key: Option<&str> = None;
        if !rest.is_empty() {
            let mut parts: Vec<&str> = rest.split('+').map(str::trim).collect();
            // "Super++" style: a literal plus key
            if rest.ends_with("++") || rest == "+" {
                parts = rest
                    .trim_end_matches('+')
                    .split('+')
                    .map(str::trim)
                    .filter(|p| !p.is_empty())
                    .collect();
                parts.push("plus");
            }
            let last = parts.pop().unwrap_or("");
            for p in parts {
                if !modifier(p, &mut mods, primary) {
                    return Err(ParseError::UnknownModifier(p.to_string()));
                }
            }
            key = Some(last);
        }
        let key = key
            .filter(|k| !k.is_empty())
            .ok_or(ParseError::MissingKey)?;
        let sym = keysym::from_name(key).ok_or_else(|| ParseError::UnknownKey(key.to_string()))?;
        Ok(KeyCombo::new(mods, sym))
    }
}

impl FromStr for KeyCombo {
    type Err = ParseError;

    /// `Primary` is the Super key, as under `cupertino`.
    fn from_str(s: &str) -> Result<Self, ParseError> {
        KeyCombo::parse_with(s, Physical::Super)
    }
}

impl fmt::Display for KeyCombo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let m = self.modifiers;
        for (on, n) in [
            (m.ctrl, "Ctrl"),
            (m.alt, "Alt"),
            (m.shift, "Shift"),
            (m.super_key, "Super"),
        ] {
            if on {
                write!(f, "{n}+")?;
            }
        }
        match keysym::name_of(self.keysym) {
            Some(n) => f.write_str(&n),
            None => write!(f, "0x{:x}", self.keysym),
        }
    }
}

/// What reading a shortcut file found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConfigReport {
    /// Lines that did not apply, with their 1-based line numbers.
    pub errors: Vec<(usize, ParseError)>,
    /// Lines outside every section that name Super, read for `classic-pc` (line number and text).
    pub retargeted: Vec<(usize, String)>,
}

/// The physical key when `lhs` is a modifier name and nothing else (a tap), whatever the key.
fn lone_modifier(lhs: &str, primary: Physical) -> Option<Physical> {
    let lhs = lhs.trim().trim_start_matches('<').trim_end_matches('>');
    if lhs.contains(['+', '<', '>']) || lhs.is_empty() {
        return None;
    }
    let mut m = Modifiers::NONE;
    if !modifier(lhs, &mut m, primary) {
        return None;
    }
    Some(if m.super_key {
        Physical::Super
    } else if m.ctrl {
        Physical::Ctrl
    } else if m.alt {
        Physical::Alt
    } else {
        Physical::Shift
    })
}

/// Whether the combination `lhs` names the Super key (and not `Primary`).
pub fn names_super(lhs: &str) -> bool {
    lhs.split(['+', '<', '>'])
        .any(|part| SUPER_NAMES.contains(&part.trim().to_ascii_lowercase().as_str()))
}

#[derive(Clone, Debug, Default)]
pub struct ShortcutMap {
    bindings: HashMap<KeyCombo, Action>,
    order: Vec<KeyCombo>,
    /// What the Super key runs when it is pressed and released alone (`Super = launcher`).
    tap: Option<Action>,
}

impl ShortcutMap {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn bind(&mut self, combo: KeyCombo, action: Action) -> Option<Action> {
        let combo = combo.normalized();
        if !self.bindings.contains_key(&combo) {
            self.order.push(combo);
        }
        self.bindings.insert(combo, action)
    }

    /// The action of the Super key tapped alone.
    pub fn tap(&self) -> Option<&Action> {
        self.tap.as_ref()
    }

    pub fn bind_tap(&mut self, action: Action) -> Option<Action> {
        self.tap.replace(action)
    }

    pub fn unbind_tap(&mut self) -> Option<Action> {
        self.tap.take()
    }

    pub fn bind_str(&mut self, combo: &str, action: Action) -> Result<Option<Action>, ParseError> {
        Ok(self.bind(combo.parse()?, action))
    }

    pub fn unbind(&mut self, combo: KeyCombo) -> Option<Action> {
        let combo = combo.normalized();
        self.order.retain(|c| *c != combo);
        self.bindings.remove(&combo)
    }

    pub fn lookup(&self, modifiers: Modifiers, keysym: u32) -> Option<&Action> {
        self.bindings.get(&KeyCombo::new(modifiers, keysym))
    }

    /// Bindings in insertion order.
    pub fn bindings(&self) -> impl Iterator<Item = (&KeyCombo, &Action)> {
        self.order.iter().map(|c| (c, &self.bindings[c]))
    }

    pub fn len(&self) -> usize {
        self.order.len()
    }

    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    /// First combination bound to `action` (for menu shortcut hints).
    pub fn combo_for(&self, action: &Action) -> Option<KeyCombo> {
        self.order
            .iter()
            .copied()
            .find(|c| self.bindings[c] == *action)
    }

    /// Parse `combination = action` lines; `#` starts a comment. Every line that parses is applied;
    /// the errors of the others are returned with their 1-based line numbers. `Primary` is Super, as under
    /// `cupertino`, and sections are read for that profile ([`ShortcutMap::apply_config_for`]).
    pub fn apply_config(&mut self, text: &str) -> Vec<(usize, ParseError)> {
        self.apply_config_for(text, "cupertino").errors
    }

    /// [`ShortcutMap::apply_config`] for the profile named `profile`: `Primary` is its command key, the lines of a
    /// `[section]` apply when it names the profile, and the lines before any section apply always. The report also
    /// lists the lines outside every section that name Super, when the profile is `classic-pc`: they were written
    /// when Super was the command key and now need the physical Super key.
    pub fn apply_config_for(&mut self, text: &str, profile: &str) -> ConfigReport {
        let primary = Profile::by_name(profile)
            .map(|p| p.physical_for(Modifier::Primary))
            .unwrap_or(Physical::Super);
        let mut report = ConfigReport::default();
        // `None` before the first section, then whether the current section is this profile's
        let mut section: Option<bool> = None;
        for (i, raw) in text.lines().enumerate() {
            let line = raw.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                let name = name.trim();
                section = Some(name == profile);
                if Profile::by_name(name).is_none() {
                    report
                        .errors
                        .push((i + 1, ParseError::UnknownSection(name.to_string())));
                }
                continue;
            }
            if section == Some(false) {
                continue;
            }
            let Some((lhs, rhs)) = line.split_once('=') else {
                report
                    .errors
                    .push((i + 1, ParseError::MissingSeparator(line.to_string())));
                continue;
            };
            let rhs = rhs.trim();
            if let Some(modifier) = lone_modifier(lhs, primary) {
                let tap = match modifier {
                    Physical::Super => Ok(()),
                    _ => Err(ParseError::TapNotSuper(lhs.trim().to_string())),
                };
                match (tap, rhs) {
                    (Err(e), _) => report.errors.push((i + 1, e)),
                    (Ok(()), "none") => {
                        self.unbind_tap();
                    }
                    (Ok(()), rhs) => match rhs.parse::<Action>() {
                        Ok(a) => {
                            self.bind_tap(a);
                            if section.is_none() && profile == "classic-pc" && names_super(lhs) {
                                report.retargeted.push((i + 1, line.to_string()));
                            }
                        }
                        Err(e) => report.errors.push((i + 1, e)),
                    },
                }
                continue;
            }
            let combo = KeyCombo::parse_with(lhs, primary);
            if section.is_none() && profile == "classic-pc" && names_super(lhs) && combo.is_ok() {
                report.retargeted.push((i + 1, line.to_string()));
            }
            if rhs == "none" {
                match combo {
                    Ok(c) => {
                        self.unbind(c);
                    }
                    Err(e) => report.errors.push((i + 1, e)),
                }
                continue;
            }
            match (combo, rhs.parse::<Action>()) {
                (Ok(c), Ok(a)) => {
                    self.bind(c, a);
                }
                (Err(e), _) | (_, Err(e)) => report.errors.push((i + 1, e)),
            }
        }
        report
    }

    /// The chord that releases the keys and the pointer an application holds, when no other is bound.
    pub fn release_chord() -> KeyCombo {
        KeyCombo::new(
            Modifiers {
                super_key: true,
                ..Modifiers::NONE
            },
            keysym::ESCAPE,
        )
    }

    /// Keep a way to get the keys back: some chord must be bound to [`Action::ReleaseShortcuts`], and every such chord
    /// holds Ctrl, Alt or Super (a bare key would be taken from the application that is meant to get it). A binding
    /// without one is dropped; when no chord is left, [`ShortcutMap::release_chord`] is bound, replacing whatever the
    /// file bound it to or freeing it from a `none`. Returns that chord when it had to be restored.
    pub fn keep_release_chord(&mut self) -> Option<KeyCombo> {
        let weak: Vec<KeyCombo> = self
            .bindings()
            .filter(|(c, a)| {
                **a == Action::ReleaseShortcuts
                    && !(c.modifiers.ctrl || c.modifiers.alt || c.modifiers.super_key)
            })
            .map(|(c, _)| *c)
            .collect();
        for chord in weak {
            self.unbind(chord);
        }
        if self.combo_for(&Action::ReleaseShortcuts).is_some() {
            return None;
        }
        let chord = Self::release_chord();
        self.bind(chord, Action::ReleaseShortcuts);
        Some(chord)
    }

    /// The default map of the profile named `profile` (`cupertino` for a name no profile has).
    pub fn defaults_for(profile: &str) -> Self {
        match profile {
            "classic-pc" => Self::classic_pc_defaults(),
            _ => Self::defaults(),
        }
    }

    /// The `classic-pc` map: the system follows Windows and Linux habits, and the chords of the applications (Ctrl+W,
    /// Ctrl+Q, Ctrl+Tab, Ctrl+Space, Ctrl+Shift+arrows, F11) stay with them. The modifiers are the physical keys.
    pub fn classic_pc_defaults() -> Self {
        let mut m = Self::new();
        let text = "\
Alt+Tab = switch-app
Alt+Shift+Tab = switch-app-reverse
Super+grave = cycle-windows
Super+Shift+grave = cycle-windows-reverse
Alt+F4 = close-window
Super+Down = minimize-or-restore-window
Super+h = hide-app
Super+Home = hide-other-apps
Super+q = quit-app
Super+f = toggle-fullscreen
Super+Left = tile-left
Super+Right = tile-right
Super+Up = tile-fill
Super+Tab = overview
XF86LaunchA = overview
Super+F10 = app-overview
Super+d = show-desktop
Super+a = app-grid
Ctrl+Shift+Escape = force-quit
Super+Page_Up = workspace-previous
Super+Page_Down = workspace-next
Super+Shift+Page_Up = move-window-to-workspace-previous
Super+Shift+Page_Down = move-window-to-workspace-next
Super+s = launcher
Super+space = input-source-next
Super+Shift+space = input-source-previous
Super+F2 = focus-menu-bar
# a proposal: no known desktop habit uses Super+? (Ctrl+Shift+/ stays with the applications)
Super+Shift+slash = help-search
Super+F3 = focus-dock
Super+Alt+d = toggle-dock-auto-hide
Super+l = lock
Super+Shift+q = logout
XF86PowerOff = power-key
Ctrl+XF86PowerOff = power-dialog
Ctrl+XF86Eject = power-dialog
Ctrl+Alt+Delete = power-dialog
Super+Escape = release-shortcuts
Print = screenshot-toolbar
Shift+Print = screenshot
Super+Shift+s = screenshot-area
Ctrl+Print = screenshot-clipboard
Ctrl+Shift+Print = screenshot-area-clipboard
";
        m.apply_config(text);
        m.bind_tap(Action::Launcher);
        for n in 1..=9u32 {
            let sym = keysym::from_name(&n.to_string()).unwrap_or(0x30 + n);
            m.bind(
                KeyCombo::new(
                    Modifiers {
                        alt: true,
                        super_key: true,
                        ..Default::default()
                    },
                    sym,
                ),
                Action::Workspace(n),
            );
        }
        m
    }

    /// The default map (approximate conventions, primary modifier = Super).
    pub fn defaults() -> Self {
        let mut m = Self::new();
        let text = "\
Super+Tab = switch-app
Super+Shift+Tab = switch-app-reverse
Super+grave = cycle-windows
Super+Shift+grave = cycle-windows-reverse
Super+w = close-window
Super+m = minimize-window
Super+h = hide-app
Super+Alt+h = hide-other-apps
Super+q = quit-app
Super+Ctrl+f = toggle-fullscreen
Super+Ctrl+Left = tile-left
Super+Ctrl+Right = tile-right
Super+Ctrl+Up = tile-fill
Ctrl+Up = overview
F3 = overview
XF86LaunchA = overview
Ctrl+Down = app-overview
F11 = show-desktop
F4 = app-grid
Super+Alt+Escape = force-quit
Ctrl+Left = workspace-previous
Ctrl+Right = workspace-next
Super+Shift+Left = move-window-to-workspace-previous
Super+Shift+Right = move-window-to-workspace-next
Super+space = launcher
Ctrl+F2 = focus-menu-bar
Ctrl+F3 = focus-dock
Super+Alt+d = toggle-dock-auto-hide
Super+Ctrl+q = lock
Super+Shift+q = logout
XF86PowerOff = power-key
Ctrl+XF86PowerOff = power-dialog
Ctrl+XF86Eject = power-dialog
Super+Escape = release-shortcuts
Super+Shift+3 = screenshot
Super+Shift+4 = screenshot-area
Super+Shift+5 = screenshot-toolbar
Super+Ctrl+Shift+3 = screenshot-clipboard
Super+Ctrl+Shift+4 = screenshot-area-clipboard
";
        m.apply_config(text);
        for n in 1..=9u32 {
            let sym = keysym::from_name(&n.to_string()).unwrap_or(0x30 + n);
            m.bind(
                KeyCombo::new(
                    Modifiers {
                        ctrl: true,
                        ..Default::default()
                    },
                    sym,
                ),
                Action::Workspace(n),
            );
        }
        m
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SUPER: Modifiers = Modifiers {
        shift: false,
        ctrl: false,
        alt: false,
        super_key: true,
    };

    #[test]
    fn parses_plus_and_angle_forms() {
        let c: KeyCombo = "Super+Tab".parse().unwrap();
        assert_eq!(
            c,
            KeyCombo {
                modifiers: SUPER,
                keysym: 0xff09
            }
        );
        let c: KeyCombo = "<Super><Shift>grave".parse().unwrap();
        assert_eq!(
            c.modifiers,
            Modifiers {
                shift: true,
                super_key: true,
                ..Default::default()
            }
        );
        assert_eq!(c.keysym, 0x60);
        let c: KeyCombo = "ctrl + alt + Left".parse().unwrap();
        assert_eq!(
            c.modifiers,
            Modifiers {
                ctrl: true,
                alt: true,
                ..Default::default()
            }
        );
        assert_eq!(
            "F11".parse::<KeyCombo>().unwrap().modifiers,
            Modifiers::NONE
        );
        assert_eq!("Primary+q".parse::<KeyCombo>().unwrap().modifiers, SUPER);
        assert_eq!("Super++".parse::<KeyCombo>().unwrap().keysym, 0x2b);
    }

    #[test]
    fn rejects_bad_input() {
        assert_eq!("".parse::<KeyCombo>(), Err(ParseError::Empty));
        assert!(matches!(
            "Hyper+x".parse::<KeyCombo>(),
            Err(ParseError::UnknownModifier(_))
        ));
        assert!(matches!(
            "Super+Nonsense".parse::<KeyCombo>(),
            Err(ParseError::UnknownKey(_))
        ));
        assert_eq!("Super+".parse::<KeyCombo>(), Err(ParseError::MissingKey));
        assert!(matches!(
            "<Super".parse::<KeyCombo>(),
            Err(ParseError::UnknownModifier(_))
        ));
    }

    #[test]
    fn letter_case_and_iso_left_tab_fold() {
        let a: KeyCombo = "Super+Q".parse().unwrap();
        let b: KeyCombo = "Super+q".parse().unwrap();
        assert_eq!(a, b);
        let c: KeyCombo = "Super+ISO_Left_Tab".parse().unwrap();
        assert_eq!(c, "Super+Shift+Tab".parse().unwrap());
    }

    #[test]
    fn display_roundtrips() {
        for s in [
            "Super+Tab",
            "Ctrl+Alt+Left",
            "Super+Shift+grave",
            "F11",
            "Ctrl+5",
            "Super+q",
        ] {
            let c: KeyCombo = s.parse().unwrap();
            assert_eq!(c.to_string().parse::<KeyCombo>().unwrap(), c, "{s}");
        }
        assert_eq!(
            "Super+Shift+Tab".parse::<KeyCombo>().unwrap().to_string(),
            "Shift+Super+Tab"
        );
    }

    #[test]
    fn lookup_is_exact_on_modifiers() {
        let mut m = ShortcutMap::new();
        m.bind_str("Super+Tab", Action::SwitchApp).unwrap();
        assert_eq!(m.lookup(SUPER, 0xff09), Some(&Action::SwitchApp));
        assert_eq!(m.lookup(Modifiers::NONE, 0xff09), None);
        assert_eq!(
            m.lookup(
                Modifiers {
                    super_key: true,
                    alt: true,
                    ..Default::default()
                },
                0xff09
            ),
            None
        );
        m.bind_str("Super+Shift+Tab", Action::SwitchAppReverse)
            .unwrap();
        let sh = Modifiers {
            shift: true,
            super_key: true,
            ..Default::default()
        };
        assert_eq!(
            m.lookup(sh, 0xfe20),
            Some(&Action::SwitchAppReverse),
            "ISO_Left_Tab arrives with shift"
        );
        assert_eq!(m.lookup(SUPER, 0x51), None);
    }

    #[test]
    fn rebinding_unbinding_and_reverse_lookup() {
        let mut m = ShortcutMap::new();
        assert_eq!(m.bind_str("Super+w", Action::CloseWindow).unwrap(), None);
        assert_eq!(
            m.bind_str("Super+W", Action::QuitApp).unwrap(),
            Some(Action::CloseWindow)
        );
        assert_eq!(m.len(), 1);
        assert_eq!(
            m.combo_for(&Action::QuitApp).unwrap().to_string(),
            "Super+w"
        );
        assert_eq!(m.unbind("Super+w".parse().unwrap()), Some(Action::QuitApp));
        assert!(m.is_empty());
        assert_eq!(m.combo_for(&Action::QuitApp), None);
    }

    #[test]
    fn actions_roundtrip_through_names() {
        for a in [
            Action::CloseWindow,
            Action::Overview,
            Action::ShowDesktop,
            Action::ForceQuit,
            Action::Workspace(3),
            Action::MoveWindowToWorkspace(2),
            Action::Launcher,
            Action::Exec("foot -e htop".into()),
            Action::TileFill,
            Action::MoveWindowToWorkspaceNext,
            Action::InputSourcePrevious,
            Action::InputSourceNext,
        ] {
            assert_eq!(a.name().parse::<Action>().unwrap(), a);
        }
        assert!(matches!(
            "fly".parse::<Action>(),
            Err(ParseError::UnknownAction(_))
        ));
        assert!(matches!(
            "workspace:x".parse::<Action>(),
            Err(ParseError::BadNumber(_))
        ));
    }

    #[test]
    fn config_text_applies_good_lines_and_reports_bad_ones() {
        let mut m = ShortcutMap::new();
        let errs = m.apply_config(
            "# comment\nSuper+space = launcher  # trailing\nSuper+x = bogus\nnonsense\nCtrl+Alt+t = exec:foot\n\nSuper+space = none\nHyper+z = overview\n",
        );
        assert_eq!(m.len(), 1);
        assert_eq!(
            m.lookup(
                Modifiers {
                    ctrl: true,
                    alt: true,
                    ..Default::default()
                },
                0x74
            ),
            Some(&Action::Exec("foot".into()))
        );
        assert_eq!(errs.iter().map(|e| e.0).collect::<Vec<_>>(), vec![3, 4, 8]);
    }

    #[test]
    fn toggle_appearance_is_bindable_and_has_no_default_chord() {
        assert_eq!(
            ShortcutMap::defaults().combo_for(&Action::ToggleAppearance),
            None
        );
        let mut m = ShortcutMap::defaults();
        assert_eq!(m.apply_config("Super+Alt+a = toggle-appearance\n"), vec![]);
        assert_eq!(
            m.lookup(
                Modifiers {
                    super_key: true,
                    alt: true,
                    ..Default::default()
                },
                0x61
            ),
            Some(&Action::ToggleAppearance)
        );
    }

    #[test]
    fn toggle_stage_is_bindable_and_has_no_default_chord() {
        assert_eq!(
            ShortcutMap::defaults().combo_for(&Action::ToggleStage),
            None
        );
        let mut m = ShortcutMap::defaults();
        assert_eq!(m.apply_config("Super+Alt+s = toggle-stage\n"), vec![]);
        assert_eq!(
            m.lookup(
                Modifiers {
                    super_key: true,
                    alt: true,
                    ..Default::default()
                },
                0x73
            ),
            Some(&Action::ToggleStage)
        );
    }

    #[test]
    fn the_five_screenshot_actions_parse_print_and_have_the_reference_chords() {
        let m = ShortcutMap::defaults();
        let shift_super = Modifiers {
            shift: true,
            super_key: true,
            ..Default::default()
        };
        let ctrl_shift_super = Modifiers {
            ctrl: true,
            ..shift_super
        };
        let shot = |r| Some(Action::Screenshot(r));
        assert_eq!(
            m.lookup(shift_super, 0x33).cloned(),
            shot(ScreenshotRequest::Screen)
        );
        assert_eq!(
            m.lookup(ctrl_shift_super, 0x33).cloned(),
            shot(ScreenshotRequest::ScreenToClipboard)
        );
        assert_eq!(
            m.lookup(shift_super, 0x34).cloned(),
            shot(ScreenshotRequest::Selection)
        );
        assert_eq!(
            m.lookup(ctrl_shift_super, 0x34).cloned(),
            shot(ScreenshotRequest::SelectionToClipboard)
        );
        assert_eq!(
            m.lookup(shift_super, 0x35).cloned(),
            shot(ScreenshotRequest::Toolbar)
        );
        for request in ScreenshotRequest::ALL {
            assert_eq!(
                request.name().parse::<Action>(),
                Ok(Action::Screenshot(request))
            );
            assert_eq!(Action::Screenshot(request).name(), request.name());
        }
    }

    #[test]
    fn defaults_cover_the_action_groups() {
        let m = ShortcutMap::defaults();
        assert_eq!(m.lookup(SUPER, 0xff09), Some(&Action::SwitchApp));
        assert_eq!(
            m.lookup(
                Modifiers {
                    ctrl: true,
                    ..Default::default()
                },
                0xff52
            ),
            Some(&Action::Overview)
        );
        assert_eq!(
            m.lookup(
                Modifiers {
                    ctrl: true,
                    ..Default::default()
                },
                0x33
            ),
            Some(&Action::Workspace(3))
        );
        assert_eq!(m.lookup(SUPER, 0x20), Some(&Action::Launcher));
        assert_eq!(
            m.lookup(Modifiers::NONE, 0xffc8),
            Some(&Action::ShowDesktop),
            "F11"
        );
        assert!(m.combo_for(&Action::Lock).is_some());
        assert_eq!(
            m.lookup(Modifiers::NONE, 0x1008ff2a),
            Some(&Action::PowerKey),
            "the power key"
        );
        assert!(m.bindings().count() >= 35);
    }

    fn combo(text: &str) -> KeyCombo {
        text.parse().unwrap()
    }

    #[test]
    fn primary_is_the_command_key_of_the_profile() {
        let mut cupertino = ShortcutMap::new();
        let mut classic = ShortcutMap::new();
        let text = "Primary+j = launcher\nSuper+k = lock\n";
        assert!(cupertino
            .apply_config_for(text, "cupertino")
            .errors
            .is_empty());
        assert!(classic
            .apply_config_for(text, "classic-pc")
            .errors
            .is_empty());
        assert_eq!(
            cupertino.lookup(combo("Super+j").modifiers, combo("j").keysym),
            Some(&Action::Launcher)
        );
        assert_eq!(
            classic.lookup(combo("Ctrl+j").modifiers, combo("j").keysym),
            Some(&Action::Launcher)
        );
        // Super is the Super key in both
        assert_eq!(
            cupertino.lookup(combo("Super+k").modifiers, combo("k").keysym),
            Some(&Action::Lock)
        );
        assert_eq!(
            classic.lookup(combo("Super+k").modifiers, combo("k").keysym),
            Some(&Action::Lock)
        );
    }

    #[test]
    fn a_section_applies_to_its_profile_only_and_lines_before_any_section_to_both() {
        let text =
            "Super+a = launcher\n[cupertino]\nSuper+b = lock\n[classic-pc]\nSuper+c = lock\n";
        for (profile, bound, unbound) in [("cupertino", "b", "c"), ("classic-pc", "c", "b")] {
            let mut map = ShortcutMap::new();
            let report = map.apply_config_for(text, profile);
            assert!(report.errors.is_empty(), "{profile}: {:?}", report.errors);
            let at = |k: &str| {
                map.lookup(combo(&format!("Super+{k}")).modifiers, combo(k).keysym)
                    .cloned()
            };
            assert_eq!(at("a"), Some(Action::Launcher), "{profile}");
            assert_eq!(at(bound), Some(Action::Lock), "{profile}");
            assert_eq!(at(unbound), None, "{profile}");
        }
    }

    #[test]
    fn an_unknown_section_is_reported_and_its_lines_do_not_apply() {
        let mut map = ShortcutMap::new();
        let report = map.apply_config_for("[windows]\nSuper+a = launcher\n", "cupertino");
        assert_eq!(
            report.errors,
            [(1, ParseError::UnknownSection("windows".into()))]
        );
        assert!(map.is_empty());
    }

    #[test]
    fn super_lines_outside_a_section_are_reported_as_retargeted_under_classic_pc_only() {
        let text =
            "Super+a = launcher\nPrimary+b = lock\nAlt+c = lock\n[classic-pc]\nSuper+d = lock\n";
        let report = ShortcutMap::new().apply_config_for(text, "classic-pc");
        assert_eq!(report.retargeted, [(1, "Super+a = launcher".to_string())]);
        assert!(ShortcutMap::new()
            .apply_config_for(text, "cupertino")
            .retargeted
            .is_empty());
    }

    /// The chords of applications that the system does not take under `classic-pc`.
    const RESERVED: [&str; 9] = [
        "Ctrl+w",
        "Ctrl+q",
        "Ctrl+Tab",
        "Ctrl+space",
        "Ctrl+Shift+Left",
        "Ctrl+Shift+Right",
        "Ctrl+Shift+Up",
        "Ctrl+Shift+Down",
        "F11",
    ];

    #[test]
    fn the_classic_pc_table_never_takes_a_chord_of_the_applications() {
        let map = ShortcutMap::classic_pc_defaults();
        for chord in RESERVED {
            let c = combo(chord);
            assert_eq!(map.lookup(c.modifiers, c.keysym), None, "{chord}");
        }
        // AltGr is often typed as Ctrl+Alt: no Ctrl+Alt+character is taken
        for (c, a) in map.bindings() {
            let character = keysym::name_of(c.keysym).is_some_and(|n| n.chars().count() == 1);
            assert!(
                !(c.modifiers.ctrl && c.modifiers.alt && character),
                "{c} = {a}"
            );
        }
    }

    #[test]
    fn each_profile_binds_its_chords_once() {
        for (profile, text_lines) in [
            ("cupertino", ShortcutMap::defaults()),
            ("classic-pc", ShortcutMap::classic_pc_defaults()),
        ] {
            let mut seen = std::collections::HashSet::new();
            for (c, a) in text_lines.bindings() {
                assert!(seen.insert(*c), "{profile}: {c} bound twice (last {a})");
            }
        }
    }

    #[test]
    fn every_action_of_cupertino_has_a_chord_under_classic_pc() {
        let classic = ShortcutMap::classic_pc_defaults();
        let bound: Vec<&Action> = classic.bindings().map(|(_, a)| a).collect();
        for (c, a) in ShortcutMap::defaults().bindings() {
            // classic-pc restores a tiled window before it minimizes
            let stands_in = if *a == Action::MinimizeWindow {
                &Action::MinimizeOrRestoreWindow
            } else {
                a
            };
            assert!(
                bound.contains(&stands_in),
                "{a} (cupertino {c}) has no chord under classic-pc"
            );
        }
    }

    #[test]
    fn help_search_round_trips_by_name_and_has_a_super_chord_under_classic_pc() {
        assert_eq!("help-search".parse::<Action>(), Ok(Action::HelpSearch));
        assert_eq!(Action::HelpSearch.name(), "help-search");
        let combo: KeyCombo = "Super+Shift+slash".parse().unwrap();
        assert_eq!(
            ShortcutMap::classic_pc_defaults().lookup(combo.modifiers, combo.keysym),
            Some(&Action::HelpSearch)
        );
    }

    #[test]
    fn defaults_for_picks_the_table_of_the_profile() {
        assert_eq!(
            ShortcutMap::defaults_for("classic-pc").len(),
            ShortcutMap::classic_pc_defaults().len()
        );
        assert_eq!(
            ShortcutMap::defaults_for("cupertino").len(),
            ShortcutMap::defaults().len()
        );
        assert_eq!(
            ShortcutMap::defaults_for("nope").len(),
            ShortcutMap::defaults().len()
        );
    }

    #[test]
    fn names_super_finds_the_super_key_but_not_primary() {
        for yes in [
            "Super+a",
            "super+Shift+a",
            "<Super>a",
            "Win+a",
            "Ctrl+Mod4+a",
        ] {
            assert!(names_super(yes), "{yes}");
        }
        for no in ["Primary+a", "Ctrl+Alt+a", "Shift+Super_L", "a"] {
            assert!(!names_super(no), "{no}");
        }
    }

    #[test]
    fn the_super_key_alone_names_a_tap() {
        let mut map = ShortcutMap::new();
        let report = map.apply_config_for("Super = launcher\n", "cupertino");
        assert!(report.errors.is_empty());
        assert_eq!(map.tap(), Some(&Action::Launcher));
        assert!(map.is_empty(), "a tap is not a chord");
        map.apply_config_for("Super = none\n", "cupertino");
        assert_eq!(map.tap(), None);
        // Primary is the Super key under cupertino and the Ctrl key, which cannot be tapped, under classic-pc
        let mut cupertino = ShortcutMap::new();
        cupertino.apply_config_for("Primary = lock\n", "cupertino");
        assert_eq!(cupertino.tap(), Some(&Action::Lock));
        let mut classic = ShortcutMap::new();
        let report = classic.apply_config_for("Primary = lock\n", "classic-pc");
        assert_eq!(
            report.errors,
            [(1, ParseError::TapNotSuper("Primary".into()))]
        );
        assert_eq!(classic.tap(), None);
        for other in ["Ctrl", "Alt", "Shift"] {
            let report =
                ShortcutMap::new().apply_config_for(&format!("{other} = lock\n"), "cupertino");
            assert_eq!(
                report.errors,
                [(1, ParseError::TapNotSuper(other.into()))],
                "{other}"
            );
        }
    }

    #[test]
    fn only_classic_pc_taps_super_by_default() {
        assert_eq!(
            ShortcutMap::classic_pc_defaults().tap(),
            Some(&Action::Launcher)
        );
        assert_eq!(ShortcutMap::defaults().tap(), None);
    }

    #[test]
    fn a_tap_line_follows_the_sections_and_the_retarget_report() {
        let text = "[cupertino]\nSuper = lock\n[classic-pc]\nSuper = launcher\n";
        let mut cupertino = ShortcutMap::new();
        cupertino.apply_config_for(text, "cupertino");
        assert_eq!(cupertino.tap(), Some(&Action::Lock));
        let mut classic = ShortcutMap::new();
        let report = classic.apply_config_for(text, "classic-pc");
        assert_eq!(classic.tap(), Some(&Action::Launcher));
        assert!(report.retargeted.is_empty(), "a section names its profile");
        let report = ShortcutMap::new().apply_config_for("Super = lock\n", "classic-pc");
        assert_eq!(report.retargeted, [(1, "Super = lock".to_string())]);
    }

    #[test]
    fn both_default_tables_release_the_shortcuts_on_super_escape() {
        for map in [ShortcutMap::defaults(), ShortcutMap::classic_pc_defaults()] {
            assert_eq!(
                map.lookup(combo("Super+Escape").modifiers, combo("Escape").keysym),
                Some(&Action::ReleaseShortcuts)
            );
            assert_eq!(
                map.combo_for(&Action::ReleaseShortcuts),
                Some(ShortcutMap::release_chord())
            );
        }
    }

    #[test]
    fn a_file_cannot_leave_the_shortcuts_without_a_chord_to_release_them() {
        for (text, restored) in [
            ("Super+Escape = none\n", true),
            ("Super+Escape = lock\n", true),
            ("Escape = release-shortcuts\nSuper+Escape = none\n", true),
            (
                "Shift+Escape = release-shortcuts\nSuper+Escape = none\n",
                true,
            ),
            (
                "Ctrl+Alt+r = release-shortcuts\nSuper+Escape = none\n",
                false,
            ),
            ("Super+x = none\n", false),
        ] {
            let mut map = ShortcutMap::defaults();
            map.apply_config(text);
            assert_eq!(map.keep_release_chord().is_some(), restored, "{text:?}");
            let chord = map.combo_for(&Action::ReleaseShortcuts).expect(text);
            assert!(
                chord.modifiers.ctrl || chord.modifiers.alt || chord.modifiers.super_key,
                "{text:?}: {chord}"
            );
            assert_eq!(
                map.lookup(chord.modifiers, chord.keysym),
                Some(&Action::ReleaseShortcuts)
            );
        }
    }

    #[test]
    fn a_chord_without_ctrl_alt_or_super_is_dropped_not_kept_beside_the_default() {
        let mut map = ShortcutMap::defaults();
        map.apply_config("Escape = release-shortcuts\n");
        assert_eq!(map.keep_release_chord(), None);
        assert_eq!(map.lookup(Modifiers::NONE, combo("Escape").keysym), None);
    }
}
