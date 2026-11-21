//! What the portal asks of the dialogs over `org.cupertino.Dialogs1` ([`crate::service`]), checked again here, and
//! how each dialog ends. The portal checked every value already; the component bounds them once more, so a value it
//! draws is never larger, nor holds a control or bidirectional-override character, whatever reaches the name.

use std::collections::HashMap;
use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};

use zbus::zvariant::OwnedValue;

/// The most dialogs that may be open at once, and of one app; more are refused at once.
pub const MAX_PENDING: usize = 16;
pub const MAX_PENDING_PER_APP: usize = 4;
/// Bytes of an id, a title, a label, a name and a parent window.
pub const MAX_TEXT: usize = 1024;
/// Bytes of a path.
pub const MAX_PATH: usize = 4096;
/// Bytes of a filter's pattern, and the most filters, patterns of one filter, choices, options of one choice and
/// files of a save.
pub const MAX_PATTERN: usize = 256;
pub const MAX_FILTERS: usize = 64;
pub const MAX_PATTERNS: usize = 64;
pub const MAX_CHOICES: usize = 16;
pub const MAX_OPTIONS: usize = 64;
pub const MAX_FILES: usize = 1024;
/// The most applications an app chooser lists, and the bytes of the file name or URI it names.
pub const MAX_APPS: usize = 256;
pub const MAX_TARGET: usize = 32 * 1024;

/// What a file chooser does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    /// Open files, or a folder.
    Open,
    /// Save one file under a name.
    Save,
    /// Save the named files into a folder.
    SaveFiles,
}

impl Kind {
    /// The kind `ChooseFile` names: `open`, `save` or `save-files`.
    pub fn from_name(name: &str) -> Option<Kind> {
        match name {
            "open" => Some(Kind::Open),
            "save" => Some(Kind::Save),
            "save-files" => Some(Kind::SaveFiles),
            _ => None,
        }
    }
}

/// One pattern of a filter: a shell glob or a MIME type.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Pattern {
    Glob(String),
    Mime(String),
}

/// A named set of patterns; a file one of them matches is shown under it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Filter {
    pub name: String,
    pub patterns: Vec<Pattern>,
}

/// A choice the app offers beside the files: a check box without options, else one of its options.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice {
    pub id: String,
    pub label: String,
    /// Each option's id and label.
    pub options: Vec<(String, String)>,
    pub initial: String,
}

/// A file chooser, as `ChooseFile` asks for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileAsk {
    pub kind: Kind,
    pub app_id: String,
    /// The app's window the dialog belongs to, as the portal names it (`wayland:HANDLE`), or empty.
    pub parent_window: String,
    pub title: String,
    pub accept_label: Option<String>,
    pub modal: bool,
    pub multiple: bool,
    pub directory: bool,
    pub filters: Vec<Filter>,
    /// The filter shown first, an index into `filters`.
    pub current_filter: Option<usize>,
    pub choices: Vec<Choice>,
    /// A save's suggested name.
    pub current_name: Option<String>,
    pub current_folder: Option<PathBuf>,
    pub current_file: Option<PathBuf>,
    /// The names of the files `SaveFiles` saves.
    pub files: Vec<PathBuf>,
}

/// How a file chooser ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileAnswer {
    /// The chosen paths (absolute), the filter shown, as an index into the ask's, and each choice's id and value.
    Chosen {
        paths: Vec<PathBuf>,
        filter: Option<usize>,
        choices: Vec<(String, String)>,
    },
    Cancelled,
}

/// An application an app chooser offers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppChoice {
    /// The desktop file's id, without `.desktop`.
    pub id: String,
    pub name: String,
    /// The icon its entry names, if any.
    pub icon: Option<String>,
}

/// An app chooser, as `ChooseApplication` asks for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppAsk {
    pub app_id: String,
    pub parent_window: String,
    /// The file name or URI to open, as the prompt names it; none when the app gave none.
    pub target: Option<String>,
    pub choices: Vec<AppChoice>,
    pub last_choice: Option<String>,
    /// The app wants an activation token for the chosen application.
    pub wants_token: bool,
}

