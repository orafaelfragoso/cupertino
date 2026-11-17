//! Input routing layer: key chords, bindings, actions and their translation into window-model
//! commands. Pure logic (no Wayland types) so it is unit-testable and can be extended by the
//! integration of the input crate: add `Action` variants, extend `translate_action`, or install
//! a different [`KeyBindings`] table.
//!
//! Flow: backend key event → [`KeyRouter::key`] → [`KeyOutcome::Consumed`] with an [`ActionPlan`]
//! (model commands + shell operations) or [`KeyOutcome::Forward`] to the focused client.
//! Pointer routing lives in `state.rs` (`Cupertino::pointer_*`) and uses the same plan type.

use std::collections::HashMap;

use cupertino_defaults::accessibility::DisplayOption;
use cupertino_input::ScreenshotRequest;
use cupertino_wm::{AppId, Command, OutputId, TileKind, WindowId};
use smithay::input::keyboard::{keysyms, xkb, ModifiersState};

/// Modifier mask used by chords. `logo` is the primary modifier.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Mods {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub logo: bool,
}

impl From<&ModifiersState> for Mods {
    fn from(m: &ModifiersState) -> Self {
        Mods {
            ctrl: m.ctrl,
            alt: m.alt,
            shift: m.shift,
            logo: m.logo,
        }
    }
}

impl Mods {
    pub const NONE: Mods = Mods {
        ctrl: false,
        alt: false,
        shift: false,
        logo: false,
    };
    pub fn any(&self) -> bool {
        self.ctrl || self.alt || self.shift || self.logo
    }
}

/// A key chord: modifiers plus a lower-cased keysym.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Chord {
    pub mods: Mods,
    pub sym: u32,
}

/// Whether `ev` is the command key and W, without another modifier.
fn is_command_w(ev: &KeyEvent) -> bool {
    normalize_sym(ev.sym) == keysyms::KEY_w
        && ev.mods
            == (Mods {
                logo: true,
                ..Mods::NONE
            })
}

/// Normalise a keysym for matching: letters lower-case, the shifted tab symbol becomes tab.
pub fn normalize_sym(sym: u32) -> u32 {
    if sym == keysyms::KEY_ISO_Left_Tab {
        return keysyms::KEY_Tab;
    }
    if (keysyms::KEY_A..=keysyms::KEY_Z).contains(&sym) {
        sym + (keysyms::KEY_a - keysyms::KEY_A)
    } else {
        sym
    }
}

/// Parse `"Super+Shift+Tab"`. Modifier names: `Super`/`Logo`/`Mod4`, `Ctrl`/`Control`, `Alt`/`Mod1`,
/// `Shift`; the last token is an xkb keysym name (case-insensitive). The modifiers are the physical keys, and `Primary`
/// is Super, the command key of the `cupertino` profile.
pub fn parse_chord(text: &str) -> Result<Chord, String> {
    parse_chord_for(text, "cupertino")
}

/// [`parse_chord`] for the modifier profile `profile`, whose command key is what `Primary` names (Ctrl under
/// `classic-pc`).
pub fn parse_chord_for(text: &str, profile: &str) -> Result<Chord, String> {
    let primary_is_ctrl = profile == "classic-pc";
    let mut mods = Mods::NONE;
    let mut sym = None;
    for tok in text.split('+').map(str::trim).filter(|t| !t.is_empty()) {
        match tok.to_ascii_lowercase().as_str() {
            "primary" if primary_is_ctrl => mods.ctrl = true,
            "super" | "logo" | "mod4" | "primary" => mods.logo = true,
            "ctrl" | "control" => mods.ctrl = true,
            "alt" | "mod1" | "option" => mods.alt = true,
            "shift" => mods.shift = true,
            _ => {
                let s = xkb::keysym_from_name(tok, xkb::KEYSYM_CASE_INSENSITIVE);
                if s.raw() == keysyms::KEY_NoSymbol {
                    return Err(format!("unknown key {tok:?} in {text:?}"));
                }
                if sym.replace(normalize_sym(s.raw())).is_some() {
                    return Err(format!("two keys in {text:?}"));
                }
            }
        }
    }
    sym.map(|sym| Chord { mods, sym })
        .ok_or_else(|| format!("no key in {text:?}"))
}

/// Everything a binding can trigger.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    SwitcherNext,
    SwitcherPrev,
    CycleWindowsNext,
    CycleWindowsPrev,
    QuitApp,
    CloseWindow,
    Minimize,
    /// A tiled, filled or zoomed window goes back to its frame; any other is minimized.
    MinimizeOrRestore,
    HideApp,
    HideOthers,
    ToggleZoom,
    ToggleFullscreen,
    TileLeft,
    TileRight,
    TileFill,
    Overview,
    AppOverview,
    WorkspaceLeft,
    WorkspaceRight,
    MoveToWorkspaceLeft,
    MoveToWorkspaceRight,
    AddWorkspace,
    SpawnTerminal,
    /// Start the screenshot process in one of its modes.
    Screenshot(ScreenshotRequest),
    QuitCompositor,
    /// Ask the login process whether to log out of the session.
    LogOut,
    /// Have the login process log out at once, without its confirmation panel.
    LogOutNow,
    /// Open the login process's power panel.
    PowerDialog,
    /// Give the keys and the pointer back to the system while an application holds them.
    ReleaseShortcuts,
    SwitchVt(i32),
    ShowDesktop,
    AppGrid,
    /// Open the Force Quit dialog (the dock process draws it).
    ForceQuit,
    Launcher,
    FocusMenuBar,
    /// Open the menu bar's Help menu with its search field focused.
    HelpSearch,
    /// Give the dock the keyboard on its first tile.
    FocusDock,
    ToggleDockAutoHide,
    NotificationCenter,
    ControlPanel,
    /// Switch between the light and the dark appearance.
    ToggleAppearance,
    /// Turn an accessibility display option on or off (`accessibility.conf`).
    ToggleDisplayOption(DisplayOption),
    /// Turn the stage on or off (the window manager process).
    ToggleStage,
    Lock,
    /// Ask the login process whether to shut the computer down.
    PowerKey,
    /// Step to the previously used input source (HUD while the modifiers are held).
    InputSourcePrevious,
    /// Step to the next input source in menu order.
    InputSourceNext,
    /// Workspace by 1-based index.
    Workspace(u8),
    MoveToWorkspace(u8),
    /// Index into [`KeyBindings::exec_commands`].
    Exec(u16),
    /// A volume, brightness or keyboard-backlight key, for the overlay process; `fine` is the Shift+Alt step.
    Osd {
        key: OsdKey,
        fine: bool,
    },
}

