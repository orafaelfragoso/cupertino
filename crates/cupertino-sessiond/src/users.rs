//! The accounts of the machine: the ordinary users the login screen lists, the administrators, and single lookups. The list
//! comes from `/etc/passwd` and `/etc/group`, as the picker always did; accounts of a directory service are not listed, the
//! "Other…" tile reaches them because PAM resolves them.

use std::ffi::{CStr, CString};
use std::fmt;
use std::io;

const PASSWD: &str = "/etc/passwd";
const GROUP: &str = "/etc/group";
const LOGIN_DEFS: &str = "/etc/login.defs";
/// The group whose members are the administrators (approximate: the design's admin group has no counterpart; `wheel`
/// is the one the distribution's `sudo` rules name).
const ADMIN_GROUP: &str = "wheel";
/// The user id range of ordinary accounts when `login.defs` does not set `UID_MIN` or `UID_MAX`
/// (`login.defs(5)).
const DEFAULT_UID_MIN: u32 = 1000;
const DEFAULT_UID_MAX: u32 = 60000;
/// The search path of a session when `login.defs` has no `ENV_PATH` (default of Arch's `login.defs`).
const DEFAULT_ENV_PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/bin";
/// Shells (by file name) that refuse a login; an account with one is not offered.
const REFUSING_SHELLS: [&str; 2] = ["nologin", "false"];

#[derive(Debug)]
pub enum UsersError {
    /// The password database cannot be searched.
    Database(io::Error),
    /// A file the list is read from cannot be read.
    File {
        path: &'static str,
        source: io::Error,
    },
    /// A line of one of those files is not in the format the list needs.
    Malformed {
        path: &'static str,
        line: usize,
        reason: &'static str,
    },
}

impl fmt::Display for UsersError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Database(e) => write!(f, "reading the password database: {e}"),
            Self::File { path, source } => write!(f, "reading {path}: {source}"),
            Self::Malformed { path, line, reason } => write!(f, "{path} line {line}: {reason}"),
        }
    }
}

impl std::error::Error for UsersError {}

/// The fields of a password database entry the manager needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Account {
    pub uid: u32,
    pub gid: u32,
    /// The account name PAM authenticates.
    pub login: String,
    pub gecos: String,
    pub home: String,
    pub shell: String,
}

impl Account {
    /// The full name from the comment field, else the account name.
    pub fn display_name(&self) -> String {
        let full_name = self.gecos.split(',').next().unwrap_or("").trim();
        if full_name.is_empty() {
            self.login.clone()
        } else {
            full_name.to_owned()
        }
    }
}

/// The accounts the picker lists, in the order of the password database: user ids from `UID_MIN` to `UID_MAX` of
/// `/etc/login.defs`, without a shell that refuses logins.
pub fn login_accounts() -> Result<Vec<Account>, UsersError> {
    let range = parse_uid_range(&read(LOGIN_DEFS)?)?;
    parse_passwd(&read(PASSWD)?, range)
}

/// The login names of the administrators: the members `/etc/group` lists for the group [`ADMIN_GROUP`]. An account whose
/// primary group that is, and is not listed, is not found.
pub fn admin_logins() -> Result<Vec<String>, UsersError> {
    parse_group_members(&read(GROUP)?, ADMIN_GROUP)
}

/// `ENV_PATH` of `login.defs`: the search path of a session.
pub fn session_path() -> Result<String, UsersError> {
    Ok(parse_env_path(&read(LOGIN_DEFS)?))
}

fn read(path: &'static str) -> Result<String, UsersError> {
    std::fs::read_to_string(path).map_err(|source| UsersError::File { path, source })
}

/// The account named `login`, from the password database (so also a directory service's); `None` when there is none or the
/// text is not a name the database can hold (it has a nul byte).
pub fn by_name(login: &str) -> Result<Option<Account>, UsersError> {
    let Ok(name) = CString::new(login) else {
        return Ok(None);
    };
    // Safety: `getpwnam_r` gets valid pointers, and the buffer length it is given.
    read_entry(|passwd, buffer, found| unsafe {
        libc::getpwnam_r(
            name.as_ptr(),
            passwd,
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            found,
        )
    })
}

/// The account with user id `uid`.
pub fn by_uid(uid: u32) -> Result<Option<Account>, UsersError> {
    // Safety: as for `by_name`.
    read_entry(|passwd, buffer, found| unsafe {
        libc::getpwuid_r(uid, passwd, buffer.as_mut_ptr().cast(), buffer.len(), found)
    })
}

