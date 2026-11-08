//! One Files process per session. The process that starts first claims [`BUS_NAME`] on the session bus and serves
//! `org.freedesktop.Application` at [`OBJECT_PATH`]; every later launch hands its request to that process and exits
//! ([`serve_or_forward`]), so file operations, their progress window and the pasteboard stay in one process.
//!
//! * `Activate` brings the most recently opened browser window forward, or opens one on the home folder when none is
//!   open.
//! * `Open` opens a window on each folder URI: `file://` (local) and `trash:///`. Anything else refuses the whole call.
//! * `ActivateAction` takes `show-progress` (the progress window, when it lists something) and `new-window`.
//!
//! An `activation-token` (or else `desktop-startup-id`) in the platform data raises the window the call opens or
//! brings forward. Only an unsandboxed process of the same user may call: the caller's process is named by the bus
//! daemon's credentials and refused when it runs in a Flatpak or a Snap.
//!
//! The service's connection reads no signals: the name is claimed with a plain `RequestName` call, so the one bus call
//! made for each incoming call (the caller's credentials) never waits behind a full signal queue. The window's thread
//! never waits on the bus: a call reaches it through a channel and the bus thread waits for its answer. A call that
//! comes while Files quits fails with `org.freedesktop.DBus.Error.Disconnected`, and the launch claims the name again.

use std::collections::HashMap;
use std::ffi::OsString;
use std::io;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use percent_encoding::{percent_decode_str, percent_encode, AsciiSet, NON_ALPHANUMERIC};
use zbus::fdo::ConnectionCredentials;
use zbus::names::{OwnedUniqueName, UniqueName};
use zbus::zvariant::{OwnedValue, Value};
use zbus::{connection, Connection};

use crate::trash::Trash;

/// The name Files owns on the session bus. A desktop file ID must equal it for the desktop to activate Files over
/// the bus (`DBusActivatable`); the desktop file is still `cupertino-files.desktop`, so launchers start the program,
/// which hands over to the running one.
pub const BUS_NAME: &str = "org.cupertino.Files";
/// Where the application object is: the bus name with its dots as slashes.
pub const OBJECT_PATH: &str = "/org/cupertino/Files";
pub const INTERFACE: &str = "org.freedesktop.Application";

/// How long a launch waits for the running Files to answer. A Files that has not answered by then and still owns the
/// name is taken to hang: the launch fails rather than starting a second process.
pub const FORWARD_TIMEOUT: Duration = Duration::from_secs(3);
/// How long a launch keeps trying while the running Files is quitting and has not yet let go of the name.
const HANDOVER_WINDOW: Duration = Duration::from_secs(5);
/// The pause between two tries while the running Files quits.
const RETRY_PAUSE: Duration = Duration::from_millis(50);

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
    /// when nothing was trashed yet (the freedesktop Trash specification lets the implementation create it).
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
}

/// A call that reached the window's thread: what it asks, the activation token it brought, and the answer the bus
/// thread waits for ([`Call::done`]). A call dropped unanswered tells the caller that Files is quitting.
#[derive(Debug)]
pub struct Call {
    pub request: Request,
    pub token: Option<String>,
    reply: async_channel::Sender<()>,
}

impl Call {
    /// Answer the caller: the request was taken.
    pub fn done(self) {
        let _ = self.reply.try_send(());
    }
}

/// The served name: the calls waiting for the window's thread and what wakes it for them. Dropping it refuses every
/// waiting and later call and gives the name up.
pub struct Service {
    connection: Connection,
    calls: async_channel::Receiver<Call>,
    woken: async_channel::Receiver<()>,
}

