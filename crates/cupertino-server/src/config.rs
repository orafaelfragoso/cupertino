//! Compositor configuration: window style, shell geometry, key bindings and spawn defaults.
//!
//! Every value that is not is approximate or unknown and therefore configurable. An
//! optional JSON file (`$CUPERTINO_SERVER_CONFIG`, else `$XDG_CONFIG_HOME/cupertino/compositor.json` over the system
//! file `/etc/cupertino/compositor.json`) overrides the defaults; unknown keys are ignored.

use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

use crate::authorization::ShellRole;
use crate::input::{Action, KeyBindings};
use cupertino_defaults::compositor::{
    AppearanceName, CompositorSettings, MinimizeEffectName, ScrollBarsName,
};
use cupertino_theme::Appearance;

/// Environment variable naming the configuration file.
pub use cupertino_defaults::compositor::COMPOSITOR_CONF_ENV as CONFIG_ENV;

/// The system configuration file the packages install: the shell processes and whatever else a fresh account needs.
pub const SYSTEM_CONFIG: &str = "/etc/cupertino/compositor.json";

/// The system file of the login screen's server, which `$CUPERTINO_SERVER_CONFIG` names when the packages start it.
pub const GREETER_CONFIG: &str = "/etc/cupertino/greeter.json";

/// Where the programs of the shell processes and the screenshot process must be installed. The install prefix of the
/// build is added to `/usr` when it is set (`CUPERTINO_INSTALL_PREFIX` at compile time).
pub(crate) fn installed_roots() -> impl DoubleEndedIterator<Item = &'static Path> {
    ["/usr"].into_iter().chain(INSTALL_PREFIX).map(Path::new)
}

const INSTALL_PREFIX: Option<&str> = option_env!("CUPERTINO_INSTALL_PREFIX");

const _: () = {
    if let Some(prefix) = INSTALL_PREFIX {
        assert!(
            prefix.len() > 1 && prefix.as_bytes()[0] == b'/',
            "CUPERTINO_INSTALL_PREFIX must be an absolute path other than /"
        );
    }
};

/// The screenshot process of an installation.
const INSTALLED_SCREENSHOT: &str = "/usr/bin/cupertino-screenshot";

/// A process the server starts in every user session from a fixed install path, with one role. No configuration file
/// names, changes or removes it, and no [`ShellRoleName`](cupertino_defaults::compositor::ShellRoleName) grants its role.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuiltIn {
    /// The desktop portal's backend, owner of [`PORTAL_BUS_NAME`]: it captures the screen for what the person allows.
    Portal,
    /// The polkit authentication agent, which draws the authentication prompt.
    PolkitAgent,
    /// The keyring's prompter, which draws the keyring's password prompt as the authentication prompt is drawn.
    KeyringPrompter,
    /// The native local desktop controller; each run still needs session consent.
    DesktopControl,
}

/// The bus name the portal's backend owns once it serves.
pub const PORTAL_BUS_NAME: &str = "org.freedesktop.impl.portal.desktop.cupertino";

impl BuiltIn {
    pub const ALL: [BuiltIn; 4] = [
        BuiltIn::Portal,
        BuiltIn::PolkitAgent,
        BuiltIn::KeyringPrompter,
        BuiltIn::DesktopControl,
    ];

    /// Name in log lines.
    pub const fn name(self) -> &'static str {
        match self {
            BuiltIn::Portal => "portal",
            BuiltIn::PolkitAgent => "polkit-agent",
            BuiltIn::KeyringPrompter => "keyring-prompter",
            BuiltIn::DesktopControl => "desktop-control",
        }
    }

    const fn program_name(self) -> &'static str {
        match self {
            BuiltIn::Portal => "cupertino-portal",
            BuiltIn::PolkitAgent => "cupertino-polkit-agent",
            BuiltIn::KeyringPrompter => "cupertino-keyring-prompter",
            BuiltIn::DesktopControl => "cupertino-control",
        }
    }

    /// The only role of the process's connection.
    pub const fn role(self) -> ShellRole {
        match self {
            BuiltIn::Portal => ShellRole::Capture,
            BuiltIn::PolkitAgent | BuiltIn::KeyringPrompter => ShellRole::Prompt,
            BuiltIn::DesktopControl => ShellRole::DesktopControl,
        }
    }

    /// Where the install put the program: the build's install prefix before `/usr`; `None` when it is not installed. A
    /// file [`check_built_in`] refuses is logged and the next root is tried.
    pub fn installed(self) -> Option<PathBuf> {
        installed_roots().rev().find_map(|root| {
            let program = root.join("bin").join(self.program_name());
            std::fs::symlink_metadata(&program).ok()?;
            match check_built_in(&program) {
                Ok(()) => Some(program),
                Err(reason) => {
                    tracing::warn!(name = self.name(), %reason, "the program is not started");
                    None
                }
            }
        })
    }

    /// The supervised process that runs `program`, holding [`BuiltIn::role`] alone.
    pub fn process(self, program: &Path) -> ShellProcess {
        ShellProcess {
            name: self.name().to_string(),
            command: if self == BuiltIn::DesktopControl {
                vec![program.display().to_string(), "--serve".into()]
            } else {
                vec![program.display().to_string()]
            },
            roles: vec![self.role()],
        }
    }

    /// Whether a `shell` entry names this process, by its name or its program.
    fn named_by(self, name: &str, command: &[String]) -> bool {
        name == self.name()
            || command
                .first()
                .and_then(|program| Path::new(program).file_name())
                .is_some_and(|file| file == self.program_name())
    }
}

/// Whether the built-in program at `<root>/bin/<name>` may run with its role, checked at every start: the file and its
/// `bin` directory are no links, and [`may_run_built_in`] holds for them. The reason comes back otherwise.
pub fn check_built_in(program: &Path) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    let stat = |path: &Path| {
        std::fs::symlink_metadata(path).map_err(|e| format!("{}: {e}", path.display()))
    };
    let bin = program
        .parent()
        .ok_or_else(|| format!("{} has no directory", program.display()))?;
    let root = bin
        .parent()
        .ok_or_else(|| format!("{} has no install root", bin.display()))?;
    let (file, bin_meta, root_meta) = (stat(program)?, stat(bin)?, stat(root)?);
    if !file.is_file() {
        return Err(format!("{} is not a regular file", program.display()));
    }
    if !bin_meta.is_dir() {
        return Err(format!("{} is not a directory", bin.display()));
    }
    let user = rustix::process::getuid().as_raw();
    if may_run_built_in(
        (file.uid(), file.mode()),
        (bin_meta.uid(), bin_meta.mode()),
        root_meta.uid(),
        user,
    ) {
        Ok(())
    } else {
        Err(format!(
            "{} is not root's alone: it or its directory has another owner, or its group or others can write it",
            program.display()
        ))
    }
}

/// Whether a built-in program file and its `bin` directory, each `(owner, mode)`, under an install root of `root_owner`,
/// may run with its role for `user`: nobody else may write either, the directory has the root's owner, and the file is
/// root's, or the root is the user's own (a development prefix).
pub fn may_run_built_in(file: (u32, u32), bin: (u32, u32), root_owner: u32, user: u32) -> bool {
    let unshared = |mode: u32| mode & 0o022 == 0;
    unshared(file.1)
        && unshared(bin.1)
        && bin.0 == root_owner
        && (file.0 == 0 || (root_owner == user && file.0 == user))
}

/// The roles a shell process may hold and still start from the development folder: the session's own chrome (the menu
/// bar, the dock, the window manager and the processes that only follow the session). A process holding any other role
/// (the lock screen and login, capture, the authentication prompt, data control, desktop control, or a role added
/// later) always starts its installed program: this is a list of what is allowed, not of what is refused.
pub const DEV_FOLDER_ROLES: [ShellRole; 4] = [
    ShellRole::Session,
    ShellRole::MenuBar,
    ShellRole::Dock,
    ShellRole::WindowManager,
];

/// Whether a shell process with `roles` may start from the development folder: all of its roles are
/// [`DEV_FOLDER_ROLES`].
pub fn may_start_from_dev_folder(roles: &[ShellRole]) -> bool {
    roles.iter().all(|role| DEV_FOLDER_ROLES.contains(role))
}

/// The development folder the system file declares (`dev_folder`): the shell processes named in `components` that
/// [`may_start_from_dev_folder`] start `<path>/<file name of their program>` instead of the installed program, when that
/// file passes [`dev_folder_program`] at the start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DevFolder {
    pub path: PathBuf,
    pub components: Vec<String>,
}

impl DevFolder {
    /// The names of `processes` that may start from the folder, and a warning for each name of `components` that is
    /// not one of them (no such process, or one holding a role outside [`DEV_FOLDER_ROLES`]).
    pub fn eligible(&self, processes: &[ShellProcess]) -> (Vec<String>, Vec<String>) {
        let mut eligible = Vec::new();
        let mut warnings = Vec::new();
        for name in &self.components {
            match processes.iter().find(|p| &p.name == name) {
                Some(p) if may_start_from_dev_folder(&p.roles) => eligible.push(name.clone()),
                Some(p) => warnings.push(format!(
                    "dev_folder: {name}: a process holding {:?} always starts its installed program",
                    p.roles
                        .iter()
                        .filter(|r| !DEV_FOLDER_ROLES.contains(r))
                        .collect::<Vec<_>>()
                )),
                None => warnings.push(format!("dev_folder: {name}: no shell process has this name")),
            }
        }
        (eligible, warnings)
    }
}

