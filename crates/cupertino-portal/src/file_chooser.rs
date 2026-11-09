//! `org.freedesktop.impl.portal.FileChooser`: `OpenFile`, `SaveFile` and `SaveFiles`. A call becomes an [`Ask`] for the
//! dialog, which answers through its [`Reply`]; the call returns when it does, or when the frontend closes the request.

use std::collections::HashMap;
use std::ffi::OsString;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use percent_encoding::{utf8_percent_encode, AsciiSet, CONTROLS};
use zbus::message::Header;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};
use zbus::{interface, Connection};

use crate::request::{answered, from_frontend, Exported, PortalError, Response};

/// At most this many dialogs wait at once; another call is refused.
pub const MAX_PENDING: usize = 16;
/// At most this many of them for one app id, so one app cannot use up the others' dialogs.
pub const MAX_PENDING_PER_APP: usize = 4;
/// Bytes of a title, a label, a name or a pattern.
pub const MAX_TEXT: usize = 1024;
/// Bytes of a path.
pub const MAX_PATH: usize = 4096;
/// Bytes of one filter pattern: matching costs pattern × name steps for each file, on the dialog thread.
pub const MAX_PATTERN: usize = 256;
/// Filters, patterns of one filter, choices, options of one choice and files of `SaveFiles`.
pub const MAX_FILTERS: usize = 64;
pub const MAX_PATTERNS: usize = 64;
pub const MAX_CHOICES: usize = 16;
pub const MAX_OPTIONS: usize = 64;
pub const MAX_FILES: usize = 1024;

/// Which call asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Open,
    Save,
    SaveFiles,
}

/// One pattern of a filter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pattern {
    /// A shell glob such as `*.png`.
    Glob(String),
    /// A MIME type such as `image/png`.
    Mime(String),
}

/// A named set of patterns, shown in the filter pop-up.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Filter {
    pub name: String,
    pub patterns: Vec<Pattern>,
}

/// A pop-up of options, or a check box when there are no options (its value `"true"` or `"false"`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice {
    pub id: String,
    pub label: String,
    /// `(id, label)` pairs.
    pub options: Vec<(String, String)>,
    pub initial: String,
}

/// What the dialog shows. Every string is bounded and comes from the application, so it is shown as text only.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ask {
    pub kind: Kind,
    pub app_id: String,
    /// `wayland:<handle>`, `x11:<xid>` or empty.
    pub parent_window: String,
    pub title: String,
    pub accept_label: Option<String>,
    pub modal: bool,
    pub multiple: bool,
    pub directory: bool,
    pub filters: Vec<Filter>,
    pub current_filter: Option<Filter>,
    pub choices: Vec<Choice>,
    /// The suggested name of a saved file, without a directory.
    pub current_name: Option<String>,
    pub current_folder: Option<PathBuf>,
    /// The file a save would replace.
    pub current_file: Option<PathBuf>,
    /// The names `SaveFiles` saves into the chosen folder.
    pub files: Vec<PathBuf>,
}

/// How the dialog ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Answer {
    /// The chosen files (absolute), the filter shown and the value of each choice.
    Chosen {
        paths: Vec<PathBuf>,
        filter: Option<Filter>,
        choices: Vec<(String, String)>,
    },
    Cancelled,
}

/// The dialog's answer to one [`Ask`].
pub struct Reply(pub(crate) async_channel::Sender<Answer>);

impl Reply {
    pub fn send(self, answer: Answer) {
        let _ = self.0.try_send(answer);
    }
}

/// What the dialog side is told.
pub enum Event {
    /// Show a dialog for the call at `handle`.
    Ask {
        handle: OwnedObjectPath,
        ask: Box<Ask>,
        reply: Reply,
    },
    /// The frontend closed the call at `handle`: take its dialog down without answering.
    Close { handle: OwnedObjectPath },
}

/// The object on the bus.
pub struct FileChooser {
    events: Box<dyn Fn(Event) + Send + Sync>,
    pending: Arc<Mutex<HashMap<String, usize>>>,
}

impl FileChooser {
    pub fn new(events: impl Fn(Event) + Send + Sync + 'static) -> Self {
        Self {
            events: Box::new(events),
            pending: Arc::default(),
        }
    }

