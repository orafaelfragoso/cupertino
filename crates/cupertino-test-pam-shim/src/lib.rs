//! The stand-in for the machine's PAM stack and logging that tests load into a login or session process with
//! `LD_PRELOAD`. It is never installed: the release builds only the workspace's binaries, and this crate is a library
//! nothing depends on. `cupertino-test-pam` builds the environment that loads it.
//!
//! In a process that loads it:
//!
//! * `pam_start` and `pam_start_confdir` start the transaction from the directory in [`POLICY`], never from
//!   `/etc/pam.d`; without that variable they refuse with `PAM_ABORT` and nothing reaches PAM.
//! * The library is also the only module of that policy (`pam_sm_*`): every step does what its line's arguments say
//!   (see [`Script`]) and appends what it did to the file its `record=` argument names.
//! * `openlog`, `syslog`, `vsyslog` and their checked forms write to the [`REPORT`] file instead of the journal, and
//!   libaudit is absent (`audit_open` fails as on a kernel without audit), so libpam sends no audit record.
//! * `connect`, `sendto` and `sendmsg` refuse (`EACCES`) every address of the journal, the syslog socket, the system bus
//!   and the users' session buses, by path and by the socket file's identity, and the path in [`REFUSE`]; each refusal
//!   is written to the [`REPORT`] file and to standard error.
//! * With [`PASSWD`] set, `getpwnam_r` and `getpwuid_r` answer first from that file (the `passwd(5)` format), and
//!   `initgroups` for the process's own group is a success without root, so a session's leader can be started as the
//!   test's own user under an account of the test's own.

#![allow(clippy::missing_safety_doc)]

use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_void};
use std::ptr;

/// The directory of the private PAM policy: one file per service, as in `/etc/pam.d`.
pub const POLICY: &CStr = c"CUPERTINO_TEST_PAM_DIR";
/// The file every refusal and every syslog line is appended to.
pub const REPORT: &CStr = c"CUPERTINO_TEST_PAM_REPORT";
/// A password database of the test's own, consulted before the system's.
pub const PASSWD: &CStr = c"CUPERTINO_TEST_PAM_PASSWD";
/// One more socket path to refuse, so a test can watch a refusal without going near the real sockets.
pub const REFUSE: &CStr = c"CUPERTINO_TEST_PAM_REFUSE";

/// The journal's and syslog's sockets, the system bus and its older path.
const REFUSED_PATHS: [&str; 9] = [
    "/dev/log",
    "/run/systemd/journal/socket",
    "/run/systemd/journal/stdout",
    "/run/systemd/journal/dev-log",
    "/run/systemd/journal/syslog",
    "/run/dbus/system_bus_socket",
    "/var/run/dbus/system_bus_socket",
    "/var/run/systemd/journal/socket",
    "/var/run/systemd/journal/stdout",
];

// ---- PAM's interface ----------------------------------------------------------------------------------------------

const PAM_SUCCESS: c_int = 0;
const PAM_PERM_DENIED: c_int = 6;
const PAM_AUTH_ERR: c_int = 7;
const PAM_USER_UNKNOWN: c_int = 10;
const PAM_CONV_ERR: c_int = 19;
const PAM_ABORT: c_int = 26;

const PAM_SERVICE: c_int = 1;
const PAM_TTY: c_int = 3;
const PAM_CONV: c_int = 5;

const PAM_PROMPT_ECHO_OFF: c_int = 1;
const PAM_PROMPT_ECHO_ON: c_int = 2;
const PAM_ERROR_MSG: c_int = 3;
const PAM_TEXT_INFO: c_int = 4;
const PAM_RADIO_TYPE: c_int = 5;

/// The results a line may ask for, by the names `cupertino-test-pam` writes.
const CODES: [(&str, c_int); 15] = [
    ("success", PAM_SUCCESS),
    ("system_err", 4),
    ("perm_denied", PAM_PERM_DENIED),
    ("auth_err", PAM_AUTH_ERR),
    ("cred_insufficient", 8),
    ("authinfo_unavail", 9),
    ("user_unknown", PAM_USER_UNKNOWN),
    ("maxtries", 11),
    ("new_authtok_reqd", 12),
    ("acct_expired", 13),
    ("session_err", 14),
    ("cred_unavail", 15),
    ("cred_err", 17),
    ("conv_err", PAM_CONV_ERR),
    ("abort", PAM_ABORT),
];