/// How an app chooser ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AppAnswer {
    /// The chosen application's id, and the activation token made for it when one was asked for.
    Chosen {
        id: String,
        token: Option<String>,
    },
    Cancelled,
}

/// A filter as `ChooseFile` carries it: its name and `(0 glob | 1 MIME type, pattern)` pairs.
pub type WireFilter = (String, Vec<(u32, String)>);
/// A choice as `ChooseFile` carries it: id, label, `(id, label)` options, initial value.
pub type WireChoice = (String, String, Vec<(String, String)>, String);
/// An application as `ChooseApplication` carries it: id, name, icon (empty for none).
pub type WireApp = (String, String, String);

/// `value` without control or bidirectional-override characters, so app text cannot reorder or hide what the
/// dialog says around it.
pub fn plain(value: &str) -> String {
    value
        .chars()
        .filter(|c| {
            !c.is_control()
                && !matches!(
                    c,
                    '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'
                )
        })
        .collect()
}

fn bounded(what: &str, value: String, max: usize) -> Result<String, String> {
    if value.len() > max {
        return Err(format!("{what} is longer than {max} bytes"));
    }
    Ok(value)
}

fn text(what: &str, value: String) -> Result<String, String> {
    bounded(what, value, MAX_TEXT)
}

/// Text the dialog shows.
fn shown(what: &str, value: String) -> Result<String, String> {
    Ok(plain(&text(what, value)?))
}

fn path(what: &str, mut bytes: Vec<u8>) -> Result<PathBuf, String> {
    if bytes.last() == Some(&0) {
        bytes.pop();
    }
    if bytes.len() > MAX_PATH || bytes.contains(&0) {
        return Err(format!("{what} is not a path"));
    }
    Ok(PathBuf::from(OsString::from_vec(bytes)))
}

fn absolute(what: &str, bytes: Vec<u8>) -> Result<PathBuf, String> {
    let path = path(what, bytes)?;
    if !path.is_absolute() {
        return Err(format!("{what} is not absolute"));
    }
    Ok(path)
}

fn option<T: TryFrom<OwnedValue>>(
    options: &HashMap<String, OwnedValue>,
    key: &str,
) -> Result<Option<T>, String> {
    options
        .get(key)
        .map(|value| {
            value
                .try_clone()
                .ok()
                .and_then(|value| T::try_from(value).ok())
                .ok_or_else(|| format!("option {key} has the wrong type"))
        })
        .transpose()
}

fn filter((name, patterns): WireFilter) -> Result<Filter, String> {
    if patterns.len() > MAX_PATTERNS {
        return Err(format!("a filter has more than {MAX_PATTERNS} patterns"));
    }
    Ok(Filter {
        name: shown("a filter name", name)?,
        patterns: patterns
            .into_iter()
            .map(|(kind, pattern)| {
                let pattern = bounded("a filter pattern", pattern, MAX_PATTERN)?;
                match kind {
                    0 => Ok(Pattern::Glob(pattern)),
                    1 => Ok(Pattern::Mime(pattern)),
                    _ => Err(format!("filter pattern kind {kind} is unknown")),
                }
            })
            .collect::<Result<_, String>>()?,
    })
}

fn choice((id, label, options, initial): WireChoice) -> Result<Choice, String> {
    if options.len() > MAX_OPTIONS {
        return Err(format!("a choice has more than {MAX_OPTIONS} options"));
    }
    Ok(Choice {
        id: text("a choice id", id)?,
        label: shown("a choice label", label)?,
        options: options
            .into_iter()
            .map(|(id, label)| Ok((text("an option id", id)?, shown("an option label", label)?)))
            .collect::<Result<_, String>>()?,
        initial: text("a choice's initial value", initial)?,
    })
}