/// The media keys the overlay process (`cupertino-osd`) handles.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum OsdKey {
    VolumeUp,
    VolumeDown,
    Mute,
    MicMute,
    BrightnessUp,
    BrightnessDown,
    KeyboardBrightnessUp,
    KeyboardBrightnessDown,
}

const NAMES: &[(&str, Action)] = &[
    ("switcher-next", Action::SwitcherNext),
    ("switcher-prev", Action::SwitcherPrev),
    ("cycle-windows-next", Action::CycleWindowsNext),
    ("cycle-windows-prev", Action::CycleWindowsPrev),
    ("quit-app", Action::QuitApp),
    ("close-window", Action::CloseWindow),
    ("minimize", Action::Minimize),
    ("minimize-or-restore", Action::MinimizeOrRestore),
    ("hide-app", Action::HideApp),
    ("hide-others", Action::HideOthers),
    ("toggle-zoom", Action::ToggleZoom),
    ("toggle-fullscreen", Action::ToggleFullscreen),
    ("tile-left", Action::TileLeft),
    ("tile-right", Action::TileRight),
    ("tile-fill", Action::TileFill),
    ("overview", Action::Overview),
    ("app-overview", Action::AppOverview),
    ("workspace-left", Action::WorkspaceLeft),
    ("workspace-right", Action::WorkspaceRight),
    ("move-to-workspace-left", Action::MoveToWorkspaceLeft),
    ("move-to-workspace-right", Action::MoveToWorkspaceRight),
    ("add-workspace", Action::AddWorkspace),
    ("spawn-terminal", Action::SpawnTerminal),
    ("screenshot", Action::Screenshot(ScreenshotRequest::Screen)),
    (
        "screenshot-clipboard",
        Action::Screenshot(ScreenshotRequest::ScreenToClipboard),
    ),
    (
        "screenshot-area",
        Action::Screenshot(ScreenshotRequest::Selection),
    ),
    (
        "screenshot-area-clipboard",
        Action::Screenshot(ScreenshotRequest::SelectionToClipboard),
    ),
    (
        "screenshot-toolbar",
        Action::Screenshot(ScreenshotRequest::Toolbar),
    ),
    ("quit-compositor", Action::QuitCompositor),
    ("show-desktop", Action::ShowDesktop),
    ("app-grid", Action::AppGrid),
    ("force-quit", Action::ForceQuit),
    ("launcher", Action::Launcher),
    ("focus-menu-bar", Action::FocusMenuBar),
    ("help-search", Action::HelpSearch),
    ("focus-dock", Action::FocusDock),
    ("toggle-dock-auto-hide", Action::ToggleDockAutoHide),
    ("notification-center", Action::NotificationCenter),
    ("control-panel", Action::ControlPanel),
    ("toggle-appearance", Action::ToggleAppearance),
    (
        "toggle-reduce-motion",
        Action::ToggleDisplayOption(DisplayOption::ReduceMotion),
    ),
    (
        "toggle-reduce-transparency",
        Action::ToggleDisplayOption(DisplayOption::ReduceTransparency),
    ),
    (
        "toggle-increase-contrast",
        Action::ToggleDisplayOption(DisplayOption::IncreaseContrast),
    ),
    (
        "toggle-differentiate-without-color",
        Action::ToggleDisplayOption(DisplayOption::DifferentiateWithoutColor),
    ),
    ("toggle-stage", Action::ToggleStage),
    ("lock", Action::Lock),
    ("power-key", Action::PowerKey),
    ("log-out", Action::LogOut),
    ("log-out-now", Action::LogOutNow),
    ("power-dialog", Action::PowerDialog),
    ("release-shortcuts", Action::ReleaseShortcuts),
    ("input-source-previous", Action::InputSourcePrevious),
    ("input-source-next", Action::InputSourceNext),
    (
        "volume-up",
        Action::Osd {
            key: OsdKey::VolumeUp,
            fine: false,
        },
    ),
    (
        "volume-up-fine",
        Action::Osd {
            key: OsdKey::VolumeUp,
            fine: true,
        },
    ),
    (
        "volume-down",
        Action::Osd {
            key: OsdKey::VolumeDown,
            fine: false,
        },
    ),
    (
        "volume-down-fine",
        Action::Osd {
            key: OsdKey::VolumeDown,
            fine: true,
        },
    ),
    (
        "volume-mute",
        Action::Osd {
            key: OsdKey::Mute,
            fine: false,
        },
    ),
    (
        "microphone-mute",
        Action::Osd {
            key: OsdKey::MicMute,
            fine: false,
        },
    ),
    (
        "brightness-up",
        Action::Osd {
            key: OsdKey::BrightnessUp,
            fine: false,
        },
    ),
    (
        "brightness-up-fine",
        Action::Osd {
            key: OsdKey::BrightnessUp,
            fine: true,
        },
    ),
    (
        "brightness-down",
        Action::Osd {
            key: OsdKey::BrightnessDown,
            fine: false,
        },
    ),
    (
        "brightness-down-fine",
        Action::Osd {
            key: OsdKey::BrightnessDown,
            fine: true,
        },
    ),
    (
        "keyboard-brightness-up",
        Action::Osd {
            key: OsdKey::KeyboardBrightnessUp,
            fine: false,
        },
    ),
    (
        "keyboard-brightness-up-fine",
        Action::Osd {
            key: OsdKey::KeyboardBrightnessUp,
            fine: true,
        },
    ),
    (
        "keyboard-brightness-down",
        Action::Osd {
            key: OsdKey::KeyboardBrightnessDown,
            fine: false,
        },
    ),
    (
        "keyboard-brightness-down-fine",
        Action::Osd {
            key: OsdKey::KeyboardBrightnessDown,
            fine: true,
        },
    ),
];