#[repr(C)]
struct PamMessage {
    msg_style: c_int,
    msg: *const c_char,
}

#[repr(C)]
struct PamResponse {
    resp: *mut c_char,
    resp_retcode: c_int,
}

type Converse = unsafe extern "C" fn(
    c_int,
    *mut *const PamMessage,
    *mut *mut PamResponse,
    *mut c_void,
) -> c_int;

#[repr(C)]
struct PamConv {
    conv: Option<Converse>,
    appdata_ptr: *mut c_void,
}

#[link(name = "pam")]
extern "C" {
    fn pam_get_user(pamh: *mut c_void, user: *mut *const c_char, prompt: *const c_char) -> c_int;
    fn pam_get_item(pamh: *const c_void, item_type: c_int, item: *mut *const c_void) -> c_int;
    fn pam_putenv(pamh: *mut c_void, name_value: *const c_char) -> c_int;
}

extern "C" {
    fn vsnprintf(buffer: *mut c_char, size: usize, format: *const c_char, ap: *mut c_void)
        -> c_int;
}

/// The next definition of `name` after this library's: libc's, libpam's.
unsafe fn next(name: &CStr) -> *mut c_void {
    libc::dlsym(libc::RTLD_NEXT, name.as_ptr())
}

/// `name` from the environment, when set.
fn variable(name: &CStr) -> Option<&'static CStr> {
    // Safety: `getenv` returns null or a string of the environment, which this library never changes.
    unsafe {
        let value = libc::getenv(name.as_ptr());
        (!value.is_null()).then(|| CStr::from_ptr(value))
    }
}

fn set_errno(value: c_int) {
    // Safety: the thread's own errno.
    unsafe { *libc::__errno_location() = value };
}

/// Append `line` to `path`.
fn append(path: &CStr, line: &str) {
    // Safety: plain system calls on a descriptor this function owns.
    unsafe {
        let fd = libc::open(
            path.as_ptr(),
            libc::O_WRONLY | libc::O_APPEND | libc::O_CREAT | libc::O_CLOEXEC,
            0o600,
        );
        if fd >= 0 {
            libc::write(fd, line.as_ptr().cast(), line.len());
            libc::close(fd);
        }
    }
}

/// What the process tried that a test must never do: to standard error and the [`REPORT`] file.
fn report(what: &str) {
    let line = format!("cupertino-test-pam-shim: {what}\n");
    // Safety: a write to standard error.
    unsafe { libc::write(libc::STDERR_FILENO, line.as_ptr().cast(), line.len()) };
    if let Some(path) = variable(REPORT) {
        append(path, &line);
    }
}

/// Lets a test see that the library is loaded in a process.
#[no_mangle]
pub extern "C" fn cupertino_test_pam_shim_loaded() -> c_int {
    1
}

// ---- the transaction's start ---------------------------------------------------------------------------------------

type StartConfdir = unsafe extern "C" fn(
    *const c_char,
    *const c_char,
    *const c_void,
    *const c_char,
    *mut *mut c_void,
) -> c_int;

unsafe fn start(
    service: *const c_char,
    user: *const c_char,
    conversation: *const c_void,
    handle: *mut *mut c_void,
) -> c_int {
    let service_name = if service.is_null() {
        "(none)".into()
    } else {
        CStr::from_ptr(service).to_string_lossy()
    };
    let Some(policy) = variable(POLICY) else {
        report(&format!(
            "refused to start PAM service {service_name}: no private policy directory"
        ));
        return PAM_ABORT;
    };
    let real = next(c"pam_start_confdir");
    if real.is_null() {
        report("refused to start PAM: libpam has no pam_start_confdir");
        return PAM_ABORT;
    }
    let real: StartConfdir = std::mem::transmute(real);
    real(service, user, conversation, policy.as_ptr(), handle)
}

