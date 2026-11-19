//! D20 bus caller resolution: server (owner of `Session1`), accepted shell host component, or other caller.

use cupertino_peer::component::{classify_caller, process_start_ticks, ComponentCaller};
use cupertino_peer::ProcessHandle;

#[test]
fn classify_caller_resolves_server_host_components_and_outsiders_in_order() {
    let host_shared = (100u32, 5000u64);
    let host_separate = (200u32, 9000u64);
    let hosts = [host_shared, host_separate];

    let rows = [
        (
            "owner of Session1 is always the server",
            Some(":1.1"),
            ":1.1",
            Some(host_shared),
            Some("menubar"),
            ComponentCaller::Server,
        ),
        (
            "accepted connection of shared host resolves to its component",
            Some(":1.1"),
            ":1.42",
            Some(host_shared),
            Some("agent"),
            ComponentCaller::Component("agent".into()),
        ),
        (
            "accepted connection of separate host resolves identically",
            Some(":1.1"),
            ":1.77",
            Some(host_separate),
            Some("dock"),
            ComponentCaller::Component("dock".into()),
        ),
        (
            "unaccepted connection from a host process is Other",
            Some(":1.1"),
            ":1.43",
            Some(host_shared),
            Some(""),
            ComponentCaller::Other,
        ),
        (
            "non-host process is Other even if ComponentOf would name something",
            Some(":1.1"),
            ":1.99",
            Some((999, 1234)),
            Some("agent"),
            ComponentCaller::Other,
        ),
        (
            "reused pid with different start time is Other",
            Some(":1.1"),
            ":1.42",
            Some((100, 9999)),
            Some("agent"),
            ComponentCaller::Other,
        ),
        (
            "anonymous caller with no process is Other",
            Some(":1.1"),
            ":1.42",
            None,
            Some("agent"),
            ComponentCaller::Other,
        ),
    ];

    for (label, owner, sender, proc_id, comp, expected) in rows {
        let got = classify_caller(owner, sender, proc_id, &hosts, || comp.map(str::to_string));
        assert_eq!(got, expected, "{label}");
    }
}

#[test]
fn process_start_ticks_reads_current_process_and_rejects_exited_child() {
    let own = ProcessHandle::open(std::process::id()).unwrap();
    assert!(process_start_ticks(&own).is_some());

    let mut child = std::process::Command::new("true").spawn().unwrap();
    let handle = ProcessHandle::open(child.id()).ok();
    let _ = child.wait();
    if let Some(h) = handle {
        assert_eq!(process_start_ticks(&h), None);
    }
}
