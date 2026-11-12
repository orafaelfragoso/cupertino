//! What the test shim does in a process that loads it, checked in a copy of this test binary started through
//! [`Pam::child`]: PAM transactions start from the private policy, and without one are refused; the policy's module
//! does what its script says; syslog, libaudit and every connection or datagram to the journal's, syslog's and the
//! buses' sockets are refused and reported; and the policy's account is found first. No probe here sends anything to a
//! real socket of the machine: refusals are shown on a socket of the test's own, named in `CUPERTINO_TEST_PAM_REFUSE`.

use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::os::unix::net::UnixDatagram;
use std::process::Output;

use cupertino_test_pam::{
    require_loaded, shim_loaded, Code, Pam, Script, CHILD_VARIABLE, PASSWORD, POLICY_VARIABLE,
    REFUSE_VARIABLE,
};

/// The full name of this file's runner in the crate's test program, whose tests are named by their file's module.
const CHILD: &str = "shim::probes_in_a_process_with_the_test_shim";

/// A symbol of the loaded shim.
fn symbol(name: &CStr) -> *mut c_void {
    // Safety: a lookup by a nul-terminated name.
    let found = unsafe { libc::dlsym(libc::RTLD_DEFAULT, name.as_ptr()) };
    assert!(!found.is_null(), "{name:?} is loaded");
    found
}

fn refuses(path: &str) -> bool {
    type Refuses = unsafe extern "C" fn(*const c_char) -> c_int;
    // Safety: the shim's export has this signature.
    let refuses: Refuses =
        unsafe { std::mem::transmute(symbol(c"cupertino_test_pam_shim_refuses")) };
    let path = CString::new(path).unwrap();
    // Safety: a nul-terminated path.
    unsafe { refuses(path.as_ptr()) == 1 }
}

#[repr(C)]
struct Message {
    style: c_int,
    text: *const c_char,
}

#[repr(C)]
struct Response {
    text: *mut c_char,
    code: c_int,
}

#[repr(C)]
struct Conversation {
    converse:
        unsafe extern "C" fn(c_int, *mut *const Message, *mut *mut Response, *mut c_void) -> c_int,
    data: *mut c_void,
}

/// Answers every prompt with [`PASSWORD`].
unsafe extern "C" fn answer(
    count: c_int,
    messages: *mut *const Message,
    responses: *mut *mut Response,
    _data: *mut c_void,
) -> c_int {
    let count = count as usize;
    let out = libc::calloc(count, std::mem::size_of::<Response>()).cast::<Response>();
    for index in 0..count {
        let message = &**messages.add(index);
        if message.style == 1 || message.style == 2 {
            let password = CString::new(PASSWORD).unwrap();
            (*out.add(index)).text = libc::strdup(password.as_ptr());
        }
    }
    *responses = out;
    0
}

/// `pam_start` for `service` and `pam_authenticate` as an application calls them; the two results.
fn transaction(service: &str) -> (c_int, Option<c_int>) {
    type Start = unsafe extern "C" fn(
        *const c_char,
        *const c_char,
        *const Conversation,
        *mut *mut c_void,
    ) -> c_int;
    type Authenticate = unsafe extern "C" fn(*mut c_void, c_int) -> c_int;
    type End = unsafe extern "C" fn(*mut c_void, c_int) -> c_int;
    // Safety: the symbols are libpam's interface (`pam_start` the shim's), with these signatures.
    unsafe {
        let start: Start = std::mem::transmute(symbol(c"pam_start"));
        let authenticate: Authenticate = std::mem::transmute(symbol(c"pam_authenticate"));
        let end: End = std::mem::transmute(symbol(c"pam_end"));
        let conversation = Conversation {
            converse: answer,
            data: std::ptr::null_mut(),
        };
        let service = CString::new(service).unwrap();
        let mut handle = std::ptr::null_mut();
        let started = start(
            service.as_ptr(),
            c"cupertino-probe".as_ptr(),
            &conversation,
            &mut handle,
        );
        if started != 0 {
            return (started, None);
        }
        let authenticated = authenticate(handle, 0);
        end(handle, authenticated);
        (started, Some(authenticated))
    }
}

