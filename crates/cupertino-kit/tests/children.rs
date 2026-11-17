//! `kit::spawn`: a program started for an instance is reaped by the instance's UI loop once it ended, with no thread
//! waiting for it.

#[allow(dead_code)]
mod support;

use cupertino_kit::core::{Application, Command, Window};
use cupertino_kit::Instance;
use std::num::NonZeroU32;
use std::path::Path;
use std::process::Command as Process;

struct Idle;

impl Application for Idle {
    type Model = ();
    type Message = ();
    const RESIDENT: bool = true;

    fn init() -> ((), Vec<Command<()>>) {
        ((), Vec::new())
    }

    fn update(_: &mut (), (): ()) -> Vec<Command<()>> {
        Vec::new()
    }

    fn view(_: &()) -> Vec<Window<()>> {
        Vec::new()
    }
}

/// The state the kernel shows for process `pid`, `None` once it is gone.
fn state(pid: i32) -> Option<char> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // the state follows the parenthesised command name
    stat.rsplit_once(") ")?.1.chars().next()
}

#[test]
fn a_program_started_for_an_instance_is_reaped_by_its_loop_once_it_ended() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("kit-children");
    std::fs::create_dir_all(&dir).unwrap();
    let pid_file = dir.join("pid");
    let _ = std::fs::remove_file(&pid_file);
    let _dock = Instance::new("dock").enter();
    let mut driver = support::driver::<Idle>(support::text_system());
    driver
        .runtime_mut()
        .set_each_turn(cupertino_kit::app::each_turn());
    let mut command = Process::new("sh");
    command
        .arg("-c")
        .arg(format!("echo $$ > {}; exit 3", pid_file.display()));
    assert!(cupertino_kit::spawn("kit_test", command));
    assert!(!cupertino_kit::spawn(
        "kit_test",
        Process::new("/nonexistent/cupertino-test-program")
    ));
    // wait for the program to end without reaping it: until the loop turns it is a zombie
    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
    let waited = unsafe { libc::waitid(libc::P_ALL, 0, &mut info, libc::WEXITED | libc::WNOWAIT) };
    assert_eq!(waited, 0, "{}", std::io::Error::last_os_error());
    let pid: i32 = std::fs::read_to_string(&pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert_eq!(unsafe { info.si_pid() }, pid);
    assert_eq!(state(pid), Some('Z'));
    driver.run_until_idle(NonZeroU32::new(1).unwrap()).unwrap();
    assert_eq!(state(pid), None, "the ended program is reaped");
}