impl Service {
    /// Export the application object and claim [`BUS_NAME`] without queueing, on the bus `builder` connects to.
    /// Fails with `zbus::Error::NameTaken` when another process owns the name.
    pub fn serve(builder: connection::Builder<'static>) -> zbus::Result<Service> {
        let (calls_in, calls) = async_channel::bounded(MAX_WAITING);
        let (wake, woken) = async_channel::bounded(1);
        let object = Application {
            calls: calls_in,
            wake,
            own_uid: own_uid(),
        };
        zbus::block_on(async {
            let connection = builder.serve_at(OBJECT_PATH, object)?.build().await?;
            // after the object exists, so no call finds the name without it; a plain call, so the connection reads no
            // signals (zbus's own `request_name` follows the name's loss with a signal stream)
            match request_name(&connection).await? {
                PRIMARY_OWNER | ALREADY_OWNER => Ok(Service {
                    connection,
                    calls,
                    woken,
                }),
                _ => Err(zbus::Error::NameTaken),
            }
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
}

/// Dropping the service refuses every waiting and later call, and gives the name up on a thread of its own, so a
/// launch racing the exit starts a new Files at once instead of handing over to this one.
impl Drop for Service {
    fn drop(&mut self) {
        self.calls.close();
        drop(self.take());
        let connection = self.connection.clone();
        let _ = std::thread::Builder::new()
            .name("files-release".into())
            .spawn(move || {
                let _ = zbus::block_on(connection.call_method(
                    Some("org.freedesktop.DBus"),
                    "/org/freedesktop/DBus",
                    Some("org.freedesktop.DBus"),
                    "ReleaseName",
                    &(BUS_NAME,),
                ));
            });
    }
}

/// `RequestName`'s flag: fail at once rather than wait in line for the name.
const DO_NOT_QUEUE: u32 = 4;
/// `RequestName`'s answers that make this connection the owner.
const PRIMARY_OWNER: u32 = 1;
const ALREADY_OWNER: u32 = 4;

async fn request_name(connection: &Connection) -> zbus::Result<u32> {
    connection
        .call_method(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            Some("org.freedesktop.DBus"),
            "RequestName",
            &(BUS_NAME, DO_NOT_QUEUE),
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
            .map_err(|_| zbus::fdo::Error::Disconnected("Files is quitting".into()))
    }
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

/// How a launch went.
pub enum Launch {
    /// This process owns the name: it serves while the service lives.
    Serve(Service),
    /// The running Files took the request: this process exits.
    Forwarded,
}

/// Why a launch neither serves nor handed over.
#[derive(Debug)]
pub enum LaunchError {
    /// The name could not be claimed for another reason than its owner: no session bus, or one that refuses it. Files
    /// can still run, alone.
    NoBus(zbus::Error),
    /// Another process owns the name and did not take the request.
    Refused(String),
}

impl std::fmt::Display for LaunchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LaunchError::NoBus(e) => write!(f, "cannot claim {BUS_NAME}: {e}"),
            LaunchError::Refused(why) => f.write_str(why),
        }
    }
}

/// Serve [`BUS_NAME`] on the bus `connect` reaches, or hand `request` (with `token`) to the process that owns it,
/// waiting `timeout` for its answer. An owner that is quitting is waited for, and the name claimed once it has let
/// go; an owner that does not answer in time and still holds the name fails the launch, so two Files never run at once.
pub fn serve_or_forward(
    connect: impl Fn() -> zbus::Result<connection::Builder<'static>>,
    request: &Request,
    token: Option<&str>,
    timeout: Duration,
) -> Result<Launch, LaunchError> {
    let deadline = Instant::now() + timeout + HANDOVER_WINDOW;
    loop {
        match connect().and_then(Service::serve) {
            Ok(service) => return Ok(Launch::Serve(service)),
            Err(zbus::Error::NameTaken) => {}
            Err(e) => return Err(LaunchError::NoBus(e)),
        }
        match zbus::block_on(forward(&connect, request, token, timeout)) {
            Ok(()) => return Ok(Launch::Forwarded),
            Err(Handover::Retry(why)) if Instant::now() >= deadline => {
                return Err(LaunchError::Refused(format!(
                    "the running Files did not take the launch: {why}"
                )))
            }
            Err(Handover::Retry(_)) => std::thread::sleep(RETRY_PAUSE),
            Err(Handover::Failed(why)) => return Err(LaunchError::Refused(why)),
        }
    }
}

/// Why one hand-over did not happen.
enum Handover {
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

fn timed_out(e: &zbus::Error) -> bool {
    matches!(e, zbus::Error::InputOutput(e) if e.kind() == io::ErrorKind::TimedOut)
}

/// Hand `request` to the owner of the name, once.
async fn forward(
    connect: &impl Fn() -> zbus::Result<connection::Builder<'static>>,
    request: &Request,
    token: Option<&str>,
    timeout: Duration,
) -> Result<(), Handover> {
    let failed = |e: zbus::Error| Handover::Failed(e.to_string());
    let connection = connect()
        .map_err(failed)?
        .method_timeout(timeout)
        .build()
        .await
        .map_err(failed)?;
    let owner = name_owner(&connection).await?;
    if token.is_some() {
        // only an owner that runs this very program gets the launch's focus grant
        check_owner(&connection, &owner).await?;
    }
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
    match result {
        Ok(_) => Ok(()),
        Err(e) if timed_out(&e) => match name_owner(&connection).await {
            // the owner left while it was asked: claim the name
            Err(retry @ Handover::Retry(_)) => Err(retry),
            _ => Err(Handover::Failed(format!(
                "the running Files did not answer within {} s and still owns {BUS_NAME}",
                timeout.as_secs_f32()
            ))),
        },
        Err(e) => Err(e.into()),
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
        Err(e) if timed_out(&e) => Err(Handover::Failed(format!("the bus does not answer: {e}"))),
        Err(e) => Err(e.into()),
    }
}

/// Refuse an owner that does not run this very program.
async fn check_owner(connection: &Connection, owner: &OwnedUniqueName) -> Result<(), Handover> {
    let pid: u32 = connection
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
        .map_err(|e| Handover::Failed(e.to_string()))?;
    if runs_this_program(pid) {
        Ok(())
    } else {
        Err(Handover::Failed(format!(
            "{BUS_NAME} is owned by another program; the launch is not handed to it"
        )))
    }
}

/// Whether process `pid` runs the same executable as this process. A binary replaced by a package upgrade while it
/// runs reads "<path> (deleted)"; it still ran from the same installed path.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_name_a_path_a_file_uri_or_the_trash() {
        let folder = |p: &str| Ok(Target::Folder(PathBuf::from(p)));
        assert_eq!(Target::of_argument("trash:///"), Ok(Target::Trash));
        assert_eq!(Target::of_argument("trash:"), Ok(Target::Trash));
        assert!(
            Target::of_argument("trash:///old/report.txt").is_err(),
            "only the trash folder itself"
        );
        assert_eq!(
            Target::of_argument("file:///home/u/My%20Files"),
            folder("/home/u/My Files")
        );
        assert_eq!(Target::of_argument("file://localhost/tmp"), folder("/tmp"));
        assert_eq!(
            Target::of_argument("file:///tmp/a%3Fb?x=1#top"),
            folder("/tmp/a?b")
        );
        assert!(
            Target::of_argument("file://server/share").is_err(),
            "a remote host"
        );
        assert_eq!(Target::of_argument("Documents"), folder("Documents"));
    }