/// Run by the tests below in a process with the shim; prints what each probe found as `NAME value`.
#[test]
#[ignore = "started by the other tests of this file, in a process with the test shim"]
fn probes_in_a_process_with_the_test_shim() {
    require_loaded();
    match std::env::var(CHILD_VARIABLE).unwrap().as_str() {
        "sockets" => {
            // Safety: getuid has no preconditions.
            let uid = unsafe { libc::getuid() };
            for path in [
                "/dev/log".to_owned(),
                "/run/systemd/journal/socket".to_owned(),
                "/run/systemd/journal/stdout".to_owned(),
                "/run/systemd/journal/dev-log".to_owned(),
                "/run/dbus/system_bus_socket".to_owned(),
                "/var/run/dbus/system_bus_socket".to_owned(),
                format!("/run/user/{uid}/bus"),
            ] {
                println!("PROBE REFUSES {path} {}", refuses(&path));
            }
            println!(
                "PROBE REFUSES /run/user/{uid}/other {}",
                refuses(&format!("/run/user/{uid}/other"))
            );
            let own = std::env::var(REFUSE_VARIABLE).unwrap();
            let alias = format!("{own}-alias");
            let socket = UnixDatagram::unbound().unwrap();
            println!(
                "PROBE CONNECT {:?}",
                socket.connect(&own).map_err(|e| e.kind())
            );
            println!(
                "PROBE CONNECT_ALIAS {:?}",
                socket.connect(&alias).map_err(|e| e.kind())
            );
            println!(
                "PROBE SENDTO {:?}",
                socket.send_to(b"probe", &own).map_err(|e| e.kind())
            );
            // what libsystemd does: one `sendmsg` with the journal's address on an unconnected socket
            let path = CString::new(own.clone()).unwrap();
            // Safety: a `sockaddr_un` filled within its size, and an `iovec` over a live buffer.
            let sent = unsafe {
                let mut target: libc::sockaddr_un = std::mem::zeroed();
                target.sun_family = libc::AF_UNIX as libc::sa_family_t;
                for (at, byte) in path.as_bytes().iter().enumerate() {
                    target.sun_path[at] = *byte as c_char;
                }
                let mut data = *b"probe";
                let mut part = libc::iovec {
                    iov_base: data.as_mut_ptr().cast(),
                    iov_len: data.len(),
                };
                let mut message: libc::msghdr = std::mem::zeroed();
                message.msg_name = (&mut target as *mut libc::sockaddr_un).cast();
                message.msg_namelen = std::mem::size_of::<libc::sockaddr_un>() as libc::socklen_t;
                message.msg_iov = &mut part;
                message.msg_iovlen = 1;
                let fd = std::os::fd::AsRawFd::as_raw_fd(&socket);
                if libc::sendmsg(fd, &message, 0) < 0 {
                    Err(std::io::Error::last_os_error().kind())
                } else {
                    Ok(())
                }
            };
            println!("PROBE SENDMSG {sent:?}");
        }
        "syslog" => {
            // Safety: libc's syslog interface with a matching format.
            unsafe {
                libc::openlog(c"probe".as_ptr(), libc::LOG_NDELAY, libc::LOG_AUTHPRIV);
                libc::syslog(libc::LOG_ERR, c"probe %s %d".as_ptr(), c"text".as_ptr(), 7);
                libc::closelog();
            }
            type AuditOpen = unsafe extern "C" fn() -> c_int;
            // Safety: the shim's `audit_open` takes nothing.
            let opened =
                unsafe { std::mem::transmute::<*mut c_void, AuditOpen>(symbol(c"audit_open"))() };
            println!(
                "PROBE AUDIT {opened} {:?}",
                std::io::Error::last_os_error().raw_os_error() == Some(libc::EPROTONOSUPPORT)
            );
        }
        "transaction" => {
            let (started, authenticated) = transaction("cupertino-login");
            println!("PROBE START {started} {authenticated:?}");
            let (started, authenticated) = transaction("no-such-service");
            println!("PROBE OTHER {started} {authenticated:?}");
        }
        "accounts" => {
            // Safety: getuid and getgid have no preconditions.
            let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
            let mut entry: libc::passwd = unsafe { std::mem::zeroed() };
            let mut buffer = [0 as c_char; 1024];
            let mut found = std::ptr::null_mut();
            // Safety: the buffers are this frame's and their sizes are given.
            let read = |entry: &libc::passwd| unsafe {
                (
                    CStr::from_ptr(entry.pw_name).to_string_lossy().into_owned(),
                    entry.pw_uid,
                    CStr::from_ptr(entry.pw_dir).to_string_lossy().into_owned(),
                )
            };
            unsafe {
                libc::getpwnam_r(
                    c"cupertino-probe".as_ptr(),
                    &mut entry,
                    buffer.as_mut_ptr(),
                    buffer.len(),
                    &mut found,
                )
            };
            println!(
                "PROBE BY_NAME {:?}",
                (!found.is_null()).then(|| read(&entry))
            );
            unsafe {
                libc::getpwuid_r(
                    uid,
                    &mut entry,
                    buffer.as_mut_ptr(),
                    buffer.len(),
                    &mut found,
                )
            };
            println!(
                "PROBE BY_UID {:?}",
                (!found.is_null()).then(|| read(&entry))
            );
            unsafe {
                libc::getpwnam_r(
                    c"root".as_ptr(),
                    &mut entry,
                    buffer.as_mut_ptr(),
                    buffer.len(),
                    &mut found,
                )
            };
            println!(
                "PROBE ROOT {:?}",
                (!found.is_null()).then(|| read(&entry).1)
            );
            if uid != 0 {
                // Safety: initgroups takes a nul-terminated name and an integer.
                let own = unsafe { libc::initgroups(c"cupertino-probe".as_ptr(), gid) };
                let other = unsafe { libc::initgroups(c"cupertino-probe".as_ptr(), gid + 1) };
                println!("PROBE INITGROUPS {own} {other}");
            }
        }
        case => panic!("unknown case {case}"),
    }
}