impl FileAsk {
    /// The file chooser `ChooseFile` asks for, every value bounded: `kind` is `open`, `save` or `save-files`, and
    /// `options` the checked options the portal passes on (see the README).
    pub fn from_wire(
        kind: &str,
        app_id: String,
        parent_window: String,
        title: String,
        options: &HashMap<String, OwnedValue>,
    ) -> Result<FileAsk, String> {
        let kind = Kind::from_name(kind).ok_or_else(|| format!("unknown kind {kind:?}"))?;
        let filters: Vec<WireFilter> = option(options, "filters")?.unwrap_or_default();
        if filters.len() > MAX_FILTERS {
            return Err(format!("more than {MAX_FILTERS} filters"));
        }
        let choices: Vec<WireChoice> = option(options, "choices")?.unwrap_or_default();
        if choices.len() > MAX_CHOICES {
            return Err(format!("more than {MAX_CHOICES} choices"));
        }
        let files: Vec<Vec<u8>> = option(options, "files")?.unwrap_or_default();
        if files.len() > MAX_FILES {
            return Err(format!("more than {MAX_FILES} files"));
        }
        let current_name = option::<String>(options, "current_name")?
            .map(|name| shown("current_name", name))
            .transpose()?;
        if current_name
            .as_deref()
            .is_some_and(|name| name.contains('/'))
        {
            return Err("current_name is not a file name".into());
        }
        let files = files
            .into_iter()
            .map(|bytes| {
                let name = path("a file name", bytes)?;
                match name.file_name() {
                    Some(file) if Path::new(file) == name => Ok(name),
                    _ => Err("a file of files is not a file name".to_owned()),
                }
            })
            .collect::<Result<_, String>>()?;
        let filters: Vec<Filter> = filters.into_iter().map(filter).collect::<Result<_, _>>()?;
        let current_filter = option::<i32>(options, "current_filter")?
            .and_then(|index| usize::try_from(index).ok())
            .filter(|&index| index < filters.len());
        Ok(FileAsk {
            kind,
            app_id: text("app_id", app_id)?,
            parent_window: text("parent_window", parent_window)?,
            title: shown("title", title)?,
            accept_label: option::<String>(options, "accept_label")?
                .map(|label| shown("accept_label", label))
                .transpose()?,
            modal: option(options, "modal")?.unwrap_or(true),
            multiple: option(options, "multiple")?.unwrap_or(false) && kind == Kind::Open,
            directory: option(options, "directory")?.unwrap_or(false) && kind == Kind::Open,
            filters,
            current_filter,
            choices: choices.into_iter().map(choice).collect::<Result<_, _>>()?,
            current_name,
            current_folder: option::<Vec<u8>>(options, "current_folder")?
                .map(|bytes| absolute("current_folder", bytes))
                .transpose()?,
            current_file: option::<Vec<u8>>(options, "current_file")?
                .map(|bytes| absolute("current_file", bytes))
                .transpose()?,
            files,
        })
    }
}

/// The applications `ChooseApplication` and `UpdateChoices` offer, bounded; an icon name with a `/` or a control
/// character names none.
pub fn app_choices(choices: Vec<WireApp>) -> Result<Vec<AppChoice>, String> {
    if choices.len() > MAX_APPS {
        return Err(format!("more than {MAX_APPS} applications"));
    }
    choices
        .into_iter()
        .map(|(id, name, icon)| {
            let icon = text("an icon", icon)?;
            let named =
                !icon.is_empty() && !icon.contains('/') && !icon.chars().any(char::is_control);
            Ok(AppChoice {
                id: text("an application id", id)?,
                name: shown("an application name", name)?,
                icon: named.then_some(icon),
            })
        })
        .collect()
}

impl AppAsk {
    /// The app chooser `ChooseApplication` asks for, every value bounded; an empty `target` or `last_choice` names
    /// none.
    pub fn from_wire(
        app_id: String,
        parent_window: String,
        target: String,
        choices: Vec<WireApp>,
        last_choice: String,
        wants_token: bool,
    ) -> Result<AppAsk, String> {
        let target = plain(&bounded("target", target, MAX_TARGET)?);
        let last_choice = text("last_choice", last_choice)?;
        Ok(AppAsk {
            app_id: text("app_id", app_id)?,
            parent_window: text("parent_window", parent_window)?,
            target: (!target.is_empty()).then_some(target),
            choices: app_choices(choices)?,
            last_choice: (!last_choice.is_empty()).then_some(last_choice),
            wants_token,
        })
    }
}
