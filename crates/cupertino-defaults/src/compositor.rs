//! `compositor.json` (`$CUPERTINO_SERVER_CONFIG` overrides the path): the server's settings. One JSON object; a key the
//! file does not set is `None` here and the server's default applies. Keys this module does not know are kept as they
//! are on a write (JSON has no comments; a write orders the keys alphabetically).
//!
//! Each key is checked on its own: a key with a wrong value is a [`JsonProblem`] and, with the last good settings,
//! keeps its value from them; the other keys apply. A file that is not a JSON object is one problem for the whole file.
//! Names of key actions (`keybindings`) are the server's vocabulary and are checked there.
//!
//! | Key | Value |
//! |---|---|
//! | `appearance` | `light`, `dark`, `light-high-contrast`, `dark-high-contrast` |
//! | `terminals` | program names, tried in order |
//! | `keybindings` | object: chord → action name |
//! | `corner_radius`, `menu_bar_height`, `hot_corner_size`, `hot_corner_dwell` | number |
//! | `menu_bar_auto_hide`, `edge_tiling`, `tile_margins` | boolean: the menu bar hides until the pointer reaches the top edge; dragging a window to a screen edge tiles it; tiled windows keep a margin around and between them |
//! | `wallpaper_color` | `#rrggbb`: a solid colour in place of any picture |
//! | `material_density` | number 0 to 1: how much denser (less see-through) every material is made, 0 leaving the recipes as they are |
//! | `output_depth` | `8` or `10`: bits a channel of the displays' primary planes, 8 by default; 10 falls back to 8 where a display has no 10-bit format; read when the session starts |
//! | `show_menu_bar`, `decorations`, `closing_animation`, `mouse_acceleration`, `trackpad_acceleration`, `natural_scrolling` (touchpads), `mouse_natural_scrolling` (wheels), `shake_to_locate` | boolean |
//! | `minimize_effect` | `warp`, `funnel`, `scale`, `plain` (also `none`) |
//! | `show_scroll_bars` | `automatic` (beside the content while a mouse is connected, else over it while it scrolls), `when-scrolling`, `always` |
//! | `mouse_speed`, `trackpad_speed` | tracking speed, 0 to 3 |
//! | `mouse_dpi` | object: mouse key ([`mouse_dpi_key`]) → resolution in dots per inch, 50 to 32000 |
//! | `cursor_size` | cursor scale |
//! | `cursor_fill`, `cursor_outline` | `#rrggbb` or `#rrggbbaa` |
//! | `wallpaper_folders` | absolute paths of the folders the Settings wallpaper pane lists besides the packs' pictures; the server ignores it |
//! | `wallpaper`, `wallpaper_dark` | absolute path of a PNG or JPEG: the wallpaper, and the one of the dark appearance (else `wallpaper` is used) |
//! | `shell` | list of `{"name": …, "command": [program, args…], "roles": […]}`, commands not empty; `roles` (optional, default `["session"]`) lists what the server lets the process do: `session`, `menu_bar`, `dock`, `window_manager` |
//! | `screenshot` | the screenshot command, not empty |
//! | `dev_folder` | `{"path": …, "components": [names…]}`: an absolute directory without `..` and the names of the shell processes that may start from it ([`DevFolder`]); only the system file sets it |

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

/// Environment variable that redirects the settings file.
pub const COMPOSITOR_CONF_ENV: &str = "CUPERTINO_SERVER_CONFIG";

/// A key whose value was not applied, or the whole file (`key` empty) when it is not a JSON object.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JsonProblem {
    pub key: String,
    pub message: String,
}

/// The appearance catalogs a session can start in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppearanceName {
    Light,
    Dark,
    LightHighContrast,
    DarkHighContrast,
}

impl AppearanceName {
    pub const ALL: [AppearanceName; 4] = [
        AppearanceName::Light,
        AppearanceName::Dark,
        AppearanceName::LightHighContrast,
        AppearanceName::DarkHighContrast,
    ];

    pub fn name(self) -> &'static str {
        match self {
            AppearanceName::Light => "light",
            AppearanceName::Dark => "dark",
            AppearanceName::LightHighContrast => "light-high-contrast",
            AppearanceName::DarkHighContrast => "dark-high-contrast",
        }
    }
}

/// The effect a window minimizes with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MinimizeEffectName {
    Warp,
    Funnel,
    Scale,
    /// No effect (`plain`, also written `none`).
    Plain,
}

/// When applications show their scroll bars.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ScrollBarsName {
    /// Always while a mouse is connected, else only while the content scrolls.
    #[default]
    Automatic,
    /// Over the content while it scrolls, hidden after.
    WhenScrolling,
    /// Beside the content at all times.
    Always,
}

impl ScrollBarsName {
    pub const ALL: [ScrollBarsName; 3] = [
        ScrollBarsName::Automatic,
        ScrollBarsName::WhenScrolling,
        ScrollBarsName::Always,
    ];