fn run(child: &mut std::process::Command) -> Vec<String> {
    let Output {
        status,
        stdout,
        stderr,
    } = child.output().expect("the test binary starts again");
    let stdout = String::from_utf8_lossy(&stdout).into_owned();
    assert!(
        status.success(),
        "{stdout}\n{}",
        String::from_utf8_lossy(&stderr)
    );
    // libtest writes `test NAME ... ` before the child's first line, without a line break
    stdout
        .lines()
        .filter_map(|line| line.split_once("PROBE ").map(|(_, probe)| probe.to_owned()))
        .collect()
}

fn line<'a>(lines: &'a [String], name: &str) -> &'a str {
    lines
        .iter()
        .find_map(|line| line.strip_prefix(name)?.strip_prefix(' '))
        .unwrap_or_else(|| panic!("no {name} in {lines:?}"))
}

#[test]
fn this_process_has_no_shim() {
    assert!(
        !shim_loaded(),
        "the test runner itself is not under the shim"
    );
}

#[test]
fn the_journals_the_syslog_sockets_and_the_buses_are_refused_by_name_and_by_identity() {
    let pam = Pam::new(&Script::default());
    let dir = tempfile::tempdir().unwrap();
    let own = dir.path().join("journal");
    let listener = UnixDatagram::bind(&own).unwrap();
    std::os::unix::fs::symlink(&own, dir.path().join("journal-alias")).unwrap();
    let lines = run(pam.child(CHILD, "sockets").env(REFUSE_VARIABLE, &own));
    for wanted in [
        "/dev/log",
        "/run/systemd/journal/socket",
        "/run/systemd/journal/stdout",
        "/run/systemd/journal/dev-log",
        "/run/dbus/system_bus_socket",
        "/var/run/dbus/system_bus_socket",
    ] {
        assert!(
            lines.contains(&format!("REFUSES {wanted} true")),
            "{wanted}: {lines:?}"
        );
    }
    // the user's session bus, and nothing else of the runtime directory
    assert!(lines.iter().any(|l| l.ends_with("/bus true")), "{lines:?}");
    assert!(
        lines.iter().any(|l| l.ends_with("/other false")),
        "{lines:?}"
    );
    for probe in ["CONNECT", "CONNECT_ALIAS", "SENDTO", "SENDMSG"] {
        assert_eq!(line(&lines, probe), "Err(PermissionDenied)", "{probe}");
    }
    listener.set_nonblocking(true).unwrap();
    let mut buffer = [0; 16];
    assert!(
        listener.recv(&mut buffer).is_err(),
        "nothing reached the refused socket"
    );
    let reached = pam.take_reached();
    assert_eq!(
        reached.matches("refused").count(),
        4,
        "each refusal is reported:\n{reached}"
    );
}

