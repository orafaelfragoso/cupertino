//! One Files process per session. The process that starts first claims [`BUS_NAME`] on the session bus and serves
//! `org.freedesktop.Application` at [`OBJECT_PATH`] ([`Service::serve`]); every later launch hands its request to that
//! process and exits ([`forward`]), so file operations, their progress window and the pasteboard stay in one process.
//!
//! * `Activate` brings the most recently opened browser window forward, or opens one on the home folder when none is
//!   open.
//! * `Open` opens a window on each folder URI: `file://` (local) and `trash:///`. Anything else refuses the whole call.
//! * `ActivateAction` takes `show-progress` (the progress window, when it lists something) and `new-window`.
//!
//! The same process serves `org.freedesktop.FileManager1` at [`FILE_MANAGER_PATH`], the name other programs' "Show in
//! folder" calls, when no other file manager holds it (it waits in line for the name otherwise):
//!
//! * `ShowItems` opens a window on each item's folder with the items selected, the keyboard on the first, which the
//!   window scrolls into sight; items of one folder share a window, and a window already showing the folder is used.
//! * `ShowFolders` opens a window on each folder, as `Open` does.
//! * `ShowItemProperties` selects the items as `ShowItems` does: Files has no information window.
//!
//! Each takes `file://` URIs (an absolute path is taken too) and the caller's startup id, an activation token.
//!
//! An `activation-token` (or else `desktop-startup-id`) in the platform data raises the window the call opens or
//! brings forward. Only an unsandboxed process of the same user may call: the caller's process is named by the bus
//! daemon's credentials and refused when it runs in a Flatpak or a Snap.
//!
//! The service's connection is the program's session connection, which its UI loop ticks (`cupertino_kit::bus`). It
//! reads no signals: the name is claimed with a plain `RequestName` call, so the one bus call made for each incoming
//! call (the caller's credentials) never waits behind a full signal queue. Nothing waits on the bus: a call reaches the
//! window through a channel, and the call's handler waits for the answer as a future of the connection. A call that
//! comes while Files quits fails with `org.freedesktop.DBus.Error.Disconnected`, and the launch claims the name again.

use std::collections::HashMap;
use std::ffi::OsString;
use std::io;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

use percent_encoding::{percent_decode_str, percent_encode, AsciiSet, NON_ALPHANUMERIC};
use zbus::fdo::ConnectionCredentials;
use zbus::names::{OwnedUniqueName, UniqueName};
use zbus::zvariant::{OwnedValue, Value};
use zbus::Connection;

use crate::trash::Trash;

/// The name Files owns on the session bus. A desktop file ID must equal it for the desktop to activate Files over
/// the bus (`DBusActivatable`); the desktop file is still `cupertino-files.desktop`, so launchers start the program,
/// which hands over to the running one.
pub const BUS_NAME: &str = "org.cupertino.Files";
/// Where the application object is: the bus name with its dots as slashes.
pub const OBJECT_PATH: &str = "/org/cupertino/Files";
pub const INTERFACE: &str = "org.freedesktop.Application";
/// The name programs ask to show files in their folder, which Files claims beside [`BUS_NAME`].
pub const FILE_MANAGER_NAME: &str = "org.freedesktop.FileManager1";
/// Where the file manager object is, and its interface.
pub const FILE_MANAGER_PATH: &str = "/org/freedesktop/FileManager1";
pub const FILE_MANAGER_INTERFACE: &str = "org.freedesktop.FileManager1";

/// How long a launch waits for the running Files to answer. A Files that has not answered by then and still owns the
/// name is taken to hang: the launch fails rather than starting a second process.
pub const FORWARD_TIMEOUT: Duration = Duration::from_secs(3);

/// The longest activation token taken.
const MAX_TOKEN: usize = 4096;
/// The most URIs one `Open` takes, and the longest URI.
const MAX_URIS: usize = 64;
const MAX_URI: usize = 16 * 1024;
/// The most calls waiting for the window's thread; a call past them is refused.
const MAX_WAITING: usize = 16;

/// The `ActivateAction` names Files takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Show the progress window, when it lists something.
    ShowProgress,
    /// A new browser window on the home folder.
    NewWindow,
}