/// The system's `pam_start`, sent to the private policy.
#[no_mangle]
pub unsafe extern "C" fn pam_start(
    service: *const c_char,
    user: *const c_char,
    conversation: *const c_void,
    handle: *mut *mut c_void,
) -> c_int {
    start(service, user, conversation, handle)
}

/// `pam_start_confdir` with any directory, sent to the private policy too.
#[no_mangle]
pub unsafe extern "C" fn pam_start_confdir(
    service: *const c_char,
    user: *const c_char,
    conversation: *const c_void,
    _directory: *const c_char,
    handle: *mut *mut c_void,
) -> c_int {
    start(service, user, conversation, handle)
}

// ---- the module ----------------------------------------------------------------------------------------------------

/// What the policy's lines do, from their arguments (`name=value`; Linux-PAM's `[name=value with spaces]` keeps a
/// value with spaces in one argument). Every line of the policy carries the whole script; each step reads its own keys:
///
/// * `record=PATH`: the file each call is appended to, as `STEP service=S user=U tty=T flags=0xN [answer=right|wrong]
///   -> RESULT`;
/// * `users=a,b`: the accounts the stack knows; any other is `user_unknown` at every step (after the password prompt,
///   as a real stack asks anyway);
/// * `password=TEXT`: authentication asks for a password and accepts this one; without it, it asks nothing;
/// * `prompt=echo|radio`: ask with echo on, or as a yes/no question, instead of a hidden prompt;
/// * `lockout=TEXT`: authentication says this as an error before it asks, then fails `auth_err` whatever the answer;
/// * `auth_info=TEXT`, `account_info=TEXT`: a message once that step has run (after the answer, for authentication);
/// * `authenticate=`, `setcred=`, `account=`, `open_session=`, `close_session=`, `chauthtok=` a name of [`CODES`]: that
///   step's result, whatever happened;
/// * `env=NAME=value`: the session's opening puts it in the PAM environment (repeatable).
#[derive(Default)]
struct Script {
    record: Option<CString>,
    users: Option<Vec<String>>,
    password: Option<String>,
    prompt: Option<String>,
    lockout: Option<String>,
    info: Vec<(Step, String)>,
    results: Vec<(Step, c_int)>,
    env: Vec<String>,
}

impl Script {
    unsafe fn parse(argc: c_int, argv: *const *const c_char) -> Result<Script, String> {
        let mut script = Script::default();
        for index in 0..usize::try_from(argc).unwrap_or(0) {
            let argument = CStr::from_ptr(*argv.add(index)).to_string_lossy();
            let (name, value) = argument
                .split_once('=')
                .ok_or_else(|| format!("argument without a value: {argument}"))?;
            match name {
                "record" => script.record = CString::new(value).ok(),
                "users" => script.users = Some(value.split(',').map(str::to_owned).collect()),
                "password" => script.password = Some(value.to_owned()),
                "prompt" => script.prompt = Some(value.to_owned()),
                "lockout" => script.lockout = Some(value.to_owned()),
                "auth_info" => script.info.push((Step::Authenticate, value.to_owned())),
                "account_info" => script.info.push((Step::Account, value.to_owned())),
                "env" => script.env.push(value.to_owned()),
                _ => {
                    let step = Step::ALL
                        .into_iter()
                        .find(|step| step.name() == name)
                        .ok_or_else(|| format!("unknown argument {name}"))?;
                    let code = CODES
                        .iter()
                        .find(|(known, _)| *known == value)
                        .map(|(_, code)| *code)
                        .ok_or_else(|| format!("unknown result {value}"))?;
                    script.results.push((step, code));
                }
            }
        }
        Ok(script)
    }

    fn knows(&self, user: &str) -> bool {
        self.users
            .as_ref()
            .is_none_or(|users| users.iter().any(|known| known == user))
    }

    fn result(&self, step: Step) -> Option<c_int> {
        self.results
            .iter()
            .find(|(asked, _)| *asked == step)
            .map(|(_, code)| *code)
    }

