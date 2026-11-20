//! The session agent's place in the session (`docs/architecture/shell-host.md`, *The component list* and *Programs the
//! server starts itself*), read from the real server's plan of the shell:
//! - the `agent` component runs in the shared host like every other component of the table;
//! - the lock screen's process, `cupertino-login`, holds the `session` and `session_lock` roles and no longer the
//!   `session_agent` one, which is the `agent` component's alone.

use std::path::Path;

use cupertino_server::authorization::ShellRole;
use cupertino_server::config::{BuiltIn, HostFacts};
use cupertino_server::Config;

#[test]
fn the_session_agent_runs_in_the_shared_host() {
    let mut config = Config::default();
    config.shell_processes.clear();
    config.shell_components = vec!["osd".into(), "agent".into()];
    let facts = HostFacts {
        available: true,
        ..HostFacts::default()
    };
    let plan = config.shell_plan(false, &facts);
    assert_eq!(plan.shared, Some(vec!["osd", "agent"]));
}

#[test]
fn the_lock_screen_holds_no_session_agent_role() {
    let login = BuiltIn::Login.process(Path::new("/usr/bin/cupertino-login"));
    assert_eq!(
        login.roles,
        vec![ShellRole::Session, ShellRole::SessionLock]
    );
}