impl Action {
    pub fn name(self) -> &'static str {
        match self {
            Action::ShowProgress => "show-progress",
            Action::NewWindow => "new-window",
        }
    }

    pub fn from_name(name: &str) -> Option<Action> {
        [Action::ShowProgress, Action::NewWindow]
            .into_iter()
            .find(|a| a.name() == name)
    }
}

/// A folder a launch or a call names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// A local folder, by its absolute path (a path from the command line may be relative until [`Target::absolute`]).
    Folder(PathBuf),
    /// The session's trash.
    Trash,
}

/// Every byte but the unreserved ones and the slash is escaped in a `file://` URI.
const PATH_ESCAPED: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'/')
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

impl Target {
    /// What an argument names: a path, a local `file://` URI (the desktop entry's `%U`) or the trash (`trash:///`, the
    /// scheme the session's file manager handles).
    pub fn of_argument(arg: &str) -> Result<Target, String> {
        if arg.starts_with("trash:") || arg.starts_with("file://") {
            return Target::of_uri(arg);
        }
        Ok(Target::Folder(PathBuf::from(arg)))
    }

    /// What a URI names: `file://` with an absolute path on this host, or `trash:///`. Every other URI is refused.
    pub fn of_uri(uri: &str) -> Result<Target, String> {
        if uri.len() > MAX_URI {
            return Err("the URI is too long".into());
        }
        if let Some(rest) = uri.strip_prefix("trash:") {
            return match rest.trim_start_matches('/') {
                "" => Ok(Target::Trash),
                _ => Err(format!(
                    "{uri}: only the trash itself (trash:///) can be opened"
                )),
            };
        }
        let Some(rest) = uri.strip_prefix("file://") else {
            return Err(format!("{uri}: not a file or trash URI"));
        };
        let path = rest.strip_prefix("localhost").unwrap_or(rest);
        // the query and fragment are not part of the path (RFC 8089, RFC 3986 §3)
        let path = path.split(['?', '#']).next().unwrap_or(path);
        if !path.starts_with('/') {
            return Err(format!("{uri}: not a local file URI"));
        }
        let bytes: Vec<u8> = percent_decode_str(path).collect();
        if bytes.contains(&0) {
            return Err(format!("{uri}: the path holds a NUL byte"));
        }
        Ok(Target::Folder(PathBuf::from(OsString::from_vec(bytes))))
    }

    /// The URI that names the target: `trash:///`, or `file://` and the path with its reserved bytes escaped.
    pub fn uri(&self) -> String {
        match self {
            Target::Trash => "trash:///".into(),
            Target::Folder(path) => format!(
                "file://{}",
                percent_encode(path.as_os_str().as_bytes(), PATH_ESCAPED)
            ),
        }
    }

    /// The target with a relative path made absolute against the working directory.
    pub fn absolute(self) -> io::Result<Target> {
        match self {
            Target::Folder(path) => std::path::absolute(&path).map(Target::Folder),
            Target::Trash => Ok(Target::Trash),
        }
    }

    /// The folder a window shows for the target. The trash's folder is `files` of the home trash, made with `info`
    /// when nothing was trashed yet (the freedesktop Trash specification lets the implementation create it): two
    /// folders in the home's data folder, made by a launch before its loop starts and by a call that names the trash.
    #[expect(
        clippy::disallowed_methods,
        reason = "two mkdir calls in the home's data folder, before the loop or for a call that names the trash"
    )]
    pub fn folder(&self, trash: &Trash) -> io::Result<PathBuf> {
        match self {
            Target::Folder(path) => Ok(path.clone()),
            Target::Trash => {
                for sub in ["files", "info"] {
                    let dir = trash.home_trash().join(sub);
                    std::fs::create_dir_all(&dir).map_err(|e| {
                        io::Error::new(e.kind(), format!("cannot create {}: {e}", dir.display()))
                    })?;
                }
                Ok(trash.home_trash().join("files"))
            }
        }
    }
}

/// What a launch or a caller asks the running Files for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    Activate,
    Open(Vec<Target>),
    Action(Action),
    /// Each item selected in a window on its folder; the trash is opened itself.
    ShowItems(Vec<Target>),
}