    fn info(&self, step: Step) -> Option<&str> {
        self.info
            .iter()
            .find(|(asked, _)| *asked == step)
            .map(|(_, text)| text.as_str())
    }
}

fn code_name(code: c_int) -> String {
    CODES
        .iter()
        .find(|(_, known)| *known == code)
        .map_or_else(|| code.to_string(), |(name, _)| (*name).to_owned())
}

unsafe fn item(handle: *mut c_void, kind: c_int) -> String {
    let mut value: *const c_void = ptr::null();
    if pam_get_item(handle, kind, &mut value) != PAM_SUCCESS || value.is_null() {
        return "-".into();
    }
    CStr::from_ptr(value.cast()).to_string_lossy().into_owned()
}

/// One message through the application's conversation; the answer, if the message asked for one.
unsafe fn converse(handle: *mut c_void, style: c_int, text: &str) -> Result<Option<String>, c_int> {
    let mut conversation: *const c_void = ptr::null();
    if pam_get_item(handle, PAM_CONV, &mut conversation) != PAM_SUCCESS || conversation.is_null() {
        return Err(PAM_CONV_ERR);
    }
    let conversation = &*conversation.cast::<PamConv>();
    let converse = conversation.conv.ok_or(PAM_CONV_ERR)?;
    let text = CString::new(text).map_err(|_| PAM_CONV_ERR)?;
    let message = PamMessage {
        msg_style: style,
        msg: text.as_ptr(),
    };
    let mut messages: *const PamMessage = &message;
    let mut responses: *mut PamResponse = ptr::null_mut();
    let status = converse(1, &mut messages, &mut responses, conversation.appdata_ptr);
    if status != PAM_SUCCESS {
        return Err(PAM_CONV_ERR);
    }
    if responses.is_null() {
        return Ok(None);
    }
    let answer = (*responses).resp;
    let text = if answer.is_null() {
        None
    } else {
        let length = libc::strlen(answer);
        let copy = CStr::from_ptr(answer).to_string_lossy().into_owned();
        // the answer is a password: it is cleared before it goes back to the allocator
        ptr::write_bytes(answer, 0, length);
        libc::free(answer.cast());
        Some(copy)
    };
    libc::free(responses.cast());
    Ok(text)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    Authenticate,
    Setcred,
    Account,
    OpenSession,
    CloseSession,
    Chauthtok,
}

impl Step {
    const ALL: [Step; 6] = [
        Step::Authenticate,
        Step::Setcred,
        Step::Account,
        Step::OpenSession,
        Step::CloseSession,
        Step::Chauthtok,
    ];

    fn name(self) -> &'static str {
        match self {
            Step::Authenticate => "authenticate",
            Step::Setcred => "setcred",
            Step::Account => "account",
            Step::OpenSession => "open_session",
            Step::CloseSession => "close_session",
            Step::Chauthtok => "chauthtok",
        }
    }
}

