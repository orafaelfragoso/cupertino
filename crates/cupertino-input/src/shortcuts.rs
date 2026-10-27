//! Configurable shortcut map: key combinations → compositor [`Action`]s.
//!
//! Combination strings look like `Super+Tab`, `Ctrl+Alt+Left`, `Super+Shift+grave` or the GTK style
//! `<Super>Tab`; the key is an xkbcommon keysym name (see [`crate::keysym`]). `Super` is the
//! primary modifier (`Primary` and `Logo` are aliases).
//!
//! Matching is exact on the four modifiers (lock modifiers are not part of [`Modifiers`]).
//! `ISO_Left_Tab` (what Shift+Tab produces) is folded into `Shift+Tab`, and ASCII letters are
//! matched case-insensitively. Feed [`ShortcutMap::lookup`] the keysym of the key at its base
//! (unshifted) level so that `Super+Shift+3` matches regardless of the layout's shifted symbol.

use std::collections::HashMap;
use std::fmt;
use std::str::FromStr;

use crate::geom::Modifiers;
use crate::keysym;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    // windows and apps
    CloseWindow,
    MinimizeWindow,
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
            Action::FocusDock => "focus-dock".into(),
            Action::ToggleDockAutoHide => "toggle-dock-auto-hide".into(),
            Action::Lock => "lock".into(),
            Action::ToggleAppearance => "toggle-appearance".into(),
            Action::ToggleStage => "toggle-stage".into(),
            Action::PowerKey => "power-key".into(),
            Action::Logout => "logout".into(),
            Action::PowerDialog => "power-dialog".into(),
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
            "workspace-previous" => Action::WorkspacePrevious,
            "workspace-next" => Action::WorkspaceNext,
            "move-window-to-workspace-previous" => Action::MoveWindowToWorkspacePrevious,
            "move-window-to-workspace-next" => Action::MoveWindowToWorkspaceNext,
            "launcher" => Action::Launcher,
            "control-panel" => Action::ControlPanel,
            "notification-center" => Action::NotificationCenter,
            "focus-menu-bar" => Action::FocusMenuBar,
            "focus-dock" => Action::FocusDock,
            "toggle-dock-auto-hide" => Action::ToggleDockAutoHide,
            "lock" => Action::Lock,
            "toggle-appearance" => Action::ToggleAppearance,
            "toggle-stage" => Action::ToggleStage,
            "power-key" => Action::PowerKey,
            "logout" => Action::Logout,
            "power-dialog" => Action::PowerDialog,
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

fn modifier(name: &str, m: &mut Modifiers) -> bool {
    match name.to_ascii_lowercase().as_str() {
        "shift" => m.shift = true,
        "ctrl" | "control" | "ctl" => m.ctrl = true,
        "alt" | "option" | "opt" | "mod1" => m.alt = true,
        "super" | "logo" | "win" | "windows" | "mod4" | "primary" => m.super_key = true,
        _ => return false,
    }
    true
}

impl FromStr for KeyCombo {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, ParseError> {
        let mut rest = s.trim();
        if rest.is_empty() {
            return Err(ParseError::Empty);
        }
        let mut mods = Modifiers::NONE;
        while let Some(r) = rest.strip_prefix('<') {
            let end = r
                .find('>')
                .ok_or_else(|| ParseError::UnknownModifier(rest.to_string()))?;
            if !modifier(&r[..end], &mut mods) {
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
                if !modifier(p, &mut mods) {
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

#[derive(Clone, Debug, Default)]
pub struct ShortcutMap {
    bindings: HashMap<KeyCombo, Action>,
    order: Vec<KeyCombo>,
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
    /// the errors of the others are returned with their 1-based line numbers.
    pub fn apply_config(&mut self, text: &str) -> Vec<(usize, ParseError)> {
        let mut errors = Vec::new();
        for (i, raw) in text.lines().enumerate() {
            let line = raw.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let Some((lhs, rhs)) = line.split_once('=') else {
                errors.push((i + 1, ParseError::MissingSeparator(line.to_string())));
                continue;
            };
            let rhs = rhs.trim();
            if rhs == "none" {
                match lhs.parse::<KeyCombo>() {
                    Ok(c) => {
                        self.unbind(c);
                    }
                    Err(e) => errors.push((i + 1, e)),
                }
                continue;
            }
            match (lhs.parse::<KeyCombo>(), rhs.parse::<Action>()) {
                (Ok(c), Ok(a)) => {
                    self.bind(c, a);
                }
                (Err(e), _) | (_, Err(e)) => errors.push((i + 1, e)),
            }
        }
        errors
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
Ctrl+Down = app-overview
F11 = show-desktop
F4 = app-grid
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
}