    pub fn name(self) -> &'static str {
        match self {
            ScrollBarsName::Automatic => "automatic",
            ScrollBarsName::WhenScrolling => "when-scrolling",
            ScrollBarsName::Always => "always",
        }
    }
}

/// A role of the shell protocol a process the server starts can be given.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShellRoleName {
    /// Observe the session (appearance, display options, lock state).
    Session,
    MenuBar,
    Dock,
    WindowManager,
    /// Take the session lock (`ext-session-lock`): the lock screen.
    SessionLock,
    /// Use data control: read and set the selections without focus (a clipboard history).
    Clipboard,
}

impl ShellRoleName {
    pub const ALL: [ShellRoleName; 6] = [
        ShellRoleName::Session,
        ShellRoleName::MenuBar,
        ShellRoleName::Dock,
        ShellRoleName::WindowManager,
        ShellRoleName::SessionLock,
        ShellRoleName::Clipboard,
    ];

    pub fn name(self) -> &'static str {
        match self {
            ShellRoleName::Session => "session",
            ShellRoleName::MenuBar => "menu_bar",
            ShellRoleName::Dock => "dock",
            ShellRoleName::WindowManager => "window_manager",
            ShellRoleName::SessionLock => "session_lock",
            ShellRoleName::Clipboard => "clipboard",
        }
    }

    pub fn from_name(name: &str) -> Option<ShellRoleName> {
        Self::ALL.into_iter().find(|r| r.name() == name)
    }

    /// Whether a process holding this role may still start from the development folder: the roles of the session's
    /// own chrome. The lock screen's and data control's may not, nor may any role added later.
    pub fn allows_dev_folder(self) -> bool {
        matches!(
            self,
            ShellRoleName::Session
                | ShellRoleName::MenuBar
                | ShellRoleName::Dock
                | ShellRoleName::WindowManager
        )
    }
}

impl ShellEntry {
    /// Whether the development folder may apply to this process: every role it holds [`ShellRoleName::allows_dev_folder`].
    pub fn may_start_from_dev_folder(&self) -> bool {
        self.roles.iter().all(|r| r.allows_dev_folder())
    }
}

/// A shell process the server starts and keeps alive.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShellEntry {
    /// Name in log lines.
    pub name: String,
    /// Program and arguments; never empty.
    pub command: Vec<String>,
    /// What the server lets the process do on its connection; a process the file gives no roles observes the session.
    pub roles: Vec<ShellRoleName>,
}

/// The development folder of the system file: the shell processes named in `components` start the program of the same
/// file name from `path` when one is there, instead of the installed one. The server decides which names it accepts and
/// checks the folder and each file before every start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DevFolder {
    /// An absolute directory without `..`.
    pub path: String,
    /// Names of shell processes, each once and none empty.
    pub components: Vec<String>,
}

/// Everything in `compositor.json`; `None` is a key the file does not set.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CompositorSettings {
    pub appearance: Option<AppearanceName>,
    pub terminals: Option<Vec<String>>,
    /// Chord → action name, as written.
    pub keybindings: Option<BTreeMap<String, String>>,
    pub corner_radius: Option<f64>,
    pub menu_bar_height: Option<f64>,
    pub show_menu_bar: Option<bool>,
    /// The menu bar hides until the pointer reaches the top edge, and the work area grows.
    pub menu_bar_auto_hide: Option<bool>,
    /// Dragging a window to a screen edge tiles it.
    pub edge_tiling: Option<bool>,
    /// Tiled windows keep a margin around and between them.
    pub tile_margins: Option<bool>,
    /// A solid colour the desktop shows instead of a picture (sRGB, opaque).
    pub wallpaper_color: Option<[u8; 4]>,
    /// How much denser every material is made, 0 to 1.
    pub material_density: Option<f64>,
    /// Bits a channel of the primary planes: 8 or 10.
    pub output_depth: Option<u8>,
    pub decorations: Option<bool>,
    pub minimize_effect: Option<MinimizeEffectName>,
    pub show_scroll_bars: Option<ScrollBarsName>,
    pub closing_animation: Option<bool>,
    pub hot_corner_size: Option<f64>,
    pub hot_corner_dwell: Option<f64>,
    pub mouse_speed: Option<f64>,
    pub trackpad_speed: Option<f64>,
    /// Resolution the user stored per mouse, by [`mouse_dpi_key`].
    pub mouse_dpi: Option<BTreeMap<String, u32>>,
    pub mouse_acceleration: Option<bool>,
    /// Speed-dependent acceleration of touchpads; off gives a constant gain by `trackpad_speed`.
    pub trackpad_acceleration: Option<bool>,
    pub natural_scrolling: Option<bool>,
    pub mouse_natural_scrolling: Option<bool>,
    pub cursor_size: Option<f64>,
    /// sRGB with alpha.
    pub cursor_fill: Option<[u8; 4]>,
    pub cursor_outline: Option<[u8; 4]>,
    pub shake_to_locate: Option<bool>,
    /// Absolute path of the wallpaper image; the dark appearance uses `wallpaper_dark` when it is set.
    pub wallpaper: Option<String>,
    pub wallpaper_dark: Option<String>,
    /// Absolute paths of the folders of pictures the user added in Settings (the server does not read them).
    pub wallpaper_folders: Option<Vec<String>>,
    pub shell: Option<Vec<ShellEntry>>,
    pub screenshot: Option<Vec<String>>,
    pub dev_folder: Option<DevFolder>,
}

