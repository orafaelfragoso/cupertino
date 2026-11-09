//! A private PAM policy for tests, and the environment that keeps a process under test away from the machine's PAM
//! stack, journal and buses.
//!
//! A test that starts `cupertino-login` or `cupertino-sessiond` (or runs their PAM code in a process of its own) builds a
//! [`Pam`] from a [`Script`] and starts the process through [`Pam::isolate`] (a `Command`) or with
//! [`Pam::env_words`] (a command vector run through `env`). The process then loads the test shim
//! (`cupertino-test-pam-shim`, prepared by `ci/build-test-fixtures.py`) with `LD_PRELOAD`:
//!
//! * every PAM transaction starts from this policy's directory, whose only module is the shim, and nothing of
//!   `/etc/pam.d`, the password files or the machine's accounts is read; without the policy a transaction is refused;
//! * syslog lines, libaudit and every connection to the journal, the syslog socket, the system bus and the users' session
//!   buses are refused and written to the policy's report, which [`Pam`] checks is empty when it is dropped.
//!
//! The production binaries are unchanged: they call `pam_start` for their `/etc/pam.d` service as installed, and the
//! shim is a test fixture that no package contains.

use std::fmt::Write as _;
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

#[allow(dead_code)]
mod fixtures {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../ci/test-fixtures.rs"
    ));
}

/// The services of the installed policy (`packaging/pam`): the lock screen, the sessions, the login screen's own session
/// and the automatic login. The private policy has each, and `other` for any name else.
pub const SERVICES: [&str; 4] = [
    "cupertino-login",
    "cupertino-session",
    "cupertino-greeter",
    "cupertino-autologin",
];

/// The password the default [`Script`] accepts.
pub const PASSWORD: &str = "cupertino-test-password";

/// The variables the shim reads (`cupertino-test-pam-shim`).
pub const POLICY_VARIABLE: &str = "CUPERTINO_TEST_PAM_DIR";
pub const REPORT_VARIABLE: &str = "CUPERTINO_TEST_PAM_REPORT";
pub const PASSWD_VARIABLE: &str = "CUPERTINO_TEST_PAM_PASSWD";
pub const REFUSE_VARIABLE: &str = "CUPERTINO_TEST_PAM_REFUSE";

/// A result a step can be made to return.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Code {
    Success,
    SystemErr,
    PermDenied,
    AuthErr,
    CredInsufficient,
    AuthinfoUnavail,
    UserUnknown,
    MaxTries,
    NewAuthtokReqd,
    AcctExpired,
    SessionErr,
    CredUnavail,
    CredErr,
    ConvErr,
    Abort,
}

impl Code {
    /// The name the shim takes and records.
    pub fn name(self) -> &'static str {
        match self {
            Code::Success => "success",
            Code::SystemErr => "system_err",
            Code::PermDenied => "perm_denied",
            Code::AuthErr => "auth_err",
            Code::CredInsufficient => "cred_insufficient",
            Code::AuthinfoUnavail => "authinfo_unavail",
            Code::UserUnknown => "user_unknown",
            Code::MaxTries => "maxtries",
            Code::NewAuthtokReqd => "new_authtok_reqd",
            Code::AcctExpired => "acct_expired",
            Code::SessionErr => "session_err",
            Code::CredUnavail => "cred_unavail",
            Code::CredErr => "cred_err",
            Code::ConvErr => "conv_err",
            Code::Abort => "abort",
        }
    }
}

/// How authentication asks for the password.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Prompt {
    /// A hidden prompt, as a password is asked for.
    #[default]
    Hidden,
    /// A prompt with echo on.
    Echo,
    /// A yes/no question.
    Radio,
}

/// What the private policy does at each step.
#[derive(Clone, Debug)]
pub struct Script {
    /// The password authentication accepts; `None`: it asks nothing and accepts.
    pub password: Option<String>,
    /// The accounts the stack knows; any other is `user_unknown`. `None`: every account.
    pub users: Option<Vec<String>>,
    pub prompt: Prompt,
    /// Said as an error before the password prompt; authentication then fails whatever the answer (a lock-out).
    pub lockout: Option<String>,
    /// Said once the password is answered.
    pub auth_info: Option<String>,
    /// Said once the account is checked.
    pub account_info: Option<String>,
    /// Each step's result, whatever happened.
    pub authenticate: Option<Code>,
    pub account: Option<Code>,
    pub setcred: Option<Code>,
    pub open_session: Option<Code>,
    pub close_session: Option<Code>,
    /// `NAME=value` entries the session's opening puts in the PAM environment.
    pub session_env: Vec<String>,
}

impl Default for Script {
    /// Every account, [`PASSWORD`] asked for with a hidden prompt, every other step a success.
    fn default() -> Self {
        Script {
            password: Some(PASSWORD.to_owned()),
            users: None,
            prompt: Prompt::Hidden,
            lockout: None,
            auth_info: None,
            account_info: None,
            authenticate: None,
            account: None,
            setcred: None,
            open_session: None,
            close_session: None,
            session_env: Vec::new(),
        }
    }
}