/// A call that reached the window's thread: what it asks, the activation token it brought, and the answer the bus
/// thread waits for ([`Call::done`]). A call dropped unanswered tells the caller that Files is quitting.
#[derive(Debug)]
pub struct Call {
    pub request: Request,
    pub token: Option<String>,
    reply: async_channel::Sender<Result<(), String>>,
}

impl Call {
    /// Answer the caller: the request was taken.
    pub fn done(self) {
        let _ = self.reply.try_send(Ok(()));
    }

    /// Answer the caller that the request was not taken, because too much is already under way.
    pub fn refuse(self, why: &str) {
        let _ = self.reply.try_send(Err(why.to_owned()));
    }
}

/// The served name: the calls waiting for the window's thread and what wakes it for them. Dropping it refuses every
/// waiting and later call; [`Service::release`] gives the name up too.
pub struct Service {
    connection: Connection,
    calls: async_channel::Receiver<Call>,
    woken: async_channel::Receiver<()>,
}

impl Service {
    /// Export the application object on `connection` and claim [`BUS_NAME`] without queueing. Fails with
    /// `zbus::Error::NameTaken` when another process owns the name, and takes the objects back then.
    pub async fn serve(connection: &Connection) -> zbus::Result<Service> {
        let (calls_in, calls) = async_channel::bounded(MAX_WAITING);
        let (wake, woken) = async_channel::bounded(1);
        let object = Application {
            calls: calls_in,
            wake,
            own_uid: own_uid(),
        };
        let file_manager = FileManager(object.clone());
        let server = connection.object_server();
        server.at(OBJECT_PATH, object).await?;
        server.at(FILE_MANAGER_PATH, file_manager).await?;
        // after the object exists, so no call finds the name without it; a plain call, so the connection reads no
        // signals (zbus's own `request_name` follows the name's loss with a signal stream)
        let claimed = match request_name(connection, BUS_NAME, DO_NOT_QUEUE).await {
            Ok(PRIMARY_OWNER | ALREADY_OWNER) => Ok(()),
            Ok(_) => Err(zbus::Error::NameTaken),
            Err(e) => Err(e),
        };
        if let Err(e) = claimed {
            // a later try exports objects of its own, whose calls reach the window
            let _ = server.remove::<Application, _>(OBJECT_PATH).await;
            let _ = server.remove::<FileManager, _>(FILE_MANAGER_PATH).await;
            return Err(e);
        }
        // in line behind another file manager that holds it: the bus hands it over once that one lets go
        if let Err(e) = request_name(connection, FILE_MANAGER_NAME, 0).await {
            eprintln!("cupertino-files: cannot claim {FILE_MANAGER_NAME}: {e}");
        }
        Ok(Service {
            connection: connection.clone(),
            calls,
            woken,
        })
    }

    /// What wakes when a call is waiting: wait on it again after each [`Service::take`].
    pub fn woken(&self) -> &async_channel::Receiver<()> {
        &self.woken
    }

    /// The calls waiting, oldest first.
    pub fn take(&self) -> Vec<Call> {
        std::iter::from_fn(|| self.calls.try_recv().ok()).collect()
    }

    /// Refuse every waiting and later call and give the names up, so a launch racing the exit starts a new Files at
    /// once instead of handing over to this one.
    pub fn release(self) -> impl std::future::Future<Output = ()> + Send + 'static {
        let connection = self.connection.clone();
        drop(self);
        async move {
            for name in [BUS_NAME, FILE_MANAGER_NAME] {
                let _ = connection
                    .call_method(
                        Some("org.freedesktop.DBus"),
                        "/org/freedesktop/DBus",
                        Some("org.freedesktop.DBus"),
                        "ReleaseName",
                        &(name,),
                    )
                    .await;
            }
        }
    }
}

/// Dropping the service refuses every waiting and later call.
impl Drop for Service {
    fn drop(&mut self) {
        self.calls.close();
        drop(self.take());
    }
}

/// `RequestName`'s flag: fail at once rather than wait in line for the name.
const DO_NOT_QUEUE: u32 = 4;
/// `RequestName`'s answers that make this connection the owner.
const PRIMARY_OWNER: u32 = 1;
const ALREADY_OWNER: u32 = 4;