/// Whether a file in the development folder may start for `user`, from the `(owner, st_mode)` of the file and of the
/// folder: the folder is a directory and the file a regular file (no link), each owned by the user or root and
/// writable by no group or other user, and the file has an execute bit. The reason comes back otherwise.
pub fn may_start_dev_program(
    file: (u32, u32),
    folder: (u32, u32),
    user: u32,
) -> Result<(), String> {
    let owned = |owner: u32| owner == user || owner == 0;
    let unshared = |mode: u32| mode & 0o022 == 0;
    let kind = |mode: u32| mode & libc::S_IFMT;
    if kind(folder.1) != libc::S_IFDIR {
        return Err("the development folder is not a directory".into());
    }
    if !owned(folder.0) || !unshared(folder.1) {
        return Err(
            "the development folder belongs to another user, or its group or others can write it"
                .into(),
        );
    }
    if kind(file.1) != libc::S_IFREG {
        return Err("it is not a regular file".into());
    }
    if !owned(file.0) || !unshared(file.1) {
        return Err("it belongs to another user, or its group or others can write it".into());
    }
    if file.1 & 0o111 == 0 {
        return Err("it is not executable".into());
    }
    Ok(())
}

/// The program `<folder>/<file name of installed>` when it may start for `user` ([`may_start_dev_program`], and the user
/// may execute it); `Ok(None)` when the folder or the file is not there, the reason when the file is there and refused.
pub fn dev_folder_program(
    folder: &Path,
    installed: &str,
    user: u32,
) -> Result<Option<PathBuf>, String> {
    use std::os::unix::fs::MetadataExt;
    let Some(file_name) = Path::new(installed).file_name() else {
        return Ok(None);
    };
    let program = folder.join(file_name);
    let file = match std::fs::symlink_metadata(&program) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{}: {e}", program.display())),
    };
    let dir =
        std::fs::symlink_metadata(folder).map_err(|e| format!("{}: {e}", folder.display()))?;
    may_start_dev_program((file.uid(), file.mode()), (dir.uid(), dir.mode()), user)
        .map_err(|reason| format!("{}: {reason}", program.display()))?;
    rustix::fs::access(&program, rustix::fs::Access::EXEC_OK)
        .map_err(|e| format!("{}: not executable: {e}", program.display()))?;
    // a build left in the folder never replaces a newer installed program: once a package built after it is in place,
    // the package's program starts
    let installed_at = std::fs::metadata(installed).and_then(|m| m.modified()).ok();
    if let (Ok(dev_at), Some(installed_at)) = (file.modified(), installed_at) {
        if dev_at < installed_at {
            return Err(format!(
                "{}: older than the installed {installed}",
                program.display()
            ));
        }
    }
    Ok(Some(program))
}

/// Whether the server runs the login screen: its configuration is the login screen's file, or logind made its session
/// one of class `greeter`.
pub fn login_screen(
    config: Option<&std::ffi::OsStr>,
    session_class: Option<&std::ffi::OsStr>,
) -> bool {
    config.is_some_and(|path| Path::new(path) == Path::new(GREETER_CONFIG))
        || session_class.is_some_and(|class| class == "greeter")
}

/// Whether `command` starts a program the install put in place: an absolute path under an install root, with no `..`.
pub fn is_installed_program(command: &[String]) -> bool {
    let Some(program) = command.first().map(Path::new) else {
        return false;
    };
    program.is_absolute()
        && !program
            .components()
            .any(|c| matches!(c, Component::ParentDir))
        && installed_roots().any(|root| program.starts_with(root))
}

/// What a configuration file may decide about the processes the server starts with privileges (the shell processes
/// and the screenshot process). Everything else a file sets is the same for every layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayerTrust {
    /// The system file, owned by root: it may name privileged processes, only programs the install put in place.
    System,
    /// A file of the person: it may disable or reorder what the system file names, never add or change a privileged
    /// process.
    User,
    /// A development or nested run (`--dev-shell`): every file may name any program.
    Dev,
}

/// The trust of the layer at `path`. It comes from which file the layer is, never from where the path resolves: only
/// the two system files, named exactly, are [`LayerTrust::System`], and a link to them is a file of the person.
fn layer_trust(path: &Path, dev_shell: bool) -> LayerTrust {
    if dev_shell {
        LayerTrust::Dev
    } else if [SYSTEM_CONFIG, GREETER_CONFIG]
        .into_iter()
        .any(|system| path == Path::new(system))
    {
        LayerTrust::System
    } else {
        LayerTrust::User
    }
}

pub(crate) use cupertino_defaults::system::read_root_file as read_system_file;

/// A shell process the server starts and keeps alive (see [`crate::supervisor`]).
#[derive(Clone, Debug, PartialEq)]
pub struct ShellProcess {
    /// Name in log lines.
    pub name: String,
    /// Program and arguments; never empty.
    pub command: Vec<String>,
    /// The roles the server gives the process's connection.
    pub roles: Vec<ShellRole>,
}

/// One window shadow preset: Gaussian σ and y offset in points, density as peak alpha.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShadowPreset {
    pub sigma: f64,
    pub offset_y: f64,
    pub density: f64,
}

/// Window shadows.: standard active σ 20 / offset 18 /
/// density 0.40, inactive σ 12 / offset 8 / density 0.25, dark appearance active density 0.70.
/// The dark inactive density is not part of the approximate as the light ratio
/// (0.25 / 0.40) applied to the dark active density.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShadowStyle {
    pub active: ShadowPreset,
    pub inactive: ShadowPreset,
    pub dark_active_density: f64,
    pub dark_inactive_density: f64,
    /// Remove the shadow underneath the window shape so translucent windows stay clean.
    /// approximate design choice.
    pub cutout: bool,
}

impl Default for ShadowStyle {
    fn default() -> Self {
        Self {
            active: ShadowPreset {
                sigma: 20.0,
                offset_y: 18.0,
                density: 0.40,
            },
            inactive: ShadowPreset {
                sigma: 12.0,
                offset_y: 8.0,
                density: 0.25,
            },
            dark_active_density: 0.70,
            dark_inactive_density: 0.70 * (0.25 / 0.40),
            cutout: true,
        }
    }
}

impl ShadowStyle {
    /// Preset for a window state and appearance.
    pub fn preset(&self, active: bool, dark: bool) -> ShadowPreset {
        let mut p = if active { self.active } else { self.inactive };
        if dark {
            p.density = if active {
                self.dark_active_density
            } else {
                self.dark_inactive_density
            };
        }
        p
    }
}

/// Space the compositor reserves around a server-side decorated client surface (title bar and
/// borders). Zero by default; the decoration component sets it (unknown until its metrics exist).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Insets {
    pub top: f64,
    pub left: f64,
    pub right: f64,
    pub bottom: f64,
}

impl Insets {
    pub fn horizontal(&self) -> f64 {
        self.left + self.right
    }
    pub fn vertical(&self) -> f64 {
        self.top + self.bottom
    }
}

/// Window corner style. radius for regular windows: 16 pt, the decorations' window corner in every window style; the
/// continuous-corner expansion comes from `cupertino-geometry` (approximate).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindowStyle {
    pub corner_radius: f64,
    pub continuous_corners: bool,
    pub shadow: ShadowStyle,
    /// Apply shadow and corner mask to clients that draw their own decorations.
    pub style_client_decorated: bool,
    /// Insets around the surface of server-side decorated windows (integration point for the
    /// decoration component: window frame = surface geometry + insets).
    pub ssd_insets: Insets,
    /// Draw the built-in server-side decorations (`cupertino-decorations`) for clients that
    /// accept them; their insets then replace `ssd_insets`.
    pub decorations: bool,
}

impl Default for WindowStyle {
    fn default() -> Self {
        Self {
            corner_radius: cupertino_decorations::WINDOW_CORNER_RADIUS,
            continuous_corners: true,
            shadow: ShadowStyle::default(),
            style_client_decorated: true,
            ssd_insets: Insets::default(),
            decorations: true,
        }
    }
}

/// Shell geometry (approximate unless noted; menu bar height from `cupertino-theme`).
#[derive(Clone, Debug, PartialEq)]
pub struct ShellConfig {
    pub menu_bar_height: f64,
    /// How dark the desktop is behind the overview's windows, from 0 (not at all, the default) to 1.
    pub overview_dim: f64,
    /// Reserve the menu bar's height in the usable area (off: windows use the whole output).
    pub show_menu_bar: bool,
    /// The bar hides until the pointer reaches the top edge (`menu_bar_auto_hide`).
    pub menu_bar_auto_hide: bool,
}