unsafe fn step(
    kind: Step,
    handle: *mut c_void,
    flags: c_int,
    argc: c_int,
    argv: *const *const c_char,
) -> c_int {
    let script = match Script::parse(argc, argv) {
        Ok(script) => script,
        Err(problem) => {
            report(&format!("a policy line is wrong: {problem}"));
            return PAM_ABORT;
        }
    };
    let mut user: *const c_char = ptr::null();
    if pam_get_user(handle, &mut user, ptr::null()) != PAM_SUCCESS || user.is_null() {
        return PAM_USER_UNKNOWN;
    }
    let user = CStr::from_ptr(user).to_string_lossy().into_owned();
    let mut note = String::new();
    let decided = (|| -> c_int {
        let mut code = PAM_SUCCESS;
        if !script.knows(&user) {
            code = PAM_USER_UNKNOWN;
        }
        match kind {
            Step::Authenticate => {
                if let Some(text) = &script.lockout {
                    if let Err(e) = converse(handle, PAM_ERROR_MSG, text) {
                        return e;
                    }
                    code = PAM_AUTH_ERR;
                }
                if let Some(password) = &script.password {
                    let style = match script.prompt.as_deref() {
                        Some("echo") => PAM_PROMPT_ECHO_ON,
                        Some("radio") => PAM_RADIO_TYPE,
                        _ => PAM_PROMPT_ECHO_OFF,
                    };
                    let answer = match converse(handle, style, "Password: ") {
                        Ok(answer) => answer,
                        Err(e) => return e,
                    };
                    let right = answer.as_deref() == Some(password.as_str());
                    note = format!(" answer={}", if right { "right" } else { "wrong" });
                    if !right && code == PAM_SUCCESS {
                        code = PAM_AUTH_ERR;
                    }
                }
            }
            Step::OpenSession => {
                for entry in &script.env {
                    let Ok(entry) = CString::new(entry.as_str()) else {
                        return PAM_ABORT;
                    };
                    if pam_putenv(handle, entry.as_ptr()) != PAM_SUCCESS {
                        return PAM_ABORT;
                    }
                }
            }
            _ => {}
        }
        if let Some(text) = script.info(kind) {
            if let Err(e) = converse(handle, PAM_TEXT_INFO, text) {
                return e;
            }
        }
        script.result(kind).unwrap_or(code)
    })();
    if let Some(record) = &script.record {
        append(
            record,
            &format!(
                "{} service={} user={user} tty={} flags={:#x}{note} -> {}\n",
                kind.name(),
                item(handle, PAM_SERVICE),
                item(handle, PAM_TTY),
                flags,
                code_name(decided)
            ),
        );
    }
    decided
}

#[no_mangle]
pub unsafe extern "C" fn pam_sm_authenticate(
    handle: *mut c_void,
    flags: c_int,
    argc: c_int,
    argv: *const *const c_char,
) -> c_int {
    step(Step::Authenticate, handle, flags, argc, argv)
}

#[no_mangle]
pub unsafe extern "C" fn pam_sm_setcred(
    handle: *mut c_void,
    flags: c_int,
    argc: c_int,
    argv: *const *const c_char,
) -> c_int {
    step(Step::Setcred, handle, flags, argc, argv)
}

#[no_mangle]
pub unsafe extern "C" fn pam_sm_acct_mgmt(
    handle: *mut c_void,
    flags: c_int,
    argc: c_int,
    argv: *const *const c_char,
) -> c_int {
    step(Step::Account, handle, flags, argc, argv)
}

#[no_mangle]
pub unsafe extern "C" fn pam_sm_open_session(
    handle: *mut c_void,
    flags: c_int,
    argc: c_int,
    argv: *const *const c_char,
) -> c_int {
    step(Step::OpenSession, handle, flags, argc, argv)
}

#[no_mangle]
pub unsafe extern "C" fn pam_sm_close_session(
    handle: *mut c_void,
    flags: c_int,
    argc: c_int,
    argv: *const *const c_char,
) -> c_int {
    step(Step::CloseSession, handle, flags, argc, argv)
}

#[no_mangle]
pub unsafe extern "C" fn pam_sm_chauthtok(
    handle: *mut c_void,
    flags: c_int,
    argc: c_int,
    argv: *const *const c_char,
) -> c_int {
    step(Step::Chauthtok, handle, flags, argc, argv)
}

// ---- logging -------------------------------------------------------------------------------------------------------

fn logged(priority: c_int, text: &str) {
    report(&format!("syslog <{priority}>: {text}"));
}

unsafe fn formatted(format: *const c_char, arguments: *mut c_void) -> String {
    if format.is_null() {
        return String::new();
    }
    let mut buffer = [0 as c_char; 1024];
    vsnprintf(buffer.as_mut_ptr(), buffer.len(), format, arguments);
    CStr::from_ptr(buffer.as_ptr())
        .to_string_lossy()
        .into_owned()
}

/// The words a variadic caller passed after the format. Stable Rust cannot define a variadic function; on these targets
/// the integer and pointer arguments of a variadic call travel exactly as those of a plain call, so the next words are
/// read as plain parameters and handed to `snprintf`. Floating-point arguments (no PAM message has one) are not.
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
type Words = [usize; 8];