/// Smallest and largest resolution a mouse can be given, in dots per inch.
pub const MOUSE_DPI_RANGE: std::ops::RangeInclusive<u32> = 50..=32_000;

/// The key of a mouse in `mouse_dpi`: USB vendor and product ids in hex, and the device name.
pub fn mouse_dpi_key(vendor: u32, product: u32, name: &str) -> String {
    format!("{vendor:04x}:{product:04x}:{name}")
}

/// An sRGB colour written `#rrggbb` or `#rrggbbaa`.
pub fn parse_rgba(text: &str) -> Result<[u8; 4], String> {
    let invalid = || format!("colour {text:?} is not #rrggbb or #rrggbbaa");
    let hex = text
        .strip_prefix('#')
        .filter(|h| matches!(h.len(), 6 | 8) && h.is_ascii())
        .ok_or_else(invalid)?;
    let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).map_err(|_| invalid());
    Ok([
        byte(0)?,
        byte(2)?,
        byte(4)?,
        if hex.len() == 8 { byte(6)? } else { 255 },
    ])
}

/// How a colour is written: `#rrggbb` when opaque, else `#rrggbbaa`.
pub fn rgba_text(c: [u8; 4]) -> String {
    match c[3] {
        255 => format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2]),
        a => format!("#{:02x}{:02x}{:02x}{a:02x}", c[0], c[1], c[2]),
    }
}

fn number(v: &Value) -> Result<f64, String> {
    v.as_f64()
        .ok_or_else(|| format!("expected a number, got {v}"))
}

fn boolean(v: &Value) -> Result<bool, String> {
    v.as_bool()
        .ok_or_else(|| format!("expected true or false, got {v}"))
}

fn string(v: &Value) -> Result<&str, String> {
    v.as_str()
        .ok_or_else(|| format!("expected a string, got {v}"))
}

fn strings(v: &Value) -> Result<Vec<String>, String> {
    v.as_array()
        .ok_or_else(|| format!("expected a list of strings, got {v}"))?
        .iter()
        .map(|s| string(s).map(str::to_string))
        .collect()
}

fn shell_entries(v: &Value) -> Result<Vec<ShellEntry>, String> {
    let list = v
        .as_array()
        .ok_or_else(|| format!("expected a list of shell processes, got {v}"))?;
    let mut out = Vec::new();
    for item in list {
        let obj = item
            .as_object()
            .ok_or_else(|| format!("expected {{\"name\": …, \"command\": […]}}, got {item}"))?;
        let name = string(obj.get("name").ok_or("a shell process needs a name")?)?.to_string();
        let command = strings(
            obj.get("command")
                .ok_or_else(|| format!("shell process {name} needs a command"))?,
        )?;
        if command.is_empty() {
            return Err(format!("shell process {name} has an empty command"));
        }
        let roles = match obj.get("roles") {
            None => vec![ShellRoleName::Session],
            Some(v) => strings(v)?
                .iter()
                .map(|r| {
                    ShellRoleName::from_name(r)
                        .ok_or_else(|| format!("shell process {name} has the unknown role {r:?}"))
                })
                .collect::<Result<_, _>>()?,
        };
        out.push(ShellEntry {
            name,
            command,
            roles,
        });
    }
    Ok(out)
}

/// How one key of the file is read and written.
struct JsonKey {
    name: &'static str,
    read: fn(&mut CompositorSettings, &Value) -> Result<(), String>,
    write: fn(&CompositorSettings) -> Option<Value>,
    keep: fn(&mut CompositorSettings, &CompositorSettings),
}

/// A key whose value is the `Option` field `$field`: `$parse` reads the value, `$value` writes it.
macro_rules! json_key {
    ($field:ident, $parse:expr, $value:expr) => {
        JsonKey {
            name: stringify!($field),
            read: |s, v| ($parse)(v).map(|x| s.$field = Some(x)),
            write: |s| s.$field.as_ref().map($value),
            keep: |s, g| s.$field = g.$field.clone(),
        }
    };
}

/// A list of absolute paths.
fn absolute_paths(v: &Value) -> Result<Vec<String>, String> {
    strings(v)?
        .into_iter()
        .map(|text| {
            if Path::new(&text).is_absolute() {
                Ok(text)
            } else {
                Err(format!("{text:?} is not an absolute path"))
            }
        })
        .collect()
}