/// Whether the account `uid` is an administrator: a member of [`ADMIN_GROUP`] by the system's own lookup (`getgrouplist`,
/// so its primary group and a directory service's groups count), asked again at every call. `false` for an id with no
/// account, or when the group does not exist.
pub fn is_administrator(uid: u32) -> Result<bool, UsersError> {
    let Some(account) = by_uid(uid)? else {
        return Ok(false);
    };
    let Some(admin) = group_id(ADMIN_GROUP)? else {
        return Ok(false);
    };
    Ok(group_list(&account)?.contains(&admin))
}

/// The id of the group `name`, from the group database.
fn group_id(name: &str) -> Result<Option<u32>, UsersError> {
    let name = CString::new(name).map_err(|e| UsersError::Database(io::Error::other(e)))?;
    let mut buffer = vec![0u8; 1024];
    // Safety: all-zero is a valid `group`, and `getgrnam_r` overwrites it.
    let mut group: libc::group = unsafe { std::mem::zeroed() };
    let mut found: *mut libc::group = std::ptr::null_mut();
    loop {
        // Safety: valid pointers and the buffer's own length.
        match unsafe {
            libc::getgrnam_r(
                name.as_ptr(),
                &mut group,
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                &mut found,
            )
        } {
            0 => break,
            libc::ERANGE => buffer.resize(buffer.len() * 2, 0),
            errno => return Err(UsersError::Database(io::Error::from_raw_os_error(errno))),
        }
    }
    Ok((!found.is_null()).then_some(group.gr_gid))
}