#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
unsafe fn literal(format: *const c_char, w: Words) -> String {
    if format.is_null() {
        return String::new();
    }
    let mut buffer = [0 as c_char; 1024];
    libc::snprintf(
        buffer.as_mut_ptr(),
        buffer.len(),
        format,
        w[0],
        w[1],
        w[2],
        w[3],
        w[4],
        w[5],
        w[6],
        w[7],
    );
    CStr::from_ptr(buffer.as_ptr())
        .to_string_lossy()
        .into_owned()
}

#[no_mangle]
pub extern "C" fn openlog(_ident: *const c_char, _option: c_int, _facility: c_int) {}

#[no_mangle]
pub extern "C" fn closelog() {}

/// `syslog(priority, format, ...)`, formatted from the words after the format (see [`Words`]).
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn syslog(
    priority: c_int,
    format: *const c_char,
    a: usize,
    b: usize,
    c: usize,
    d: usize,
    e: usize,
    f: usize,
    g: usize,
    h: usize,
) {
    logged(priority, &literal(format, [a, b, c, d, e, f, g, h]));
}

/// `__syslog_chk(priority, flag, format, ...)`, the fortified `syslog`.
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn __syslog_chk(
    priority: c_int,
    _flag: c_int,
    format: *const c_char,
    a: usize,
    b: usize,
    c: usize,
    d: usize,
    e: usize,
    f: usize,
    g: usize,
) {
    logged(priority, &literal(format, [a, b, c, d, e, f, g, 0]));
}

#[no_mangle]
pub unsafe extern "C" fn vsyslog(priority: c_int, format: *const c_char, arguments: *mut c_void) {
    logged(priority, &formatted(format, arguments));
}

#[no_mangle]
pub unsafe extern "C" fn __vsyslog_chk(
    priority: c_int,
    _flag: c_int,
    format: *const c_char,
    arguments: *mut c_void,
) {
    logged(priority, &formatted(format, arguments));
}

/// No audit: libpam takes this as a kernel without audit support and sends nothing.
#[no_mangle]
pub extern "C" fn audit_open() -> c_int {
    set_errno(libc::EPROTONOSUPPORT);
    -1
}

// ---- sockets -------------------------------------------------------------------------------------------------------

/// The device and inode of the socket file at `path`.
fn identity(path: &CStr) -> Option<(u64, u64)> {
    // Safety: `stat` fills `info`.
    unsafe {
        let mut info: libc::stat = std::mem::zeroed();
        (libc::stat(path.as_ptr(), &mut info) == 0 && info.st_mode & libc::S_IFMT == libc::S_IFSOCK)
            .then_some((info.st_dev, info.st_ino))
    }
}

/// Whether a socket path is one a test must never reach.
fn refuses(path: &str) -> bool {
    let named = REFUSED_PATHS.contains(&path)
        || path.starts_with("/run/systemd/journal/")
        || path
            .strip_prefix("/run/user/")
            .and_then(|rest| rest.strip_suffix("/bus"))
            .is_some_and(|uid| !uid.is_empty() && uid.bytes().all(|b| b.is_ascii_digit()))
        || variable(REFUSE).is_some_and(|extra| extra.to_bytes() == path.as_bytes());
    if named {
        return true;
    }
    // another name for one of them (a symbolic link, a bind mount)
    let Some(target) = CString::new(path).ok().as_deref().and_then(identity) else {
        return false;
    };
    let session_bus = CString::new(format!("/run/user/{}/bus", unsafe { libc::getuid() })).ok();
    REFUSED_PATHS
        .iter()
        .filter_map(|known| CString::new(*known).ok())
        .chain(session_bus)
        .chain(variable(REFUSE).map(CStr::to_owned))
        .any(|known| identity(&known) == Some(target))
}

