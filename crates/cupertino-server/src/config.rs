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

/// The system file of the login screen's server, which `$CUPERTINO_SERVER_CONFIG` names when the login manager starts it.
pub const GREETER_CONFIG: &str = cupertino_defaults::compositor::GREETER_CONF_PATH;

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
    /// The session's prompts (`cupertino-prompts`): the polkit authentication agent, the keyring's prompter and the
    /// consent dialogs, drawn as prompts.
    Prompts,
    /// The native local desktop controller; each run still needs session consent.
    DesktopControl,
    /// The lock screen / login screen program (`cupertino-login`); started by the server in both user and login sessions (W5.5).
    Login,
    /// The shell's host (`cupertino-shell`), started while a component runs in it, not with the others of [`BuiltIn::ALL`]
    /// (`crate::supervisor::HostProgram`).
    Host,
}

/// The bus name the portal's backend owns once it serves.
pub const PORTAL_BUS_NAME: &str = "org.freedesktop.impl.portal.desktop.cupertino";

impl BuiltIn {
    pub const ALL: [BuiltIn; 4] = [
        BuiltIn::Portal,
        BuiltIn::Prompts,
        BuiltIn::DesktopControl,
        BuiltIn::Login,
    ];

    /// Name in log lines.
    pub const fn name(self) -> &'static str {
        match self {
            BuiltIn::Portal => "portal",
            BuiltIn::Prompts => "prompts",
            BuiltIn::DesktopControl => "desktop-control",
            BuiltIn::Login => "login",
            BuiltIn::Host => "host",
        }
    }

    /// The file name of the program under an install root's `bin`.
    pub(crate) const fn program_name(self) -> &'static str {
        match self {
            BuiltIn::Portal => "cupertino-portal",
            BuiltIn::Prompts => "cupertino-prompts",
            BuiltIn::DesktopControl => "cupertino-control",
            BuiltIn::Login => "cupertino-login",
            BuiltIn::Host => "cupertino-shell",
        }
    }

    /// The role of the process's connection; the portal holds [`ShellRole::GlobalShortcuts`] besides, login
    /// [`ShellRole::Session`] besides [`ShellRole::SessionLock`], and the prompts [`ShellRole::Session`] and
    /// [`ShellRole::Consent`] besides [`ShellRole::Prompt`] ([`BuiltIn::process`]). The session agent is the `agent`
    /// component a host runs, never a process of its own.
    pub const fn role(self) -> ShellRole {
        match self {
            BuiltIn::Portal => ShellRole::Capture,
            BuiltIn::Prompts => ShellRole::Prompt,
            BuiltIn::DesktopControl => ShellRole::DesktopControl,
            BuiltIn::Login => ShellRole::SessionLock,
            BuiltIn::Host => ShellRole::Host,
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
            roles: match self {
                // the portal also binds the shortcuts the person allows applications
                BuiltIn::Portal => vec![self.role(), ShellRole::GlobalShortcuts],
                // the lock screen follows the session (its lock and idle events) beside its own role
                BuiltIn::Login => vec![ShellRole::Session, ShellRole::SessionLock],
                // the prompts follow the session (its lock) and ask the person's consent beside their own role
                BuiltIn::Prompts => vec![ShellRole::Session, ShellRole::Prompt, ShellRole::Consent],
                _ => vec![self.role()],
            },
        }
    }

    /// The supervised process that runs `cupertino-login --greeter` in a login session (session role only).
    pub fn login_greeter_process(program: &Path) -> ShellProcess {
        ShellProcess {
            name: BuiltIn::Login.name().to_string(),
            command: vec![program.display().to_string(), "--greeter".into()],
            roles: vec![ShellRole::Session],
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
pub const DEV_FOLDER_ROLES: [ShellRole; 6] = [
    ShellRole::Session,
    ShellRole::MenuBar,
    ShellRole::Dock,
    ShellRole::WindowManager,
    ShellRole::Overview,
    ShellRole::Windows,
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
pub fn layer_trust(path: &Path, dev_shell: bool) -> LayerTrust {
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

pub use cupertino_server_render::config::Insets;

/// Window corner style. radius for regular windows: 16 pt, the decorations' window corner in every window style; the
/// continuous-corner expansion comes from `cupertino-geometry` (approximate).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindowStyle {
    pub corner_radius: f64,
    pub continuous_corners: bool,
    pub shadow: ShadowStyle,
    /// Apply the corner mask, outline and shadow to clients that draw their own decorations, as to the windows the
    /// compositor decorates.
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
    /// Reserve the menu bar's height in the usable area (off: windows use the whole output).
    pub show_menu_bar: bool,
    /// The bar hides until the pointer reaches the top edge (`menu_bar_auto_hide`).
    pub menu_bar_auto_hide: bool,
}

impl Default for ShellConfig {
    fn default() -> Self {
        Self {
            menu_bar_height: cupertino_theme::ShapeTokens::default().menu_bar_height,
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
    /// Shell host components configured for the session (`shell.components`, system file only; defaults to all 11).
    pub shell_components: Vec<String>,
    /// Shell host components disabled (`shell.off`).
    pub shell_off: Vec<String>,
    /// Shell host components running in separate host processes (`shell.alone`).
    pub shell_alone: Vec<String>,
    /// Legacy shell processes supervised by the server (`shell.processes` or legacy array form); none by default.
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
    /// How each display at a fractional scale is drawn (`fractional_scale_mode`): at the next whole scale and shrunk
    /// to its pixels by default.
    pub fractional_scale_mode: cupertino_defaults::compositor::FractionalScaleModes,
    /// Whether the server makes the toolkit's GPU programs into the per-user program cache when the session starts
    /// (`prepare_toolkit_programs`, [`crate::toolkit_programs`]), read then.
    pub prepare_toolkit_programs: bool,
    /// Windows take a slight tint of the wallpaper's colour (`tint_windows` of `appearance.conf`).
    pub tint_windows: bool,
    /// What a double click on a server-drawn title bar does (`titlebar_double_click` of `appearance.conf`).
    pub titlebar_double_click: cupertino_defaults::appearance::TitlebarAction,
}

pub use cupertino_server_render::config::WallpaperSettings;

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

impl cupertino_server_input::events::PointerAccelConfig for PointerSettings {
    fn mouse_speed(&self) -> f64 {
        self.mouse_speed
    }

    fn trackpad_speed(&self) -> f64 {
        self.trackpad_speed
    }

    fn mouse_dpi(&self, key: &str) -> Option<u32> {
        self.mouse_dpi.get(key).copied()
    }

    fn mouse_acceleration(&self) -> bool {
        self.mouse_acceleration
    }

    fn trackpad_acceleration(&self) -> bool {
        self.trackpad_acceleration
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
            shell_components: cupertino_defaults::compositor::SHELL_COMPONENTS
                .into_iter()
                .map(String::from)
                .collect(),
            shell_off: Vec::new(),
            shell_alone: Vec::new(),
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
            output_depth: crate::output_depth::Depth::Ten,
            fractional_scale_mode: Default::default(),
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
        if let Some(m) = &f.fractional_scale_mode {
            self.fractional_scale_mode = m.clone();
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
        if let Some(section) = &f.shell {
            let filter_known =
                |names: &[String], key: &str, problems: &mut Vec<String>| -> Vec<String> {
                    let mut out = Vec::new();
                    for name in names {
                        if crate::host::component_spec(name).is_some() {
                            if !out.contains(name) {
                                out.push(name.clone());
                            }
                        } else {
                            problems.push(format!("shell.{key}: unknown component {name:?}"));
                        }
                    }
                    out
                };
            if let Some(components) = &section.components {
                match trust {
                    LayerTrust::System | LayerTrust::Dev => {
                        self.shell_components =
                            filter_known(components, "components", &mut problems);
                    }
                    LayerTrust::User => problems.push(
                        "shell.components: only the system configuration sets the component list"
                            .to_string(),
                    ),
                }
            }
            if let Some(off) = &section.off {
                self.shell_off = filter_known(off, "off", &mut problems);
            }
            if let Some(alone) = &section.alone {
                self.shell_alone = filter_known(alone, "alone", &mut problems);
            }
            if let Some(list) = &section.processes {
                let list: Vec<_> = list
                    .iter()
                    .filter(|e| {
                        let built_in = BuiltIn::ALL
                            .into_iter()
                            .chain([BuiltIn::Host])
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

    /// Components that should run in the session: `shell_components` minus `shell_off`, empty in the login session.
    pub fn active_shell_components(&self, login_screen: bool) -> Vec<&'static str> {
        if login_screen {
            return Vec::new();
        }
        self.shell_components
            .iter()
            .filter(|name| !self.shell_off.contains(name))
            .filter_map(|name| crate::host::component_spec(name).map(|s| s.name))
            .collect()
    }

    /// Components that run in a separate host (`shell_alone` or eligible `dev_folder`).
    pub fn alone_host_components(&self, login_screen: bool) -> Vec<&'static str> {
        let dev_list: &[String] = self
            .dev_folder
            .as_ref()
            .map(|d| d.components.as_slice())
            .unwrap_or(&[]);
        self.active_shell_components(login_screen)
            .into_iter()
            .filter(|&name| {
                self.shell_alone.iter().any(|a| a == name)
                    || (dev_list.iter().any(|d| d == name)
                        && crate::host::component_spec(name)
                            .is_some_and(|s| may_start_from_dev_folder(s.roles)))
            })
            .collect()
    }

    /// Components that run in the shared host (`active_shell_components` minus `alone_host_components`).
    pub fn shared_host_components(&self, login_screen: bool) -> Vec<&'static str> {
        let alone = self.alone_host_components(login_screen);
        self.active_shell_components(login_screen)
            .into_iter()
            .filter(|name| !alone.contains(name))
            .collect()
    }

    /// Fallback legacy supervised process entry for `name` when the host is unavailable or not installed (until W6.close).
    pub fn process_fallback_for(&self, name: &str) -> Option<&ShellProcess> {
        if self.shell_off.iter().any(|o| o == name) {
            return None;
        }
        self.shell_processes.iter().find(|p| p.name == name)
    }
}

/// What the server knows about the hosts when it works out where the components run ([`Config::shell_plan`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HostFacts {
    /// The host program is installed and was not refused.
    pub available: bool,
    /// Components that run alone whatever the configuration says: lost ones, or every one after the shared host's
    /// crashes (D18).
    pub isolated: Vec<&'static str>,
    /// Components a host was built without (`unavailable` with `absent`): they run as their process entries.
    pub absent: Vec<&'static str>,
    /// Components of the development folder that run in the installed host: the folder's file is absent or refused, or
    /// a host from the folder gave the component back, until `cupertino shell restart NAME`.
    pub not_from_dev: Vec<&'static str>,
}

/// Where each component of the session runs, and which supervised processes run beside the hosts.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ShellPlan {
    /// The list of the shared host, in the table's order; `None` when no component runs in a host, so no host runs.
    pub shared: Option<Vec<&'static str>>,
    /// The components that run in a separate host each, in the table's order, and whether that host starts from the
    /// development folder.
    pub separate: Vec<(&'static str, bool)>,
    /// The configured processes that run: those that are no component, and the entries of components no host runs.
    pub processes: Vec<ShellProcess>,
    /// The process entries of components that run in a host, which are left alone.
    pub left_alone: Vec<String>,
}

impl ShellPlan {
    /// The components that run in a host.
    pub fn hosted(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.shared
            .iter()
            .flatten()
            .copied()
            .chain(self.separate.iter().map(|(name, _)| *name))
    }
}

impl Config {
    /// Where the components run (`docs/architecture/shell-host.md`, *The component list*): the components of the
    /// session ([`Config::active_shell_components`]) run in a host when the host program is there, except
    /// the ones a host was built without; those of `alone`, of the development folder and the
    /// isolated ones each in a separate host, the others in the shared host. A process entry of a component runs only
    /// when no host runs the component, and an entry turned `off` never runs.
    pub fn shell_plan(&self, login_screen: bool, facts: &HostFacts) -> ShellPlan {
        let active = self.active_shell_components(login_screen);
        let hosted: Vec<&'static str> = if facts.available {
            active
                .iter()
                .copied()
                .filter(|name| !facts.absent.contains(name))
                .collect()
        } else {
            Vec::new()
        };
        // `alone`, and the development folder's components while their folder build may run
        let alone = self.alone_host_components(login_screen);
        let configured_alone = |name: &str| self.shell_alone.iter().any(|a| a == name);
        let dev_listed = |name: &str| {
            self.dev_folder
                .as_ref()
                .is_some_and(|d| d.components.iter().any(|c| c == name))
        };
        let from_dev = |name: &&'static str| {
            alone.contains(name) && dev_listed(name) && !facts.not_from_dev.contains(name)
        };
        let separate: Vec<(&'static str, bool)> = hosted
            .iter()
            .filter(|name| {
                configured_alone(name) || from_dev(name) || facts.isolated.contains(name)
            })
            .map(|name| (*name, from_dev(name)))
            .collect();
        let shared = (!hosted.is_empty()).then(|| {
            hosted
                .iter()
                .copied()
                .filter(|name| !separate.iter().any(|(s, _)| s == name))
                .collect()
        });
        let mut processes = Vec::new();
        let mut left_alone = Vec::new();
        for entry in &self.shell_processes {
            if self.shell_off.contains(&entry.name) {
                continue;
            }
            match crate::host::component_spec(&entry.name) {
                None => processes.push(entry.clone()),
                Some(spec) if hosted.contains(&spec.name) => left_alone.push(entry.name.clone()),
                Some(spec) if active.contains(&spec.name) => {
                    processes.extend(self.process_fallback_for(spec.name).cloned());
                }
                Some(_) => {}
            }
        }
        ShellPlan {
            shared,
            separate,
            processes,
            left_alone,
        }
    }

    /// Take the `shell` section of the system file `system` and of the person's file `user` again, each by its trust,
    /// as at start: the component list, `off`, `alone` and the process entries. Returns what is refused.
    pub fn apply_shell_layers(
        &mut self,
        system: &CompositorSettings,
        user: &CompositorSettings,
        trust: (LayerTrust, LayerTrust),
    ) -> Vec<String> {
        let defaults = Config::default();
        let mut layered = Config {
            dev_shell: self.dev_shell,
            ..defaults
        };
        let only_shell = |f: &CompositorSettings| CompositorSettings {
            shell: f.shell.clone(),
            ..CompositorSettings::default()
        };
        let mut problems = layered.apply_privileged(&only_shell(system), trust.0);
        problems.extend(layered.apply_privileged(&only_shell(user), trust.1));
        self.shell_components = layered.shell_components;
        self.shell_off = layered.shell_off;
        self.shell_alone = layered.shell_alone;
        self.shell_processes = layered.shell_processes;
        problems
    }
}