impl Default for ShellConfig {
    fn default() -> Self {
        Self {
            menu_bar_height: cupertino_theme::ShapeTokens::default().menu_bar_height,
            overview_dim: 0.0,
            show_menu_bar: true,
            menu_bar_auto_hide: false,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Config {
    pub appearance: Appearance,
    pub window: WindowStyle,
    pub shell: ShellConfig,
    /// Shell processes supervised by the server (`shell` in the JSON file); none by default.
    pub shell_processes: Vec<ShellProcess>,
    pub keybindings: KeyBindings,
    pub effects: crate::fx::EffectsConfig,
    /// Hot-corner detection: all four corners arm (the binding's action is a placeholder; the dock
    /// maps a corner to its action), `size` and `dwell` from the file.: the dwell
    /// defaults to 0.0 s.
    pub hot_corners: cupertino_input::HotCornerConfig,
    /// Terminal candidates tried in order for the spawn action ([`cupertino_defaults::terminal`]).
    pub terminals: Vec<String>,
    /// Program and leading arguments of the screenshot process (`screenshot` in the JSON file), started by the
    /// screenshot shortcuts with the mode appended ([`crate::capture::launch`]); never empty.
    pub screenshot_command: Vec<String>,
    /// `--dev-shell`: a checkout or nested run, where the programs of the session are found by name.
    pub dev_shell: bool,
    /// The development folder some shell processes may start from (`dev_folder`); only the system file sets it.
    pub dev_folder: Option<DevFolder>,
    pub wallpaper: WallpaperSettings,
    /// Cascade step of new windows without a position, right and down (points). 29 pt
    /// (the design's horizontal and vertical cascade amounts).
    pub cascade_step: f64,
    /// Fraction of the usable area a client without a size preference opens at.
    pub default_window_fraction: f64,
    /// Key repeat, modifier profile, input sources, caps-lock delay, accent picker (`keyboard.conf`).
    pub keyboard: crate::kbd::KeyboardSettings,
    /// Tracking speed, acceleration and scroll direction.
    pub pointer: PointerSettings,
    /// Dim, display sleep, the password delay and system sleep (`power.conf`).
    pub power: cupertino_defaults::power::PowerSettings,
    /// When applications show their scroll bars (`show_scroll_bars`); automatic by default.
    pub show_scroll_bars: ScrollBarsName,
    /// A solid colour the desktop shows instead of a picture (`wallpaper_color`).
    pub wallpaper_color: Option<[u8; 3]>,
    /// Dragging a window to a screen edge tiles it (`edge_tiling`).
    pub edge_tiling: bool,
    /// Tiled windows keep a margin around and between them (`tile_margins`).
    pub tile_margins: bool,
    /// How much denser every material is made, 0 to 1 (`material_density`).
    pub material_density: f64,
    /// Bits a channel of the displays' primary planes (`output_depth`), read when the session starts.
    pub output_depth: crate::output_depth::Depth,
    /// Whether the server makes the toolkit's GPU programs into the per-user program cache when the session starts
    /// (`prepare_toolkit_programs`, [`crate::toolkit_programs`]), read then.
    pub prepare_toolkit_programs: bool,
    /// Windows take a slight tint of the wallpaper's colour (`tint_windows` of `appearance.conf`).
    pub tint_windows: bool,
    /// What a double click on a server-drawn title bar does (`titlebar_double_click` of `appearance.conf`).
    pub titlebar_double_click: cupertino_defaults::appearance::TitlebarAction,
}

/// The wallpaper images of the user (`wallpaper`, `wallpaper_dark` in the JSON file); a pack's wallpaper and then the
/// procedural one stand in for what is not set or cannot be read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WallpaperSettings {
    pub light: Option<std::path::PathBuf>,
    /// The dark appearance's; `light` serves both when this is not set.
    pub dark: Option<std::path::PathBuf>,
}

impl WallpaperSettings {
    /// The user's image for an appearance, if one is set.
    pub fn for_appearance(&self, dark: bool) -> Option<&std::path::Path> {
        let chosen = if dark {
            self.dark.as_ref().or(self.light.as_ref())
        } else {
            self.light.as_ref()
        };
        chosen.map(std::path::PathBuf::as_path)
    }
}

/// Pointer settings.
#[derive(Clone, Debug, PartialEq)]
pub struct PointerSettings {
    /// Mouse tracking speed, 0…3 (stops: `cupertino_input::TRACKING_SPEED_STOPS`).
    /// default 0.6875.
    pub mouse_speed: f64,
    /// Touchpad tracking speed, 0…3. default 0.6875.
    pub trackpad_speed: f64,
    /// Resolution the user stored per mouse, by `cupertino_defaults::compositor::mouse_dpi_key`; a mouse without an
    /// entry uses the hardware database's, else 1000.
    pub mouse_dpi: std::collections::BTreeMap<String, u32>,
    /// Speed-dependent mouse acceleration (default on); off gives a constant gain by `mouse_speed`, per
    /// count of a 1000 dpi mouse and scaled by the real resolution (a negative speed passes counts through).
    pub mouse_acceleration: bool,
    /// Speed-dependent touchpad acceleration (default on); off gives a constant gain by `trackpad_speed`.
    pub trackpad_acceleration: bool,
    /// On touchpads, content follows the fingers (default on).
    pub natural_scrolling: bool,
    /// On mouse wheels, content follows the wheel (default off: a wheel scrolls the view, as it does on every other
    /// desktop, and a mouse user rarely expects it reversed).
    pub mouse_natural_scrolling: bool,
    /// Cursor size as a scale of the theme's nominal size, 1…4 (range, the
    /// fresh 1.0 is derived from the design default of the size key). It scales the
    /// cursors the server draws and the `XCURSOR_SIZE` of the clients it starts.
    pub cursor_scale: f64,
    /// Fill and outline of the server-drawn cursors (defaults black / white).
    pub cursor_colors: crate::cursor::CursorColors,
    /// Shaking the pointer grows the cursor; default on: the design only has
    /// a disable key.
    pub shake_to_locate: bool,
}

impl PointerSettings {
    /// Natural scrolling for a touchpad, else for a wheel.
    pub fn natural_scrolling_for(&self, touchpad: bool) -> bool {
        if touchpad {
            self.natural_scrolling
        } else {
            self.mouse_natural_scrolling
        }
    }

    /// Apply the pointer keys `f` sets.
    pub fn apply_file(&mut self, f: &CompositorSettings) {
        if let Some(v) = f.mouse_speed {
            self.mouse_speed = v;
        }
        if let Some(v) = f.trackpad_speed {
            self.trackpad_speed = v;
        }
        if let Some(m) = &f.mouse_dpi {
            self.mouse_dpi = m.clone();
        }
        if let Some(b) = f.mouse_acceleration {
            self.mouse_acceleration = b;
        }
        if let Some(b) = f.trackpad_acceleration {
            self.trackpad_acceleration = b;
        }
        if let Some(b) = f.natural_scrolling {
            self.natural_scrolling = b;
        }
        if let Some(b) = f.mouse_natural_scrolling {
            self.mouse_natural_scrolling = b;
        }
        if let Some(s) = f.cursor_size {
            // JSON has no NaN, so the clamp cannot panic
            self.cursor_scale = cupertino_input::clamp_cursor_scale(s);
        }
        if let Some(c) = f.cursor_fill {
            self.cursor_colors.fill = c;
        }
        if let Some(c) = f.cursor_outline {
            self.cursor_colors.outline = c;
        }
        if let Some(b) = f.shake_to_locate {
            self.shake_to_locate = b;
        }
    }
}

impl Default for PointerSettings {
    fn default() -> Self {
        let speed = cupertino_input::DEFAULT_TRACKING_SPEED;
        Self {
            mouse_speed: speed,
            trackpad_speed: speed,
            mouse_dpi: std::collections::BTreeMap::new(),
            mouse_acceleration: cupertino_input::DEFAULT_ACCELERATION,
            trackpad_acceleration: cupertino_input::DEFAULT_ACCELERATION,
            natural_scrolling: true,
            mouse_natural_scrolling: false,
            cursor_scale: cupertino_input::CURSOR_SCALE_MIN,
            cursor_colors: crate::cursor::CursorColors::DEFAULT,
            shake_to_locate: true,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            appearance: Appearance::Light,
            window: WindowStyle::default(),
            shell: ShellConfig::default(),
            shell_processes: Vec::new(),
            keybindings: KeyBindings::from_shortcuts(
                &crate::kbd::default_shortcut_map("cupertino").0,
                "cupertino",
            ),
            effects: crate::fx::EffectsConfig::default(),
            hot_corners: default_hot_corners(),
            terminals: cupertino_defaults::terminal::DEFAULT_TERMINALS
                .map(String::from)
                .to_vec(),
            screenshot_command: vec![INSTALLED_SCREENSHOT.to_string()],
            dev_shell: false,
            dev_folder: None,
            cascade_step: 29.0,
            default_window_fraction: 0.6,
            keyboard: crate::kbd::KeyboardSettings::default(),
            pointer: PointerSettings::default(),
            power: cupertino_defaults::power::PowerSettings::default(),
            wallpaper: WallpaperSettings::default(),
            show_scroll_bars: ScrollBarsName::Automatic,
            wallpaper_color: None,
            edge_tiling: true,
            tile_margins: true,
            material_density: 0.0,
            output_depth: crate::output_depth::Depth::Eight,
            prepare_toolkit_programs: true,
            tint_windows: true,
            titlebar_double_click: cupertino_defaults::appearance::TitlebarAction::Zoom,
        }
    }
}

/// Path of the optional shortcut file: `$CUPERTINO_SHORTCUTS`, else
/// `$XDG_CONFIG_HOME/cupertino/shortcuts.conf` (default `~/.config`).
pub fn shortcuts_path() -> Option<PathBuf> {
    cupertino_defaults::config_file("CUPERTINO_SHORTCUTS", "shortcuts.conf")
}

/// The default shortcut map of the modifier profile `profile` with the lines of the shortcut file applied; invalid
/// lines are reported and skipped.
pub fn shortcut_map(profile: &str) -> cupertino_input::ShortcutMap {
    let (mut map, report) = crate::kbd::default_shortcut_map(profile);
    for (chord, table, kept) in &report.conflicts {
        tracing::warn!(
            chord,
            table,
            kept,
            "reference shortcut table row dropped: chord already bound"
        );
    }
    if let Some(path) = shortcuts_path() {
        if let Ok(text) = std::fs::read_to_string(&path) {
            let report = map.apply_config_for(&text, profile);
            for (line, err) in report.errors {
                tracing::warn!(path = %path.display(), line, error = %err, "ignoring invalid shortcut");
            }
            for (line, text) in report.retargeted {
                tracing::info!(path = %path.display(), line, shortcut = %text, "under classic-pc a Super line names the Super key, not the command key (use Primary for that)");
            }
        }
    }
    if let Some(chord) = map.keep_release_chord() {
        tracing::warn!(%chord, "no usable chord releases the shortcuts an application holds: the default is bound again");
    }
    map
}

/// Every corner armed, so the recognizer reports each one; which action a corner runs is the
/// dock's setting (`dock.conf`), so the binding's action is never read.
fn default_hot_corners() -> cupertino_input::HotCornerConfig {
    let mut cfg = cupertino_input::HotCornerConfig {
        dwell: 0.0,
        ..cupertino_input::HotCornerConfig::default()
    };
    for corner in cupertino_input::Corner::ALL {
        cfg.set(
            corner,
            Some(cupertino_input::CornerBinding::new(
                cupertino_input::CornerAction::Overview,
            )),
        );
    }
    cfg
}

/// The power settings with `power.conf` applied; invalid lines are reported and keep the defaults, and an unreadable
/// file leaves the defaults (reported).
fn load_power() -> cupertino_defaults::power::PowerSettings {
    use cupertino_defaults::power::PowerSettings;
    let Some(path) = PowerSettings::path() else {
        return PowerSettings::default();
    };
    match PowerSettings::load(&path, None) {
        Ok((settings, problems)) => {
            for p in problems {
                tracing::warn!(path = %path.display(), line = p.line, error = %p.message, "ignoring invalid power setting");
            }
            settings
        }
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "cannot read the power settings: the defaults apply");
            PowerSettings::default()
        }
    }
}