/// An absolute path as text.
fn absolute_path(v: &Value) -> Result<String, String> {
    let text = string(v)?;
    if Path::new(text).is_absolute() {
        Ok(text.to_string())
    } else {
        Err(format!("{text:?} is not an absolute path"))
    }
}

/// The `dev_folder` object: an absolute `path` without `..` and a list of distinct, non-empty `components`.
fn dev_folder(v: &Value) -> Result<DevFolder, String> {
    let obj = v
        .as_object()
        .ok_or_else(|| format!("expected {{\"path\": …, \"components\": […]}}, got {v}"))?;
    let path = absolute_path(
        obj.get("path")
            .ok_or("the development folder needs a path")?,
    )?;
    if Path::new(&path)
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(format!("{path:?} goes up with `..`"));
    }
    let components = strings(
        obj.get("components")
            .ok_or("the development folder needs a list of components")?,
    )?;
    for (i, name) in components.iter().enumerate() {
        if name.is_empty() {
            return Err("a component name is empty".to_string());
        }
        if components[..i].contains(name) {
            return Err(format!("the component {name:?} is listed twice"));
        }
    }
    Ok(DevFolder { path, components })
}

fn dev_folder_value(folder: &DevFolder) -> Value {
    let mut m = Map::new();
    m.insert("path".into(), Value::String(folder.path.clone()));
    m.insert("components".into(), strings_value(&folder.components));
    Value::Object(m)
}

fn number_value(n: &f64) -> Value {
    serde_json::Number::from_f64(*n).map_or(Value::Null, Value::Number)
}

fn bool_value(b: &bool) -> Value {
    Value::Bool(*b)
}

fn strings_value(list: &[String]) -> Value {
    Value::Array(list.iter().cloned().map(Value::String).collect())
}

const KEYS: &[JsonKey] = &[
    json_key!(
        appearance,
        |v: &Value| string(v).and_then(|s| AppearanceName::ALL
            .iter()
            .copied()
            .find(|a| a.name() == s)
            .ok_or_else(|| format!("unknown appearance {s:?}"))),
        |a: &AppearanceName| Value::String(a.name().to_string())
    ),
    json_key!(terminals, strings, |l: &Vec<String>| strings_value(l)),
    json_key!(
        keybindings,
        |v: &Value| {
            let obj = v
                .as_object()
                .ok_or_else(|| format!("expected an object of chord to action, got {v}"))?;
            obj.iter()
                .map(|(chord, action)| string(action).map(|a| (chord.clone(), a.to_string())))
                .collect::<Result<BTreeMap<_, _>, _>>()
        },
        |m: &BTreeMap<String, String>| Value::Object(
            m.iter()
                .map(|(k, v)| (k.clone(), Value::String(v.clone())))
                .collect()
        )
    ),
    json_key!(corner_radius, number, number_value),
    json_key!(menu_bar_height, number, number_value),
    json_key!(show_menu_bar, boolean, bool_value),
    json_key!(decorations, boolean, bool_value),
    json_key!(
        minimize_effect,
        |v: &Value| match string(v)? {
            "warp" => Ok(MinimizeEffectName::Warp),
            "funnel" => Ok(MinimizeEffectName::Funnel),
            "scale" => Ok(MinimizeEffectName::Scale),
            "plain" | "none" => Ok(MinimizeEffectName::Plain),
            other => Err(format!("unknown minimize effect {other}")),
        },
        |e: &MinimizeEffectName| Value::String(
            match e {
                MinimizeEffectName::Warp => "warp",
                MinimizeEffectName::Funnel => "funnel",
                MinimizeEffectName::Scale => "scale",
                MinimizeEffectName::Plain => "plain",
            }
            .to_string()
        )
    ),
    json_key!(
        show_scroll_bars,
        |v: &Value| string(v).and_then(|s| ScrollBarsName::ALL
            .iter()
            .copied()
            .find(|b| b.name() == s)
            .ok_or_else(|| format!("unknown scroll bar setting {s:?}"))),
        |b: &ScrollBarsName| Value::String(b.name().to_string())
    ),
    json_key!(closing_animation, boolean, bool_value),
    json_key!(hot_corner_size, number, number_value),
    json_key!(hot_corner_dwell, number, number_value),
    json_key!(mouse_speed, number, number_value),
    json_key!(trackpad_speed, number, number_value),
    json_key!(
        mouse_dpi,
        |v: &Value| {
            let obj = v
                .as_object()
                .ok_or_else(|| format!("expected an object of mouse to dpi, got {v}"))?;
            obj.iter()
                .map(|(mouse, dpi)| {
                    dpi.as_u64()
                        .and_then(|d| u32::try_from(d).ok())
                        .filter(|d| MOUSE_DPI_RANGE.contains(d))
                        .map(|d| (mouse.clone(), d))
                        .ok_or_else(|| {
                            format!(
                                "the dpi of {mouse} is {dpi}, not a whole number from {} to {}",
                                MOUSE_DPI_RANGE.start(),
                                MOUSE_DPI_RANGE.end()
                            )
                        })
                })
                .collect::<Result<BTreeMap<_, _>, _>>()
        },
        |m: &BTreeMap<String, u32>| Value::Object(
            m.iter()
                .map(|(k, v)| (k.clone(), Value::from(*v)))
                .collect()
        )
    ),
    json_key!(mouse_acceleration, boolean, bool_value),
    json_key!(trackpad_acceleration, boolean, bool_value),
    json_key!(natural_scrolling, boolean, bool_value),
    json_key!(mouse_natural_scrolling, boolean, bool_value),
    json_key!(cursor_size, number, number_value),
    json_key!(
        cursor_fill,
        |v: &Value| string(v).and_then(parse_rgba),
        |c: &[u8; 4]| Value::String(rgba_text(*c))
    ),
    json_key!(
        cursor_outline,
        |v: &Value| string(v).and_then(parse_rgba),
        |c: &[u8; 4]| Value::String(rgba_text(*c))
    ),
    json_key!(shake_to_locate, boolean, bool_value),
    json_key!(menu_bar_auto_hide, boolean, bool_value),
    json_key!(edge_tiling, boolean, bool_value),
    json_key!(tile_margins, boolean, bool_value),
    json_key!(
        wallpaper_color,
        |v: &Value| string(v).and_then(parse_rgba),
        |c: &[u8; 4]| Value::String(rgba_text(*c))
    ),
    json_key!(
        material_density,
        |v: &Value| number(v).and_then(|n| if (0.0..=1.0).contains(&n) {
            Ok(n)
        } else {
            Err(format!("material density {n} is not between 0 and 1"))
        }),
        number_value
    ),
    json_key!(
        output_depth,
        |v: &Value| match v.as_u64() {
            Some(8) => Ok(8),
            Some(10) => Ok(10),
            _ => Err(format!("the output depth is 8 or 10 bits, not {v}")),
        },
        |b: &u8| Value::from(*b)
    ),
    json_key!(wallpaper, absolute_path, |s: &String| Value::String(
        s.clone()
    )),
    json_key!(wallpaper_dark, absolute_path, |s: &String| Value::String(
        s.clone()
    )),
    json_key!(wallpaper_folders, absolute_paths, |l: &Vec<String>| {
        strings_value(l)
    }),
    json_key!(shell, shell_entries, |list: &Vec<ShellEntry>| Value::Array(
        list.iter()
            .map(|e| {
                let mut m = Map::new();
                m.insert("name".into(), Value::String(e.name.clone()));
                m.insert("command".into(), strings_value(&e.command));
                m.insert(
                    "roles".into(),
                    Value::Array(
                        e.roles
                            .iter()
                            .map(|r| Value::String(r.name().into()))
                            .collect(),
                    ),
                );
                Value::Object(m)
            })
            .collect()
    )),
    json_key!(
        screenshot,
        |v: &Value| strings(v).and_then(|c| if c.is_empty() {
            Err("the screenshot command is empty".to_string())
        } else {
            Ok(c)
        }),
        |l: &Vec<String>| strings_value(l)
    ),
    json_key!(dev_folder, dev_folder, dev_folder_value),
];