impl Action {
    /// Whether the compositor acts on the action whatever an application holds: the keys that leave the session, lock
    /// it, power the machine or release what an application holds are never given away. Every action is listed, so a
    /// new one is decided on when it is added.
    pub fn is_reserved(&self) -> bool {
        match self {
            Action::SwitchVt(_)
            | Action::Lock
            | Action::PowerKey
            | Action::PowerDialog
            | Action::QuitCompositor
            | Action::ReleaseShortcuts => true,
            Action::SwitcherNext
            | Action::SwitcherPrev
            | Action::CycleWindowsNext
            | Action::CycleWindowsPrev
            | Action::QuitApp
            | Action::CloseWindow
            | Action::Minimize
            | Action::MinimizeOrRestore
            | Action::HideApp
            | Action::HideOthers
            | Action::ToggleZoom
            | Action::ToggleFullscreen
            | Action::TileLeft
            | Action::TileRight
            | Action::TileFill
            | Action::Overview
            | Action::AppOverview
            | Action::WorkspaceLeft
            | Action::WorkspaceRight
            | Action::MoveToWorkspaceLeft
            | Action::MoveToWorkspaceRight
            | Action::AddWorkspace
            | Action::SpawnTerminal
            | Action::Screenshot(_)
            | Action::LogOut
            | Action::LogOutNow
            | Action::ShowDesktop
            | Action::AppGrid
            | Action::ForceQuit
            | Action::Launcher
            | Action::FocusMenuBar
            | Action::FocusDock
            | Action::ToggleDockAutoHide
            | Action::HelpSearch
            | Action::NotificationCenter
            | Action::ControlPanel
            | Action::ToggleAppearance
            | Action::ToggleDisplayOption(_)
            | Action::ToggleStage
            | Action::InputSourcePrevious
            | Action::InputSourceNext
            | Action::Workspace(_)
            | Action::MoveToWorkspace(_)
            | Action::Exec(_)
            | Action::Osd { .. } => false,
        }
    }

    pub fn from_name(name: &str) -> Option<Action> {
        if let Some(n) = name.strip_prefix("workspace:").and_then(|n| n.parse().ok()) {
            return Some(Action::Workspace(n));
        }
        if let Some(n) = name
            .strip_prefix("move-to-workspace:")
            .and_then(|n| n.parse().ok())
        {
            return Some(Action::MoveToWorkspace(n));
        }
        NAMES.iter().find(|(n, _)| *n == name).map(|(_, a)| *a)
    }
    pub fn name(&self) -> String {
        NAMES
            .iter()
            .find(|(_, a)| a == self)
            .map(|(n, _)| (*n).to_string())
            .unwrap_or_else(|| match self {
                Action::SwitchVt(n) => format!("switch-vt-{n}"),
                Action::Workspace(n) => format!("workspace:{n}"),
                Action::MoveToWorkspace(n) => format!("move-to-workspace:{n}"),
                Action::Exec(i) => format!("exec:{i}"),
                _ => "unknown".into(),
            })
    }
}

/// Configurable chord → action table.
#[derive(Clone, Debug)]
pub struct KeyBindings {
    entries: Vec<(Chord, Action)>,
    /// What the Super key runs when it is pressed and released alone.
    tap: Option<Action>,
    /// Commands of `exec:` bindings, indexed by [`Action::Exec`].
    exec: Vec<String>,
}

impl KeyBindings {
    pub fn empty() -> Self {
        Self {
            entries: Vec::new(),
            tap: None,
            exec: Vec::new(),
        }
    }

    /// Command line of an `Exec` binding.
    pub fn exec_command(&self, index: u16) -> Option<&str> {
        self.exec.get(usize::from(index)).map(String::as_str)
    }

    /// Bindings of an input-crate shortcut map (`cupertino-input`): its actions are translated into
    /// the compositor's actions, the ones without a counterpart (focus dock, dock auto-hide, move to next output)
    /// are skipped. Compositor-only defaults are added unless the chord is taken.
    pub fn from_shortcuts(map: &cupertino_input::ShortcutMap, profile: &str) -> Self {
        let mut k = KeyBindings::empty();
        for (combo, action) in map.bindings() {
            let Some(a) = k.convert(action) else { continue };
            let m = combo.modifiers;
            k.bind(
                Chord {
                    mods: Mods {
                        ctrl: m.ctrl,
                        alt: m.alt,
                        shift: m.shift,
                        logo: m.super_key,
                    },
                    sym: normalize_sym(combo.keysym),
                },
                a,
            );
        }
        // under `classic-pc` no chord of Ctrl+Alt and a character is taken: AltGr is often typed that way
        let add_workspace = if profile == "classic-pc" {
            "Super+Alt+n"
        } else {
            "Ctrl+Alt+n"
        };
        k.tap = map.tap().and_then(|a| k.convert(a));
        let extras: &[(&str, Action)] = &[
            ("Super+Return", Action::SpawnTerminal),
            ("Super+Ctrl+z", Action::ToggleZoom),
            (add_workspace, Action::AddWorkspace),
            ("Ctrl+Alt+BackSpace", Action::QuitCompositor),
        ];
        for (c, a) in extras {
            if let Ok(chord) = parse_chord(c) {
                if !k.entries.iter().any(|(e, _)| *e == chord) {
                    k.bind(chord, *a);
                }
            }
        }
        k.bind_media_keys();
        k
    }