    #[test]
    fn a_call_takes_only_local_folder_and_trash_uris() {
        assert_eq!(Target::of_uri("trash:///"), Ok(Target::Trash));
        assert_eq!(
            Target::of_uri("file:///srv/x"),
            Ok(Target::Folder("/srv/x".into()))
        );
        for bad in [
            "/home/u",
            "Documents",
            "http://example.com/",
            "sftp://host/home",
            "file://host/home",
            "file:relative",
            "file:///a%00b",
            "trash:///file.txt",
            "",
        ] {
            assert!(Target::of_uri(bad).is_err(), "{bad} is refused");
        }
        assert!(Target::of_uri(&format!("file:///{}", "a".repeat(MAX_URI))).is_err());
        assert!(targets_of(&[]).is_err(), "Open names at least one folder");
        let many = vec!["trash:///".to_owned(); MAX_URIS + 1];
        assert!(targets_of(&many).is_err());
        let one_bad = ["file:///tmp".to_owned(), "http://x/".to_owned()];
        assert!(
            targets_of(&one_bad).is_err(),
            "one bad URI refuses them all"
        );
    }

    #[test]
    fn a_target_round_trips_through_its_uri() {
        for path in ["/home/u/My Files", "/tmp/a?b#c%d", "/srv/ünïcode/x y", "/"] {
            let target = Target::Folder(path.into());
            assert_eq!(Target::of_uri(&target.uri()), Ok(target.clone()), "{path}");
        }
        let odd = Target::Folder(PathBuf::from(OsString::from_vec(b"/tmp/\xff\x01".to_vec())));
        assert_eq!(Target::of_uri(&odd.uri()), Ok(odd));
        assert_eq!(Target::Trash.uri(), "trash:///");
    }