/// The path of a named Unix socket address.
unsafe fn unix_path(address: *const libc::sockaddr, length: libc::socklen_t) -> Option<String> {
    if address.is_null() || (*address).sa_family != libc::AF_UNIX as libc::sa_family_t {
        return None;
    }
    let base = std::mem::offset_of!(libc::sockaddr_un, sun_path);
    let length = (length as usize).checked_sub(base)?;
    let unix = &*address.cast::<libc::sockaddr_un>();
    let bytes: Vec<u8> = unix.sun_path[..length.min(unix.sun_path.len())]
        .iter()
        .map(|&b| b as u8)
        .take_while(|&b| b != 0)
        .collect();
    // an abstract address starts with a nul byte and has no file
    (!bytes.is_empty()).then(|| String::from_utf8_lossy(&bytes).into_owned())
}

/// Report and refuse a send or connection to `address` when it is refused.
unsafe fn refused(what: &str, address: *const libc::sockaddr, length: libc::socklen_t) -> bool {
    let Some(path) = unix_path(address, length) else {
        return false;
    };
    if !refuses(&path) {
        return false;
    }
    report(&format!("refused {what} to {path}"));
    set_errno(libc::EACCES);
    true
}

/// Exported for the tests of this library: whether a connection to `path` is refused.
#[no_mangle]
pub unsafe extern "C" fn cupertino_test_pam_shim_refuses(path: *const c_char) -> c_int {
    c_int::from(!path.is_null() && refuses(&CStr::from_ptr(path).to_string_lossy()))
}

#[no_mangle]
pub unsafe extern "C" fn connect(
    fd: c_int,
    address: *const libc::sockaddr,
    length: libc::socklen_t,
) -> c_int {
    if refused("a connection", address, length) {
        return -1;
    }
    type Connect = unsafe extern "C" fn(c_int, *const libc::sockaddr, libc::socklen_t) -> c_int;
    let real = next(c"connect");
    if real.is_null() {
        set_errno(libc::ENOSYS);
        return -1;
    }
    std::mem::transmute::<*mut c_void, Connect>(real)(fd, address, length)
}

#[no_mangle]
pub unsafe extern "C" fn sendto(
    fd: c_int,
    buffer: *const c_void,
    length: usize,
    flags: c_int,
    address: *const libc::sockaddr,
    address_length: libc::socklen_t,
) -> isize {
    if refused("a datagram", address, address_length) {
        return -1;
    }
    type SendTo = unsafe extern "C" fn(
        c_int,
        *const c_void,
        usize,
        c_int,
        *const libc::sockaddr,
        libc::socklen_t,
    ) -> isize;
    let real = next(c"sendto");
    if real.is_null() {
        set_errno(libc::ENOSYS);
        return -1;
    }
    std::mem::transmute::<*mut c_void, SendTo>(real)(
        fd,
        buffer,
        length,
        flags,
        address,
        address_length,
    )
}

#[no_mangle]
pub unsafe extern "C" fn sendmsg(fd: c_int, message: *const libc::msghdr, flags: c_int) -> isize {
    if !message.is_null()
        && refused(
            "a message",
            (*message).msg_name.cast(),
            (*message).msg_namelen,
        )
    {
        return -1;
    }
    type SendMsg = unsafe extern "C" fn(c_int, *const libc::msghdr, c_int) -> isize;
    let real = next(c"sendmsg");
    if real.is_null() {
        set_errno(libc::ENOSYS);
        return -1;
    }
    std::mem::transmute::<*mut c_void, SendMsg>(real)(fd, message, flags)
}

// ---- the password database ------------------------------------------------------------------------------------------

/// One entry of the [`PASSWD`] file.
struct Entry {
    fields: [String; 7],
}

impl Entry {
    fn uid(&self) -> Option<libc::uid_t> {
        self.fields[2].parse().ok()
    }
}

fn test_accounts() -> Vec<Entry> {
    let Some(path) = variable(PASSWD) else {
        return Vec::new();
    };
    let Ok(text) = std::fs::read_to_string(&*path.to_string_lossy()) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split(':').collect();
            let fields: [&str; 7] = fields.try_into().ok()?;
            Some(Entry {
                fields: fields.map(str::to_owned),
            })
        })
        .collect()
}

