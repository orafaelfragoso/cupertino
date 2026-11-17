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

fn app(entries: &[&str]) -> ClientKind {
    ClientKind::App(entries.iter().map(|e| e.to_string()).collect())
}

#[test]
fn the_nearest_display_client_up_the_chain_is_the_app_or_else_the_parent_executable() {
    // requester 40 <- shell 30 <- terminal 20 <- init 1
    let chain = [(1, 0, 1), (20, 1, 10), (30, 20, 20), (40, 30, 30)];
    let parent_of_40 = Ok(Responsible::Executable { pid: 30, start: 20 });
    let parent_of_30 = Ok(Responsible::Executable { pid: 20, start: 10 });
    for (name, procs, clients, pid, start, expected) in [
        (
            "a terminal is the app of the tools run in it",
            &chain[..],
            vec![(20, app(&["foot", "foot"]))],
            40,
            30,
            Ok(Responsible::Desktop("foot".into())),
        ),
        (
            "the requester is the display client",
            &chain[..2],
            vec![(20, app(&["org.app"]))],
            20,
            10,
            Ok(Responsible::Desktop("org.app".into())),
        ),
        (
            "the nearest client decides",
            &chain[..],
            vec![(20, app(&["outer"])), (30, app(&["inner"]))],
            40,
            30,
            Ok(Responsible::Desktop("inner".into())),
        ),
        (
            "an app without a desktop entry",
            &chain[..3],
            vec![(20, app(&[]))],
            30,
            20,
            parent_of_30.clone(),
        ),
        (
            "an app with several desktop entries",
            &chain[..3],
            vec![(20, app(&["a", "b"]))],
            30,
            20,
            parent_of_30.clone(),
        ),
        (
            "the shell is never the app",
            &chain[..3],
            vec![(20, ClientKind::Shell)],
            30,
            20,
            parent_of_30.clone(),
        ),
        (
            "the X11 bridge is never the app",
            &chain[..3],
            vec![(20, ClientKind::Xwayland)],
            30,
            20,
            parent_of_30.clone(),
        ),
        (
            "no display client",
            &chain[..3],
            vec![],
            30,
            20,
            parent_of_30,
        ),
        (
            // 20's id was reused by a later process, so the chain above 30 is not trusted.
            "the walk stops where a parent started after its child",
            &[(1, 0, 1), (20, 1, 90), (30, 20, 20), (40, 30, 30)][..],
            vec![(20, app(&["late"]))],
            40,
            30,
            parent_of_40,
        ),
        (
            "an unknown process",
            &[][..],
            vec![],
            5,
            1,
            Err(Unidentified::Gone),
        ),
        (
            "a reused process id",
            &[(1, 0, 1), (30, 1, 20)][..],
            vec![],
            30,
            19,
            Err(Unidentified::Reused),
        ),
        (
            "an orphan",
            &[(30, 0, 20)][..],
            vec![],
            30,
            20,
            Err(Unidentified::NoParent),
        ),
        (
            "a parent that is gone",
            &[(30, 7, 20)][..],
            vec![],
            30,
            20,
            Err(Unidentified::NoParent),
        ),
        (
            "a parent id reused by a later process",
            &[(7, 1, 50), (30, 7, 20)][..],
            vec![],
            30,
            20,
            Err(Unidentified::NoParent),
        ),
    ] {
        let clients: HashMap<u32, ClientKind> = clients.into_iter().collect();
        assert_eq!(run(&table(procs), &clients, pid, start), expected, "{name}");
    }
}

#[test]
fn the_walk_gives_up_after_the_ancestry_limit() {
    // A chain of MAX_ANCESTRY + 3 processes whose oldest one is a display client.
    let n = MAX_ANCESTRY as u32 + 3;
    let rows: Vec<(u32, u32, u64)> = (1..=n)
        .map(|pid| (pid, if pid == 1 { 0 } else { pid - 1 }, u64::from(pid)))
        .collect();
    let procs = table(&rows);
    let clients = HashMap::from([(1, app(&["far"]))]);
    assert_eq!(
        run(&procs, &clients, n, u64::from(n)),
        Ok(Responsible::Executable {
            pid: n - 1,
            start: u64::from(n - 1)
        })
    );
    let near = MAX_ANCESTRY as u32;
    assert_eq!(
        run(&procs, &clients, near, u64::from(near)),
        Ok(Responsible::Desktop("far".into())),
        "a client exactly inside the limit is still found"
    );
}