/// Every group `account` is in, its primary group first.
fn group_list(account: &Account) -> Result<Vec<u32>, UsersError> {
    let login = CString::new(account.login.as_str())
        .map_err(|e| UsersError::Database(io::Error::other(e)))?;
    let mut groups: Vec<libc::gid_t> = vec![0; 64];
    loop {
        let mut count = groups.len() as libc::c_int;
        // Safety: `groups` holds `count` entries, which `getgrouplist` fills at most, setting `count` to how many it needs.
        let result = unsafe {
            libc::getgrouplist(login.as_ptr(), account.gid, groups.as_mut_ptr(), &mut count)
        };
        let count = usize::try_from(count).unwrap_or(0);
        if result >= 0 {
            groups.truncate(count);
            return Ok(groups);
        }
        if count <= groups.len() {
            return Err(UsersError::Database(io::Error::other(
                "getgrouplist failed without asking for more room",
            )));
        }
        groups.resize(count, 0);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct UidRange {
    min: u32,
    max: u32,
}

/// `UID_MIN` and `UID_MAX` from the text of `login.defs`: lines of a name and a value, `#` starts a comment.
fn parse_uid_range(login_defs: &str) -> Result<UidRange, UsersError> {
    let mut range = UidRange {
        min: DEFAULT_UID_MIN,
        max: DEFAULT_UID_MAX,
    };
    for (number, line) in login_defs.lines().enumerate() {
        let mut words = line
            .split_once('#')
            .map_or(line, |(before, _)| before)
            .split_whitespace();
        let (Some(name @ ("UID_MIN" | "UID_MAX")), Some(value)) = (words.next(), words.next())
        else {
            continue;
        };
        let value = value.parse().map_err(|_| UsersError::Malformed {
            path: LOGIN_DEFS,
            line: number + 1,
            reason: "not a user id",
        })?;
        if name == "UID_MIN" {
            range.min = value;
        } else {
            range.max = value;
        }
    }
    Ok(range)
}

/// `ENV_PATH` from the text of `login.defs`, else [`DEFAULT_ENV_PATH`].
fn parse_env_path(login_defs: &str) -> String {
    login_defs
        .lines()
        .filter_map(|line| {
            let mut words = line
                .split_once('#')
                .map_or(line, |(before, _)| before)
                .split_whitespace();
            (words.next() == Some("ENV_PATH"))
                .then(|| words.next())
                .flatten()
        })
        .next_back()
        .map_or_else(
            || DEFAULT_ENV_PATH.to_owned(),
            |value| value.strip_prefix("PATH=").unwrap_or(value).to_owned(),
        )
}

/// The accounts of `/etc/passwd` text with a user id in `range` and a shell that allows logins.
fn parse_passwd(passwd: &str, range: UidRange) -> Result<Vec<Account>, UsersError> {
    let mut accounts = Vec::new();
    for (number, line) in passwd
        .lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
    {
        let malformed = |reason| UsersError::Malformed {
            path: PASSWD,
            line: number + 1,
            reason,
        };
        let fields: Vec<&str> = line.split(':').collect();
        let [login, _password, uid, gid, gecos, home, shell] = fields[..] else {
            return Err(malformed("not seven fields"));
        };
        let uid: u32 = uid
            .parse()
            .map_err(|_| malformed("the user id is not a number"))?;
        let gid: u32 = gid
            .parse()
            .map_err(|_| malformed("the group id is not a number"))?;
        let refuses = shell
            .rsplit('/')
            .next()
            .is_some_and(|name| REFUSING_SHELLS.contains(&name));
        if (range.min..=range.max).contains(&uid) && !refuses {
            accounts.push(Account {
                uid,
                gid,
                login: login.to_owned(),
                gecos: gecos.to_owned(),
                home: home.to_owned(),
                shell: shell.to_owned(),
            });
        }
    }
    Ok(accounts)
}

/// The members of the group `group` in `/etc/group` text (`name:password:gid:member,member`); empty when the file has no
/// such group.
fn parse_group_members(text: &str, group: &str) -> Result<Vec<String>, UsersError> {
    let mut members = Vec::new();
    for (number, line) in text
        .lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
    {
        let malformed = |reason| UsersError::Malformed {
            path: GROUP,
            line: number + 1,
            reason,
        };
        let fields: Vec<&str> = line.split(':').collect();
        let [name, _password, gid, list] = fields[..] else {
            return Err(malformed("not four fields"));
        };
        gid.parse::<u32>()
            .map_err(|_| malformed("the group id is not a number"))?;
        if name == group {
            members.extend(
                list.split(',')
                    .filter(|member| !member.is_empty())
                    .map(str::to_owned),
            );
        }
    }
    Ok(members)
}

/// Run a `getpw*_r` call with a buffer that grows until the entry fits; `None` when there is no such entry. `call` gets the
/// `passwd` to fill, the buffer and the result pointer.
fn read_entry(
    call: impl Fn(&mut libc::passwd, &mut [u8], &mut *mut libc::passwd) -> libc::c_int,
) -> Result<Option<Account>, UsersError> {
    let mut buffer = vec![0u8; 1024];
    // Safety: all-zero is a valid `passwd` (null pointers and zero ids), and `call` overwrites it.
    let mut passwd: libc::passwd = unsafe { std::mem::zeroed() };
    let mut found: *mut libc::passwd = std::ptr::null_mut();
    loop {
        match call(&mut passwd, &mut buffer, &mut found) {
            0 => break,
            libc::ERANGE => buffer.resize(buffer.len() * 2, 0),
            errno => return Err(UsersError::Database(io::Error::from_raw_os_error(errno))),
        }
    }
    if found.is_null() {
        return Ok(None);
    }
    // Safety: on success the fields point at nul-terminated strings inside `buffer`.
    Ok(Some(unsafe {
        Account {
            uid: passwd.pw_uid,
            gid: passwd.pw_gid,
            login: field(passwd.pw_name),
            gecos: field(passwd.pw_gecos),
            home: field(passwd.pw_dir),
            shell: field(passwd.pw_shell),
        }
    }))
}

/// # Safety
/// `ptr` must point at a nul-terminated string.
unsafe fn field(ptr: *const libc::c_char) -> String {
    CStr::from_ptr(ptr).to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_administrator_is_a_member_of_the_group_by_the_systems_own_lookup() {
        // Safety: `getuid` has no preconditions.
        let me = unsafe { libc::getuid() };
        let mine = group_list(&by_uid(me).unwrap().unwrap()).unwrap();
        let wheel = group_id(ADMIN_GROUP).unwrap();
        assert_eq!(
            is_administrator(me).unwrap(),
            wheel.is_some_and(|gid| mine.contains(&gid))
        );
        assert!(
            !is_administrator(u32::MAX - 7).unwrap(),
            "no account, no administrator"
        );
        // an account that is not in the group is no administrator
        if let (Some(nobody), Some(wheel)) = (by_name("nobody").unwrap(), wheel) {
            assert!(!group_list(&nobody).unwrap().contains(&wheel));
            assert!(!is_administrator(nobody.uid).unwrap());
        }
    }

    #[test]
    fn an_account_is_found_by_name_and_by_id() {
        let root = by_name("root").unwrap().unwrap();
        assert_eq!((root.uid, root.login.as_str()), (0, "root"));
        assert_eq!(by_uid(0).unwrap().unwrap().login, "root");
        assert!(by_name("no-such-account").unwrap().is_none());
        assert!(by_name("a\0b").unwrap().is_none());
    }

    #[test]
    fn the_uid_range_is_read_from_login_defs_and_not_confused_with_the_system_range() {
        let defs = "# comment\nSYS_UID_MIN\t500\nUID_MIN\t\t 1000 # first\nUID_MAX 2000\nSUB_UID_MAX 600100000\n";
        assert_eq!(
            parse_uid_range(defs).unwrap(),
            UidRange {
                min: 1000,
                max: 2000
            }
        );
        assert_eq!(
            parse_uid_range("PASS_MAX_DAYS 99999\n").unwrap(),
            UidRange {
                min: 1000,
                max: 60000
            }
        );
        assert!(matches!(
            parse_uid_range("UID_MIN abc\n"),
            Err(UsersError::Malformed { line: 1, .. })
        ));
    }

    #[test]
    fn the_session_path_is_env_path_or_the_default() {
        assert_eq!(
            parse_env_path("ENV_PATH PATH=/usr/bin:/bin # sessions\nENV_SUPATH PATH=/sbin\n"),
            "/usr/bin:/bin"
        );
        assert_eq!(parse_env_path("ENV_PATH\t/opt/bin\n"), "/opt/bin");
        assert_eq!(parse_env_path("UID_MIN 1000\n"), DEFAULT_ENV_PATH);
    }

    #[test]
    fn only_ordinary_accounts_with_a_login_shell_are_listed() {
        let passwd = "root:x:0:0:root:/root:/bin/bash\n\
                      greeter:x:970:970:Greeter:/var/lib/greetd:/usr/bin/nologin\n\
                      alice:x:1000:1000:Alice Liddell,,,:/home/alice:/bin/bash\n\
                      svc:x:1001:1001::/home/svc:/usr/bin/nologin\n\
                      guest:x:1002:1002::/home/guest:/bin/false\n\
                      bob:x:1003:1004::/home/bob:\n\
                      nobody:x:65534:65534:Kernel Overflow User:/:/usr/bin/nologin\n";
        let accounts = parse_passwd(
            passwd,
            UidRange {
                min: 1000,
                max: 60000,
            },
        )
        .unwrap();
        let listed: Vec<(u32, u32, String, String)> = accounts
            .iter()
            .map(|a| (a.uid, a.gid, a.login.clone(), a.display_name()))
            .collect();
        assert_eq!(
            listed,
            [
                (1000, 1000, "alice".to_owned(), "Alice Liddell".to_owned()),
                (1003, 1004, "bob".to_owned(), "bob".to_owned())
            ]
        );
        assert!(matches!(
            parse_passwd("alice:x:1000\n", UidRange { min: 0, max: 9 }),
            Err(UsersError::Malformed { line: 1, .. })
        ));
    }

    #[test]
    fn the_administrators_are_the_members_listed_for_wheel() {
        let group = "root:x:0:\nwheel:x:998:alice,carol\nusers:x:100:bob\nwheelchair:x:999:dave\n\nadm:x:4:\n";
        assert_eq!(
            parse_group_members(group, "wheel").unwrap(),
            ["alice", "carol"]
        );
        assert_eq!(
            parse_group_members("wheel:x:998:\n", "wheel").unwrap(),
            Vec::<String>::new()
        );
        assert_eq!(
            parse_group_members("users:x:100:bob\n", "wheel").unwrap(),
            Vec::<String>::new()
        );
    }

    #[test]
    fn a_malformed_group_line_is_an_error_with_its_number() {
        assert!(matches!(
            parse_group_members("root:x:0:\nwheel:x:998\n", "wheel"),
            Err(UsersError::Malformed {
                path: "/etc/group",
                line: 2,
                ..
            })
        ));
        assert!(matches!(
            parse_group_members("wheel:x:abc:alice\n", "wheel"),
            Err(UsersError::Malformed {
                line: 1,
                reason: "the group id is not a number",
                ..
            })
        ));
    }
}