/// The top-level object of `text`; `{}` for an empty file.
fn object(text: &str) -> Result<Map<String, Value>, JsonProblem> {
    if text.trim().is_empty() {
        return Ok(Map::new());
    }
    match serde_json::from_str::<Value>(text) {
        Ok(Value::Object(m)) => Ok(m),
        Ok(other) => Err(JsonProblem {
            key: String::new(),
            message: format!("expected a JSON object, got {other}"),
        }),
        Err(e) => Err(JsonProblem {
            key: String::new(),
            message: e.to_string(),
        }),
    }
}

fn read_object(
    map: &Map<String, Value>,
    last_good: Option<&CompositorSettings>,
) -> (CompositorSettings, Vec<JsonProblem>) {
    let mut settings = CompositorSettings::default();
    let mut problems = Vec::new();
    for k in KEYS {
        let Some(value) = map.get(k.name) else {
            continue;
        };
        if let Err(message) = (k.read)(&mut settings, value) {
            problems.push(JsonProblem {
                key: k.name.to_string(),
                message,
            });
            if let Some(good) = last_good {
                (k.keep)(&mut settings, good);
            }
        }
    }
    (settings, problems)
}

impl CompositorSettings {
    /// Store `dpi` for the mouse `key`, or with `None` forget it; the other mice keep theirs. A resolution outside
    /// [`MOUSE_DPI_RANGE`] is refused and nothing changes. With no mouse left the key leaves the file.
    pub fn set_mouse_dpi(&mut self, key: &str, dpi: Option<u32>) -> Result<(), String> {
        let mut map = self.mouse_dpi.take().unwrap_or_default();
        match dpi {
            Some(d) if !MOUSE_DPI_RANGE.contains(&d) => {
                self.mouse_dpi = Some(map).filter(|m| !m.is_empty());
                return Err(format!(
                    "{d} dpi is not from {} to {}",
                    MOUSE_DPI_RANGE.start(),
                    MOUSE_DPI_RANGE.end()
                ));
            }
            Some(d) => map.insert(key.to_string(), d),
            None => map.remove(key),
        };
        self.mouse_dpi = Some(map).filter(|m| !m.is_empty());
        Ok(())
    }