    async fn ask(
        &self,
        connection: &Connection,
        handle: OwnedObjectPath,
        ask: Ask,
    ) -> Result<(u32, HashMap<String, OwnedValue>), PortalError> {
        let _pending = Pending::take(&self.pending, &ask.app_id).ok_or_else(|| {
            PortalError::LimitsExceeded("too many file choosers are open".to_owned())
        })?;
        let request = Exported::at(connection, handle.as_ref()).await?;
        let (reply, answer) = async_channel::bounded(1);
        let kind = ask.kind;
        (self.events)(Event::Ask {
            handle: handle.clone(),
            ask: Box::new(ask),
            reply: Reply(reply),
        });
        let answer = futures_lite::future::or(async { Some(answer.recv().await.ok()) }, async {
            request.closed().await;
            None
        })
        .await;
        request.remove().await;
        Ok(match answer {
            Some(Some(Answer::Chosen {
                paths,
                filter,
                choices,
            })) => (
                Response::Success as u32,
                results(kind, &paths, filter, choices)?,
            ),
            Some(Some(Answer::Cancelled)) => (Response::Cancelled as u32, HashMap::new()),
            Some(None) => (Response::Other as u32, HashMap::new()),
            None => {
                (self.events)(Event::Close { handle });
                (Response::Other as u32, HashMap::new())
            }
        })
    }
}

/// Counts one open dialog of an app id for as long as it lives.
struct Pending {
    counts: Arc<Mutex<HashMap<String, usize>>>,
    app_id: String,
}

impl Pending {
    fn take(counts: &Arc<Mutex<HashMap<String, usize>>>, app_id: &str) -> Option<Pending> {
        let mut open = counts.lock().unwrap_or_else(|e| e.into_inner());
        let mine = open.get(app_id).copied().unwrap_or(0);
        if open.values().sum::<usize>() >= MAX_PENDING || mine >= MAX_PENDING_PER_APP {
            return None;
        }
        open.insert(app_id.to_owned(), mine + 1);
        Some(Pending {
            counts: Arc::clone(counts),
            app_id: app_id.to_owned(),
        })
    }
}

impl Drop for Pending {
    fn drop(&mut self) {
        let mut open = self.counts.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(n) = open.get_mut(&self.app_id) {
            *n -= 1;
            if *n == 0 {
                open.remove(&self.app_id);
            }
        }
    }
}

type Options = HashMap<String, OwnedValue>;

#[interface(name = "org.freedesktop.impl.portal.FileChooser")]
impl FileChooser {
    #[allow(clippy::too_many_arguments)]
    async fn open_file(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
        handle: OwnedObjectPath,
        app_id: String,
        parent_window: String,
        title: String,
        options: Options,
    ) -> Result<(u32, HashMap<String, OwnedValue>), PortalError> {
        let app = app_id.clone();
        let result = async {
            from_frontend(&header, connection).await?;
            let ask = parse(Kind::Open, app_id, parent_window, title, &options)?;
            self.ask(connection, handle, ask).await
        }
        .await;
        answered(
            "org.freedesktop.impl.portal.FileChooser",
            "OpenFile",
            Some(&app),
            result,
        )
    }

    #[allow(clippy::too_many_arguments)]
    async fn save_file(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
        handle: OwnedObjectPath,
        app_id: String,
        parent_window: String,
        title: String,
        options: Options,
    ) -> Result<(u32, HashMap<String, OwnedValue>), PortalError> {
        let app = app_id.clone();
        let result = async {
            from_frontend(&header, connection).await?;
            let ask = parse(Kind::Save, app_id, parent_window, title, &options)?;
            self.ask(connection, handle, ask).await
        }
        .await;
        answered(
            "org.freedesktop.impl.portal.FileChooser",
            "SaveFile",
            Some(&app),
            result,
        )
    }

    #[allow(clippy::too_many_arguments)]
    async fn save_files(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
        handle: OwnedObjectPath,
        app_id: String,
        parent_window: String,
        title: String,
        options: Options,
    ) -> Result<(u32, HashMap<String, OwnedValue>), PortalError> {
        let app = app_id.clone();
        let result = async {
            from_frontend(&header, connection).await?;
            let ask = parse(Kind::SaveFiles, app_id, parent_window, title, &options)?;
            self.ask(connection, handle, ask).await
        }
        .await;
        answered(
            "org.freedesktop.impl.portal.FileChooser",
            "SaveFiles",
            Some(&app),
            result,
        )
    }
}

fn invalid(why: impl Into<String>) -> PortalError {
    PortalError::InvalidArgs(why.into())
}

fn text(what: &str, value: String) -> Result<String, PortalError> {
    if value.len() > MAX_TEXT {
        return Err(invalid(format!("{what} is longer than {MAX_TEXT} bytes")));
    }
    Ok(value)
}

/// App text the dialog shows, without control or bidirectional-override characters, so it cannot reorder or hide
/// what the trusted window says around it.
fn display_text(what: &str, value: String) -> Result<String, PortalError> {
    Ok(plain(&text(what, value)?))
}