/// Fill `entry` into the caller's `passwd` and buffer, as `getpw*_r` do.
unsafe fn fill(
    entry: &Entry,
    passwd: *mut libc::passwd,
    buffer: *mut c_char,
    size: usize,
    result: *mut *mut libc::passwd,
) -> c_int {
    *result = ptr::null_mut();
    let needed: usize = [0, 1, 4, 5, 6]
        .iter()
        .map(|&i| entry.fields[i].len() + 1)
        .sum();
    if needed > size {
        return libc::ERANGE;
    }
    let mut at = buffer;
    let mut put = |text: &str| {
        let start = at;
        ptr::copy_nonoverlapping(text.as_ptr().cast::<c_char>(), at, text.len());
        *at.add(text.len()) = 0;
        at = at.add(text.len() + 1);
        start
    };
    let [name, password, uid, gid, gecos, home, shell] = &entry.fields;
    (*passwd).pw_name = put(name);
    (*passwd).pw_passwd = put(password);
    (*passwd).pw_uid = uid.parse().unwrap_or(libc::uid_t::MAX);
    (*passwd).pw_gid = gid.parse().unwrap_or(libc::gid_t::MAX);
    (*passwd).pw_gecos = put(gecos);
    (*passwd).pw_dir = put(home);
    (*passwd).pw_shell = put(shell);
    *result = passwd;
    0
}

type GetPwNam = unsafe extern "C" fn(
    *const c_char,
    *mut libc::passwd,
    *mut c_char,
    usize,
    *mut *mut libc::passwd,
) -> c_int;
type GetPwUid = unsafe extern "C" fn(
    libc::uid_t,
    *mut libc::passwd,
    *mut c_char,
    usize,
    *mut *mut libc::passwd,
) -> c_int;

#[no_mangle]
pub unsafe extern "C" fn getpwnam_r(
    name: *const c_char,
    passwd: *mut libc::passwd,
    buffer: *mut c_char,
    size: usize,
    result: *mut *mut libc::passwd,
) -> c_int {
    if !name.is_null() {
        let wanted = CStr::from_ptr(name).to_string_lossy();
        if let Some(entry) = test_accounts().iter().find(|e| e.fields[0] == wanted) {
            return fill(entry, passwd, buffer, size, result);
        }
    }
    let real = next(c"getpwnam_r");
    if real.is_null() {
        *result = ptr::null_mut();
        return libc::ENOSYS;
    }
    std::mem::transmute::<*mut c_void, GetPwNam>(real)(name, passwd, buffer, size, result)
}

#[no_mangle]
pub unsafe extern "C" fn getpwuid_r(
    uid: libc::uid_t,
    passwd: *mut libc::passwd,
    buffer: *mut c_char,
    size: usize,
    result: *mut *mut libc::passwd,
) -> c_int {
    if let Some(entry) = test_accounts().iter().find(|e| e.uid() == Some(uid)) {
        return fill(entry, passwd, buffer, size, result);
    }
    let real = next(c"getpwuid_r");
    if real.is_null() {
        *result = ptr::null_mut();
        return libc::ENOSYS;
    }
    std::mem::transmute::<*mut c_void, GetPwUid>(real)(uid, passwd, buffer, size, result)
}

/// Without root a process cannot set its groups, not even to the ones it has: with a password database of the test's
/// own, the leader's `initgroups` for the process's own group is a success that changes nothing.
#[no_mangle]
pub unsafe extern "C" fn initgroups(user: *const c_char, group: libc::gid_t) -> c_int {
    if variable(PASSWD).is_some() && libc::getuid() != 0 && group == libc::getgid() {
        return 0;
    }
    type InitGroups = unsafe extern "C" fn(*const c_char, libc::gid_t) -> c_int;
    let real = next(c"initgroups");
    if real.is_null() {
        set_errno(libc::ENOSYS);
        return -1;
    }
    std::mem::transmute::<*mut c_void, InitGroups>(real)(user, group)
}