async fn request_name(connection: &Connection, name: &str, flags: u32) -> zbus::Result<u32> {
    connection
        .call_method(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            Some("org.freedesktop.DBus"),
            "RequestName",
            &(name, flags),
        )
        .await?
        .body()
        .deserialize()
}

fn own_uid() -> u32 {
    // Safety: getuid has no preconditions and cannot fail.
    unsafe { libc::getuid() }
}

/// The application object on the bus.
#[derive(Clone)]
struct Application {
    calls: async_channel::Sender<Call>,
    wake: async_channel::Sender<()>,
    own_uid: u32,
}

#[zbus::interface(name = "org.freedesktop.Application")]
impl Application {
    async fn activate(
        &self,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] connection: &Connection,
        platform_data: HashMap<String, OwnedValue>,
    ) -> zbus::fdo::Result<()> {
        self.admit(connection, &header).await?;
        let token = token_of(&platform_data)?;
        self.hand_over(Request::Activate, token).await
    }

    async fn open(
        &self,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] connection: &Connection,
        uris: Vec<String>,
        _hint: String,
        platform_data: HashMap<String, OwnedValue>,
    ) -> zbus::fdo::Result<()> {
        self.admit(connection, &header).await?;
        let targets = targets_of(&uris)?;
        let token = token_of(&platform_data)?;
        self.hand_over(Request::Open(targets), token).await
    }

    async fn activate_action(
        &self,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] connection: &Connection,
        action_name: String,
        parameter: Vec<OwnedValue>,
        platform_data: HashMap<String, OwnedValue>,
    ) -> zbus::fdo::Result<()> {
        self.admit(connection, &header).await?;
        let action = Action::from_name(&action_name)
            .ok_or_else(|| zbus::fdo::Error::InvalidArgs(format!("no action {action_name:?}")))?;
        if !parameter.is_empty() {
            return Err(zbus::fdo::Error::InvalidArgs(format!(
                "{action_name} takes no parameter"
            )));
        }
        let token = token_of(&platform_data)?;
        self.hand_over(Request::Action(action), token).await
    }
}

impl Application {
    /// Let the call in only from an unsandboxed process of this user.
    async fn admit(
        &self,
        connection: &Connection,
        header: &zbus::message::Header<'_>,
    ) -> zbus::fdo::Result<()> {
        let refused = |why: &str| zbus::fdo::Error::AccessDenied(why.into());
        let sender = header
            .sender()
            .ok_or_else(|| refused("the call names no sender"))?;
        let credentials = credentials(connection, sender)
            .await
            .map_err(|_| refused("the bus gave no credentials for the caller"))?;
        judge(&credentials, self.own_uid, Path::new("/proc")).map_err(refused)
    }

    /// Hand `request` to the window's thread and wait until it is taken.
    async fn hand_over(&self, request: Request, token: Option<String>) -> zbus::fdo::Result<()> {
        let (reply, replied) = async_channel::bounded(1);
        let call = Call {
            request,
            token,
            reply,
        };
        match self.calls.try_send(call) {
            Ok(()) => {}
            Err(async_channel::TrySendError::Full(_)) => {
                return Err(zbus::fdo::Error::LimitsExceeded(
                    "too many calls are waiting".into(),
                ))
            }
            Err(async_channel::TrySendError::Closed(_)) => {
                return Err(zbus::fdo::Error::Disconnected("Files is quitting".into()))
            }
        }
        let _ = self.wake.try_send(());
        replied
            .recv()
            .await
            .map_err(|_| zbus::fdo::Error::Disconnected("Files is quitting".into()))?
            .map_err(zbus::fdo::Error::LimitsExceeded)
    }
}

/// The file manager object on the bus: its calls go where the application object's go.
struct FileManager(Application);

