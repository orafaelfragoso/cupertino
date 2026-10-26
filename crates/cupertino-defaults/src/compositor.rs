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
//! | `show_menu_bar`, `decorations`, `closing_animation`, `mouse_acceleration`, `natural_scrolling`, `shake_to_locate` | boolean |
//! | `minimize_effect` | `warp`, `funnel`, `scale`, `plain` (also `none`) |
//! | `mouse_speed`, `trackpad_speed` | tracking speed, 0 to 3 |
//! | `cursor_size` | cursor scale |
//! | `cursor_fill`, `cursor_outline` | `#rrggbb` or `#rrggbbaa` |
//! | `shell` | list of `{"name": …, "command": [program, args…]}`, commands not empty |
//! | `screenshot` | the screenshot command, not empty |

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

/// A shell process the server starts and keeps alive.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShellEntry {
    /// Name in log lines.
    pub name: String,
    /// Program and arguments; never empty.
    pub command: Vec<String>,
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
    pub decorations: Option<bool>,
    pub minimize_effect: Option<MinimizeEffectName>,
    pub closing_animation: Option<bool>,
    pub hot_corner_size: Option<f64>,
    pub hot_corner_dwell: Option<f64>,
    pub mouse_speed: Option<f64>,
    pub trackpad_speed: Option<f64>,
    pub mouse_acceleration: Option<bool>,
    pub natural_scrolling: Option<bool>,
    pub cursor_size: Option<f64>,
    /// sRGB with alpha.
    pub cursor_fill: Option<[u8; 4]>,
    pub cursor_outline: Option<[u8; 4]>,
    pub shake_to_locate: Option<bool>,
    pub shell: Option<Vec<ShellEntry>>,
    pub screenshot: Option<Vec<String>>,
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
        out.push(ShellEntry { name, command });
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
    json_key!(closing_animation, boolean, bool_value),
    json_key!(hot_corner_size, number, number_value),
    json_key!(hot_corner_dwell, number, number_value),
    json_key!(mouse_speed, number, number_value),
    json_key!(trackpad_speed, number, number_value),
    json_key!(mouse_acceleration, boolean, bool_value),
    json_key!(natural_scrolling, boolean, bool_value),
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
    json_key!(shell, shell_entries, |list: &Vec<ShellEntry>| Value::Array(
        list.iter()
            .map(|e| {
                let mut m = Map::new();
                m.insert("name".into(), Value::String(e.name.clone()));
                m.insert("command".into(), strings_value(&e.command));
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
    fn keys_are_read_with_their_types() {
        let (s, problems) = CompositorSettings::parse(
            r##"{"appearance":"dark","corner_radius":10,"terminals":["kitty"],"keybindings":{"Super+t":"spawn-terminal"},
                "minimize_effect":"none","decorations":false,"closing_animation":false,"hot_corner_dwell":0.2,"mouse_speed":0.6875,
                "cursor_fill":"#ff0000","cursor_outline":"#00ff0080","shell":[{"name":"dock","command":["cupertino-dock","--bar"]}],
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
                command: vec!["cupertino-dock".into(), "--bar".into()]
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
