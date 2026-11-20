//! The accounts of the password database (`getpwuid_r`, which also asks NSS): who the process runs as, and the account
//! of a user id.

use std::ffi::{CStr, OsStr};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;

/// The first size of the buffer an entry is read into, in bytes; it doubles while an entry does not fit.
const FIRST_BUFFER: usize = 1024;

/// The largest buffer an entry is read into, in bytes: an entry larger still is an error.
const MAX_BUFFER: usize = 1 << 20;

/// An entry of the password database.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Account {
    pub uid: u32,
    /// The account name.
    pub login: String,
    /// The comment field (GECOS): the full name, then optional fields after commas; may be empty.
    pub gecos: String,
    /// The home directory.
    pub home: PathBuf,
}

impl Account {
    /// The first field of the comment, the full name, as written (not trimmed); empty when there is none.
    pub fn full_name(&self) -> &str {
        self.gecos.split(',').next().unwrap_or("")
    }
}

/// The real user id of the process.
pub fn uid() -> u32 {
    rustix::process::getuid().as_raw()
}

/// The account of the process's real user; `None` when the database has no entry for it.
pub fn current() -> io::Result<Option<Account>> {
    by_uid(uid())
}

/// The account of `uid`; `None` when the database has no entry for it, an error when reading the database fails.
pub fn by_uid(uid: u32) -> io::Result<Option<Account>> {
    let mut buffer = vec![0u8; FIRST_BUFFER];
    // SAFETY: all-zero is a valid `passwd` (null pointers and zero ids), which `getpwuid_r` overwrites.
    let mut passwd: libc::passwd = unsafe { std::mem::zeroed() };
    let mut found: *mut libc::passwd = std::ptr::null_mut();
    loop {
        // SAFETY: `passwd`, `buffer` and `found` outlive the call, and the length passed is the buffer's.
        let status = unsafe {
            libc::getpwuid_r(
                uid,
                &mut passwd,
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                &mut found,
            )
        };
        match status {
            0 => break,
            libc::EINTR => {}
            libc::ERANGE if buffer.len() < MAX_BUFFER => buffer.resize(buffer.len() * 2, 0),
            errno => return Err(io::Error::from_raw_os_error(errno)),
        }
    }
    if found.is_null() || passwd.pw_name.is_null() {
        return Ok(None);
    }
    // SAFETY: on success the fields point at nul-terminated strings inside `buffer`, which is still alive, or are null.
    let field = |p: *const libc::c_char| -> &[u8] {
        if p.is_null() {
            &[]
        } else {
            unsafe { CStr::from_ptr(p) }.to_bytes()
        }
    };
    Ok(Some(Account {
        uid,
        login: String::from_utf8_lossy(field(passwd.pw_name)).into_owned(),
        gecos: String::from_utf8_lossy(field(passwd.pw_gecos)).into_owned(),
        home: PathBuf::from(OsStr::from_bytes(field(passwd.pw_dir))),
    }))
}
