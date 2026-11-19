//! The limits of `cupertino_transaction_v1` that the server checks and clients check before they send, as
//! `protocols/cupertino-transaction-v1.xml` and `docs/architecture/shell-host.md` document them.

/// Extra milliseconds added to one output frame interval for the default transaction deadline when `set_deadline` is
/// not sent.
pub const DEFAULT_DEADLINE_EXTRA_MS: u32 = 4;
/// Longest deadline a creator may ask for with `set_deadline`, in milliseconds: higher values are clamped here.
pub const MAX_DEADLINE_MS: u32 = 100;
/// Milliseconds after `create` or `join` after which a participant that has not sent `submit` is treated as submitted.
pub const AUTO_SUBMIT_MS: u32 = 100;
/// Milliseconds after `submit` that a staged surface has to commit before leaving the participant's part.
pub const COMMIT_GRACE_MS: u32 = 100;
/// Milliseconds a `join` arriving before the creator's `create` waits for the creator before starting standalone.
pub const JOIN_WAIT_MS: u32 = 100;
/// Seconds the server remembers an ended transaction so a late `join` applies with animations at their end.
pub const ENDED_RETENTION_SECS: u64 = 10;
/// Most unended transactions one connection may take part in at once (created and joined together).
pub const MAX_ACTIVE_TRANSACTIONS: usize = 16;
/// Most open window-change brackets one connection may hold at once.
pub const MAX_OPEN_BRACKETS: usize = 1;
/// Most surfaces one participant may stage in a single transaction.
pub const MAX_STAGED_SURFACES: usize = 32;
/// Most expected components one transaction creator may name with `expect`.
pub const MAX_EXPECTED_COMPONENTS: usize = 16;
/// Most holes (`cupertino_hole_v1`) one connection may hold at once.
pub const MAX_HOLES_PER_CONNECTION: usize = 256;
/// Most holes on one parent surface (bounded by `cupertino_layers_v1`'s per-parent layer cap).
pub const MAX_HOLES_PER_PARENT: usize = 64;
/// Shortest and longest settle spring response in seconds for `bind_gesture`: outside is `invalid_spring`.
pub const SPRING_RESPONSE: (f64, f64) = (0.01, 10.0);
/// Lowest and highest settle spring damping ratio for `bind_gesture`: outside is `invalid_spring`.
pub const SPRING_DAMPING: (f64, f64) = (0.05, 4.0);