    fn convert(&mut self, action: &cupertino_input::Action) -> Option<Action> {
        use cupertino_input::Action as I;
        Some(match action {
            I::CloseWindow => Action::CloseWindow,
            I::MinimizeWindow => Action::Minimize,
            I::MinimizeOrRestoreWindow => Action::MinimizeOrRestore,
            I::ZoomWindow => Action::ToggleZoom,
            I::ToggleFullscreen => Action::ToggleFullscreen,
            I::HideApp => Action::HideApp,
            I::HideOtherApps => Action::HideOthers,
            I::QuitApp => Action::QuitApp,
            I::SwitchApp => Action::SwitcherNext,
            I::SwitchAppReverse => Action::SwitcherPrev,
            I::CycleWindows => Action::CycleWindowsNext,
            I::CycleWindowsReverse => Action::CycleWindowsPrev,
            I::TileLeft => Action::TileLeft,
            I::TileRight => Action::TileRight,
            I::TileFill => Action::TileFill,
            I::Overview => Action::Overview,
            I::AppOverview => Action::AppOverview,
            I::ShowDesktop => Action::ShowDesktop,
            I::AppGrid => Action::AppGrid,
            I::ForceQuit => Action::ForceQuit,
            I::WorkspacePrevious => Action::WorkspaceLeft,
            I::WorkspaceNext => Action::WorkspaceRight,
            I::Workspace(n) => Action::Workspace(u8::try_from(*n).ok()?),
            I::MoveWindowToWorkspacePrevious => Action::MoveToWorkspaceLeft,
            I::MoveWindowToWorkspaceNext => Action::MoveToWorkspaceRight,
            I::MoveWindowToWorkspace(n) => Action::MoveToWorkspace(u8::try_from(*n).ok()?),
            I::Launcher => Action::Launcher,
            I::ControlPanel => Action::ControlPanel,
            I::NotificationCenter => Action::NotificationCenter,
            I::FocusMenuBar => Action::FocusMenuBar,
            I::HelpSearch => Action::HelpSearch,
            I::FocusDock => Action::FocusDock,
            I::ToggleDockAutoHide => Action::ToggleDockAutoHide,
            I::Lock => Action::Lock,
            I::ToggleAppearance => Action::ToggleAppearance,
            I::ToggleStage => Action::ToggleStage,
            I::PowerKey => Action::PowerKey,
            I::InputSourcePrevious => Action::InputSourcePrevious,
            I::InputSourceNext => Action::InputSourceNext,
            I::Logout => Action::LogOut,
            I::LogoutNow => Action::LogOutNow,
            I::PowerDialog => Action::PowerDialog,
            I::ReleaseShortcuts => Action::ReleaseShortcuts,
            I::Screenshot(r) => Action::Screenshot(*r),
            I::Exec(cmd) => {
                self.exec.push(cmd.clone());
                Action::Exec(u16::try_from(self.exec.len() - 1).ok()?)
            }
            I::MoveWindowToNextOutput => return None,
        })
    }

    /// Bind `chord` to `action`. Whatever the source of the binding, the person keeps a chord that releases the
    /// shortcuts: a release chord without Ctrl, Alt or Super is refused, and so is a binding that would take the last
    /// release chord away. Returns whether it was bound.
    pub fn bind(&mut self, chord: Chord, action: Action) -> bool {
        let weak = !(chord.mods.ctrl || chord.mods.alt || chord.mods.logo);
        let last_release = action != Action::ReleaseShortcuts
            && self
                .entries
                .iter()
                .any(|(c, a)| *c == chord && *a == Action::ReleaseShortcuts)
            && !self
                .entries
                .iter()
                .any(|(c, a)| *c != chord && *a == Action::ReleaseShortcuts);
        if (action == Action::ReleaseShortcuts && weak) || last_release {
            return false;
        }
        self.entries.retain(|(c, _)| *c != chord);
        self.entries.push((chord, action));
        true
    }

    pub fn bind_str(&mut self, chord: &str, action: Action) -> Result<(), String> {
        self.bind_checked(parse_chord(chord)?, action)
    }

    fn bind_checked(&mut self, chord: Chord, action: Action) -> Result<(), String> {
        if self.bind(chord, action) {
            Ok(())
        } else {
            Err("the chord that releases the shortcuts an application holds stays bound, and takes Ctrl, Alt or Super".into())
        }
    }

    /// [`KeyBindings::bind_str`] for the modifier profile `profile`.
    pub fn bind_str_for(
        &mut self,
        chord: &str,
        action: Action,
        profile: &str,
    ) -> Result<(), String> {
        self.bind_checked(parse_chord_for(chord, profile)?, action)
    }

    pub fn lookup(&self, sym: u32, mods: Mods) -> Option<Action> {
        let sym = normalize_sym(sym);
        if mods.ctrl && mods.alt && !mods.logo {
            if let Some(n) = (keysyms::KEY_F1..=keysyms::KEY_F12)
                .contains(&sym)
                .then(|| (sym - keysyms::KEY_F1 + 1) as i32)
            {
                return Some(Action::SwitchVt(n));
            }
        }
        self.entries
            .iter()
            .find(|(c, _)| c.sym == sym && c.mods == mods)
            .map(|(_, a)| *a)
    }

    pub fn entries(&self) -> &[(Chord, Action)] {
        &self.entries
    }

    /// Default bindings; the primary modifier is Super.
    pub fn defaults() -> Self {
        let mut k = KeyBindings::empty();
        let table: &[(&str, Action)] = &[
            ("Super+Tab", Action::SwitcherNext),
            ("Super+Shift+Tab", Action::SwitcherPrev),
            ("Super+grave", Action::CycleWindowsNext),
            ("Super+Shift+grave", Action::CycleWindowsPrev),
            ("Super+q", Action::QuitApp),
            ("Super+w", Action::CloseWindow),
            ("Super+m", Action::Minimize),
            ("Super+h", Action::HideApp),
            ("Super+Alt+h", Action::HideOthers),
            ("Super+Ctrl+f", Action::ToggleFullscreen),
            ("Super+Ctrl+z", Action::ToggleZoom),
            ("Super+Ctrl+Left", Action::TileLeft),
            ("Super+Ctrl+Right", Action::TileRight),
            ("Super+Ctrl+Up", Action::TileFill),
            ("Ctrl+Up", Action::Overview),
            ("Ctrl+Down", Action::AppOverview),
            ("Ctrl+Left", Action::WorkspaceLeft),
            ("Ctrl+Right", Action::WorkspaceRight),
            ("Ctrl+Shift+Left", Action::MoveToWorkspaceLeft),
            ("Ctrl+Shift+Right", Action::MoveToWorkspaceRight),
            ("Ctrl+Alt+n", Action::AddWorkspace),
            ("Super+Return", Action::SpawnTerminal),
            (
                "Super+Shift+s",
                Action::Screenshot(ScreenshotRequest::Screen),
            ),
            ("Ctrl+Alt+BackSpace", Action::QuitCompositor),
        ];
        for (c, a) in table {
            k.bind_str(c, *a).expect("default chords parse");
        }
        k.bind_media_keys();
        k
    }