    /// Where the file is: `$CUPERTINO_SERVER_CONFIG`, else `compositor.json` in the settings directory.
    pub fn path() -> Option<PathBuf> {
        crate::config_file(COMPOSITOR_CONF_ENV, "compositor.json")
    }

    /// The settings of `text` and their problems. With `last_good`, a key whose value is wrong keeps its value from
    /// it, and so does every key when the text is not a JSON object.
    pub fn parse(text: &str, last_good: Option<&Self>) -> (Self, Vec<JsonProblem>) {
        match object(text) {
            Ok(map) => read_object(&map, last_good),
            Err(problem) => (last_good.cloned().unwrap_or_default(), vec![problem]),
        }
    }

    /// [`Self::parse`] of the file at `path`; a missing file sets nothing.
    pub fn load(path: &Path, last_good: Option<&Self>) -> io::Result<(Self, Vec<JsonProblem>)> {
        match std::fs::read_to_string(path) {
            Ok(text) => Ok(Self::parse(&text, last_good)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok((Self::default(), Vec::new())),
            Err(e) => Err(e),
        }
    }

    /// Write the keys whose value differs from the file's into `path` atomically; every other key stays as it is. A
    /// file that is not a JSON object is not overwritten (an error).
    pub fn store(&self, path: &Path) -> io::Result<Vec<JsonProblem>> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(e),
        };
        let mut map = object(&text).map_err(|p| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{}: {}", path.display(), p.message),
            )
        })?;
        let (before, problems) = read_object(&map, None);
        for k in KEYS {
            let new = (k.write)(self);
            if (k.write)(&before) == new {
                continue;
            }
            match new {
                Some(value) => map.insert(k.name.to_string(), value),
                None => map.remove(k.name),
            };
        }
        let mut out =
            serde_json::to_string_pretty(&Value::Object(map)).map_err(io::Error::other)?;
        out.push('\n');
        crate::atomic::write_atomic(path, out.as_bytes())?;
        Ok(problems)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shell_process_has_the_roles_the_file_lists_and_the_session_role_by_default() {
        let (s, problems) = CompositorSettings::parse(
            r#"{"shell":[{"name":"bar","command":["bar"],"roles":["session","menu_bar"]},{"name":"osd","command":["osd"]}]}"#,
            None,
        );
        assert_eq!(problems, vec![]);
        let roles: Vec<_> = s.shell.unwrap().into_iter().map(|e| e.roles).collect();
        assert_eq!(
            roles,
            vec![
                vec![ShellRoleName::Session, ShellRoleName::MenuBar],
                vec![ShellRoleName::Session]
            ]
        );
    }

    #[test]
    fn an_unknown_shell_role_is_a_problem_and_the_list_is_ignored() {
        let (s, problems) = CompositorSettings::parse(
            r#"{"shell":[{"name":"bar","command":["bar"],"roles":["root"]}]}"#,
            None,
        );
        assert_eq!(s.shell, None);
        assert_eq!(problems.len(), 1);
        assert_eq!(problems[0].key, "shell");
        assert!(
            problems[0].message.contains("unknown role \"root\""),
            "{problems:?}"
        );
    }

    #[test]
    fn the_wallpapers_are_absolute_paths() {
        let (read, problems) = CompositorSettings::parse(
            r#"{"wallpaper": "/home/u/day.png", "wallpaper_dark": "night.png"}"#,
            None,
        );
        assert_eq!(read.wallpaper.as_deref(), Some("/home/u/day.png"));
        assert_eq!(read.wallpaper_dark, None);
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert_eq!(problems[0].key, "wallpaper_dark");
    }

    #[test]
    fn the_wallpaper_folders_are_a_list_of_absolute_paths() {
        let (read, problems) = CompositorSettings::parse(
            r#"{"wallpaper_folders": ["/home/u/Pictures", "/mnt/art"]}"#,
            None,
        );
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(
            read.wallpaper_folders,
            Some(vec!["/home/u/Pictures".to_string(), "/mnt/art".to_string()])
        );
        let (read, problems) =
            CompositorSettings::parse(r#"{"wallpaper_folders": ["/ok", "relative"]}"#, None);
        assert_eq!(read.wallpaper_folders, None);
        assert_eq!(problems[0].key, "wallpaper_folders");
    }

    #[test]
    fn the_resolution_of_each_mouse_is_stored_by_its_key() {
        let key = mouse_dpi_key(0x46d, 0xc077, "USB Optical Mouse");
        assert_eq!(key, "046d:c077:USB Optical Mouse");
        let (read, problems) = CompositorSettings::parse(
            r#"{"mouse_dpi": {"046d:c077:USB Optical Mouse": 1600, "1:2:Slow": 49, "1:3:Half": 800.5}}"#,
            None,
        );
        assert_eq!(read.mouse_dpi, None);
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].message.contains("1:2:Slow"), "{problems:?}");
        let (read, problems) = CompositorSettings::parse(
            r#"{"mouse_dpi": {"046d:c077:USB Optical Mouse": 1600}}"#,
            None,
        );
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(read.mouse_dpi, Some([(key, 1600)].into()));
    }

    #[test]
    fn one_mouse_is_set_or_forgotten_and_the_file_keeps_everything_else() {
        let dir = std::env::temp_dir().join(format!("cupertino-mouse-dpi-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("compositor.json");
        std::fs::write(
            &path,
            r#"{"corner_radius": 10, "future_key": {"kept": true}, "mouse_dpi": {"a:1:First": 800}}"#,
        )
        .unwrap();
        let (mut s, _) = CompositorSettings::load(&path, None).unwrap();
        s.set_mouse_dpi("b:2:Second", Some(3200)).unwrap();
        assert!(s.set_mouse_dpi("b:2:Second", Some(49)).is_err());
        assert!(s.set_mouse_dpi("b:2:Second", Some(32_001)).is_err());
        s.store(&path).unwrap();
        let (read, problems) = CompositorSettings::load(&path, None).unwrap();
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(
            read.mouse_dpi,
            Some(
                [
                    ("a:1:First".to_string(), 800),
                    ("b:2:Second".to_string(), 3200)
                ]
                .into()
            )
        );
        assert_eq!(read.corner_radius, Some(10.0));
        assert!(std::fs::read_to_string(&path)
            .unwrap()
            .contains("future_key"));
        s.set_mouse_dpi("a:1:First", None).unwrap();
        s.set_mouse_dpi("b:2:Second", None).unwrap();
        s.store(&path).unwrap();
        assert!(!std::fs::read_to_string(&path)
            .unwrap()
            .contains("mouse_dpi"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn show_scroll_bars_takes_its_three_names_and_refuses_others() {
        for b in ScrollBarsName::ALL {
            let (s, problems) = CompositorSettings::parse(
                &format!(r#"{{"show_scroll_bars":"{}"}}"#, b.name()),
                None,
            );
            assert!(problems.is_empty(), "{problems:?}");
            assert_eq!(s.show_scroll_bars, Some(b));
        }
        let (s, problems) = CompositorSettings::parse(r#"{"show_scroll_bars":"never"}"#, None);
        assert_eq!(s.show_scroll_bars, None);
        assert_eq!(problems.len(), 1);
        assert_eq!(ScrollBarsName::default(), ScrollBarsName::Automatic);
    }

    #[test]
    fn the_output_depth_is_8_or_10_bits() {
        for bits in [8, 10] {
            let (s, problems) =
                CompositorSettings::parse(&format!(r#"{{"output_depth":{bits}}}"#), None);
            assert!(problems.is_empty(), "{problems:?}");
            assert_eq!(s.output_depth, Some(bits));
        }
        for bad in ["12", "10.5", "\"10\""] {
            let (s, problems) =
                CompositorSettings::parse(&format!(r#"{{"output_depth":{bad}}}"#), None);
            assert_eq!(s.output_depth, None, "{bad}");
            assert_eq!(problems.len(), 1, "{bad}");
        }
        assert_eq!(CompositorSettings::default().output_depth, None);
    }

    #[test]
    fn keys_are_read_with_their_types() {
        let (s, problems) = CompositorSettings::parse(
            r##"{"appearance":"dark","corner_radius":10,"terminals":["kitty"],"keybindings":{"Super+t":"spawn-terminal"},
                "minimize_effect":"none","decorations":false,"closing_animation":false,"hot_corner_dwell":0.2,"mouse_speed":0.6875,
                "cursor_fill":"#ff0000","cursor_outline":"#00ff0080","shell":[{"name":"dock","command":["cupertino-dock","--bar"],"roles":["dock"]}],
                "screenshot":["/opt/cupertino/bin/cupertino-screenshot"],"future_key":{"kept":true}}"##,
            None,
        );
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(s.appearance, Some(AppearanceName::Dark));
        assert_eq!(
            (s.corner_radius, s.hot_corner_dwell, s.mouse_speed),
            (Some(10.0), Some(0.2), Some(0.6875))
        );
        assert_eq!(
            s.keybindings.unwrap().get("Super+t").map(String::as_str),
            Some("spawn-terminal")
        );
        assert_eq!(s.minimize_effect, Some(MinimizeEffectName::Plain));
        assert_eq!(
            (s.cursor_fill, s.cursor_outline),
            (Some([255, 0, 0, 255]), Some([0, 255, 0, 128]))
        );
        assert_eq!(
            s.shell,
            Some(vec![ShellEntry {
                name: "dock".into(),
                command: vec!["cupertino-dock".into(), "--bar".into()],
                roles: vec![ShellRoleName::Dock],
            }])
        );
        assert_eq!(
            CompositorSettings::parse("", None),
            (CompositorSettings::default(), vec![])
        );
    }

    #[test]
    fn each_bad_key_is_a_problem_and_the_others_apply() {
        let (s, problems) = CompositorSettings::parse(
            r##"{"cursor_fill":"red","minimize_effect":"nope","shell":[{"name":"dock","command":[]}],"screenshot":[],"natural_scrolling":false}"##,
            None,
        );
        let keys: Vec<&str> = problems.iter().map(|p| p.key.as_str()).collect();
        assert_eq!(
            keys,
            vec!["minimize_effect", "cursor_fill", "shell", "screenshot"]
        );
        assert_eq!(s.natural_scrolling, Some(false));
        for bad in [
            r##"{"cursor_fill":"#12345"}"##,
            r##"{"cursor_outline":"#gg0000"}"##,
            r#"{"mouse_speed":"fast"}"#,
        ] {
            assert_eq!(CompositorSettings::parse(bad, None).1.len(), 1, "{bad}");
        }
    }

    #[test]
    fn the_development_folder_is_an_absolute_path_and_a_list_of_distinct_names() {
        let (s, problems) = CompositorSettings::parse(
            r#"{"dev_folder":{"path":"/home/me/.local/lib/cupertino/dev","components":["dock","menubar"]}}"#,
            None,
        );
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(
            s.dev_folder,
            Some(DevFolder {
                path: "/home/me/.local/lib/cupertino/dev".into(),
                components: vec!["dock".into(), "menubar".into()],
            })
        );
        for bad in [
            r#"{"dev_folder":"/home/me/dev"}"#,
            r#"{"dev_folder":{"components":["dock"]}}"#,
            r#"{"dev_folder":{"path":"dev","components":["dock"]}}"#,
            r#"{"dev_folder":{"path":"/home/me/../root","components":["dock"]}}"#,
            r#"{"dev_folder":{"path":"/home/me/dev"}}"#,
            r#"{"dev_folder":{"path":"/home/me/dev","components":"dock"}}"#,
            r#"{"dev_folder":{"path":"/home/me/dev","components":[""]}}"#,
            r#"{"dev_folder":{"path":"/home/me/dev","components":["dock","dock"]}}"#,
        ] {
            let (s, problems) = CompositorSettings::parse(bad, None);
            assert_eq!(s.dev_folder, None, "{bad}");
            assert_eq!(problems.len(), 1, "{bad}");
            assert_eq!(problems[0].key, "dev_folder", "{bad}");
        }
    }

    #[test]
    fn a_broken_file_keeps_the_last_good_settings() {
        let good = CompositorSettings {
            appearance: Some(AppearanceName::Dark),
            mouse_speed: Some(1.5),
            ..CompositorSettings::default()
        };
        let (s, problems) = CompositorSettings::parse("{\"appearance\": ", Some(&good));
        assert_eq!(s, good);
        assert_eq!(problems.len(), 1);
        assert!(problems[0].key.is_empty());
        let (s, _) =
            CompositorSettings::parse(r#"{"appearance":"sepia","mouse_speed":2}"#, Some(&good));
        assert_eq!(
            (s.appearance, s.mouse_speed),
            (Some(AppearanceName::Dark), Some(2.0)),
            "only the bad key keeps its value"
        );
    }

    #[test]
    fn a_store_keeps_keybindings_shell_and_unknown_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("compositor.json");
        std::fs::write(&path, r#"{"keybindings":{"Super+t":"spawn-terminal"},"shell":[{"name":"dock","command":["cupertino-dock"]}],"future_key":[1,2],"mouse_speed":1}"#).unwrap();
        let (mut s, _) = CompositorSettings::load(&path, None).unwrap();
        s.mouse_speed = Some(2.5);
        s.appearance = Some(AppearanceName::Dark);
        s.cursor_outline = Some([0, 0, 0, 128]);
        assert!(s.store(&path).unwrap().is_empty());
        let written: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let expected: Value = serde_json::from_str(r##"{"appearance":"dark","cursor_outline":"#00000080","future_key":[1,2],"keybindings":{"Super+t":"spawn-terminal"},"mouse_speed":2.5,"shell":[{"command":["cupertino-dock"],"name":"dock"}]}"##).unwrap();
        assert_eq!(written, expected);
        assert_eq!(CompositorSettings::load(&path, None).unwrap().0, s);
    }

    #[test]
    fn a_store_refuses_to_overwrite_a_file_that_is_not_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("compositor.json");
        std::fs::write(&path, "{ half").unwrap();
        assert!(CompositorSettings {
            mouse_speed: Some(1.0),
            ..CompositorSettings::default()
        }
        .store(&path)
        .is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ half");
    }
}
