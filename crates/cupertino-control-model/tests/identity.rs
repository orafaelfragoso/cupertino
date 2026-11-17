//! The app of a requester is found up its parent chain through the process table and the display clients.

use cupertino_control_model::identity::{
    responsible, ClientKind, ProcessInfo, Responsible, Unidentified, MAX_ANCESTRY,
};
use std::collections::HashMap;

fn table(rows: &[(u32, u32, u64)]) -> HashMap<u32, ProcessInfo> {
    rows.iter()
        .map(|&(pid, parent, start)| (pid, ProcessInfo { parent, start }))
        .collect()
}

fn run(
    procs: &HashMap<u32, ProcessInfo>,
    clients: &HashMap<u32, ClientKind>,
    pid: u32,
    start: u64,
) -> Result<Responsible, Unidentified> {
    responsible(
        pid,
        start,
        |p| procs.get(&p).copied(),
        |p, _| clients.get(&p).cloned(),
    )
}

#[test]
fn a_terminal_window_is_the_app_of_the_tools_run_in_it() {
    // bridge 40 <- shell 30 <- terminal 20 (a display client) <- init 1
    let procs = table(&[(1, 0, 1), (20, 1, 10), (30, 20, 20), (40, 30, 30)]);
    let clients = HashMap::from([(20, ClientKind::App(vec!["foot".into(), "foot".into()]))]);
    assert_eq!(
        run(&procs, &clients, 40, 30),
        Ok(Responsible::Desktop("foot".into()))
    );
}

#[test]
fn the_requester_may_be_the_display_client_itself() {
    let procs = table(&[(1, 0, 1), (20, 1, 10)]);
    let clients = HashMap::from([(20, ClientKind::App(vec!["org.app".into()]))]);
    assert_eq!(
        run(&procs, &clients, 20, 10),
        Ok(Responsible::Desktop("org.app".into()))
    );
}

#[test]
fn the_nearest_display_client_decides() {
    let procs = table(&[(1, 0, 1), (20, 1, 10), (30, 20, 20), (40, 30, 30)]);
    let clients = HashMap::from([
        (20, ClientKind::App(vec!["outer".into()])),
        (30, ClientKind::App(vec!["inner".into()])),
    ]);
    assert_eq!(
        run(&procs, &clients, 40, 30),
        Ok(Responsible::Desktop("inner".into()))
    );
}

#[test]
fn an_app_without_or_with_several_entries_falls_back_to_the_parent_executable() {
    let procs = table(&[(1, 0, 1), (20, 1, 10), (30, 20, 20)]);
    let fallback = Ok(Responsible::Executable { pid: 20, start: 10 });
    for entries in [vec![], vec!["a".to_string(), "b".to_string()]] {
        let clients = HashMap::from([(20, ClientKind::App(entries))]);
        assert_eq!(run(&procs, &clients, 30, 20), fallback);
    }
}

#[test]
fn shells_and_the_x11_bridge_are_never_the_app() {
    let procs = table(&[(1, 0, 1), (20, 1, 10), (30, 20, 20)]);
    for kind in [ClientKind::Shell, ClientKind::Xwayland] {
        let clients = HashMap::from([(20, kind)]);
        assert_eq!(
            run(&procs, &clients, 30, 20),
            Ok(Responsible::Executable { pid: 20, start: 10 })
        );
    }
}

#[test]
fn no_display_client_up_the_chain_falls_back_to_the_parent_executable() {
    let procs = table(&[(1, 0, 1), (20, 1, 10), (30, 20, 20)]);
    assert_eq!(
        run(&procs, &HashMap::new(), 30, 20),
        Ok(Responsible::Executable { pid: 20, start: 10 })
    );
}

#[test]
fn an_unknown_process_is_gone() {
    assert_eq!(
        run(&HashMap::new(), &HashMap::new(), 5, 1),
        Err(Unidentified::Gone)
    );
}

#[test]
fn a_reused_process_id_is_another_process() {
    let procs = table(&[(1, 0, 1), (30, 1, 20)]);
    assert_eq!(
        run(&procs, &HashMap::new(), 30, 19),
        Err(Unidentified::Reused)
    );
}

#[test]
fn a_requester_without_a_live_parent_is_unidentified() {
    let orphan = table(&[(30, 0, 20)]);
    assert_eq!(
        run(&orphan, &HashMap::new(), 30, 20),
        Err(Unidentified::NoParent)
    );
    let parent_gone = table(&[(30, 7, 20)]);
    assert_eq!(
        run(&parent_gone, &HashMap::new(), 30, 20),
        Err(Unidentified::NoParent)
    );
    // The parent id now names a process that started after the child: it was reused.
    let reused_parent = table(&[(7, 1, 50), (30, 7, 20)]);
    assert_eq!(
        run(&reused_parent, &HashMap::new(), 30, 20),
        Err(Unidentified::NoParent)
    );
}

#[test]
fn the_walk_stops_where_a_parent_started_after_its_child() {
    // 20 is a client but its id was reused by a later process, so the chain above 30 is not trusted.
    let procs = table(&[(1, 0, 1), (20, 1, 90), (30, 20, 20), (40, 30, 30)]);
    let clients = HashMap::from([(20, ClientKind::App(vec!["late".into()]))]);
    assert_eq!(
        run(&procs, &clients, 40, 30),
        Ok(Responsible::Executable { pid: 30, start: 20 })
    );
}

#[test]
fn the_walk_gives_up_after_the_ancestry_limit() {
    // A chain of MAX_ANCESTRY + 3 processes whose oldest one is a display client.
    let n = MAX_ANCESTRY as u32 + 3;
    let rows: Vec<(u32, u32, u64)> = (1..=n)
        .map(|pid| (pid, if pid == 1 { 0 } else { pid - 1 }, u64::from(pid)))
        .collect();
    let procs = table(&rows);
    let clients = HashMap::from([(1, ClientKind::App(vec!["far".into()]))]);
    assert_eq!(
        run(&procs, &clients, n, u64::from(n)),
        Ok(Responsible::Executable {
            pid: n - 1,
            start: u64::from(n - 1)
        })
    );
    // One that is exactly inside the limit is still found.
    let near = MAX_ANCESTRY as u32;
    assert_eq!(
        run(&procs, &clients, near, u64::from(near)),
        Ok(Responsible::Desktop("far".into()))
    );
}
