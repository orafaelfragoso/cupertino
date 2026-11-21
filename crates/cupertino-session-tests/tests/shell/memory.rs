//! How the restart tests of the shell's components measure what their restarts leave in the host: each starts the
//! host with [`steady_allocator`], crashes its component [`RESTARTS`] times, and has [`HostMemory`] measure the
//! host's anonymous memory after every restart from [`WARM_RESTARTS`] on. Once the last instance runs, no bus
//! connection of an ended one may stay open (Kit finishes each on a thread of its own, [`ENDED_CONNECTION`]), and the
//! least the host held over the last [`FLOOR_RESTARTS`] restarts must not exceed the least it held over the first of
//! them by more than [`ANONYMOUS_GROWTH_KIB`]. An instance measured just as it started holds more or less of what its
//! start needs (up to 2 MiB in the launcher's host), which the floor leaves out; a leak, which every ended instance adds
//! to, raises the floor.
//!
//! What the allocator keeps is not what the instances keep. glibc gives each thread an arena of its own, up to eight
//! per core, and returns an arena's freed memory to the kernel only from its top: with many arenas, the threads of each
//! new instance land on arenas of their own choosing, each of which keeps what its threads once needed, so the host's
//! anonymous memory drifts by megabytes with how the threads are spread, the more so under load, while the live heap
//! stays where it was (heaptrack: 82 KiB over forty restarts of the menu bar whose anonymous memory grew by 3.6
//! MiB). With one arena, which every thread shares, the next instance takes up what the last one freed, and the
//! measure is left with what the host holds, which an ended instance's leftovers then grow: a leaked connection, its
//! thread and what its tasks hold.

use super::rig::Rig;

/// Crashes injected into the component, after which each restarts alone.
pub const RESTARTS: u32 = 50;
/// Crashes after which the server gives a component up (D16: five within a minute); a restart the person asks for
/// then starts it again and forgets them.
pub const CRASHES_IN_A_ROW: u32 = 5;
/// Restarts after which the host's memory is measured the first time: the host's first allocations of a component
/// (its caches, the allocator's arena, the I/O pool's threads) are made by then.
pub const WARM_RESTARTS: u32 = 10;
/// How many restarts' measurements make a floor, at the start of the measure and at its end: enough that one of them
/// mostly finds the instance without what its start needs (the launcher's host measures 2 MiB less in about one
/// measurement of five).
const FLOOR_RESTARTS: usize = 10;
/// How much the host's anonymous memory may grow between the two floors, in KiB. Anonymous memory is the heap and the
/// stack pages threads touched; neither a stack's untouched pages nor the windows' shared-memory buffers count. What
/// the bound catches is heap the ended instances leave behind: a leaked bus connection with what its tasks hold (its
/// thread, [`ENDED_CONNECTION`], is caught on its own), a model or a cache entry each instance adds, at about 100 KiB a
/// restart over the forty measured.
const ANONYMOUS_GROWTH_KIB: u64 = 4 * 1024;

/// The name of the thread on which Kit finishes a bus connection whose runtime ended (`cupertino_kit::bus`): it ends
/// once the connection's last clone is dropped, so one that stays is a connection an ended instance left behind, with
/// whatever its tasks hold.
const ENDED_CONNECTION: &str = "kit-bus-ended";

/// The arenas of glibc's allocator in the host: one, which every thread of every instance shares.
const ARENAS: &str = "1";

/// Have the hosts this test starts from now on run glibc's allocator with one arena ([`ARENAS`]). Called before the
/// server starts its first host, which takes its environment.
pub fn steady_allocator() {
    std::env::set_var("MALLOC_ARENA_MAX", ARENAS);
}

/// The host's anonymous memory over the restarts: what it measured after each one from [`WARM_RESTARTS`] on.
pub struct HostMemory {
    pid: u32,
    /// The restart and the host's anonymous memory after it, in KiB.
    after: Vec<(u32, u64)>,
}

impl HostMemory {
    /// The memory of the host process `pid`.
    pub fn of(pid: u32) -> Self {
        Self {
            pid,
            after: Vec::new(),
        }
    }

    /// The instance that restart `restart` started runs: measured from [`WARM_RESTARTS`] on.
    pub fn restarted(&mut self, restart: u32) {
        if restart >= WARM_RESTARTS {
            self.after.push((restart, anonymous(self.pid)));
        }
    }

    /// After the last restart: the host kept nothing of the ended instances. Turns until the threads that finish
    /// their bus connections ended, then its anonymous memory over the last [`FLOOR_RESTARTS`] restarts, at its
    /// least, grew by no more than [`ANONYMOUS_GROWTH_KIB`] since its least over the first.
    pub fn assert_nothing_kept(&mut self, rig: &mut Rig) {
        let pid = self.pid;
        rig.until(
            "the bus connections of the ended instances are closed",
            |_| ended_connections(pid) == 0,
        );
        self.after.push((RESTARTS, anonymous(self.pid)));
        assert!(
            self.after.len() >= 2 * FLOOR_RESTARTS,
            "the host was measured after {} restarts only",
            self.after.len()
        );
        let floor = |samples: &[(u32, u64)]| samples.iter().map(|&(_, kib)| kib).min().unwrap();
        let warm = floor(&self.after[..FLOOR_RESTARTS]);
        let last = floor(&self.after[self.after.len() - FLOOR_RESTARTS..]);
        let series: Vec<String> = self
            .after
            .iter()
            .map(|(restart, kib)| format!("{restart}: {kib}"))
            .collect();
        eprintln!(
            "host anonymous memory at its least over the first {FLOOR_RESTARTS} restarts from {WARM_RESTARTS}: {warm} \
             KiB; over the last {FLOOR_RESTARTS}: {last} KiB; after each (KiB): {}",
            series.join(", ")
        );
        assert!(
            last <= warm + ANONYMOUS_GROWTH_KIB,
            "the host's anonymous memory grew from {warm} KiB to {last} KiB over {} restarts; after each (KiB): {}",
            RESTARTS - WARM_RESTARTS,
            series.join(", ")
        );
    }
}

/// How many threads of the host process `pid` finish a bus connection of an ended instance ([`ENDED_CONNECTION`]).
fn ended_connections(pid: u32) -> usize {
    let threads = std::fs::read_dir(format!("/proc/{pid}/task")).expect("the host's threads");
    threads
        .filter_map(Result::ok)
        .filter_map(|thread| std::fs::read_to_string(thread.path().join("comm")).ok())
        .filter(|name| name.trim_end() == ENDED_CONNECTION)
        .count()
}

/// The host process's anonymous memory (its heap and stacks, which a leak grows), in KiB.
fn anonymous(pid: u32) -> u64 {
    let rollup =
        std::fs::read_to_string(format!("/proc/{pid}/smaps_rollup")).expect("the host's memory");
    rollup
        .lines()
        .find_map(|line| line.strip_prefix("Anonymous:"))
        .and_then(|kib| kib.trim().trim_end_matches("kB").trim().parse().ok())
        .expect("the host's anonymous memory")
}