#[zbus::interface(name = "org.freedesktop.FileManager1")]
impl FileManager {
    async fn show_items(
        &self,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] connection: &Connection,
        uris: Vec<String>,
        startup_id: String,
    ) -> zbus::fdo::Result<()> {
        self.show(connection, &header, &uris, &startup_id, Request::ShowItems)
            .await
    }

    async fn show_folders(
        &self,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] connection: &Connection,
        uris: Vec<String>,
        startup_id: String,
    ) -> zbus::fdo::Result<()> {
        self.show(connection, &header, &uris, &startup_id, Request::Open)
            .await
    }

    /// Files has no information window: the items are shown selected, as `ShowItems` shows them.
    async fn show_item_properties(
        &self,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] connection: &Connection,
        uris: Vec<String>,
        startup_id: String,
    ) -> zbus::fdo::Result<()> {
        self.show(connection, &header, &uris, &startup_id, Request::ShowItems)
            .await
    }
}

impl FileManager {
    /// Admit the caller, then hand over what `request` makes of the items of `uris`, raised with `startup_id`. No
    /// item is nothing to do.
    async fn show(
        &self,
        connection: &Connection,
        header: &zbus::message::Header<'_>,
        uris: &[String],
        startup_id: &str,
        request: fn(Vec<Target>) -> Request,
    ) -> zbus::fdo::Result<()> {
        self.0.admit(connection, header).await?;
        let items = items_of(uris)?;
        let token = startup_token(startup_id)?;
        if items.is_empty() {
            return Ok(());
        }
        self.0.hand_over(request(items), token).await
    }
}

/// What the URIs of a file manager call name, as [`Target::of_uri`] reads them, an absolute path taken as it is, each
/// path spelled plainly ([`plain_path`]) so it finds the window that shows its folder; no URI is no item, and one URI
/// that names no local item refuses them all.
fn items_of(uris: &[String]) -> Result<Vec<Target>, zbus::fdo::Error> {
    if uris.len() > MAX_URIS {
        return Err(zbus::fdo::Error::InvalidArgs(format!(
            "a call takes at most {MAX_URIS} URIs"
        )));
    }
    uris.iter()
        .map(|uri| match uri.starts_with('/') {
            true if uri.len() > MAX_URI || uri.contains('\0') => {
                Err(format!("{uri}: not a usable path"))
            }
            true => Ok(Target::Folder(PathBuf::from(uri))),
            false => Target::of_uri(uri),
        })
        .map(|item| match item {
            Ok(Target::Folder(path)) => Ok(Target::Folder(plain_path(&path))),
            Ok(other) => Ok(other),
            Err(e) => Err(zbus::fdo::Error::InvalidArgs(e)),
        })
        .collect()
}

/// `path` spelled plainly, without reading the disk: repeated and trailing slashes and `.` dropped. Each `..` is kept:
/// only the disk knows the folder it leads to (that of a symlinked folder is its target's parent), and the window's
/// thread reads no disk, so a thread of its own resolves it ([`crate::kit`]).
fn plain_path(path: &Path) -> PathBuf {
    path.components().collect()
}

/// The activation token of a call's startup id: none when it is empty; one too long refuses the call.
fn startup_token(startup_id: &str) -> Result<Option<String>, zbus::fdo::Error> {
    if startup_id.len() > MAX_TOKEN {
        return Err(zbus::fdo::Error::InvalidArgs(format!(
            "the startup id exceeds {MAX_TOKEN} bytes"
        )));
    }
    Ok((!startup_id.is_empty()).then(|| startup_id.to_owned()))
}

/// The folders of `uris`; one URI that is not a local folder's or the trash's refuses them all.
fn targets_of(uris: &[String]) -> Result<Vec<Target>, zbus::fdo::Error> {
    if uris.is_empty() || uris.len() > MAX_URIS {
        return Err(zbus::fdo::Error::InvalidArgs(format!(
            "Open takes 1 to {MAX_URIS} URIs"
        )));
    }
    uris.iter()
        .map(|uri| Target::of_uri(uri).map_err(zbus::fdo::Error::InvalidArgs))
        .collect()
}