/// One module argument; a value with spaces goes in Linux-PAM's brackets.
fn argument(out: &mut String, name: &str, value: &str) {
    assert!(
        !value.contains(['\n', '\r']),
        "a policy value is one line: {value:?}"
    );
    if value.contains(char::is_whitespace) || value.contains('[') {
        write!(out, " [{name}={}]", value.replace(']', "\\]")).unwrap();
    } else {
        write!(out, " {name}={value}").unwrap();
    }
}

impl Script {
    /// The module arguments of every line.
    fn arguments(&self, record: &Path) -> String {
        let mut out = String::new();
        argument(&mut out, "record", &record.display().to_string());
        if let Some(users) = &self.users {
            argument(&mut out, "users", &users.join(","));
        }
        if let Some(password) = &self.password {
            argument(&mut out, "password", password);
        }
        match self.prompt {
            Prompt::Hidden => {}
            Prompt::Echo => argument(&mut out, "prompt", "echo"),
            Prompt::Radio => argument(&mut out, "prompt", "radio"),
        }
        for (name, text) in [
            ("lockout", &self.lockout),
            ("auth_info", &self.auth_info),
            ("account_info", &self.account_info),
        ] {
            if let Some(text) = text {
                argument(&mut out, name, text);
            }
        }
        for (name, code) in [
            ("authenticate", self.authenticate),
            ("account", self.account),
            ("setcred", self.setcred),
            ("open_session", self.open_session),
            ("close_session", self.close_session),
        ] {
            if let Some(code) = code {
                argument(&mut out, name, code.name());
            }
        }
        for entry in &self.session_env {
            argument(&mut out, "env", entry);
        }
        out
    }
}

/// The shim, as Cargo built it for this run.
pub fn shim() -> PathBuf {
    fixtures::artifact(
        "cupertino-test-pam-shim",
        "cdylib",
        "cupertino_test_pam_shim",
    )
}

/// A private PAM policy in a directory of its own, with the record of every step and the report of everything refused.
pub struct Pam {
    dir: tempfile::TempDir,
    shim: PathBuf,
    passwd: Option<PathBuf>,
    /// Checked on drop: a shared policy lives in a static and is never dropped.
    check_on_drop: bool,
}

impl Pam {
    /// The policy `script` for every service.
    pub fn new(script: &Script) -> Pam {
        Pam::in_dir(tempfile::tempdir().unwrap(), script)
    }

    fn in_dir(dir: tempfile::TempDir, script: &Script) -> Pam {
        let shim = shim();
        let module = shim.display().to_string();
        assert!(
            !module.contains(char::is_whitespace),
            "the shim's path has no spaces: {module}"
        );
        let policy = dir.path().join("pam.d");
        std::fs::create_dir(&policy).unwrap();
        std::fs::create_dir(dir.path().join("home")).unwrap();
        let arguments = script.arguments(&dir.path().join("calls"));
        let mut stack = String::from("#%PAM-1.0\n");
        for kind in ["auth", "account", "password", "session"] {
            writeln!(stack, "{kind} required {module}{arguments}").unwrap();
        }
        for service in SERVICES.iter().chain(&["other"]) {
            std::fs::write(policy.join(service), &stack).unwrap();
        }
        Pam {
            dir,
            shim,
            passwd: None,
            check_on_drop: true,
        }
    }

    /// The policy also has `login`, an account of the process's own user and group with `home` and `/bin/sh`, which the
    /// password database lookups of the process find before the machine's.
    pub fn with_account(mut self, login: &str, home: &Path) -> Pam {
        // Safety: getuid and getgid have no preconditions.
        let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
        let path = self.dir.path().join("passwd");
        std::fs::write(
            &path,
            format!(
                "{login}:x:{uid}:{gid}:Test Account:{}:/bin/sh\n",
                home.display()
            ),
        )
        .unwrap();
        self.passwd = Some(path);
        self
    }

    /// The directory of the policy files.
    pub fn policy_dir(&self) -> PathBuf {
        self.dir.path().join("pam.d")
    }

    /// The home the isolated process gets unless the test sets another.
    pub fn home(&self) -> PathBuf {
        self.dir.path().join("home")
    }

    /// The file of refusals and syslog lines.
    pub fn report_path(&self) -> PathBuf {
        self.dir.path().join("report")
    }

    /// The variables that load the shim with this policy.
    pub fn variables(&self) -> Vec<(String, String)> {
        let mut variables = vec![
            ("LD_PRELOAD".to_owned(), self.shim.display().to_string()),
            (
                POLICY_VARIABLE.to_owned(),
                self.policy_dir().display().to_string(),
            ),
            (
                REPORT_VARIABLE.to_owned(),
                self.report_path().display().to_string(),
            ),
        ];
        if let Some(passwd) = &self.passwd {
            variables.push((PASSWD_VARIABLE.to_owned(), passwd.display().to_string()));
        }
        variables
    }