/// What the settings files held when they were last read: the start of the next change's comparison
/// (`crate::live`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SettingsFiles {
    /// The user's (or the configured) `compositor.json`.
    pub compositor: CompositorSettings,
    /// The system file under it (`SYSTEM_CONFIG`); empty when there is none or the file is configured by the person.
    pub system: CompositorSettings,
    pub keyboard: cupertino_defaults::keyboard::KeyboardSettings,
    pub power: cupertino_defaults::power::PowerSettings,
}

/// The files to apply, first to last: the system file when `CUPERTINO_SERVER_CONFIG` is unset, then the configured or
/// the user's file. A file the person sets in `CUPERTINO_SERVER_CONFIG` stands alone.
fn config_layers(env: Option<OsString>, user: Option<PathBuf>, system: &Path) -> Vec<PathBuf> {
    let explicit = env.is_some_and(|p| !p.is_empty());
    let system = (!explicit && system.is_file()).then(|| system.to_path_buf());
    system.into_iter().chain(user).collect()
}

/// The system file to watch: where `SYSTEM_CONFIG` is when the person did not set `env` (`CUPERTINO_SERVER_CONFIG`) and
/// its directory exists (the watcher would have to create it). The file itself may be created later.
pub fn system_config_path(env: Option<OsString>) -> Option<PathBuf> {
    let explicit = env.is_some_and(|p| !p.is_empty());
    let system = Path::new(SYSTEM_CONFIG);
    (!explicit && system.parent().is_some_and(Path::is_dir)).then(|| system.to_path_buf())
}

impl Config {
    /// What the system file and then the user's file mean on their own: the defaults (with the shortcut file's
    /// chords), the system file's keys, the user file's keys over them. Returns what the server refuses.
    pub fn from_layers(
        system: &CompositorSettings,
        user: &CompositorSettings,
        profile: &str,
    ) -> (Self, Vec<String>) {
        let mut cfg = Config {
            keybindings: KeyBindings::from_shortcuts(&shortcut_map(profile), profile),
            ..Config::default()
        };
        let mut problems = cfg.apply_file(system);
        problems.extend(cfg.apply_file(user));
        (cfg, problems)
    }

    /// Path of the configuration file, if one is configured or present.
    pub fn path() -> Option<PathBuf> {
        if let Some(p) = std::env::var_os(CONFIG_ENV).filter(|p| !p.is_empty()) {
            return Some(PathBuf::from(p));
        }
        cupertino_defaults::config_file(CONFIG_ENV, "compositor.json").filter(|p| p.is_file())
    }

    /// Defaults overridden by the settings files, and what the files held. Problems are reported and their keys keep
    /// the defaults.
    pub fn load_with_files(dev_shell: bool) -> (Self, SettingsFiles) {
        let (keyboard, keyboard_file) = crate::kbd::KeyboardSettings::load();
        let power = load_power();
        let mut cfg = Config {
            keybindings: KeyBindings::from_shortcuts(
                &shortcut_map(&keyboard.profile),
                &keyboard.profile,
            ),
            keyboard,
            power,
            dev_shell,
            ..Config::default()
        };
        if dev_shell {
            cfg.screenshot_command = vec!["cupertino-screenshot".to_string()];
        }
        let mut compositor = CompositorSettings::default();
        let mut system_file = CompositorSettings::default();
        let user = Self::path();
        for path in config_layers(
            std::env::var_os(CONFIG_ENV),
            user.clone(),
            Path::new(SYSTEM_CONFIG),
        ) {
            let trust = layer_trust(&path, dev_shell);
            let file = cfg.apply_path(&path, trust);
            if path == Path::new(SYSTEM_CONFIG) && file.shell.is_none() {
                tracing::error!(path = %path.display(), "the system configuration is unreadable or has no shell list: the shell list is empty unless the user's file sets one");
            }
            if Some(&path) == user.as_ref() {
                compositor = file;
            } else if path == Path::new(SYSTEM_CONFIG) {
                system_file = file;
            }
        }
        cfg.apply_appearance_settings(
            &cupertino_defaults::appearance::AppearanceSettings::current(),
        );
        (
            cfg,
            SettingsFiles {
                compositor,
                system: system_file,
                keyboard: keyboard_file,
                power,
            },
        )
    }

    /// The keys of `appearance.conf` the server follows: the tint of windows and the title bar's double click.
    pub fn apply_appearance_settings(
        &mut self,
        settings: &cupertino_defaults::appearance::AppearanceSettings,
    ) {
        self.tint_windows = settings.tint_windows;
        self.titlebar_double_click = settings.titlebar_double_click;
    }