    /// Bind the media keys to the overlay process where the chord is still free: plain for a whole step, with
    /// Shift+Alt for the fine step; mute has no fine form.
    fn bind_media_keys(&mut self) {
        let steps: &[(&str, OsdKey)] = &[
            ("XF86AudioRaiseVolume", OsdKey::VolumeUp),
            ("XF86AudioLowerVolume", OsdKey::VolumeDown),
            ("XF86MonBrightnessUp", OsdKey::BrightnessUp),
            ("XF86MonBrightnessDown", OsdKey::BrightnessDown),
            ("XF86KbdBrightnessUp", OsdKey::KeyboardBrightnessUp),
            ("XF86KbdBrightnessDown", OsdKey::KeyboardBrightnessDown),
        ];
        let mut media = vec![
            (
                "XF86AudioMute".to_string(),
                Action::Osd {
                    key: OsdKey::Mute,
                    fine: false,
                },
            ),
            (
                "XF86AudioMicMute".to_string(),
                Action::Osd {
                    key: OsdKey::MicMute,
                    fine: false,
                },
            ),
        ];
        for (sym, key) in steps {
            media.push((
                (*sym).to_string(),
                Action::Osd {
                    key: *key,
                    fine: false,
                },
            ));
            media.push((
                format!("Shift+Alt+{sym}"),
                Action::Osd {
                    key: *key,
                    fine: true,
                },
            ));
        }
        for (text, action) in media {
            let chord = parse_chord(&text).expect("media chords parse");
            if !self.entries.iter().any(|(e, _)| *e == chord) {
                self.bind(chord, action);
            }
        }
    }
}

impl Default for KeyBindings {
    fn default() -> Self {
        Self::defaults()
    }
}

/// Shell-level operations an action can request besides model commands.
#[derive(Clone, Debug, PartialEq)]
pub enum ShellOp {
    ToggleOverview,
    ToggleAppOverview,
    /// Escape while the overview is open (its other keys are the dock's).
    CloseOverview,
    SpawnTerminal,
    /// Start the screenshot process.
    Screenshot(ScreenshotRequest),
    Quit,
    SwitchVt(i32),
    ToggleShowDesktop,
    OpenForceQuit,
    /// Open or close the application grid (the dock process draws it).
    ToggleAppGrid,
    /// Open or close the launcher panel.
    ToggleLauncher,
    /// Open or close the notification centre list.
    ToggleNotificationCenter,
    /// Open or close the control panel.
    ToggleControlPanel,
    /// Switch between the light and the dark appearance.
    ToggleAppearance,
    /// Turn an accessibility display option on or off.
    ToggleDisplayOption(DisplayOption),
    /// Input-source switcher: `menu` selects the menu-order pair member, `step` the direction.
    InputSource {
        menu: bool,
        step: cupertino_keyboard::Step,
    },
    FocusMenuBar,
    /// Open the Help menu of the active output's bar with its search field focused.
    HelpSearch,
    /// The dock of the output under the pointer takes the keyboard.
    FocusDock,
    /// Turn the docks' auto-hide on or off.
    ToggleDockAutoHide,
    /// Tell the window managers the stage shortcut was pressed.
    ToggleStage,
    /// Run the command of an `exec:` binding.
    Exec(u16),
    /// Forward a media key to the overlay process.
    Osd {
        key: OsdKey,
        fine: bool,
    },
    /// Ask the login process to lock the session.
    Lock,
    /// Ask the login process whether to log out.
    LogOut,
    /// Have the login process log out without asking.
    LogOutNow,
    /// Open the login process's power panel.
    PowerDialog,
    /// End what an application holds of the keys and the pointer.
    ReleaseShortcuts,
    /// Tell the login process the power key went down (`true`) or up. It times the hold.
    PowerKey {
        pressed: bool,
    },
    /// Tell the portal the chord of one of its shortcuts went down (`true`) or its key up.
    GlobalShortcut {
        key: GlobalKey,
        pressed: bool,
    },
}

/// A shortcut the portal bound ([`crate::global_shortcuts`]): the binding object it came from and the id it named.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct GlobalKey {
    pub owner: u32,
    pub id: u32,
}

/// Result of translating one action.
#[derive(Clone, Debug, Default)]
pub struct ActionPlan {
    pub commands: Vec<Command>,
    pub ops: Vec<ShellOp>,
}

impl ActionPlan {
    pub fn is_empty(&self) -> bool {
        self.commands.is_empty() && self.ops.is_empty()
    }

    /// Whether holding the key that produced this plan repeats it: plans that step a selection or a level (switcher,
    /// window cycling, relative workspace switch, input-source steps, volume and brightness). The overview's
    /// navigation keys reach the dock's surface, which repeats them as any client does.
    /// Toggles, launches and one-shot commands do not repeat. The design repeats every key except its exclusion
    /// list; which shortcut handlers act on repeats is not known, so this set is approximate.
    pub fn repeats(&self) -> bool {
        let command = |c: &Command| {
            matches!(
                c,
                Command::SwitcherOpen { .. }
                    | Command::SwitcherStep { .. }
                    | Command::CycleWindows { .. }
                    | Command::SwitchWorkspaceRelative { .. }
            )
        };
        let op = |o: &ShellOp| match o {
            ShellOp::InputSource { .. } => true,
            ShellOp::Osd { key, .. } => !matches!(key, OsdKey::Mute | OsdKey::MicMute),
            _ => false,
        };
        !self.is_empty() && self.commands.iter().all(command) && self.ops.iter().all(op)
    }
}