    /// The variables as `NAME=value` words, for a command vector that runs through `env`.
    pub fn env_words(&self) -> Vec<String> {
        self.variables()
            .into_iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect()
    }

    /// `command` with the shim and this policy, no login session of the test's own (`XDG_SESSION_ID`, the journal's
    /// `JOURNAL_STREAM`) and [`Pam::home`] as `HOME`; a test sets its own `HOME` after this call.
    pub fn isolate<'c>(&self, command: &'c mut Command) -> &'c mut Command {
        command
            .env_remove("XDG_SESSION_ID")
            .env_remove("JOURNAL_STREAM")
            .env("HOME", self.home())
            .envs(self.variables())
    }

    /// What the policy's steps did, one line each: `STEP service=S user=U tty=T flags=0xN [answer=right|wrong] ->
    /// RESULT`.
    pub fn calls(&self) -> Vec<String> {
        std::fs::read_to_string(self.dir.path().join("calls"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    /// The steps [`Pam::calls`] names, with their results: `authenticate -> success`.
    pub fn steps(&self) -> Vec<String> {
        self.calls()
            .iter()
            .map(|line| {
                let step = line.split_whitespace().next().unwrap_or_default();
                let result = line.rsplit(" -> ").next().unwrap_or_default();
                format!("{step} -> {result}")
            })
            .collect()
    }

    /// What a process of this policy tried to reach and was refused, and what it sent to syslog.
    pub fn reached(&self) -> String {
        std::fs::read_to_string(self.report_path()).unwrap_or_default()
    }

    /// [`Pam::reached`], emptied: a test that provokes refusals on purpose takes them so the drop check passes.
    pub fn take_reached(&self) -> String {
        let reached = self.reached();
        let _ = std::fs::remove_file(self.report_path());
        reached
    }
}

impl Drop for Pam {
    fn drop(&mut self) {
        if self.check_on_drop && !std::thread::panicking() {
            let reached = self.reached();
            assert!(
                reached.is_empty(),
                "a process under test tried to reach the machine's PAM stack, journal or buses:\n{reached}"
            );
        }
    }
}

/// The policy with the default [`Script`] for the processes a test starts without one of its own (the shell processes
/// of the compositor's tests). It lives as long as the fixture session of `ci/build-test-fixtures.py`, in whose
/// directory it is; its report is not checked on drop, the refusals themselves keep the machine's services untouched.
pub fn shared() -> &'static Pam {
    static SHARED: OnceLock<Pam> = OnceLock::new();
    SHARED.get_or_init(|| {
        let session = std::env::var_os("CUPERTINO_TEST_FIXTURE_SESSION")
            .expect("run through python3 ci/build-test-fixtures.py");
        let dir = tempfile::Builder::new()
            .prefix("pam-")
            .tempdir_in(session)
            .unwrap();
        let mut pam = Pam::in_dir(dir, &Script::default());
        pam.check_on_drop = false;
        pam
    })
}

/// Whether the shim is loaded in this process.
pub fn shim_loaded() -> bool {
    // Safety: a lookup by a nul-terminated name.
    !unsafe {
        libc::dlsym(
            libc::RTLD_DEFAULT,
            c"cupertino_test_pam_shim_loaded".as_ptr(),
        )
    }
    .is_null()
}

/// Panic unless the shim is loaded: a test that runs PAM in its own process calls this first, so it never runs
/// against the machine's stack.
pub fn require_loaded() {
    assert!(
        shim_loaded(),
        "this test runs PAM in its own process and must be started through cupertino_test_pam::Pam::child"
    );
}

/// The variable that tells a test started by [`Pam::child`] which case it runs.
pub const CHILD_VARIABLE: &str = "CUPERTINO_TEST_PAM_CHILD";

impl Pam {
    /// This test binary again, running only `test` (an `#[ignore]`d test that starts with [`require_loaded`]) with the
    /// shim and this policy, and `case` in [`CHILD_VARIABLE`]. The fixtures' variables are kept.
    pub fn child(&self, test: &str, case: &str) -> Command {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", test, "--include-ignored", "--nocapture"])
            .args(["--test-threads", "1"])
            .env(CHILD_VARIABLE, case)
            .env("CUPERTINO_ASSET_SEARCH_PATH", "");
        self.isolate(&mut command);
        fixtures::forward(&mut command);
        // the child dies with the test that started it
        // Safety: prctl takes integers and is async-signal-safe.
        unsafe {
            command.pre_exec(|| {
                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL);
                Ok(())
            });
        }
        command
    }
}

/// [`Pam::isolate`] as a step of a `Command` chain.
pub trait PamIsolated {
    /// The command with the shim and `pam` (see [`Pam::isolate`]).
    fn pam_isolated(&mut self, pam: &Pam) -> &mut Self;
}

impl PamIsolated for Command {
    fn pam_isolated(&mut self, pam: &Pam) -> &mut Self {
        pam.isolate(self)
    }
}
