//! Input routing layer: key chords, bindings, actions and their translation into window-model
//! commands. Pure logic (no Wayland types) so it is unit-testable and can be extended by the
//! integration of the input crate: add `Action` variants, extend `translate_action`, or install
//! a different [`KeyBindings`] table.
//!
//! Flow: backend key event → [`KeyRouter::key`] → [`KeyOutcome::Consumed`] with an [`ActionPlan`]
//! (model commands + shell operations) or [`KeyOutcome::Forward`] to the focused client.
//! Pointer routing lives in `state.rs` (`Cupertino::pointer_*`) and uses the same plan type.

use std::collections::HashMap;

use cupertino_input::ScreenshotRequest;
use cupertino_defaults::accessibility::DisplayOption;
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
        Mods { ctrl: m.ctrl, alt: m.alt, shift: m.shift, logo: m.logo }
    }
}

impl Mods {
    pub const NONE: Mods = Mods { ctrl: false, alt: false, shift: false, logo: false };
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
/// `Shift`; the last token is an xkb keysym name (case-insensitive).
pub fn parse_chord(text: &str) -> Result<Chord, String> {
    let mut mods = Mods::NONE;
    let mut sym = None;
    for tok in text.split('+').map(str::trim).filter(|t| !t.is_empty()) {
        match tok.to_ascii_lowercase().as_str() {
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
    sym.map(|sym| Chord { mods, sym }).ok_or_else(|| format!("no key in {text:?}"))
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
    SwitchVt(i32),
    ShowDesktop,
    AppGrid,
    Launcher,
    FocusMenuBar,
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
    Osd { key: OsdKey, fine: bool },
}

/// The media keys the overlay process (`cupertino-osd`) handles.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum OsdKey {
    VolumeUp,
    VolumeDown,
    Mute,
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
    ("screenshot-clipboard", Action::Screenshot(ScreenshotRequest::ScreenToClipboard)),
    ("screenshot-area", Action::Screenshot(ScreenshotRequest::Selection)),
    ("screenshot-area-clipboard", Action::Screenshot(ScreenshotRequest::SelectionToClipboard)),
    ("screenshot-toolbar", Action::Screenshot(ScreenshotRequest::Toolbar)),
    ("quit-compositor", Action::QuitCompositor),
    ("show-desktop", Action::ShowDesktop),
    ("app-grid", Action::AppGrid),
    ("launcher", Action::Launcher),
    ("focus-menu-bar", Action::FocusMenuBar),
    ("toggle-dock-auto-hide", Action::ToggleDockAutoHide),
    ("notification-center", Action::NotificationCenter),
    ("control-panel", Action::ControlPanel),
    ("toggle-appearance", Action::ToggleAppearance),
    ("toggle-reduce-motion", Action::ToggleDisplayOption(DisplayOption::ReduceMotion)),
    ("toggle-reduce-transparency", Action::ToggleDisplayOption(DisplayOption::ReduceTransparency)),
    ("toggle-increase-contrast", Action::ToggleDisplayOption(DisplayOption::IncreaseContrast)),
    ("toggle-differentiate-without-color", Action::ToggleDisplayOption(DisplayOption::DifferentiateWithoutColor)),
    ("toggle-stage", Action::ToggleStage),
    ("lock", Action::Lock),
    ("power-key", Action::PowerKey),
    ("input-source-previous", Action::InputSourcePrevious),
    ("input-source-next", Action::InputSourceNext),
    ("volume-up", Action::Osd { key: OsdKey::VolumeUp, fine: false }),
    ("volume-up-fine", Action::Osd { key: OsdKey::VolumeUp, fine: true }),
    ("volume-down", Action::Osd { key: OsdKey::VolumeDown, fine: false }),
    ("volume-down-fine", Action::Osd { key: OsdKey::VolumeDown, fine: true }),
    ("volume-mute", Action::Osd { key: OsdKey::Mute, fine: false }),
    ("brightness-up", Action::Osd { key: OsdKey::BrightnessUp, fine: false }),
    ("brightness-up-fine", Action::Osd { key: OsdKey::BrightnessUp, fine: true }),
    ("brightness-down", Action::Osd { key: OsdKey::BrightnessDown, fine: false }),
    ("brightness-down-fine", Action::Osd { key: OsdKey::BrightnessDown, fine: true }),
    ("keyboard-brightness-up", Action::Osd { key: OsdKey::KeyboardBrightnessUp, fine: false }),
    ("keyboard-brightness-up-fine", Action::Osd { key: OsdKey::KeyboardBrightnessUp, fine: true }),
    ("keyboard-brightness-down", Action::Osd { key: OsdKey::KeyboardBrightnessDown, fine: false }),
    ("keyboard-brightness-down-fine", Action::Osd { key: OsdKey::KeyboardBrightnessDown, fine: true }),
];

impl Action {
    pub fn from_name(name: &str) -> Option<Action> {
        if let Some(n) = name.strip_prefix("workspace:").and_then(|n| n.parse().ok()) {
            return Some(Action::Workspace(n));
        }
        if let Some(n) = name.strip_prefix("move-to-workspace:").and_then(|n| n.parse().ok()) {
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
    /// Commands of `exec:` bindings, indexed by [`Action::Exec`].
    exec: Vec<String>,
}

impl KeyBindings {
    pub fn empty() -> Self {
        Self { entries: Vec::new(), exec: Vec::new() }
    }

    /// Command line of an `Exec` binding.
    pub fn exec_command(&self, index: u16) -> Option<&str> {
        self.exec.get(usize::from(index)).map(String::as_str)
    }

    /// Bindings of an input-crate shortcut map (`cupertino-input`): its actions are translated into
    /// the compositor's actions, the ones without a counterpart (focus dock, dock auto-hide, move to next output)
    /// are skipped. Compositor-only defaults are added unless the chord is taken.
    pub fn from_shortcuts(map: &cupertino_input::ShortcutMap) -> Self {
        let mut k = KeyBindings::empty();
        for (combo, action) in map.bindings() {
            let Some(a) = k.convert(action) else { continue };
            let m = combo.modifiers;
            k.bind(Chord { mods: Mods { ctrl: m.ctrl, alt: m.alt, shift: m.shift, logo: m.super_key }, sym: normalize_sym(combo.keysym) }, a);
        }
        let extras: &[(&str, Action)] = &[
            ("Super+Return", Action::SpawnTerminal),
            ("Super+Ctrl+z", Action::ToggleZoom),
            ("Ctrl+Alt+n", Action::AddWorkspace),
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
            I::ToggleDockAutoHide => Action::ToggleDockAutoHide,
            I::Lock => Action::Lock,
            I::ToggleAppearance => Action::ToggleAppearance,
            I::ToggleStage => Action::ToggleStage,
            I::PowerKey => Action::PowerKey,
            I::InputSourcePrevious => Action::InputSourcePrevious,
            I::InputSourceNext => Action::InputSourceNext,
            I::Logout => Action::QuitCompositor,
            I::Screenshot(r) => Action::Screenshot(*r),
            I::Exec(cmd) => {
                self.exec.push(cmd.clone());
                Action::Exec(u16::try_from(self.exec.len() - 1).ok()?)
            }
            I::MoveWindowToNextOutput | I::FocusDock => return None,
        })
    }

    pub fn bind(&mut self, chord: Chord, action: Action) {
        self.entries.retain(|(c, _)| *c != chord);
        self.entries.push((chord, action));
    }

    pub fn bind_str(&mut self, chord: &str, action: Action) -> Result<(), String> {
        self.bind(parse_chord(chord)?, action);
        Ok(())
    }

    pub fn lookup(&self, sym: u32, mods: Mods) -> Option<Action> {
        let sym = normalize_sym(sym);
        if mods.ctrl && mods.alt && !mods.logo {
            if let Some(n) = (keysyms::KEY_F1..=keysyms::KEY_F12).contains(&sym).then(|| (sym - keysyms::KEY_F1 + 1) as i32) {
                return Some(Action::SwitchVt(n));
            }
        }
        self.entries.iter().find(|(c, _)| c.sym == sym && c.mods == mods).map(|(_, a)| *a)
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
            ("Super+Shift+s", Action::Screenshot(ScreenshotRequest::Screen)),
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
        let mut media = vec![("XF86AudioMute".to_string(), Action::Osd { key: OsdKey::Mute, fine: false })];
        for (sym, key) in steps {
            media.push(((*sym).to_string(), Action::Osd { key: *key, fine: false }));
            media.push((format!("Shift+Alt+{sym}"), Action::Osd { key: *key, fine: true }));
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
    InputSource { menu: bool, step: cupertino_keyboard::Step },
    FocusMenuBar,
    /// Turn the docks' auto-hide on or off.
    ToggleDockAutoHide,
    /// Tell the window managers the stage shortcut was pressed.
    ToggleStage,
    /// Run the command of an `exec:` binding.
    Exec(u16),
    /// Forward a media key to the overlay process.
    Osd { key: OsdKey, fine: bool },
    /// Ask the login process to lock the session.
    Lock,
    /// Tell the login process the power key went down (`true`) or up. It times the hold.
    PowerKey { pressed: bool },
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
        let command = |c: &Command| matches!(c, Command::SwitcherOpen { .. } | Command::SwitcherStep { .. } | Command::CycleWindows { .. } | Command::SwitchWorkspaceRelative { .. });
        let op = |o: &ShellOp| match o {
            ShellOp::InputSource { .. } => true,
            ShellOp::Osd { key, .. } => !matches!(key, OsdKey::Mute),
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
}

/// Translate an action into commands and operations.
pub fn translate_action(action: Action, ctx: &ActionContext) -> ActionPlan {
    let mut plan = ActionPlan::default();
    let window = ctx.key_window;
    let app = ctx.front_app;
    let out = ctx.output;
    match action {
        Action::SwitcherNext | Action::SwitcherPrev => plan.commands.push(Command::SwitcherOpen { backward: action == Action::SwitcherPrev }),
        Action::CycleWindowsNext => plan.commands.push(Command::CycleWindows { backward: false }),
        Action::CycleWindowsPrev => plan.commands.push(Command::CycleWindows { backward: true }),
        Action::QuitApp => {
            plan.commands.extend(ctx.front_app_windows.iter().map(|w| Command::RequestClose { window: *w }));
        }
        Action::CloseWindow => plan.commands.extend(window.map(|window| Command::RequestClose { window })),
        Action::Minimize => plan.commands.extend(window.map(|window| Command::Minimize { window, target: None })),
        Action::HideApp => plan.commands.extend(app.map(|app| Command::HideApp { app })),
        Action::HideOthers => plan.commands.extend(app.map(|app| Command::HideOthers { app })),
        Action::ToggleZoom => plan.commands.extend(window.map(|window| Command::ToggleZoom { window })),
        Action::ToggleFullscreen => plan.commands.extend(window.map(|window| Command::Fullscreen { window, output: None })),
        Action::TileLeft => plan.commands.extend(window.map(|window| Command::Tile { window, kind: TileKind::Left })),
        Action::TileRight => plan.commands.extend(window.map(|window| Command::Tile { window, kind: TileKind::Right })),
        Action::TileFill => plan.commands.extend(window.map(|window| Command::Tile { window, kind: TileKind::Fill })),
        Action::Overview => plan.ops.push(ShellOp::ToggleOverview),
        Action::AppOverview => plan.ops.push(ShellOp::ToggleAppOverview),
        Action::WorkspaceLeft | Action::WorkspaceRight => {
            let delta = if action == Action::WorkspaceLeft { -1 } else { 1 };
            plan.commands.extend(out.map(|output| Command::SwitchWorkspaceRelative { output, delta }));
        }
        Action::MoveToWorkspaceLeft | Action::MoveToWorkspaceRight => {
            let delta: i64 = if action == Action::MoveToWorkspaceLeft { -1 } else { 1 };
            let target = ctx.current_workspace as i64 + delta;
            if let (Some(window), Some(ws)) = (window, usize::try_from(target).ok().and_then(|i| ctx.workspaces.get(i))) {
                plan.commands.push(Command::MoveWindowToWorkspace { window, workspace: *ws });
                if let Some(output) = out {
                    plan.commands.push(Command::SwitchWorkspace { output, workspace: *ws });
                }
            }
        }
        Action::AddWorkspace => plan.commands.extend(out.map(|output| Command::AddWorkspace { output })),
        Action::SpawnTerminal => plan.ops.push(ShellOp::SpawnTerminal),
        Action::Screenshot(r) => plan.ops.push(ShellOp::Screenshot(r)),
        Action::QuitCompositor => plan.ops.push(ShellOp::Quit),
        Action::SwitchVt(n) => plan.ops.push(ShellOp::SwitchVt(n)),
        Action::ShowDesktop => plan.ops.push(ShellOp::ToggleShowDesktop),
        Action::AppGrid => plan.ops.push(ShellOp::ToggleAppGrid),
        Action::Launcher => plan.ops.push(ShellOp::ToggleLauncher),
        Action::InputSourcePrevious => plan.ops.push(ShellOp::InputSource { menu: false, step: cupertino_keyboard::Step::Next }),
        Action::InputSourceNext => plan.ops.push(ShellOp::InputSource { menu: true, step: cupertino_keyboard::Step::Next }),
        Action::FocusMenuBar => plan.ops.push(ShellOp::FocusMenuBar),
        Action::ToggleDockAutoHide => plan.ops.push(ShellOp::ToggleDockAutoHide),
        Action::NotificationCenter => plan.ops.push(ShellOp::ToggleNotificationCenter),
        Action::ControlPanel => plan.ops.push(ShellOp::ToggleControlPanel),
        Action::ToggleAppearance => plan.ops.push(ShellOp::ToggleAppearance),
        Action::ToggleDisplayOption(option) => plan.ops.push(ShellOp::ToggleDisplayOption(option)),
        Action::ToggleStage => plan.ops.push(ShellOp::ToggleStage),
        Action::Lock => plan.ops.push(ShellOp::Lock),
        Action::PowerKey => plan.ops.push(ShellOp::PowerKey { pressed: true }),
        Action::Exec(i) => plan.ops.push(ShellOp::Exec(i)),
        Action::Osd { key, fine } => plan.ops.push(ShellOp::Osd { key, fine }),
        Action::Workspace(n) => {
            if let (Some(output), Some(ws)) = (out, usize::from(n).checked_sub(1).and_then(|i| ctx.workspaces.get(i))) {
                plan.commands.push(Command::SwitchWorkspace { output, workspace: *ws });
            }
        }
        Action::MoveToWorkspace(n) => {
            if let (Some(window), Some(ws)) = (window, usize::from(n).checked_sub(1).and_then(|i| ctx.workspaces.get(i))) {
                plan.commands.push(Command::MoveWindowToWorkspace { window, workspace: *ws });
                if let Some(output) = out {
                    plan.commands.push(Command::SwitchWorkspace { output, workspace: *ws });
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
    let step = |step: S| ActionPlan { commands: vec![Command::SwitcherStep { step, repeat }], ops: Vec::new() };
    let command = |c: Command| ActionPlan { commands: vec![c], ops: Vec::new() };
    Some(match sym {
        keysyms::KEY_Tab if mods.shift => step(S::Previous),
        keysyms::KEY_Tab | keysyms::KEY_Right => step(S::Next),
        keysyms::KEY_grave | keysyms::KEY_Left => step(S::Previous),
        keysyms::KEY_Home | keysyms::KEY_Page_Up => step(S::First),
        keysyms::KEY_End | keysyms::KEY_Page_Down => step(S::Last),
        keysyms::KEY_Return | keysyms::KEY_KP_Enter | keysyms::KEY_space => command(Command::SwitcherCommit),
        keysyms::KEY_Escape | keysyms::KEY_period => command(Command::SwitcherCancel),
        keysyms::KEY_Up | keysyms::KEY_Down | keysyms::KEY_1 | keysyms::KEY_exclam => ActionPlan { commands: vec![Command::SwitcherCommit], ops: vec![ShellOp::ToggleAppOverview] },
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
    /// Keys the compositor consumed and has not seen released, each with the operation its release runs.
    suppressed: HashMap<u32, Option<ShellOp>>,
}

impl KeyRouter {
    pub fn new(bindings: KeyBindings) -> Self {
        Self { bindings, suppressed: HashMap::new() }
    }

    /// Swallow the release of a key the compositor consumed outside the router (menu navigation).
    pub fn swallow(&mut self, sym: u32) {
        self.suppressed.insert(normalize_sym(sym), None);
    }

    /// Route a key press or release.
    pub fn key(&mut self, ev: KeyEvent, ctx: &ActionContext) -> KeyOutcome {
        let sym = normalize_sym(ev.sym);
        if !ev.pressed {
            return match self.suppressed.remove(&sym) {
                Some(release) => KeyOutcome::Consumed(ActionPlan { ops: release.into_iter().collect(), ..Default::default() }),
                None => KeyOutcome::Forward,
            };
        }
        // a press of a key whose release is an operation is a repeat of the press already acted on
        if self.suppressed.get(&sym).is_some_and(Option::is_some) {
            return KeyOutcome::Consumed(ActionPlan::default());
        }
        if ctx.locked {
            return match self.bindings.lookup(ev.sym, ev.mods) {
                Some(Action::SwitchVt(n)) => {
                    self.suppressed.insert(sym, None);
                    KeyOutcome::Consumed(translate_action(Action::SwitchVt(n), ctx))
                }
                _ => KeyOutcome::Forward,
            };
        }
        // the overview's navigation keys reach the dock's keyboard-exclusive layer; Escape stays
        // here so the overview leaves without a dock too
        if ctx.overview_open && sym == keysyms::KEY_Escape && ev.mods == Mods::NONE {
            self.suppressed.insert(sym, None);
            return KeyOutcome::Consumed(ActionPlan { ops: vec![ShellOp::CloseOverview], ..Default::default() });
        }
        // the open switcher takes every key but the modifiers: its own keys act on it, the others
        // do nothing (the design turns the other global shortcuts off while it is up)
        if ctx.switcher_open && !crate::overlays::is_modifier_sym(sym) {
            self.suppressed.insert(sym, None);
            return KeyOutcome::Consumed(switcher_key(sym, ev.mods, ev.repeat).unwrap_or_default());
        }
        match self.bindings.lookup(ev.sym, ev.mods) {
            Some(action) => {
                let release = (action == Action::PowerKey).then_some(ShellOp::PowerKey { pressed: false });
                self.suppressed.insert(sym, release);
                KeyOutcome::Consumed(translate_action(action, ctx))
            }
            None => KeyOutcome::Forward,
        }
    }

    /// Modifier state changed: releasing the primary modifier commits an open switcher.
    pub fn modifiers_changed(&mut self, mods: Mods, ctx: &ActionContext) -> ActionPlan {
        let mut plan = ActionPlan::default();
        if ctx.switcher_open && !mods.logo && !ctx.locked {
            plan.commands.push(Command::SwitcherCommit);
        }
        plan
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use cupertino_wm::WorkspaceId;

    fn ctx() -> ActionContext {
        ActionContext {
            key_window: Some(WindowId(7)),
            front_app: Some(AppId(3)),
            front_app_windows: vec![WindowId(7), WindowId(8)],
            output: Some(OutputId(1)),
            workspaces: vec![WorkspaceId(1), WorkspaceId(2)],
            current_workspace: 0,
            ..Default::default()
        }
    }

    fn dbg<T: std::fmt::Debug>(v: &T) -> String {
        format!("{v:?}")
    }

    fn press(r: &mut KeyRouter, chord: &str, c: &ActionContext) -> KeyOutcome {
        let ch = parse_chord(chord).unwrap();
        r.key(KeyEvent { sym: ch.sym, pressed: true, mods: ch.mods, repeat: false }, c)
    }

    #[test]
    fn chords_parse() {
        let c = parse_chord("Super+Shift+Tab").unwrap();
        assert!(c.mods.logo && c.mods.shift && !c.mods.ctrl);
        assert_eq!(c.sym, keysyms::KEY_Tab);
        assert_eq!(parse_chord("ctrl+UP").unwrap().sym, keysyms::KEY_Up);
        assert!(parse_chord("Super+").is_err());
        assert!(parse_chord("Super+nonexistentkey").is_err());
        assert!(parse_chord("a+b").is_err());
    }

    #[test]
    fn shifted_tab_and_capitals_normalise() {
        assert_eq!(normalize_sym(keysyms::KEY_ISO_Left_Tab), keysyms::KEY_Tab);
        assert_eq!(normalize_sym(keysyms::KEY_Q), keysyms::KEY_q);
    }

    #[test]
    fn default_bindings_cover_the_required_set() {
        let b = KeyBindings::default();
        let look = |s: &str| {
            let c = parse_chord(s).unwrap();
            b.lookup(c.sym, c.mods)
        };
        assert_eq!(look("Super+Tab"), Some(Action::SwitcherNext));
        assert_eq!(look("Super+q"), Some(Action::QuitApp));
        assert_eq!(look("Super+w"), Some(Action::CloseWindow));
        assert_eq!(look("Super+m"), Some(Action::Minimize));
        assert_eq!(look("Super+h"), Some(Action::HideApp));
        assert_eq!(look("Ctrl+Up"), Some(Action::Overview));
        assert_eq!(look("Ctrl+Left"), Some(Action::WorkspaceLeft));
        assert_eq!(look("Ctrl+Right"), Some(Action::WorkspaceRight));
        assert_eq!(look("Super+Return"), Some(Action::SpawnTerminal));
        assert_eq!(look("Super+x"), None);
        assert_eq!(b.lookup(keysyms::KEY_F3, Mods { ctrl: true, alt: true, ..Mods::NONE }), Some(Action::SwitchVt(3)));
    }

    #[test]
    fn a_locked_session_keeps_only_virtual_terminal_switching() {
        let mut router = KeyRouter::new(KeyBindings::default());
        let locked = ActionContext { locked: true, ..ActionContext::default() };
        let press = |sym, mods| KeyEvent { sym, pressed: true, mods, repeat: false };
        let super_ = Mods { logo: true, ..Mods::NONE };
        assert!(matches!(router.key(press(keysyms::KEY_Return, super_), &locked), KeyOutcome::Forward), "Super+Return reaches the lock client");
        assert!(matches!(router.key(press(keysyms::KEY_Tab, super_), &locked), KeyOutcome::Forward));
        let vt = router.key(press(keysyms::KEY_F2, Mods { ctrl: true, alt: true, ..Mods::NONE }), &locked);
        assert!(matches!(vt, KeyOutcome::Consumed(plan) if dbg(&plan.ops) == dbg(&vec![ShellOp::SwitchVt(2)])));
    }

    #[test]
    fn rebinding_replaces_the_old_action() {
        let mut b = KeyBindings::default();
        b.bind_str("Super+Tab", Action::Overview).unwrap();
        let c = parse_chord("Super+Tab").unwrap();
        assert_eq!(b.lookup(c.sym, c.mods), Some(Action::Overview));
    }

    #[test]
    fn window_actions_become_commands() {
        let c = ctx();
        assert_eq!(dbg(&translate_action(Action::CloseWindow, &c).commands), dbg(&vec![Command::RequestClose { window: WindowId(7) }]));
        assert_eq!(dbg(&translate_action(Action::Minimize, &c).commands), dbg(&vec![Command::Minimize { window: WindowId(7), target: None }]));
        assert_eq!(dbg(&translate_action(Action::HideApp, &c).commands), dbg(&vec![Command::HideApp { app: AppId(3) }]));
        assert_eq!(dbg(&translate_action(Action::QuitApp, &c).commands), dbg(&vec![Command::RequestClose { window: WindowId(7) }, Command::RequestClose { window: WindowId(8) }]));
        assert_eq!(dbg(&translate_action(Action::WorkspaceRight, &c).commands), dbg(&vec![Command::SwitchWorkspaceRelative { output: OutputId(1), delta: 1 }]));
        let none = ActionContext::default();
        assert!(translate_action(Action::CloseWindow, &none).is_empty());
        assert_eq!(dbg(&translate_action(Action::SpawnTerminal, &none).ops), dbg(&vec![ShellOp::SpawnTerminal]));
    }

    #[test]
    fn move_to_workspace_respects_bounds() {
        let c = ctx();
        assert!(translate_action(Action::MoveToWorkspaceLeft, &c).is_empty());
        let plan = translate_action(Action::MoveToWorkspaceRight, &c);
        assert_eq!(dbg(&plan.commands[0]), dbg(&Command::MoveWindowToWorkspace { window: WindowId(7), workspace: WorkspaceId(2) }));
        assert_eq!(plan.commands.len(), 2);
    }

    #[test]
    fn switcher_opens_then_steps_and_commits_on_release() {
        use cupertino_wm::SwitcherStep;
        let mut r = KeyRouter::default();
        let mut c = ctx();
        let KeyOutcome::Consumed(p) = press(&mut r, "Super+Tab", &c) else { panic!() };
        assert_eq!(dbg(&p.commands), dbg(&vec![Command::SwitcherOpen { backward: false }]));
        assert!(p.repeats());
        c.switcher_open = true;
        let KeyOutcome::Consumed(p) = press(&mut r, "Super+Shift+Tab", &c) else { panic!() };
        assert_eq!(dbg(&p.commands), dbg(&vec![Command::SwitcherStep { step: SwitcherStep::Previous, repeat: false }]));
        let held = Mods { logo: true, ..Mods::NONE };
        assert!(r.modifiers_changed(held, &c).is_empty());
        assert_eq!(dbg(&r.modifiers_changed(Mods::NONE, &c).commands), dbg(&vec![Command::SwitcherCommit]));
        let KeyOutcome::Consumed(p) = press(&mut r, "Escape", &c) else { panic!() };
        assert_eq!(dbg(&p.commands), dbg(&vec![Command::SwitcherCancel]));
    }

    #[test]
    fn the_open_switcher_maps_its_key_table_and_swallows_other_keys() {
        use cupertino_wm::SwitcherStep as S;
        let mut c = ctx();
        c.switcher_open = true;
        let held = Mods { logo: true, ..Mods::NONE };
        let shifted = Mods { shift: true, ..held };
        let plan = |chord: &str, mods: Mods, repeat: bool| {
            let sym = parse_chord(chord).unwrap().sym;
            match KeyRouter::default().key(KeyEvent { sym, pressed: true, mods, repeat }, &c) {
                KeyOutcome::Consumed(p) => (dbg(&p.commands), dbg(&p.ops)),
                KeyOutcome::Forward => panic!("{chord} reached the client"),
            }
        };
        let step = |step, repeat| dbg(&vec![Command::SwitcherStep { step, repeat }]);
        assert_eq!(plan("grave", held, false).0, step(S::Previous, false));
        assert_eq!(plan("Left", held, false).0, step(S::Previous, false));
        assert_eq!(plan("Right", held, true).0, step(S::Next, true), "a repeat is marked so it does not wrap");
        assert_eq!(plan("Tab", shifted, false).0, step(S::Previous, false));
        assert_eq!(plan("Home", held, false).0, step(S::First, false));
        assert_eq!(plan("Prior", held, false).0, step(S::First, false));
        assert_eq!(plan("End", held, false).0, step(S::Last, false));
        assert_eq!(plan("Next", held, false).0, step(S::Last, false));
        assert_eq!(plan("Return", held, false).0, dbg(&vec![Command::SwitcherCommit]));
        assert_eq!(plan("space", held, false).0, dbg(&vec![Command::SwitcherCommit]));
        assert_eq!(plan("period", held, false).0, dbg(&vec![Command::SwitcherCancel]));
        assert_eq!(plan("Down", held, false), (dbg(&vec![Command::SwitcherCommit]), dbg(&vec![ShellOp::ToggleAppOverview])));
        assert_eq!(plan("exclam", shifted, false).1, dbg(&vec![ShellOp::ToggleAppOverview]));
        assert_eq!(plan("q", held, false).0, dbg(&vec![Command::SwitcherQuit]), "Super+Q quits the selected application, not the front one");
        assert_eq!(plan("h", held, false).0, dbg(&vec![Command::SwitcherToggleHide]));
        assert_eq!(plan("w", held, false), (dbg(&Vec::<Command>::new()), dbg(&Vec::<ShellOp>::new())), "other shortcuts are off");
        let shift = parse_chord("Shift_L").unwrap().sym;
        assert!(matches!(KeyRouter::default().key(KeyEvent { sym: shift, pressed: true, mods: held, repeat: false }, &c), KeyOutcome::Forward), "modifiers pass");
    }

    #[test]
    fn consumed_keys_swallow_their_release() {
        let mut r = KeyRouter::default();
        let c = ctx();
        let ch = parse_chord("Super+w").unwrap();
        assert!(matches!(r.key(KeyEvent { sym: ch.sym, pressed: true, mods: ch.mods, repeat: false }, &c), KeyOutcome::Consumed(_)));
        assert!(matches!(r.key(KeyEvent { sym: ch.sym, pressed: false, mods: Mods::NONE, repeat: false }, &c), KeyOutcome::Consumed(_)));
        assert_eq!(dbg(&r.key(KeyEvent { sym: ch.sym, pressed: false, mods: Mods::NONE, repeat: false }, &c)), dbg(&KeyOutcome::Forward));
        let plain = parse_chord("a").unwrap();
        assert_eq!(dbg(&r.key(KeyEvent { sym: plain.sym, pressed: true, mods: Mods::NONE, repeat: false }, &c)), dbg(&KeyOutcome::Forward));
    }

    #[test]
    fn the_overview_keeps_only_escape_in_the_server() {
        let mut r = KeyRouter::default();
        let mut c = ctx();
        c.overview_open = true;
        // navigation keys are the dock's (its keyboard-exclusive layer)
        assert_eq!(dbg(&press(&mut r, "Left", &c)), dbg(&KeyOutcome::Forward));
        let KeyOutcome::Consumed(p) = press(&mut r, "Escape", &c) else { panic!() };
        assert_eq!(dbg(&p.ops), dbg(&vec![ShellOp::CloseOverview]));
        // the toggle chord itself still toggles
        let KeyOutcome::Consumed(p) = press(&mut r, "Ctrl+Up", &c) else { panic!() };
        assert_eq!(dbg(&p.ops), dbg(&vec![ShellOp::ToggleOverview]));
        c.overview_open = false;
        assert_eq!(dbg(&press(&mut r, "Escape", &c)), dbg(&KeyOutcome::Forward));
    }

    #[test]
    fn shortcut_map_becomes_key_bindings() {
        let mut map = cupertino_input::ShortcutMap::defaults();
        assert!(map.apply_config("Super+x = exec:foot --title scratch\nF5 = show-desktop\nSuper+Tab = none\n").is_empty());
        let b = KeyBindings::from_shortcuts(&map);
        let look = |s: &str| {
            let c = parse_chord(s).unwrap();
            b.lookup(c.sym, c.mods)
        };
        assert_eq!(look("Ctrl+Up"), Some(Action::Overview));
        assert_eq!(look("Ctrl+Left"), Some(Action::WorkspaceLeft));
        assert_eq!(look("Super+q"), Some(Action::QuitApp));
        assert_eq!(look("F5"), Some(Action::ShowDesktop));
        assert_eq!(look("Super+Tab"), None, "unbound by the configuration");
        assert_eq!(look("Ctrl+3"), Some(Action::Workspace(3)));
        assert_eq!(look("Super+Return"), Some(Action::SpawnTerminal), "compositor-only extras stay");
        let Some(Action::Exec(i)) = look("Super+x") else { panic!("exec binding") };
        assert_eq!(b.exec_command(i), Some("foot --title scratch"));
    }

    #[test]
    fn launcher_and_input_source_chords_come_from_the_merged_tables() {
        let b = Config::default().keybindings;
        let look = |s: &str| {
            let c = parse_chord(s).unwrap();
            b.lookup(c.sym, c.mods)
        };
        assert_eq!(look("Super+space"), Some(Action::Launcher));
        assert_eq!(look("Ctrl+space"), Some(Action::InputSourcePrevious));
        assert_eq!(look("Ctrl+Alt+space"), Some(Action::InputSourceNext));
        assert_eq!(dbg(&translate_action(Action::Launcher, &ctx()).ops), dbg(&vec![ShellOp::ToggleLauncher]));
        assert_eq!(dbg(&translate_action(Action::InputSourceNext, &ctx()).ops), dbg(&vec![ShellOp::InputSource { menu: true, step: cupertino_keyboard::Step::Next }]));
        assert_eq!(dbg(&translate_action(Action::NotificationCenter, &ctx()).ops), dbg(&vec![ShellOp::ToggleNotificationCenter]));
    }

    #[test]
    fn the_power_key_is_bound_and_its_press_tells_the_login_process() {
        let b = Config::default().keybindings;
        let power = parse_chord("XF86PowerOff").unwrap();
        assert_eq!(b.lookup(power.sym, power.mods), Some(Action::PowerKey));
        assert_eq!(Action::from_name("power-key"), Some(Action::PowerKey));
        assert_eq!(Action::PowerKey.name(), "power-key");
        assert_eq!(dbg(&translate_action(Action::PowerKey, &ctx()).ops), dbg(&vec![ShellOp::PowerKey { pressed: true }]));
    }

    #[test]
    fn the_power_key_release_reaches_the_login_process_and_repeats_do_not() {
        let mut r = KeyRouter::new(Config::default().keybindings);
        let c = ctx();
        let power = parse_chord("XF86PowerOff").unwrap();
        let ops = |outcome: KeyOutcome| match outcome {
            KeyOutcome::Consumed(plan) => dbg(&plan.ops),
            KeyOutcome::Forward => "forward".into(),
        };
        let event = |pressed| KeyEvent { sym: power.sym, pressed, mods: Mods::NONE, repeat: false };
        assert_eq!(ops(r.key(event(true), &c)), dbg(&vec![ShellOp::PowerKey { pressed: true }]));
        // the repeats of a held key
        assert_eq!(ops(r.key(event(true), &c)), "[]");
        assert_eq!(ops(r.key(event(true), &c)), "[]");
        assert_eq!(ops(r.key(event(false), &c)), dbg(&vec![ShellOp::PowerKey { pressed: false }]));
        // a second tap starts again
        assert_eq!(ops(r.key(event(true), &c)), dbg(&vec![ShellOp::PowerKey { pressed: true }]));
    }

    #[test]
    fn a_locked_session_forwards_the_power_key_to_the_lock_screen_without_telling_the_login_process() {
        let mut r = KeyRouter::new(Config::default().keybindings);
        let locked = ActionContext { locked: true, ..ctx() };
        let power = parse_chord("XF86PowerOff").unwrap();
        assert_eq!(dbg(&r.key(KeyEvent { sym: power.sym, pressed: true, mods: Mods::NONE, repeat: false }, &locked)), dbg(&KeyOutcome::Forward));
        assert_eq!(dbg(&r.key(KeyEvent { sym: power.sym, pressed: false, mods: Mods::NONE, repeat: false }, &locked)), dbg(&KeyOutcome::Forward));
    }

    #[test]
    fn media_keys_reach_the_overlay_with_shift_alt_as_the_fine_step() {
        let b = Config::default().keybindings;
        let look = |s: &str| {
            let c = parse_chord(s).unwrap();
            b.lookup(c.sym, c.mods)
        };
        assert_eq!(look("XF86AudioRaiseVolume"), Some(Action::Osd { key: OsdKey::VolumeUp, fine: false }));
        assert_eq!(look("Shift+Alt+XF86AudioLowerVolume"), Some(Action::Osd { key: OsdKey::VolumeDown, fine: true }));
        assert_eq!(look("XF86AudioMute"), Some(Action::Osd { key: OsdKey::Mute, fine: false }));
        assert_eq!(look("Shift+XF86MonBrightnessUp"), None);
        assert_eq!(look("Shift+Alt+XF86KbdBrightnessDown"), Some(Action::Osd { key: OsdKey::KeyboardBrightnessDown, fine: true }));
        let fine_down = Action::Osd { key: OsdKey::BrightnessDown, fine: true };
        assert_eq!(Action::from_name("brightness-down-fine"), Some(fine_down));
        assert_eq!(fine_down.name(), "brightness-down-fine");
        assert_eq!(dbg(&translate_action(fine_down, &ctx()).ops), dbg(&vec![ShellOp::Osd { key: OsdKey::BrightnessDown, fine: true }]));
    }

    #[test]
    fn workspace_by_index_translates_to_a_switch() {
        let c = ctx();
        let plan = translate_action(Action::Workspace(2), &c);
        assert_eq!(dbg(&plan.commands), dbg(&vec![Command::SwitchWorkspace { output: OutputId(1), workspace: cupertino_wm::WorkspaceId(2) }]));
        assert!(translate_action(Action::Workspace(9), &c).is_empty());
        assert_eq!(Action::from_name("workspace:4"), Some(Action::Workspace(4)));
    }

    #[test]
    fn stepping_plans_repeat_and_toggles_do_not() {
        let c = ctx();
        let repeats = |a: Action| translate_action(a, &c).repeats();
        for a in [Action::SwitcherNext, Action::CycleWindowsPrev, Action::WorkspaceRight, Action::InputSourceNext, Action::Osd { key: OsdKey::VolumeUp, fine: false }] {
            assert!(repeats(a), "{a:?}");
        }
        for a in [Action::Launcher, Action::Overview, Action::CloseWindow, Action::Lock, Action::PowerKey, Action::Osd { key: OsdKey::Mute, fine: false }, Action::SpawnTerminal] {
            assert!(!repeats(a), "{a:?}");
        }
        assert!(!ActionPlan::default().repeats(), "a swallowed release is no repeat");
    }

    #[test]
    fn action_names_roundtrip() {
        for (n, a) in NAMES {
            assert_eq!(Action::from_name(n), Some(*a));
            assert_eq!(a.name(), *n);
        }
        assert_eq!(Action::from_name("zzz"), None);
    }
}