/// Model state an action needs to know about (filled in by the compositor from the model).
#[derive(Clone, Debug, Default)]
pub struct ActionContext {
    pub key_window: Option<WindowId>,
    pub front_app: Option<AppId>,
    pub front_app_windows: Vec<WindowId>,
    pub output: Option<OutputId>,
    pub switcher_open: bool,
    pub overview_open: bool,
    /// Workspace ids of the output, in order, and the index of the current one.
    pub workspaces: Vec<cupertino_wm::WorkspaceId>,
    pub current_workspace: usize,
    /// The session is locked: only VT switching stays bound.
    pub locked: bool,
    /// An application holds the keys of the focused window: only the reserved actions stay the compositor's.
    pub input_held: bool,
    /// An application holds the pointer. The chord that releases the keys and the pointer acts only while one of the
    /// two is held, and is the application's key otherwise.
    pub pointer_held: bool,
    /// The key window is in full screen: the full-screen toggle leaves it.
    pub key_window_fullscreen: bool,
    /// The command key's W goes to the key window's application, which has the keyboard, as the chord it closes its
    /// front tab or document with ([`crate::kbd::KeyboardSettings::close_goes_to_app`]): the close-window binding on that
    /// chord leaves the key to the application.
    pub close_goes_to_app: bool,
}

/// Translate an action into commands and operations.
pub fn translate_action(action: Action, ctx: &ActionContext) -> ActionPlan {
    let mut plan = ActionPlan::default();
    let window = ctx.key_window;
    let app = ctx.front_app;
    let out = ctx.output;
    match action {
        Action::SwitcherNext | Action::SwitcherPrev => plan.commands.push(Command::SwitcherOpen {
            backward: action == Action::SwitcherPrev,
        }),
        Action::CycleWindowsNext => plan
            .commands
            .push(Command::CycleWindows { backward: false }),
        Action::CycleWindowsPrev => plan.commands.push(Command::CycleWindows { backward: true }),
        Action::QuitApp => {
            plan.commands.extend(
                ctx.front_app_windows
                    .iter()
                    .map(|w| Command::RequestClose { window: *w }),
            );
        }
        Action::CloseWindow => plan
            .commands
            .extend(window.map(|window| Command::RequestClose { window })),
        Action::Minimize => plan.commands.extend(window.map(|window| Command::Minimize {
            window,
            target: None,
        })),
        Action::MinimizeOrRestore => plan
            .commands
            .extend(window.map(|window| Command::MinimizeOrRestore { window })),
        Action::HideApp => plan
            .commands
            .extend(app.map(|app| Command::HideApp { app })),
        Action::HideOthers => plan
            .commands
            .extend(app.map(|app| Command::HideOthers { app })),
        Action::ToggleZoom => plan
            .commands
            .extend(window.map(|window| Command::ToggleZoom { window })),
        // leaving is never refused, so a window full-screened by its client always has a way out
        Action::ToggleFullscreen if ctx.key_window_fullscreen => plan
            .commands
            .extend(window.map(|window| Command::ExitFullscreen { window })),
        Action::ToggleFullscreen => {
            plan.commands
                .extend(window.map(|window| Command::Fullscreen {
                    requested_by: cupertino_wm::Requester::User,
                    window,
                    output: None,
                }))
        }
        Action::TileLeft => plan.commands.extend(window.map(|window| Command::Tile {
            window,
            kind: TileKind::Left,
        })),
        Action::TileRight => plan.commands.extend(window.map(|window| Command::Tile {
            window,
            kind: TileKind::Right,
        })),
        Action::TileFill => plan.commands.extend(window.map(|window| Command::Tile {
            window,
            kind: TileKind::Fill,
        })),
        Action::Overview => plan.ops.push(ShellOp::ToggleOverview),
        Action::AppOverview => plan.ops.push(ShellOp::ToggleAppOverview),
        Action::WorkspaceLeft | Action::WorkspaceRight => {
            let delta = if action == Action::WorkspaceLeft {
                -1
            } else {
                1
            };
            plan.commands
                .extend(out.map(|output| Command::SwitchWorkspaceRelative { output, delta }));
        }
        Action::MoveToWorkspaceLeft | Action::MoveToWorkspaceRight => {
            let delta: i64 = if action == Action::MoveToWorkspaceLeft {
                -1
            } else {
                1
            };
            let target = ctx.current_workspace as i64 + delta;
            if let (Some(window), Some(ws)) = (
                window,
                usize::try_from(target)
                    .ok()
                    .and_then(|i| ctx.workspaces.get(i)),
            ) {
                plan.commands.push(Command::MoveWindowToWorkspace {
                    window,
                    workspace: *ws,
                });
                if let Some(output) = out {
                    plan.commands.push(Command::SwitchWorkspace {
                        output,
                        workspace: *ws,
                    });
                }
            }
        }
        Action::AddWorkspace => plan
            .commands
            .extend(out.map(|output| Command::AddWorkspace { output })),
        Action::SpawnTerminal => plan.ops.push(ShellOp::SpawnTerminal),
        Action::Screenshot(r) => plan.ops.push(ShellOp::Screenshot(r)),
        Action::QuitCompositor => plan.ops.push(ShellOp::Quit),
        Action::SwitchVt(n) => plan.ops.push(ShellOp::SwitchVt(n)),
        Action::ShowDesktop => plan.ops.push(ShellOp::ToggleShowDesktop),
        Action::AppGrid => plan.ops.push(ShellOp::ToggleAppGrid),
        Action::ForceQuit => plan.ops.push(ShellOp::OpenForceQuit),
        Action::Launcher => plan.ops.push(ShellOp::ToggleLauncher),
        Action::InputSourcePrevious => plan.ops.push(ShellOp::InputSource {
            menu: false,
            step: cupertino_keyboard::Step::Next,
        }),
        Action::InputSourceNext => plan.ops.push(ShellOp::InputSource {
            menu: true,
            step: cupertino_keyboard::Step::Next,
        }),
        Action::FocusMenuBar => plan.ops.push(ShellOp::FocusMenuBar),
        Action::HelpSearch => plan.ops.push(ShellOp::HelpSearch),
        Action::FocusDock => plan.ops.push(ShellOp::FocusDock),
        Action::ToggleDockAutoHide => plan.ops.push(ShellOp::ToggleDockAutoHide),
        Action::NotificationCenter => plan.ops.push(ShellOp::ToggleNotificationCenter),
        Action::ControlPanel => plan.ops.push(ShellOp::ToggleControlPanel),
        Action::ToggleAppearance => plan.ops.push(ShellOp::ToggleAppearance),
        Action::ToggleDisplayOption(option) => plan.ops.push(ShellOp::ToggleDisplayOption(option)),
        Action::ToggleStage => plan.ops.push(ShellOp::ToggleStage),
        Action::Lock => plan.ops.push(ShellOp::Lock),
        Action::PowerKey => plan.ops.push(ShellOp::PowerKey { pressed: true }),
        Action::LogOut => plan.ops.push(ShellOp::LogOut),
        Action::LogOutNow => plan.ops.push(ShellOp::LogOutNow),
        Action::PowerDialog => plan.ops.push(ShellOp::PowerDialog),
        Action::ReleaseShortcuts => plan.ops.push(ShellOp::ReleaseShortcuts),
        Action::Exec(i) => plan.ops.push(ShellOp::Exec(i)),
        Action::Osd { key, fine } => plan.ops.push(ShellOp::Osd { key, fine }),
        Action::Workspace(n) => {
            if let (Some(output), Some(ws)) = (
                out,
                usize::from(n)
                    .checked_sub(1)
                    .and_then(|i| ctx.workspaces.get(i)),
            ) {
                plan.commands.push(Command::SwitchWorkspace {
                    output,
                    workspace: *ws,
                });
            }
        }
        Action::MoveToWorkspace(n) => {
            if let (Some(window), Some(ws)) = (
                window,
                usize::from(n)
                    .checked_sub(1)
                    .and_then(|i| ctx.workspaces.get(i)),
            ) {
                plan.commands.push(Command::MoveWindowToWorkspace {
                    window,
                    workspace: *ws,
                });
                if let Some(output) = out {
                    plan.commands.push(Command::SwitchWorkspace {
                        output,
                        workspace: *ws,
                    });
                }
            }
        }
    }
    plan
}

