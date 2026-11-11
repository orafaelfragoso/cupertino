# Review checklist

Read once per review and apply the relevant questions while tracing the selected flows. Find evidence before reporting a problem. Judge against the project's documented rules and scale; do not demand a particular architecture, framework or pattern beyond them.

| Category | Concrete checks and useful evidence |
| --- | --- |
| Correctness | Compare behavior with the docs and the crate README. Check state machines (focus, pointer and keyboard grabs, popups, lock, gestures), boundary values (zero or huge sizes, fractional scales, rounding), coordinate spaces (logical, physical, buffer, output-relative) and serials. Cite the rule and the failing path. |
| Architecture and layering | Check dependencies against the layer table in `docs/src/architecture.md` and the exceptions listed in `cupertino-layers`. Smithay only in `cupertino-server` and `cupertino-seat`; model crates pure and display-free; the server does only window-server work; privileges stay in the daemons. Explain a concrete consequence. |
| Code quality | Check confusing control flow, excessive complexity, duplication and unused code. Show a defect risk or a specific maintenance cost. |
| Patterns and consistency | Compare equivalent implementations. Settings go through `cupertino-defaults` (atomic writes, coalesced change notification); standard protocols come before private ones; Kit's semantic model is reused; constants are named design values with a comment. Do not prescribe patterns for their own sake. |
| Security and trust boundaries | Trace untrusted Wayland requests (object lifetimes, roles, resource limits, buffer sizes and offsets, file descriptors), access to shell-only protocols, D-Bus caller authorization by peer credentials and active seat user, PAM and the lock screen, desktop control consent, path handling (symlinks, `..`, XDG directories), process spawning from desktop entries, and the soundness of every `unsafe` block with its safety comment. Check upstream safeguards before declaring a missing control. |
| Privacy | Follow sensitive data through screen capture and screencast consent, clipboard, notification content, thumbnails and caches, recent files, and logs. Passwords, network keys and pairing keys never reach logs; key stores have tight permissions. Mark unseen policy as unknown. |
| State and persistence | Check settings files, key stores and caches: atomic writes, permissions and ownership, format changes and their migration, concurrent writers, and recovery from a corrupt or missing file. |
| Protocols and IPC | Check Wayland protocol conformance (versions, protocol errors versus disconnects, destructors, serials), versioning of private protocols, D-Bus interfaces and names against `docs/architecture/contracts.yaml`, the C ABI header version, compatibility with older clients and peers, and error propagation. Inspect both ends of an interface. |
| Reliability | Check panics and `unwrap` on external input, lock poisoning, cancellation, timeouts on D-Bus and kernel calls, retry limits and backoff, the supervisor's restart limit, cleanup of file descriptors, GPU resources and child processes, hotplug, suspend and resume, and peers that disconnect mid-operation. Explain a reachable failure sequence. |
| Performance | Inspect per-frame allocation and work, blocking I/O on the compositor event loop, damage tracking, backdrop and blur passes, unbounded queues and caches, and idle wakeups. Separate measured impact from workload-dependent concerns. |
| UI, input and accessibility | Where UI is in scope, check interaction states, keyboard focus and input routing, light and dark appearance, scales, reduced motion and transparency settings, AT-SPI semantics and the Kit semantic tree used for automation. Separate source inspection from rendered, captured or assistive-technology verification. |
| Testing | Check meaningful assertions on critical success and failure paths, model tests without a display, golden and capture coverage of visual changes, fixture isolation (built-in font, cleared environment, no real `HOME`, no real session bus), timing sensitivity and the two-minute limit. Passing tests are not adequate coverage. |
| Dependencies | Check exact locked versions against current advisories, `deny.toml` (licences, bans, sources), the `MPL-2.0` closure of `cupertino-wayland-client`, patches to vendored crates in `third-party/`, unused dependencies, and that no third-party control framework or MCP SDK enters the tree. An outdated version alone is not a vulnerability. |
| Build and packaging | Check feature selection (`app`), compile-time `CUPERTINO_*` paths, new build-script environment variables registered in `PROJECT_BUILD_VARIABLES` in `ci/worktree-build.py`, CI gates, the package `check()` environment, unit files, install, upgrade and rollback, and file ownership and permissions. Inspect the actual configuration. |
| Operations and diagnostics | Check that logs and diagnostic events are actionable and free of secrets, that failures are visible to the person, and that update and rollback paths match `docs/src/updates-and-rollback.md`. Mark live behavior unknown when only source was seen. |
| Documentation | Check that crate READMEs, `docs/src`, the contracts registry and the changelog match the code. Report a concrete gap that blocks or misleads work. |
| Source policy | `python3 -I ci/gate.py .` passes: no forbidden file types or names, no wording the policy rejects. Never weaken the gate to make a change pass. |

## Coverage meanings

- **Assessed in scope:** enough evidence was inspected to answer the relevant questions for the named flow or crate; this does not imply whole-repository coverage.
- **Partial:** only part of the relevant path was inspected, or a required runtime or configuration signal was missing.
- **Not reviewed:** no meaningful assessment in this pass.
- **Not applicable:** the category does not apply to the selected scope, with the reason.

For claims that depend on external facts, such as advisories or upstream protocol behavior, use current primary sources or scanner output. Report what remains unknown rather than assuming it is safe or defective.