    /// Apply the settings file at `path` and return them; problems are reported and their keys keep their values.
    fn apply_path(&mut self, path: &Path, trust: LayerTrust) -> CompositorSettings {
        let loaded = match trust {
            LayerTrust::System => read_system_file(path).map(|text| {
                text.map_or_else(
                    || (CompositorSettings::default(), Vec::new()),
                    |text| CompositorSettings::parse(&text, None),
                )
            }),
            LayerTrust::User | LayerTrust::Dev => CompositorSettings::load(path, None),
        };
        match loaded {
            Ok((file, problems)) => {
                for p in problems {
                    tracing::warn!(path = %path.display(), key = p.key, error = %p.message, "ignoring invalid configuration");
                }
                let problems = self
                    .apply_file(&file)
                    .into_iter()
                    .chain(self.apply_privileged(&file, trust));
                for problem in problems.collect::<Vec<_>>() {
                    tracing::warn!(path = %path.display(), error = %problem, "ignoring invalid configuration");
                }
                file
            }
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "cannot read configuration");
                CompositorSettings::default()
            }
        }
    }

    /// [`Config::load_with_files`] without the files.
    pub fn load() -> Self {
        Self::load_with_files(false).0
    }

    /// Apply the keys `f` sets on top of the current values. Returns what the server itself refuses (an unknown key
    /// action or chord); everything else applies.
    pub fn apply_file(&mut self, f: &CompositorSettings) -> Vec<String> {
        let mut problems = Vec::new();
        if let Some(a) = f.appearance {
            self.appearance = match a {
                AppearanceName::Light => Appearance::Light,
                AppearanceName::Dark => Appearance::Dark,
                AppearanceName::LightHighContrast => Appearance::LightHighContrast,
                AppearanceName::DarkHighContrast => Appearance::DarkHighContrast,
            };
        }
        if let Some(t) = &f.terminals {
            self.terminals = t.clone();
        }
        if let Some(r) = f.corner_radius {
            self.window.corner_radius = r;
        }
        if let Some(h) = f.menu_bar_height {
            self.shell.menu_bar_height = h;
        }
        if let Some(b) = f.show_menu_bar {
            self.shell.show_menu_bar = b;
        }
        if let Some(b) = f.menu_bar_auto_hide {
            self.shell.menu_bar_auto_hide = b;
        }
        if let Some(b) = f.edge_tiling {
            self.edge_tiling = b;
        }
        if let Some(b) = f.tile_margins {
            self.tile_margins = b;
        }
        if let Some(c) = f.wallpaper_color {
            self.wallpaper_color = Some([c[0], c[1], c[2]]);
        }
        if let Some(d) = f.material_density {
            self.material_density = d.clamp(0.0, 1.0);
        }
        if let Some(bits) = f.output_depth {
            self.output_depth = crate::output_depth::Depth::from_setting(Some(bits));
        }
        if let Some(b) = f.prepare_toolkit_programs {
            self.prepare_toolkit_programs = b;
        }
        if let Some(b) = f.decorations {
            self.window.decorations = b;
        }
        if let Some(b) = f.closing_animation {
            self.effects.closing = b;
        }
        if let Some(b) = f.show_scroll_bars {
            self.show_scroll_bars = b;
        }
        if let Some(e) = f.minimize_effect {
            self.effects.minimize = match e {
                MinimizeEffectName::Warp => Some(cupertino_effects::MinimizeKind::Warp),
                MinimizeEffectName::Funnel => Some(cupertino_effects::MinimizeKind::Funnel),
                MinimizeEffectName::Scale => Some(cupertino_effects::MinimizeKind::Scale),
                MinimizeEffectName::Plain => None,
            };
        }
        if let Some(s) = f.hot_corner_size {
            self.hot_corners.size = s;
        }
        if let Some(d) = f.hot_corner_dwell {
            self.hot_corners.dwell = d;
        }
        self.pointer.apply_file(f);
        if let Some(path) = &f.wallpaper {
            self.wallpaper.light = Some(path.into());
        }
        if let Some(path) = &f.wallpaper_dark {
            self.wallpaper.dark = Some(path.into());
        }
        for (chord, action) in f.keybindings.iter().flatten() {
            match Action::from_name(action) {
                Some(a) => {
                    let profile = self.keyboard.profile.clone();
                    if profile == "classic-pc" && cupertino_input::names_super(chord) {
                        tracing::info!(chord, "under classic-pc a Super chord names the Super key, not the command key (use Primary for that)");
                    }
                    if let Err(e) = self.keybindings.bind_str_for(chord, a, &profile) {
                        problems.push(format!("keybindings: {chord}: {e}"));
                    }
                }
                None => problems.push(format!("keybindings: {chord}: unknown action {action}")),
            }
        }
        problems
    }

    /// Apply the `shell`, `screenshot` and `dev_folder` keys of a file by what its layer is trusted with. Returns what
    /// is refused; the rest of the file applies.
    pub fn apply_privileged(&mut self, f: &CompositorSettings, trust: LayerTrust) -> Vec<String> {
        let mut problems = Vec::new();
        if let Some(list) = &f.shell {
            let list: Vec<_> = list
                .iter()
                .filter(|e| {
                    let built_in = BuiltIn::ALL
                        .into_iter()
                        .find(|b| b.named_by(&e.name, &e.command));
                    if let Some(b) = built_in {
                        problems.push(format!(
                            "shell: {}: the server starts the {} process itself",
                            e.name,
                            b.name()
                        ));
                    }
                    built_in.is_none()
                })
                .collect();
            let entry = |e: &cupertino_defaults::compositor::ShellEntry| ShellProcess {
                name: e.name.clone(),
                command: e.command.clone(),
                roles: e.roles.iter().copied().map(ShellRole::from).collect(),
            };
            self.shell_processes = match trust {
                LayerTrust::Dev => list.into_iter().map(entry).collect(),
                LayerTrust::System => list
                    .into_iter()
                    .filter(|e| {
                        let allowed = e.roles.is_empty() || is_installed_program(&e.command);
                        if !allowed {
                            problems.push(format!(
                                "shell: {}: a process with roles must be a program installed under {}",
                                e.name,
                                installed_roots().map(|r| r.display().to_string()).collect::<Vec<_>>().join(" or ")
                            ));
                        }
                        allowed
                    })
                    .map(entry)
                    .collect(),
                LayerTrust::User => list
                    .into_iter()
                    .filter_map(|e| {
                        let known = self.shell_processes.iter().find(|p| p.name == e.name);
                        match (known, e.roles.is_empty()) {
                            (Some(known), _) => {
                                if known.command != e.command
                                    || known.roles != e.roles.iter().copied().map(ShellRole::from).collect::<Vec<_>>()
                                {
                                    problems.push(format!("shell: {}: the user configuration cannot change a shell process of the system", e.name));
                                }
                                Some(known.clone())
                            }
                            (None, true) => Some(entry(e)),
                            (None, false) => {
                                problems.push(format!("shell: {}: the user configuration cannot add a process with roles", e.name));
                                None
                            }
                        }
                    })
                    .collect(),
            };
        }
        if let Some(command) = &f.screenshot {
            match trust {
                LayerTrust::Dev => self.screenshot_command = command.clone(),
                LayerTrust::System if is_installed_program(command) => {
                    self.screenshot_command = command.clone();
                }
                LayerTrust::System => problems.push(format!(
                    "screenshot: {}: the program must be installed under /usr",
                    command.join(" ")
                )),
                LayerTrust::User => problems.push(
                    "screenshot: only the system configuration sets the screenshot process"
                        .to_string(),
                ),
            }
        }
        if let Some(folder) = &f.dev_folder {
            match trust {
                LayerTrust::System | LayerTrust::Dev => {
                    self.dev_folder = Some(DevFolder {
                        path: PathBuf::from(&folder.path),
                        components: folder.components.clone(),
                    });
                }
                LayerTrust::User => problems.push(
                    "dev_folder: only the system configuration sets the development folder"
                        .to_string(),
                ),
            }
        }
        problems
    }

    pub fn is_dark(&self) -> bool {
        self.appearance.is_dark()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_primary_planes_are_8_bits_unless_the_file_asks_for_10() {
        use crate::output_depth::Depth;
        let mut c = Config::default();
        assert_eq!(c.output_depth, Depth::Eight);
        c.apply_file(&CompositorSettings::parse(r#"{"appearance":"dark"}"#, None).0);
        assert_eq!(c.output_depth, Depth::Eight, "a file without the key");
        c.apply_file(&CompositorSettings::parse(r#"{"output_depth":10}"#, None).0);
        assert_eq!(c.output_depth, Depth::Ten);
        c.apply_file(&CompositorSettings::parse(r#"{"output_depth":8}"#, None).0);
        assert_eq!(c.output_depth, Depth::Eight);
    }

    #[test]
    fn the_toolkit_s_programs_are_made_ahead_unless_the_file_turns_it_off() {
        let mut c = Config::default();
        assert!(c.prepare_toolkit_programs);
        c.apply_file(&CompositorSettings::parse(r#"{"appearance":"dark"}"#, None).0);
        assert!(c.prepare_toolkit_programs, "a file without the key");
        c.apply_file(&CompositorSettings::parse(r#"{"prepare_toolkit_programs":false}"#, None).0);
        assert!(!c.prepare_toolkit_programs);
    }

    #[test]
    fn the_system_file_is_applied_under_the_users_file_and_never_under_an_explicit_one() {
        let dir = tempfile::tempdir().unwrap();
        let system = dir.path().join("system.json");
        let user = dir.path().join("user.json");
        std::fs::write(
            &system,
            r#"{"appearance":"dark","shell":[{"name":"dock","command":["/usr/bin/cupertino-dock"]}]}"#,
        )
        .unwrap();
        std::fs::write(&user, r#"{"appearance":"light"}"#).unwrap();

        let layers = config_layers(None, Some(user.clone()), &system);
        assert_eq!(layers, vec![system.clone(), user.clone()]);
        let mut c = Config::default();
        for path in &layers {
            c.apply_path(path, LayerTrust::Dev);
        }
        assert_eq!(c.appearance, Appearance::Light);
        assert_eq!(c.shell_processes.len(), 1);

        assert_eq!(config_layers(None, None, &system), vec![system.clone()]);
        assert_eq!(
            config_layers(None, Some(user.clone()), &dir.path().join("none.json")),
            vec![user.clone()]
        );
        assert_eq!(
            config_layers(Some(user.clone().into()), Some(user.clone()), &system),
            vec![user]
        );
    }

    use super::*;

    /// Apply `json` to `c`; the problems of the file and those the server finds, as text.
    fn apply(c: &mut Config, json: &str) -> Vec<String> {
        let (file, problems) = CompositorSettings::parse(json, None);
        let mut out: Vec<String> = problems
            .into_iter()
            .map(|p| format!("{}: {}", p.key, p.message))
            .collect();
        out.extend(c.apply_file(&file));
        out.extend(c.apply_privileged(&file, LayerTrust::Dev));
        out
    }

    #[test]
    fn by_default_a_touchpad_scrolls_naturally_and_a_wheel_does_not() {
        let pointer = PointerSettings::default();
        assert!(pointer.natural_scrolling_for(true));
        assert!(!pointer.natural_scrolling_for(false));
    }

    #[test]
    fn shadow_presets_follow_evidence() {
        let s = ShadowStyle::default();
        assert_eq!(
            s.preset(true, false),
            ShadowPreset {
                sigma: 20.0,
                offset_y: 18.0,
                density: 0.40
            }
        );
        assert_eq!(
            s.preset(false, false),
            ShadowPreset {
                sigma: 12.0,
                offset_y: 8.0,
                density: 0.25
            }
        );
        assert_eq!(s.preset(true, true).density, 0.70);
        assert_eq!(s.preset(true, true).sigma, 20.0);
        assert!(s.preset(false, true).density < 0.70);
    }

    #[test]
    fn corner_radius_is_sixteen() {
        assert_eq!(WindowStyle::default().corner_radius, 16.0);
    }

    #[test]
    fn compositor_json_cannot_take_the_release_chord_away() {
        for json in [
            r#"{"keybindings":{"Super+Escape":"lock"}}"#,
            r#"{"keybindings":{"Escape":"release-shortcuts"}}"#,
            r#"{"keybindings":{"Shift+F5":"release-shortcuts","Super+Escape":"lock"}}"#,
        ] {
            let mut c = Config::default();
            let problems = apply(&mut c, json);
            let release = crate::input::parse_chord("Super+Escape").unwrap();
            let held = |c: &Config| c.keybindings.lookup(release.sym, release.mods);
            assert!(!problems.is_empty(), "{json}");
            let still = held(&c);
            assert!(
                still == Some(Action::ReleaseShortcuts),
                "{json}: Super+Escape is {still:?}"
            );
        }
    }

    #[test]
    fn json_overrides_apply() {
        let mut c = Config::default();
        assert_eq!(
            apply(
                &mut c,
                r#"{"appearance":"dark","corner_radius":10,"terminals":["kitty"],"keybindings":{"Super+t":"spawn-terminal"}}"#
            ),
            Vec::<String>::new()
        );
        assert!(c.is_dark());
        assert_eq!(c.window.corner_radius, 10.0);
        assert_eq!(c.terminals, vec!["kitty".to_string()]);
        assert!(!apply(&mut c, r#"{"keybindings":{"Super+t":"nope"}}"#).is_empty());
        assert_eq!(
            apply(
                &mut c,
                r#"{"minimize_effect":"funnel","decorations":false,"closing_animation":false}"#
            ),
            Vec::<String>::new()
        );
        assert_eq!(
            c.effects.minimize,
            Some(cupertino_effects::MinimizeKind::Funnel)
        );
        assert!(!c.window.decorations && !c.effects.closing);
        assert!(!apply(&mut c, r#"{"minimize_effect":"nope"}"#).is_empty());
        assert_eq!(c.show_scroll_bars, ScrollBarsName::Automatic);
        assert_eq!(
            apply(&mut c, r#"{"show_scroll_bars":"always"}"#),
            Vec::<String>::new()
        );
        assert_eq!(c.show_scroll_bars, ScrollBarsName::Always);
        assert_eq!(c.hot_corners.dwell, 0.0, " default");
        assert!(
            cupertino_input::Corner::ALL
                .iter()
                .all(|c2| c.hot_corners.binding(*c2).is_some()),
            "every corner arms; the dock maps it"
        );
        assert_eq!(
            apply(&mut c, r#"{"hot_corner_dwell":0.2,"hot_corner_size":3}"#),
            Vec::<String>::new()
        );
        assert_eq!((c.hot_corners.dwell, c.hot_corners.size), (0.2, 3.0));
        assert_eq!(
            apply(
                &mut c,
                r#"{"mouse_speed":0.6875,"trackpad_speed":3,"mouse_acceleration":false,"trackpad_acceleration":false,"natural_scrolling":false,"mouse_natural_scrolling":true}"#
            ),
            Vec::<String>::new()
        );
        assert_eq!(
            c.pointer,
            PointerSettings {
                mouse_speed: 0.6875,
                trackpad_speed: 3.0,
                mouse_acceleration: false,
                trackpad_acceleration: false,
                natural_scrolling: false,
                mouse_natural_scrolling: true,
                ..PointerSettings::default()
            }
        );
        assert_eq!(
            apply(
                &mut c,
                r##"{"cursor_size":7,"cursor_fill":"#ff0000","cursor_outline":"#00ff0080","shake_to_locate":false}"##
            ),
            Vec::<String>::new()
        );
        assert_eq!(
            c.pointer.cursor_scale, 4.0,
            "the size is clamped to the settings range"
        );
        assert_eq!(
            c.pointer.cursor_colors,
            crate::cursor::CursorColors {
                fill: [255, 0, 0, 255],
                outline: [0, 255, 0, 128]
            }
        );
        assert!(!c.pointer.shake_to_locate);
        for bad in [
            r#"{"cursor_fill":"red"}"#,
            r##"{"cursor_fill":"#12345"}"##,
            r##"{"cursor_outline":"#gg0000"}"##,
        ] {
            assert!(!apply(&mut c, bad).is_empty(), "{bad}");
        }
        assert!(!apply(&mut c, "{").is_empty());
        assert!(c.shell_processes.is_empty());
        assert_eq!(
            apply(
                &mut c,
                r#"{"shell":[{"name":"dock","command":["cupertino-dock","--bar"]}]}"#
            ),
            Vec::<String>::new()
        );
        assert_eq!(
            c.shell_processes,
            vec![ShellProcess {
                name: "dock".into(),
                command: vec!["cupertino-dock".into(), "--bar".into()],
                roles: vec![ShellRole::Session],
            }]
        );
        assert!(!apply(&mut c, r#"{"shell":[{"name":"dock","command":[]}]}"#).is_empty());
        assert_eq!(
            c.screenshot_command,
            vec!["/usr/bin/cupertino-screenshot".to_string()]
        );
        assert_eq!(
            apply(
                &mut c,
                r#"{"screenshot":["/opt/cupertino/bin/cupertino-screenshot"]}"#
            ),
            Vec::<String>::new()
        );
        assert_eq!(
            c.screenshot_command,
            vec!["/opt/cupertino/bin/cupertino-screenshot".to_string()]
        );
        assert!(!apply(&mut c, r#"{"screenshot":[]}"#).is_empty());
    }

    fn settings(json: &str) -> CompositorSettings {
        CompositorSettings::parse(json, None).0
    }

    const SYSTEM_SHELL: &str = r#"{"screenshot":["/usr/bin/cupertino-screenshot"],"shell":[
        {"name":"dock","command":["/usr/bin/cupertino-dock"],"roles":["session","dock"]},
        {"name":"menubar","command":["/usr/bin/cupertino-menubar"],"roles":["session","menu_bar"]}]}"#;

    fn system_config() -> Config {
        let mut c = Config::default();
        assert!(c
            .apply_privileged(&settings(SYSTEM_SHELL), LayerTrust::System)
            .is_empty());
        c
    }

    fn names(c: &Config) -> Vec<&str> {
        c.shell_processes.iter().map(|p| p.name.as_str()).collect()
    }

    #[test]
    fn only_programs_the_install_put_in_place_are_installed_programs() {
        let program = |s: &str| is_installed_program(&[s.to_string()]);
        assert!(program("/usr/bin/cupertino-dock"));
        assert!(program("/usr/lib/cupertino/greeter"));
        assert!(!program("cupertino-dock"));
        assert!(!program("/home/me/bin/cupertino-dock"));
        assert!(!program("/tmp/usr/bin/x"));
        assert!(!program("/usr/../tmp/x"));
        assert!(!program("/usrx/bin/x"));
        assert!(!is_installed_program(&[]));
    }

    #[test]
    fn the_system_file_names_privileged_processes_only_under_usr() {
        let c = system_config();
        assert_eq!(names(&c), ["dock", "menubar"]);

        let mut c = Config::default();
        let problems = c.apply_privileged(
            &settings(
                r#"{"shell":[
                {"name":"dock","command":["/usr/bin/cupertino-dock"],"roles":["dock"]},
                {"name":"evil","command":["/tmp/evil"],"roles":["window_manager"]},
                {"name":"relative","command":["cupertino-menubar"],"roles":["menu_bar"]}],
                "screenshot":["/tmp/evil"]}"#,
            ),
            LayerTrust::System,
        );
        assert_eq!(names(&c), ["dock"]);
        assert_eq!(problems.len(), 3, "{problems:?}");
        assert_eq!(c.screenshot_command, ["/usr/bin/cupertino-screenshot"]);
    }

    #[test]
    fn the_user_file_may_disable_and_reorder_but_never_add_or_change_a_privileged_process() {
        let mut c = system_config();
        let problems = c.apply_privileged(
            &settings(
                r#"{"shell":[
                {"name":"menubar","command":["/usr/bin/cupertino-menubar"],"roles":["session","menu_bar"]},
                {"name":"dock","command":["/home/me/dock"],"roles":["session","dock","window_manager"]},
                {"name":"mine","command":["/home/me/bin/wm"],"roles":["window_manager"]}],
                "screenshot":["/usr/bin/python3"]}"#,
            ),
            LayerTrust::User,
        );
        assert_eq!(
            names(&c),
            ["menubar", "dock"],
            "reordered, and nothing added"
        );
        let dock = &c.shell_processes[1];
        assert_eq!(dock.command, ["/usr/bin/cupertino-dock"]);
        assert_eq!(dock.roles, [ShellRole::Session, ShellRole::Dock]);
        assert_eq!(problems.len(), 3, "{problems:?}");
        assert_eq!(c.screenshot_command, ["/usr/bin/cupertino-screenshot"]);

        c.apply_privileged(&settings(r#"{"shell":[]}"#), LayerTrust::User);
        assert!(c.shell_processes.is_empty(), "the user disables them all");
    }

    #[test]
    fn a_user_process_without_roles_is_allowed_and_a_dev_run_trusts_every_layer() {
        let mut c = system_config();
        let json = r#"{"shell":[{"name":"mine","command":["/home/me/bin/tool"],"roles":[]}]}"#;
        assert!(c
            .apply_privileged(&settings(json), LayerTrust::User)
            .is_empty());
        assert_eq!(names(&c), ["mine"]);

        let mut dev = Config::default();
        let json = r#"{"shell":[{"name":"wm","command":["target/debug/wm"],"roles":["window_manager"]}],"screenshot":["target/debug/shot"]}"#;
        assert!(dev
            .apply_privileged(&settings(json), LayerTrust::Dev)
            .is_empty());
        assert_eq!(dev.shell_processes[0].roles, [ShellRole::WindowManager]);
        assert_eq!(dev.screenshot_command, ["target/debug/shot"]);
    }

    #[test]
    fn only_the_system_files_by_name_are_system_layers() {
        assert_eq!(
            layer_trust(Path::new(SYSTEM_CONFIG), false),
            LayerTrust::System
        );
        assert_eq!(
            layer_trust(Path::new(GREETER_CONFIG), false),
            LayerTrust::System
        );
        assert_eq!(
            layer_trust(Path::new("/etc/cupertino/other.json"), false),
            LayerTrust::User
        );
        assert_eq!(layer_trust(Path::new(SYSTEM_CONFIG), true), LayerTrust::Dev);

        let dir = tempfile::tempdir().unwrap();
        let link = dir.path().join("compositor.json");
        std::os::unix::fs::symlink(SYSTEM_CONFIG, &link).unwrap();
        assert_eq!(layer_trust(&link, false), LayerTrust::User);
        let mut c = system_config();
        let problems = c.apply_privileged(
            &settings(r#"{"shell":[{"name":"evil","command":["/usr/bin/sh","-c","x"],"roles":["dock","session_lock"]}]}"#),
            layer_trust(&link, false),
        );
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert_eq!(names(&c), Vec::<&str>::new());
    }

    #[test]
    fn no_file_names_a_built_in_process_and_no_role_name_grants_a_built_in_role() {
        let json = r#"{"shell":[
            {"name":"portal","command":["/usr/bin/cupertino-dock"],"roles":["session"]},
            {"name":"agent","command":["/usr/bin/cupertino-polkit-agent"],"roles":[]},
            {"name":"prompter","command":["/usr/bin/cupertino-keyring-prompter"],"roles":["session"]},
            {"name":"capture","command":["target/debug/cupertino-portal"],"roles":["session"]},
            {"name":"dock","command":["/usr/bin/cupertino-dock"],"roles":["session","dock"]}]}"#;
        for trust in [LayerTrust::System, LayerTrust::User, LayerTrust::Dev] {
            let mut c = Config::default();
            let problems = c.apply_privileged(&settings(json), trust);
            assert!(
                names(&c).iter().all(|n| *n == "dock"),
                "{trust:?}: {:?}",
                names(&c)
            );
            assert!(problems.len() >= 4, "{trust:?}: {problems:?}");
        }
        let granted: Vec<ShellRole> = cupertino_defaults::compositor::ShellRoleName::ALL
            .into_iter()
            .map(ShellRole::from)
            .collect();
        for b in BuiltIn::ALL {
            assert!(
                !granted.contains(&b.role()),
                "a file can grant {:?}",
                b.role()
            );
        }
    }

    #[test]
    fn a_built_in_program_runs_only_from_a_file_nobody_else_can_change() {
        let (root, me, other) = (0, 1000, 1001);
        let file = |owner: u32, mode: u32| (owner, 0o100000 | mode);
        let bin = |owner: u32, mode: u32| (owner, 0o040000 | mode);
        let cases = [
            (file(root, 0o755), bin(root, 0o755), root, true, "root's"),
            (
                file(root, 0o775),
                bin(root, 0o755),
                root,
                false,
                "a group-writable file",
            ),
            (
                file(root, 0o757),
                bin(root, 0o755),
                root,
                false,
                "a file others can write",
            ),
            (
                file(root, 0o755),
                bin(root, 0o775),
                root,
                false,
                "a group-writable bin",
            ),
            (
                file(root, 0o755),
                bin(root, 0o757),
                root,
                false,
                "a bin others can write",
            ),
            (
                file(root, 0o755),
                bin(me, 0o755),
                root,
                false,
                "the user's bin in root's tree",
            ),
            (
                file(me, 0o755),
                bin(root, 0o755),
                root,
                false,
                "the user's file in root's tree",
            ),
            (
                file(me, 0o755),
                bin(me, 0o755),
                me,
                true,
                "the user's own prefix",
            ),
            (
                file(me, 0o755),
                bin(other, 0o755),
                me,
                false,
                "another user's bin in the user's prefix",
            ),
            (
                file(other, 0o755),
                bin(me, 0o755),
                me,
                false,
                "another user's file in the user's prefix",
            ),
            (
                file(other, 0o755),
                bin(other, 0o755),
                other,
                false,
                "another user's prefix",
            ),
        ];
        for (file, bin, root_owner, allowed, case) in cases {
            assert_eq!(
                may_run_built_in(file, bin, root_owner, me),
                allowed,
                "{case}"
            );
        }
    }

    #[test]
    fn a_built_in_program_that_is_a_link_or_in_a_shared_bin_is_refused() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let bin = root.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        let program = bin.join("cupertino-portal");
        std::fs::write(&program, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(check_built_in(&program), Ok(()), "the user's own prefix");

        let link = bin.join("cupertino-polkit-agent");
        std::os::unix::fs::symlink(&program, &link).unwrap();
        assert!(check_built_in(&link)
            .unwrap_err()
            .contains("not a regular file"));

        let linked_root = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(&bin, linked_root.path().join("bin")).unwrap();
        assert!(
            check_built_in(&linked_root.path().join("bin/cupertino-portal"))
                .unwrap_err()
                .contains("not a directory")
        );

        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o777)).unwrap();
        assert!(check_built_in(&program)
            .unwrap_err()
            .contains("not root's alone"));
    }

    const DEV_FOLDER: &str =
        r#"{"dev_folder":{"path":"/home/me/.local/lib/cupertino/dev","components":["dock"]}}"#;

    #[test]
    fn only_the_system_file_declares_the_development_folder() {
        let mut c = system_config();
        assert_eq!(c.dev_folder, None, "no folder unless one is declared");
        assert!(c
            .apply_privileged(&settings(DEV_FOLDER), LayerTrust::System)
            .is_empty());
        let declared = Some(DevFolder {
            path: "/home/me/.local/lib/cupertino/dev".into(),
            components: vec!["dock".into()],
        });
        assert_eq!(c.dev_folder, declared);

        let widened =
            r#"{"dev_folder":{"path":"/tmp/evil","components":["dock","menubar","login"]}}"#;
        let problems = c.apply_privileged(&settings(widened), LayerTrust::User);
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(
            problems[0].contains("only the system configuration"),
            "{problems:?}"
        );
        assert_eq!(
            c.dev_folder, declared,
            "the user file neither moves nor widens it"
        );

        let mut c = system_config();
        c.apply_privileged(&settings(DEV_FOLDER), LayerTrust::User);
        assert_eq!(c.dev_folder, None, "the user file cannot enable it");

        let mut dev = Config::default();
        assert!(dev
            .apply_privileged(&settings(DEV_FOLDER), LayerTrust::Dev)
            .is_empty());
        assert!(
            dev.dev_folder.is_some(),
            "a development run trusts every file"
        );
    }

    #[test]
    fn only_processes_whose_roles_are_all_chrome_roles_may_start_from_the_development_folder() {
        let packaged = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .ancestors()
                .nth(2)
                .unwrap()
                .join("packaging/files/etc/cupertino/compositor.json"),
        )
        .unwrap();
        let mut c = Config::default();
        assert!(c
            .apply_privileged(&settings(&packaged), LayerTrust::System)
            .is_empty());
        c.shell_processes.push(ShellProcess {
            name: "clipboard".into(),
            command: vec!["/usr/bin/clipboard-manager".into()],
            roles: vec![ShellRole::Session, ShellRole::DataControl],
        });
        let folder = DevFolder {
            path: "/home/me/dev".into(),
            components: [
                "dock",
                "menubar",
                "launcher",
                "notifications",
                "controlcenter",
                "osd",
                "windowmanager",
                "login",
                "clipboard",
                "portal",
                "polkit-agent",
                "keyring-prompter",
                "desktop-control",
                "nonexistent",
            ]
            .map(String::from)
            .to_vec(),
        };
        let (eligible, warnings) = folder.eligible(&c.shell_processes);
        assert_eq!(
            eligible,
            [
                "dock",
                "menubar",
                "launcher",
                "notifications",
                "controlcenter",
                "osd",
                "windowmanager"
            ]
        );
        assert_eq!(warnings.len(), 7, "{warnings:?}");
        assert!(warnings[0].contains("login") && warnings[0].contains("SessionLock"));
        assert!(warnings[1].contains("clipboard") && warnings[1].contains("DataControl"));

        for role in [
            ShellRole::SessionLock,
            ShellRole::Capture,
            ShellRole::Prompt,
            ShellRole::DataControl,
            ShellRole::DesktopControl,
        ] {
            assert!(
                !may_start_from_dev_folder(&[ShellRole::Session, role]),
                "{role:?}"
            );
        }
        for b in BuiltIn::ALL {
            assert!(!may_start_from_dev_folder(&[b.role()]), "{b:?}");
        }
        assert!(may_start_from_dev_folder(&[]));
        assert!(may_start_from_dev_folder(&DEV_FOLDER_ROLES));
        // the file's role names agree, so the root setup command offers exactly the processes the server accepts
        for name in cupertino_defaults::compositor::ShellRoleName::ALL {
            assert_eq!(
                name.allows_dev_folder(),
                DEV_FOLDER_ROLES.contains(&ShellRole::from(name)),
                "{name:?}"
            );
        }
    }

    #[test]
    fn a_development_program_starts_only_from_a_file_nobody_else_can_change() {
        let (root, me, other) = (0, 1000, 1001);
        let file = |owner: u32, mode: u32| (owner, libc::S_IFREG | mode);
        let dir = |owner: u32, mode: u32| (owner, libc::S_IFDIR | mode);
        let link = (me, libc::S_IFLNK | 0o777);
        let cases = [
            (
                file(me, 0o755),
                dir(me, 0o700),
                true,
                "the user's file in the user's folder",
            ),
            (file(root, 0o755), dir(me, 0o700), true, "root's file"),
            (file(me, 0o700), dir(root, 0o755), true, "root's folder"),
            (
                file(other, 0o755),
                dir(me, 0o700),
                false,
                "another user's file",
            ),
            (
                file(me, 0o755),
                dir(other, 0o700),
                false,
                "another user's folder",
            ),
            (
                file(me, 0o775),
                dir(me, 0o700),
                false,
                "a group-writable file",
            ),
            (
                file(me, 0o757),
                dir(me, 0o700),
                false,
                "a file others can write",
            ),
            (
                file(me, 0o755),
                dir(me, 0o770),
                false,
                "a group-writable folder",
            ),
            (
                file(me, 0o755),
                dir(me, 0o707),
                false,
                "a folder others can write",
            ),
            (
                file(me, 0o644),
                dir(me, 0o700),
                false,
                "a file without an execute bit",
            ),
            (link, dir(me, 0o700), false, "a link"),
            (dir(me, 0o755), dir(me, 0o700), false, "a directory"),
            (file(me, 0o755), link, false, "a folder that is a link"),
        ];
        for (file, folder, allowed, case) in cases {
            assert_eq!(
                may_start_dev_program(file, folder, me).is_ok(),
                allowed,
                "{case}"
            );
        }
    }

    #[test]
    fn a_development_program_that_is_missing_a_link_or_shared_is_not_started() {
        use std::os::unix::fs::PermissionsExt;
        let mode = |path: &Path, mode: u32| {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap()
        };
        let me = rustix::process::getuid().as_raw();
        let base = tempfile::tempdir().unwrap();
        let folder = base.path().join("dev");
        let installed = "/usr/bin/cupertino-dock";
        assert_eq!(
            dev_folder_program(&folder, installed, me),
            Ok(None),
            "no folder"
        );
        std::fs::create_dir(&folder).unwrap();
        mode(&folder, 0o700);
        assert_eq!(
            dev_folder_program(&folder, installed, me),
            Ok(None),
            "no file"
        );

        let program = folder.join("cupertino-dock");
        std::fs::write(&program, "#!/bin/sh\n").unwrap();
        mode(&program, 0o755);
        assert_eq!(
            dev_folder_program(&folder, installed, me),
            Ok(Some(program.clone()))
        );
        assert_eq!(
            dev_folder_program(&folder, "cupertino-dock", me),
            Ok(Some(program.clone())),
            "a development run's program by name"
        );

        mode(&program, 0o644);
        assert!(dev_folder_program(&folder, installed, me)
            .unwrap_err()
            .contains("not executable"));
        mode(&program, 0o775);
        assert!(dev_folder_program(&folder, installed, me)
            .unwrap_err()
            .contains("group or others"));
        mode(&program, 0o755);
        mode(&folder, 0o777);
        assert!(dev_folder_program(&folder, installed, me)
            .unwrap_err()
            .contains("development folder"));
        mode(&folder, 0o700);

        std::fs::remove_file(&program).unwrap();
        let elsewhere = base.path().join("cupertino-dock");
        std::fs::write(&elsewhere, "#!/bin/sh\n").unwrap();
        mode(&elsewhere, 0o755);
        std::os::unix::fs::symlink(&elsewhere, &program).unwrap();
        assert!(dev_folder_program(&folder, installed, me)
            .unwrap_err()
            .contains("not a regular file"));

        std::fs::remove_file(&program).unwrap();
        let linked = base.path().join("linked");
        std::os::unix::fs::symlink(&folder, &linked).unwrap();
        std::fs::write(&program, "#!/bin/sh\n").unwrap();
        mode(&program, 0o755);
        assert!(dev_folder_program(&linked, installed, me)
            .unwrap_err()
            .contains("not a directory"));
        if me != 0 {
            assert!(
                dev_folder_program(&folder, installed, me + 1).is_err(),
                "another user's session does not start it"
            );
        }
    }

    #[test]
    fn the_login_screen_is_told_by_its_file_or_its_session_class() {
        let os = |s: &'static str| Some(std::ffi::OsStr::new(s));
        assert!(login_screen(os(GREETER_CONFIG), None));
        assert!(login_screen(None, os("greeter")));
        assert!(!login_screen(os(SYSTEM_CONFIG), os("user")));
        assert!(!login_screen(None, None));
    }

    #[test]
    fn a_system_file_is_read_through_one_checked_descriptor() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("compositor.json");
        std::fs::write(&file, "{}").unwrap();
        assert!(read_system_file(&dir.path().join("missing.json"))
            .unwrap()
            .is_none());

        let link = dir.path().join("link.json");
        std::os::unix::fs::symlink(&file, &link).unwrap();
        assert!(read_system_file(&link).is_err(), "a link is never followed");
        if unsafe { libc::geteuid() } != 0 {
            assert_eq!(
                read_system_file(&file).unwrap_err().kind(),
                std::io::ErrorKind::PermissionDenied,
                "a file the person owns is no system file"
            );
        }
    }

    #[test]
    fn the_packaged_system_files_pass_their_own_screening() {
        let etc = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .unwrap()
            .join("packaging/files/etc/cupertino");
        let files = ["compositor.json", "greeter.json"]
            .map(|name| std::fs::read_to_string(etc.join(name)).unwrap());
        for json in &files {
            let file = settings(json);
            let mut c = Config::default();
            let problems = c.apply_privileged(&file, LayerTrust::System);
            assert!(problems.is_empty(), "{problems:?}");
            assert_eq!(c.shell_processes.len(), file.shell.unwrap().len());
        }
        let mut c = Config::default();
        c.apply_privileged(&settings(&files[0]), LayerTrust::System);
        let login = c
            .shell_processes
            .iter()
            .find(|p| p.name == "login")
            .unwrap();
        assert!(login.roles.contains(&ShellRole::SessionLock));
        let others = c.shell_processes.iter().filter(|p| p.name != "login");
        assert!(others
            .flat_map(|p| &p.roles)
            .all(|r| *r != ShellRole::SessionLock));
    }
}