#[test]
fn syslog_and_audit_reach_the_report_and_nothing_else() {
    let pam = Pam::new(&Script::default());
    let lines = run(&mut pam.child(CHILD, "syslog"));
    assert_eq!(line(&lines, "AUDIT"), "-1 true");
    let reached = pam.take_reached();
    assert_eq!(
        reached.trim(),
        "cupertino-test-pam-shim: syslog <3>: probe text 7"
    );
}

#[test]
fn a_transaction_starts_from_the_private_policy_or_not_at_all() {
    let pam = Pam::new(&Script::default());
    let lines = run(&mut pam.child(CHILD, "transaction"));
    assert_eq!(line(&lines, "START"), "0 Some(0)");
    // a service the policy has no file for gets its `other` stack, which is the same script
    assert_eq!(line(&lines, "OTHER"), "0 Some(0)");
    assert_eq!(
        pam.calls(),
        [
            "authenticate service=cupertino-login user=cupertino-probe tty=- flags=0x0 answer=right -> success",
            "authenticate service=no-such-service user=cupertino-probe tty=- flags=0x0 answer=right -> success",
        ]
    );

    // the policy's script decides the result
    let denying = Pam::new(&Script {
        authenticate: Some(Code::PermDenied),
        ..Script::default()
    });
    let lines = run(&mut denying.child(CHILD, "transaction"));
    assert_eq!(line(&lines, "START"), "0 Some(6)");

    // without a policy the shim refuses the start, and nothing of the machine's stack runs
    let lines = run(pam.child(CHILD, "transaction").env_remove(POLICY_VARIABLE));
    assert_eq!(line(&lines, "START"), "26 None");
    let reached = pam.take_reached();
    assert!(
        reached
            .contains("refused to start PAM service cupertino-login: no private policy directory"),
        "{reached}"
    );
}

#[test]
fn the_policys_account_is_found_before_the_machines() {
    let home = tempfile::tempdir().unwrap();
    let pam = Pam::new(&Script::default()).with_account("cupertino-probe", home.path());
    let lines = run(&mut pam.child(CHILD, "accounts"));
    // Safety: getuid has no preconditions.
    let uid = unsafe { libc::getuid() };
    let wanted = format!(
        "Some((\"cupertino-probe\", {uid}, {:?}))",
        home.path().display().to_string()
    );
    assert_eq!(line(&lines, "BY_NAME"), wanted);
    assert_eq!(line(&lines, "BY_UID"), wanted);
    // any other account is the machine's
    assert_eq!(line(&lines, "ROOT"), "Some(0)");
    if uid != 0 {
        assert_eq!(line(&lines, "INITGROUPS"), "0 -1");
    }
}
