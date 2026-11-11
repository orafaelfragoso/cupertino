# Concurrency

## Do not reach through mutex poison

Do not recover access to a poisoned mutex's contents by reaching through the
poison error. The contents may no longer satisfy their invariants.

**Bad — treating poison as permission to continue:**

```rust
let state = shared_state.lock().unwrap_or_else(|error| error.into_inner());
use_state(&state);
```

**Good — a broken invariant stops the operation:**

```rust
let state = shared_state
    .lock()
    .expect("shared state invariants may have been broken by a previous panic");
use_state(&state);
```

Propagating an appropriate failure without accessing the contents is also a
valid way to stop the operation. The rule does not require a panic in every API.

## Respect the host's concurrency model

Async support, thread creation, and mutex primitives can be platform policies.
They are not universal restrictions on Rust.

- Do not introduce `async` or `.await` in a host integration that excludes them.
- Prefer host task sequences over new standard-library threads when sequences
  are the host's required execution model.
- Use a platform's mandated mutex primitive where it requires its own
  synchronization integration, rather than substituting `std::sync::Mutex` or
  `parking_lot`.

These conditions and related build/test policies are collected in
[platform integration](platform-integration.md). A normal Cargo application
does not inherit another environment's async, thread, or mutex restrictions.