/// One key event as seen by the router.
#[derive(Clone, Copy, Debug)]
pub struct KeyEvent {
    pub sym: u32,
    pub pressed: bool,
    pub mods: Mods,
    /// A press the compositor's key repeat produced.
    pub repeat: bool,
    /// With Shift down, the symbol the key types at its shift level, which a chord matches when the key's own symbol
    /// matches none: Shift+7 types `/` on a German layout, so it is Shift+slash there.
    pub shifted: Option<u32>,
    /// The Alt of `mods` is AltGr alone, not the Option key ([`crate::kbd::KeyboardSettings::alt_gr`]): the server's
    /// bindings take it for Alt, the applications' shortcuts do not.
    pub alt_gr: bool,
}

/// What a key does while the application switcher is open (key table), or `None`
/// for a key without a switcher action. Tab steps forward (with Shift back), backtick and the left
/// arrow back, the right arrow forward, Home and Page Up to the first entry, End and Page Down to the
/// last; Return and Space commit, Escape and `.` cancel; the up and down arrows, `1` and `! commit
/// and open the application overview of the selected application; Q quits it, H hides or shows it.
/// Only a fresh press wraps the selection round the ends; a repeat stops there. The arrows are not
/// mirrored for right-to-left layouts (the design does).
pub fn switcher_key(sym: u32, mods: Mods, repeat: bool) -> Option<ActionPlan> {
    use cupertino_wm::SwitcherStep as S;
    let step = |step: S| ActionPlan {
        commands: vec![Command::SwitcherStep { step, repeat }],
        ops: Vec::new(),
    };
    let command = |c: Command| ActionPlan {
        commands: vec![c],
        ops: Vec::new(),
    };
    Some(match sym {
        keysyms::KEY_Tab if mods.shift => step(S::Previous),
        keysyms::KEY_Tab | keysyms::KEY_Right => step(S::Next),
        keysyms::KEY_grave | keysyms::KEY_Left => step(S::Previous),
        keysyms::KEY_Home | keysyms::KEY_Page_Up => step(S::First),
        keysyms::KEY_End | keysyms::KEY_Page_Down => step(S::Last),
        keysyms::KEY_Return | keysyms::KEY_KP_Enter | keysyms::KEY_space => {
            command(Command::SwitcherCommit)
        }
        keysyms::KEY_Escape | keysyms::KEY_period => command(Command::SwitcherCancel),
        keysyms::KEY_Up | keysyms::KEY_Down | keysyms::KEY_1 | keysyms::KEY_exclam => ActionPlan {
            commands: vec![Command::SwitcherCommit],
            ops: vec![ShellOp::ToggleAppOverview],
        },
        keysyms::KEY_q => command(Command::SwitcherQuit),
        keysyms::KEY_h => command(Command::SwitcherToggleHide),
        _ => return None,
    })
}

#[derive(Clone, Debug)]
pub enum KeyOutcome {
    /// Deliver to the focused client.
    Forward,
    /// Handled by the compositor.
    Consumed(ActionPlan),
}

/// Maps key events to plans. Keeps track of consumed keys (their releases are swallowed too).
#[derive(Debug, Default)]
pub struct KeyRouter {
    pub bindings: KeyBindings,
    /// The modifiers of the chord that opened the switcher: releasing them commits it.
    switcher_hold: Option<Mods>,
    /// Keys the compositor consumed and has not seen released, each with the operation its release runs.
    suppressed: HashMap<u32, Option<ShellOp>>,
    /// The chords the portal bound for applications. They act only where no binding of [`KeyRouter::bindings`] does.
    pub global: HashMap<Chord, GlobalKey>,
}

impl KeyRouter {
    pub fn new(bindings: KeyBindings) -> Self {
        Self {
            bindings,
            switcher_hold: None,
            suppressed: HashMap::new(),
            global: HashMap::new(),
        }
    }

    /// Swallow the release of a key the compositor consumed outside the router (menu navigation).
    pub fn swallow(&mut self, sym: u32) {
        self.suppressed.insert(normalize_sym(sym), None);
    }

    fn lookup(&self, ev: &KeyEvent) -> Option<Action> {
        self.bindings.lookup(ev.sym, ev.mods).or_else(|| {
            ev.shifted
                .and_then(|sym| self.bindings.lookup(sym, ev.mods))
        })
    }