/// The activation token of a call's platform data: `activation-token`, else `desktop-startup-id`. A token that is not
/// a string is ignored; one too long refuses the call.
fn token_of(
    platform_data: &HashMap<String, OwnedValue>,
) -> Result<Option<String>, zbus::fdo::Error> {
    for key in ["activation-token", "desktop-startup-id"] {
        let Some(Value::Str(token)) = platform_data.get(key).map(|v| &**v) else {
            continue;
        };
        if token.len() > MAX_TOKEN {
            return Err(zbus::fdo::Error::InvalidArgs(format!(
                "the activation token exceeds {MAX_TOKEN} bytes"
            )));
        }
        if !token.is_empty() {
            return Ok(Some(token.to_string()));
        }
    }
    Ok(None)
}

/// What the bus daemon knows of `sender`, asked with a plain call (a proxy could subscribe to signals).
async fn credentials(
    connection: &Connection,
    sender: &UniqueName<'_>,
) -> zbus::Result<ConnectionCredentials> {
    connection
        .call_method(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            Some("org.freedesktop.DBus"),
            "GetConnectionCredentials",
            &(sender.as_str(),),
        )
        .await?
        .body()
        .deserialize()
}

/// Whether the caller `credentials` describe may use Files: a process of user `own_uid` that runs in no Flatpak (a
/// `/.flatpak-info` in its root) and no Snap (an AppArmor label starting `snap.`). The process is held by the
/// descriptor the daemon passed (`ProcessFD`), so its id cannot name another process while it is judged; a daemon
/// that passes none is asked for the `ProcessID`, and a descriptor is opened for it here. A process that is gone, or
/// whose root cannot be read, is refused. `proc_root` is `/proc`.
#[expect(
    clippy::disallowed_methods,
    reason = "reads the kernel's process table (/proc), which answers from memory and never waits on a disk"
)]
fn judge(
    credentials: &ConnectionCredentials,
    own_uid: u32,
    proc_root: &Path,
) -> Result<(), &'static str> {
    if credentials.unix_user_id() != Some(own_uid) {
        return Err("the caller runs as another user");
    }
    let opened;
    let (pid, pidfd) = match credentials.process_fd() {
        Some(fd) => {
            let fd = fd.as_fd();
            let pid = pid_of(fd).filter(|pid| credentials.process_id().is_none_or(|p| p == *pid));
            (pid, fd)
        }
        None => {
            let pid = credentials
                .process_id()
                .ok_or("the bus gave no process for the caller")?;
            opened = pidfd_open(pid).map_err(|_| "the caller is gone")?;
            (Some(pid), opened.as_fd())
        }
    };
    let pid = pid.ok_or("the bus gave no process for the caller")?;
    if credentials
        .linux_security_label()
        .is_some_and(|label| label.starts_with(b"snap."))
    {
        return Err("a Snap may not use Files over the bus");
    }
    let process = proc_root.join(pid.to_string());
    if apparmor_label(&process).is_some_and(|l| l.starts_with(b"snap.")) {
        return Err("a Snap may not use Files over the bus");
    }
    match std::fs::metadata(process.join("root/.flatpak-info")) {
        Ok(_) => return Err("a Flatpak may not use Files over the bus"),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(_) => return Err("the caller's process cannot be checked"),
    }
    // what was read describes the caller only while the descriptor still names a live process
    if exited(pidfd) {
        return Err("the caller is gone");
    }
    Ok(())
}

/// The AppArmor label of the process at `process` (its `/proc` entry): AppArmor's own entry, else the one of the first
/// security module, which is AppArmor's when it is the only one that keeps a label.
#[expect(
    clippy::disallowed_methods,
    reason = "reads the kernel's process table (/proc), which answers from memory and never waits on a disk"
)]
fn apparmor_label(process: &Path) -> Option<Vec<u8>> {
    std::fs::read(process.join("attr/apparmor/current"))
        .or_else(|_| std::fs::read(process.join("attr/current")))
        .ok()
}

/// A descriptor that holds process `pid`; fails when it is gone.
fn pidfd_open(pid: u32) -> io::Result<std::os::fd::OwnedFd> {
    let pid =
        libc::pid_t::try_from(pid).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    // Safety: pidfd_open takes a pid and flags and returns a new descriptor or -1.
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // Safety: the descriptor was just made and nothing else owns it.
    Ok(unsafe { std::os::fd::FromRawFd::from_raw_fd(fd as i32) })
}

