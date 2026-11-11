# Conditional build and platform integration

These policies apply to particular integration models. Do not impose them as
universal restrictions on Cargo projects, async Rust, threads, mutexes, or test
attributes.

## Small library targets

In large integrated builds that adopt fine-grained Rust library targets, prefer
one abstraction per small crate, ideally one source file. This limits build
graph effects and helps parallel compilation and caching.

Decide whether APIs need to share a crate before adding submodules. Larger
foundational libraries and trait-coherence requirements justify larger crates.
A root module may re-export leaf APIs while keeping those leaf modules private.

## Flat target layout

When the integrated build requires a flat Rust library layout, keep a crate's
source files in one directory. Needing subdirectories is a signal to consider
splitting the crate.

**Bad — under a build policy requiring flat source layout:**

```text
parser-target/
  lib.rs
  parser/
    tokens.rs
```

**Good — under that same policy:**

```text
parser-target/
  lib.rs
  parser.rs
  tokens.rs
```

Cargo-built third-party code should retain conventional Cargo layout instead of
inheriting this integrated-build rule. Ordinary Cargo projects need not adopt a
flat source layout merely because another build environment requires it.

## Integrated imports

Use the import mechanism required by an integrated build, including its import
macro where applicable. Under a small, single-purpose first-party crate policy,
directly importing free functions may be permitted instead of module-qualified
calls.

These are exceptions to the ordinary import rules in
[code organization](code-organization.md#free-functions-and-type-names).

## Host execution model

If a host integration excludes async Rust, do not introduce `async` or `.await`.
In hosts that schedule work through sequences, prefer those sequences to
creating standard-library threads.

**Bad — in a host whose work must stay on its task sequences:**

```rust
std::thread::spawn(work);
```

**Good — submit work through that host's existing scheduling API:**

```rust
host_sequence.post_task(work);
```

The example uses a schematic host API. Use the actual integration's scheduler;
do not add a new scheduler dependency merely to follow this example.

## Platform synchronization

Use a platform's mandated mutex primitive when it requires its own
synchronization integration, rather than substituting `std::sync::Mutex` or
`parking_lot`. Do not treat this as a blanket preference for a particular mutex
in programs without that requirement.

Mutex poison handling is covered in
[concurrency](concurrency.md#do-not-reach-through-mutex-poison).

## Test integration

Use the codebase's supported test framework. A platform may require its own test
attribute. A C++ integration may require a Rust-to-C++ test harness so Rust tests
participate in C++ test binaries instead of relying on plain `#[test]` discovery.

**Bad:** Add a standalone Rust test that the integrated test runner never
discovers or runs.

**Good:** Register the test with the existing integrated harness and verify
that the configured runner discovers it.

Plain `#[test]` remains appropriate when it is the codebase's supported test
mechanism. A host-specific test framework is not a requirement to replace it in
an unrelated project.