    #[test]
    fn the_token_comes_from_the_platform_data() {
        let data = |pairs: &[(&str, Value<'static>)]| -> HashMap<String, OwnedValue> {
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), OwnedValue::try_from(v.clone()).unwrap()))
                .collect()
        };
        assert_eq!(token_of(&HashMap::new()), Ok(None));
        assert_eq!(
            token_of(&data(&[("desktop-startup-id", Value::from("b"))])),
            Ok(Some("b".into()))
        );
        assert_eq!(
            token_of(&data(&[
                ("activation-token", Value::from("a")),
                ("desktop-startup-id", Value::from("b"))
            ])),
            Ok(Some("a".into())),
            "the activation token first"
        );
        assert_eq!(
            token_of(&data(&[("activation-token", Value::from(7u32))])),
            Ok(None),
            "a token that is not a string is ignored"
        );
        assert!(token_of(&data(&[(
            "activation-token",
            Value::from("x".repeat(MAX_TOKEN + 1))
        )]))
        .is_err());
        assert_eq!(token_of(&platform_data(Some("t"))), Ok(Some("t".into())));
    }

    #[test]
    fn actions_go_by_name() {
        for action in [Action::ShowProgress, Action::NewWindow] {
            assert_eq!(Action::from_name(action.name()), Some(action));
        }
        assert_eq!(Action::from_name("quit"), None);
    }

    /// The pidfd of this process.
    fn own_pidfd() -> std::os::fd::OwnedFd {
        pidfd_open(std::process::id()).expect("pidfd_open")
    }

    #[test]
    fn only_an_unsandboxed_process_of_this_user_is_let_in() {
        let uid = own_uid();
        let me = ConnectionCredentials::default()
            .set_unix_user_id(uid)
            .set_process_id(std::process::id());
        assert_eq!(judge(&me, uid, Path::new("/proc")), Ok(()));
        let pinned = ConnectionCredentials::default()
            .set_unix_user_id(uid)
            .set_process_fd(own_pidfd().into());
        assert_eq!(judge(&pinned, uid, Path::new("/proc")), Ok(()));

        assert!(
            judge(&me, uid.wrapping_add(1), Path::new("/proc")).is_err(),
            "another user"
        );
        let anonymous = ConnectionCredentials::default().set_unix_user_id(uid);
        assert!(
            judge(&anonymous, uid, Path::new("/proc")).is_err(),
            "no process"
        );
        let no_user = ConnectionCredentials::default().set_process_id(std::process::id());
        assert!(judge(&no_user, uid, Path::new("/proc")).is_err(), "no user");
        let snap = ConnectionCredentials::default()
            .set_unix_user_id(uid)
            .set_process_id(std::process::id())
            .set_linux_security_label(b"snap.app.app (enforce)".to_vec());
        assert!(judge(&snap, uid, Path::new("/proc")).is_err(), "a Snap");

        // a process whose root holds /.flatpak-info, and one whose root cannot be read
        let proc_root = tempfile::tempdir().unwrap();
        let process = proc_root.path().join(std::process::id().to_string());
        std::fs::create_dir_all(process.join("root")).unwrap();
        assert_eq!(judge(&me, uid, proc_root.path()), Ok(()));
        // a Snap's label in AppArmor's own entry, behind another security module's
        std::fs::create_dir_all(process.join("attr/apparmor")).unwrap();
        std::fs::write(process.join("attr/current"), "unconfined").unwrap();
        std::fs::write(
            process.join("attr/apparmor/current"),
            "snap.app.app (enforce)",
        )
        .unwrap();
        assert!(
            judge(&me, uid, proc_root.path()).is_err(),
            "a Snap by AppArmor's entry"
        );
        std::fs::remove_dir_all(process.join("attr")).unwrap();
        std::fs::write(process.join("root/.flatpak-info"), "[Application]\n").unwrap();
        assert!(judge(&me, uid, proc_root.path()).is_err(), "a Flatpak");
        std::fs::remove_file(process.join("root/.flatpak-info")).unwrap();
        std::fs::write(process.join("root.tmp"), "").unwrap();
        std::fs::remove_dir(process.join("root")).unwrap();
        std::os::unix::fs::symlink(process.join("root.tmp"), process.join("root")).unwrap();
        assert!(
            judge(&me, uid, proc_root.path()).is_err(),
            "a root that cannot be looked into"
        );
    }

    #[test]
    fn a_process_that_exited_is_refused() {
        // named by its process id alone, as a daemon without process descriptors names it
        let mut gone = std::process::Command::new("true").spawn().unwrap();
        let gone_pid = gone.id();
        gone.wait().unwrap();
        let uid = own_uid();
        let named = ConnectionCredentials::default()
            .set_unix_user_id(uid)
            .set_process_id(gone_pid);
        assert_eq!(
            judge(&named, uid, Path::new("/proc")),
            Err("the caller is gone")
        );

        let mut child = std::process::Command::new("true").spawn().unwrap();
        let fd = pidfd_open(child.id()).unwrap();
        child.wait().unwrap();
        assert!(exited(fd.as_fd()));
        assert!(!exited(own_pidfd().as_fd()));
    }

    #[test]
    fn only_this_program_counts_as_the_owner() {
        assert!(runs_this_program(std::process::id()));
        assert!(!runs_this_program(u32::MAX));
    }
}