    /// The portal's shortcut `ev` (its symbol normalised as `sym`) stands for: the chord, else with Shift down the
    /// symbol the key types at its shift level, with Shift and without it, as the Shift typed the symbol (Ctrl+Shift+=
    /// is Ctrl+plus on a US layout). A modifier or lock key is none, so the focused client always sees its modifiers,
    /// and nothing is while AltGr is held, so AltGr and a key types its symbol in the focused client.
    fn global_key(&self, ev: &KeyEvent, sym: u32) -> Option<GlobalKey> {
        if ev.alt_gr || crate::global_shortcuts::is_modifier_key(sym) {
            return None;
        }
        let get = |mods: Mods, sym: u32| self.global.get(&Chord { mods, sym }).copied();
        get(ev.mods, sym).or_else(|| {
            let shifted = normalize_sym(ev.shifted?);
            if shifted == sym {
                return None;
            }
            get(ev.mods, shifted).or_else(|| {
                get(
                    Mods {
                        shift: false,
                        ..ev.mods
                    },
                    shifted,
                )
            })
        })
    }

    /// Desktop controllers cannot trigger reserved session actions or log out through a binding.
    pub fn control_reserved(&self, event: &KeyEvent) -> bool {
        self.lookup(event).is_some_and(|action| {
            action.is_reserved() || matches!(action, Action::LogOut | Action::LogOutNow)
        })
    }

    /// Route a key press or release.
    pub fn key(&mut self, ev: KeyEvent, ctx: &ActionContext) -> KeyOutcome {
        let sym = normalize_sym(ev.sym);
        if !ev.pressed {
            return match self.suppressed.remove(&sym) {
                Some(release) => KeyOutcome::Consumed(ActionPlan {
                    ops: release.into_iter().collect(),
                    ..Default::default()
                }),
                None => KeyOutcome::Forward,
            };
        }
        // a press of a key whose release is an operation is a repeat of the press already acted on
        if self.suppressed.get(&sym).is_some_and(Option::is_some) {
            return KeyOutcome::Consumed(ActionPlan::default());
        }
        // while the session is locked no shortcut of the desktop or its applications acts: every key goes to the lock
        // screen, but the switch to another console. The screen reader's Super+Alt+S is not a binding here: it is matched
        // before the router (`Cupertino::screen_reader_key`) and acts while the session is locked too, on purpose, so a
        // person who needs the reader to unlock can turn it on at the lock screen.
        if ctx.locked {
            return match self.lookup(&ev) {
                Some(Action::SwitchVt(n)) => {
                    self.suppressed.insert(sym, None);
                    KeyOutcome::Consumed(translate_action(Action::SwitchVt(n), ctx))
                }
                _ => KeyOutcome::Forward,
            };
        }
        // an application holds the shortcuts: only the reserved actions stay the compositor's, matched on the physical
        // chord before anything else looks at the key
        if ctx.input_held {
            return match self.lookup(&ev).filter(Action::is_reserved) {
                Some(action) => {
                    self.suppressed.insert(sym, None);
                    KeyOutcome::Consumed(translate_action(action, ctx))
                }
                None => KeyOutcome::Forward,
            };
        }
        // the overview's navigation keys reach the dock's keyboard-exclusive layer; Escape stays
        // here so the overview leaves without a dock too
        if ctx.overview_open && sym == keysyms::KEY_Escape && ev.mods == Mods::NONE {
            self.suppressed.insert(sym, None);
            return KeyOutcome::Consumed(ActionPlan {
                ops: vec![ShellOp::CloseOverview],
                ..Default::default()
            });
        }
        // the open switcher takes every key but the modifiers: its own keys act on it, the others
        // do nothing (the design turns the other global shortcuts off while it is up)
        if ctx.switcher_open && !crate::overlays::is_modifier_sym(sym) {
            self.suppressed.insert(sym, None);
            return KeyOutcome::Consumed(switcher_key(sym, ev.mods, ev.repeat).unwrap_or_default());
        }
        match self
            .lookup(&ev)
            .filter(|action| ctx.pointer_held || *action != Action::ReleaseShortcuts)
        {
            // a third-party application closes its front tab or document with its own chord, which the rewrite gives it
            // for the command key's W; any other chord bound to close-window still closes the window
            Some(Action::CloseWindow) if ctx.close_goes_to_app && is_command_w(&ev) => {
                KeyOutcome::Forward
            }
            Some(action) => {
                if matches!(action, Action::SwitcherNext | Action::SwitcherPrev) {
                    self.switcher_hold = Some(Mods {
                        shift: false,
                        ..ev.mods
                    });
                }
                let release =
                    (action == Action::PowerKey).then_some(ShellOp::PowerKey { pressed: false });
                self.suppressed.insert(sym, release);
                KeyOutcome::Consumed(translate_action(action, ctx))
            }
            // a shortcut an application bound through the portal: the focused client sees neither the press nor the
            // release, and the release tells the portal the shortcut ended
            None => match self.global_key(&ev, sym) {
                Some(key) => {
                    let release = ShellOp::GlobalShortcut {
                        key,
                        pressed: false,
                    };
                    self.suppressed.insert(sym, Some(release));
                    KeyOutcome::Consumed(ActionPlan {
                        ops: vec![ShellOp::GlobalShortcut { key, pressed: true }],
                        ..Default::default()
                    })
                }
                None => KeyOutcome::Forward,
            },
        }
    }

    /// The plan of the Super key tapped alone, when one is bound and the session is not locked.
    pub fn tap_plan(&self, ctx: &ActionContext) -> Option<ActionPlan> {
        if ctx.locked {
            return None;
        }
        self.bindings
            .tap
            .map(|action| translate_action(action, ctx))
    }

    /// Modifier state changed: releasing the modifiers of the chord that opened the switcher (Super for the Super+Tab
    /// of `cupertino`, Alt for the Alt+Tab of `classic-pc`) commits it.
    pub fn modifiers_changed(&mut self, mods: Mods, ctx: &ActionContext) -> ActionPlan {
        let mut plan = ActionPlan::default();
        if !ctx.switcher_open {
            self.switcher_hold = None;
            return plan;
        }
        // a switcher opened without a chord is held by the Super key
        let hold = self.switcher_hold.unwrap_or(Mods {
            logo: true,
            ..Mods::NONE
        });
        let held = (hold.ctrl && mods.ctrl) || (hold.alt && mods.alt) || (hold.logo && mods.logo);
        if !held && !ctx.locked {
            plan.commands.push(Command::SwitcherCommit);
        }
        plan
    }
}