/// The process id a pidfd names, as the kernel shows it.
#[expect(
    clippy::disallowed_methods,
    reason = "reads the kernel's process table (/proc), which answers from memory and never waits on a disk"
)]
fn pid_of(fd: BorrowedFd<'_>) -> Option<u32> {
    let info = std::fs::read_to_string(format!("/proc/self/fdinfo/{}", fd.as_raw_fd())).ok()?;
    info.lines()
        .find_map(|l| l.strip_prefix("Pid:"))
        .and_then(|p| p.trim().parse::<i64>().ok())
        .filter(|p| *p > 0)
        .and_then(|p| u32::try_from(p).ok())
}

/// Whether the process of a pidfd has exited: the descriptor is readable then.
fn exited(fd: BorrowedFd<'_>) -> bool {
    let mut poll = libc::pollfd {
        fd: fd.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    // Safety: one valid pollfd, no wait.
    let ready = unsafe { libc::poll(&mut poll, 1, 0) };
    ready > 0 && poll.revents & libc::POLLIN != 0
}

/// How long a launch keeps trying while the running Files is quitting and has not yet let go of the name, beyond
/// [`FORWARD_TIMEOUT`].
pub const HANDOVER_WINDOW: Duration = Duration::from_secs(5);
/// The pause between two tries while the running Files quits.
pub const RETRY_PAUSE: Duration = Duration::from_millis(50);

/// Why one hand-over did not happen.
#[derive(Debug)]
pub enum Handover {
    /// The owner left, or is quitting: claim the name again.
    Retry(String),
    Failed(String),
}

impl From<zbus::Error> for Handover {
    fn from(e: zbus::Error) -> Self {
        if error_name(&e).is_some_and(|name| {
            [
                "org.freedesktop.DBus.Error.NoReply",
                "org.freedesktop.DBus.Error.ServiceUnknown",
                "org.freedesktop.DBus.Error.NameHasNoOwner",
                "org.freedesktop.DBus.Error.Disconnected",
                "org.freedesktop.DBus.Error.LimitsExceeded",
            ]
            .contains(&name.as_str())
        }) {
            Handover::Retry(e.to_string())
        } else {
            Handover::Failed(format!("the running Files refused the launch: {e}"))
        }
    }
}

/// The D-Bus error name of a failed call.
fn error_name(e: &zbus::Error) -> Option<String> {
    match e {
        zbus::Error::MethodError(name, _, _) => Some(name.to_string()),
        zbus::Error::FDO(e) => Some(zbus::DBusError::name(&**e).to_string()),
        _ => None,
    }
}

/// Hand `request` (with the launch's activation `token`) to the owner of the name on `connection`, once. The caller
/// bounds the wait for its answer ([`FORWARD_TIMEOUT`]); then [`unanswered`] tells whether to claim the name again.
pub async fn forward(
    connection: &Connection,
    request: &Request,
    token: Option<&str>,
) -> Result<(), Handover> {
    let owner = name_owner(connection).await?;
    // only an owner that runs this very program gets the launch's focus grant; another one (an older install at
    // another path, or a program squatting the name) still gets the request, as a launch without a token does
    let token = match token {
        Some(token) if runs_this_program(owner_pid(connection, &owner).await?) => Some(token),
        Some(_) => {
            eprintln!(
                "cupertino-files: {BUS_NAME} is owned by another program; the launch is handed to it without its \
                 activation token"
            );
            None
        }
        None => None,
    };
    let mut data: HashMap<&str, Value<'_>> = HashMap::new();
    if let Some(token) = token {
        data.insert("activation-token", Value::from(token));
        data.insert("desktop-startup-id", Value::from(token));
    }
    let result = match request {
        Request::Activate => {
            connection
                .call_method(
                    Some(owner.as_str()),
                    OBJECT_PATH,
                    Some(INTERFACE),
                    "Activate",
                    &(data,),
                )
                .await
        }
        Request::Open(targets) => {
            let uris: Vec<String> = targets.iter().map(Target::uri).collect();
            connection
                .call_method(
                    Some(owner.as_str()),
                    OBJECT_PATH,
                    Some(INTERFACE),
                    "Open",
                    &(uris, "", data),
                )
                .await
        }
        Request::ShowItems(targets) => {
            let uris: Vec<String> = targets.iter().map(Target::uri).collect();
            connection
                .call_method(
                    Some(owner.as_str()),
                    FILE_MANAGER_PATH,
                    Some(FILE_MANAGER_INTERFACE),
                    "ShowItems",
                    &(uris, token.unwrap_or_default()),
                )
                .await
        }
        Request::Action(action) => {
            let parameter: Vec<Value<'_>> = Vec::new();
            connection
                .call_method(
                    Some(owner.as_str()),
                    OBJECT_PATH,
                    Some(INTERFACE),
                    "ActivateAction",
                    &(action.name(), parameter, data),
                )
                .await
        }
    };
    result.map(drop).map_err(Handover::from)
}

/// What a hand-over that was not answered within [`FORWARD_TIMEOUT`] comes to: claim the name again when its owner
/// left while it was asked, else the launch fails, so two Files never run at once.
pub async fn unanswered(connection: &Connection) -> Handover {
    match name_owner(connection).await {
        Err(retry @ Handover::Retry(_)) => retry,
        _ => Handover::Failed(format!(
            "the running Files did not answer within {} s and still owns {BUS_NAME}",
            FORWARD_TIMEOUT.as_secs_f32()
        )),
    }
}

/// The unique name that owns [`BUS_NAME`]; a name with no owner is a [`Handover::Retry`].
async fn name_owner(connection: &Connection) -> Result<OwnedUniqueName, Handover> {
    let reply = connection
        .call_method(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            Some("org.freedesktop.DBus"),
            "GetNameOwner",
            &(BUS_NAME,),
        )
        .await;
    match reply {
        Ok(reply) => reply
            .body()
            .deserialize::<OwnedUniqueName>()
            .map_err(|e| Handover::Failed(e.to_string())),
        Err(e) => Err(e.into()),
    }
}

/// The process of the connection `owner`.
async fn owner_pid(connection: &Connection, owner: &OwnedUniqueName) -> Result<u32, Handover> {
    connection
        .call_method(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            Some("org.freedesktop.DBus"),
            "GetConnectionUnixProcessID",
            &(owner.as_str(),),
        )
        .await?
        .body()
        .deserialize()
        .map_err(|e| Handover::Failed(e.to_string()))
}

/// Whether process `pid` runs the same executable as this process. A binary replaced by a package upgrade while it
/// runs reads "<path> (deleted)"; it still ran from the same installed path.
#[expect(
    clippy::disallowed_methods,
    reason = "reads the kernel's process table (/proc), which answers from memory and never waits on a disk"
)]
fn runs_this_program(pid: u32) -> bool {
    match (
        std::fs::read_link(format!("/proc/{pid}/exe")),
        std::env::current_exe(),
    ) {
        (Ok(owner), Ok(own)) => {
            let owner = owner.as_os_str().as_bytes();
            let owner = owner.strip_suffix(b" (deleted)").unwrap_or(owner);
            owner == own.as_os_str().as_bytes()
        }
        _ => false,
    }
}

/// The activation token this process was started with (`XDG_ACTIVATION_TOKEN`, else `DESKTOP_STARTUP_ID`), taken out
/// of the environment so no program Files starts inherits a spent token. Call it before any thread starts.
pub fn take_activation_token() -> Option<String> {
    let token = ["XDG_ACTIVATION_TOKEN", "DESKTOP_STARTUP_ID"]
        .into_iter()
        .find_map(|key| std::env::var(key).ok().filter(|t| !t.is_empty()));
    for key in ["XDG_ACTIVATION_TOKEN", "DESKTOP_STARTUP_ID"] {
        std::env::remove_var(key);
    }
    token.filter(|t| t.len() <= MAX_TOKEN)
}

/// The platform data a launch hands over, for a test that calls the service as a launch does.
pub fn platform_data(token: Option<&str>) -> HashMap<String, OwnedValue> {
    token
        .into_iter()
        .flat_map(|token| {
            ["activation-token", "desktop-startup-id"].map(|key| {
                (
                    key.to_owned(),
                    OwnedValue::try_from(Value::from(token)).expect("a string has no descriptor"),
                )
            })
        })
        .collect()
}