/// `value` without control or bidirectional-override characters.
pub(crate) fn plain(value: &str) -> String {
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

fn option<T>(options: &Options, key: &str) -> Result<Option<T>, PortalError>
where
    T: TryFrom<OwnedValue>,
{
    options
        .get(key)
        .map(|value| {
            value
                .try_clone()
                .ok()
                .and_then(|value| T::try_from(value).ok())
                .ok_or_else(|| invalid(format!("option {key} has the wrong type")))
        })
        .transpose()
}

/// A path the frontend sends as a byte string, with or without its terminating NUL.
fn path(what: &str, mut bytes: Vec<u8>) -> Result<PathBuf, PortalError> {
    if bytes.last() == Some(&0) {
        bytes.pop();
    }
    if bytes.len() > MAX_PATH || bytes.contains(&0) {
        return Err(invalid(format!("{what} is not a path")));
    }
    Ok(PathBuf::from(OsString::from_vec(bytes)))
}

fn absolute(what: &str, bytes: Vec<u8>) -> Result<PathBuf, PortalError> {
    let path = path(what, bytes)?;
    if !path.is_absolute() {
        return Err(invalid(format!("{what} is not absolute")));
    }
    Ok(path)
}

type WireFilter = (String, Vec<(u32, String)>);
type WireChoice = (String, String, Vec<(String, String)>, String);

fn filter((name, patterns): WireFilter) -> Result<Filter, PortalError> {
    if patterns.len() > MAX_PATTERNS {
        return Err(invalid(format!(
            "a filter has more than {MAX_PATTERNS} patterns"
        )));
    }
    Ok(Filter {
        name: display_text("a filter name", name)?,
        patterns: patterns
            .into_iter()
            .map(|(kind, pattern)| {
                if pattern.len() > MAX_PATTERN {
                    return Err(invalid(format!(
                        "a filter pattern is longer than {MAX_PATTERN} bytes"
                    )));
                }
                match kind {
                    0 => Ok(Pattern::Glob(pattern)),
                    1 => Ok(Pattern::Mime(pattern)),
                    _ => Err(invalid(format!("filter pattern kind {kind} is unknown"))),
                }
            })
            .collect::<Result<_, _>>()?,
    })
}

fn choice((id, label, options, initial): WireChoice) -> Result<Choice, PortalError> {
    if options.len() > MAX_OPTIONS {
        return Err(invalid(format!(
            "a choice has more than {MAX_OPTIONS} options"
        )));
    }
    Ok(Choice {
        id: text("a choice id", id)?,
        label: display_text("a choice label", label)?,
        options: options
            .into_iter()
            .map(|(id, label)| {
                Ok((
                    text("an option id", id)?,
                    display_text("an option label", label)?,
                ))
            })
            .collect::<Result<_, PortalError>>()?,
        initial: text("a choice's initial value", initial)?,
    })
}

/// The dialog a call asks for, with every option checked and bounded.
pub fn parse(
    kind: Kind,
    app_id: String,
    parent_window: String,
    title: String,
    options: &Options,
) -> Result<Ask, PortalError> {
    let filters: Vec<WireFilter> = option(options, "filters")?.unwrap_or_default();
    if filters.len() > MAX_FILTERS {
        return Err(invalid(format!("more than {MAX_FILTERS} filters")));
    }
    let choices: Vec<WireChoice> = option(options, "choices")?.unwrap_or_default();
    if choices.len() > MAX_CHOICES {
        return Err(invalid(format!("more than {MAX_CHOICES} choices")));
    }
    let files: Vec<Vec<u8>> = option(options, "files")?.unwrap_or_default();
    if files.len() > MAX_FILES {
        return Err(invalid(format!("more than {MAX_FILES} files")));
    }
    let current_name = option::<String>(options, "current_name")?
        .map(|name| text("current_name", name))
        .transpose()?;
    if current_name
        .as_deref()
        .is_some_and(|name| name.contains('/') || name.contains('\0'))
    {
        return Err(invalid("current_name is not a file name"));
    }
    let files = files
        .into_iter()
        .map(|bytes| {
            let name = path("a file name", bytes)?;
            match name.file_name() {
                Some(file) if Path::new(file) == name => Ok(name),
                _ => Err(invalid("a file of files is not a file name")),
            }
        })
        .collect::<Result<_, _>>()?;
    Ok(Ask {
        kind,
        app_id: text("app_id", app_id)?,
        parent_window: text("parent_window", parent_window)?,
        title: display_text("title", title)?,
        accept_label: option::<String>(options, "accept_label")?
            .map(|label| display_text("accept_label", label))
            .transpose()?,
        modal: option(options, "modal")?.unwrap_or(true),
        multiple: option(options, "multiple")?.unwrap_or(false) && kind == Kind::Open,
        directory: option(options, "directory")?.unwrap_or(false) && kind == Kind::Open,
        filters: filters.into_iter().map(filter).collect::<Result<_, _>>()?,
        current_filter: option::<WireFilter>(options, "current_filter")?
            .map(filter)
            .transpose()?,
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

/// Everything but the unreserved characters and `/` is percent-encoded in a path of a URI.
const PATH: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'%')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'`')
    .add(b'{')
    .add(b'}')
    .add(b'[')
    .add(b']')
    .add(b'\\')
    .add(b'^')
    .add(b'|')
    .add(b'!')
    .add(b'$')
    .add(b'&')
    .add(b'\'')
    .add(b'(')
    .add(b')')
    .add(b'*')
    .add(b'+')
    .add(b',')
    .add(b':')
    .add(b';')
    .add(b'=')
    .add(b'@');

/// The `file://` URI of an absolute path; bytes that are not UTF-8 are percent-encoded too.
pub fn file_uri(path: &Path) -> Option<String> {
    if !path.is_absolute() {
        return None;
    }
    let mut uri = String::from("file://");
    for chunk in path.as_os_str().as_bytes().utf8_chunks() {
        uri.extend(utf8_percent_encode(chunk.valid(), PATH));
        for byte in chunk.invalid() {
            uri.push_str(&format!("%{byte:02X}"));
        }
    }
    Some(uri)
}

fn owned(value: Value<'_>) -> Result<OwnedValue, PortalError> {
    OwnedValue::try_from(value).map_err(|e| PortalError::ZBus(e.into()))
}

fn wire(filter: Filter) -> WireFilter {
    let patterns = filter
        .patterns
        .into_iter()
        .map(|pattern| match pattern {
            Pattern::Glob(glob) => (0, glob),
            Pattern::Mime(mime) => (1, mime),
        })
        .collect();
    (filter.name, patterns)
}

/// The `results` of a chosen answer. A `Save` answers one file; paths that are not absolute are dropped, as the
/// interface asks of URIs that cannot be normalised.
fn results(
    kind: Kind,
    paths: &[PathBuf],
    filter: Option<Filter>,
    choices: Vec<(String, String)>,
) -> Result<HashMap<String, OwnedValue>, PortalError> {
    let take = if kind == Kind::Save { 1 } else { usize::MAX };
    let uris: Vec<String> = paths
        .iter()
        .filter_map(|p| file_uri(p))
        .take(take)
        .collect();
    let mut results = HashMap::from([("uris".to_owned(), owned(Value::from(uris))?)]);
    if !choices.is_empty() {
        results.insert("choices".to_owned(), owned(Value::from(choices))?);
    }
    if let Some(filter) = filter {
        results.insert(
            "current_filter".to_owned(),
            owned(Value::from(wire(filter)))?,
        );
    }
    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_app_holds_at_most_its_share_of_the_open_dialogs() {
        let counts = Arc::default();
        let editor: Vec<Pending> = (0..MAX_PENDING_PER_APP)
            .map(|_| Pending::take(&counts, "org.example.Editor").unwrap())
            .collect();
        assert!(Pending::take(&counts, "org.example.Editor").is_none());
        let others: Vec<Pending> = (0..MAX_PENDING - MAX_PENDING_PER_APP)
            .map(|i| {
                Pending::take(
                    &counts,
                    &format!("org.example.App{}", i / MAX_PENDING_PER_APP),
                )
                .unwrap()
            })
            .collect();
        assert!(
            Pending::take(&counts, "org.example.Viewer").is_none(),
            "the process-wide cap holds"
        );
        drop(editor);
        assert!(Pending::take(&counts, "org.example.Viewer").is_some());
        drop(others);
        assert!(
            counts.lock().unwrap().is_empty(),
            "every count is given back"
        );
    }

    #[test]
    fn shown_app_text_loses_control_and_direction_override_characters() {
        assert_eq!(
            display_text(
                "title",
                "Open\u{202e}txt.exe\u{2066}\n\u{7}Report".to_owned()
            )
            .unwrap(),
            "Opentxt.exeReport"
        );
        assert!(display_text("title", "x".repeat(MAX_TEXT + 1)).is_err());
    }

    #[test]
    fn a_path_becomes_a_file_uri_with_its_reserved_and_raw_bytes_escaped() {
        assert_eq!(
            file_uri(Path::new("/home/me/My Notes #1.txt")).unwrap(),
            "file:///home/me/My%20Notes%20%231.txt"
        );
        assert_eq!(
            file_uri(Path::new("/tmp/caf\u{e9}")).unwrap(),
            "file:///tmp/caf%C3%A9"
        );
        let raw = PathBuf::from(OsString::from_vec(b"/tmp/\xff".to_vec()));
        assert_eq!(file_uri(&raw).unwrap(), "file:///tmp/%FF");
        assert_eq!(file_uri(Path::new("relative")), None);
    }
}
