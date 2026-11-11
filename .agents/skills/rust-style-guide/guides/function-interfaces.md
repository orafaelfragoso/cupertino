# Function inputs, outputs, and errors

## Accept flexible borrowed inputs

Borrow the underlying data rather than a particular owning wrapper. This lets
callers use different storage without copying.

| Prefer | Instead of |
| --- | --- |
| `&T` | `&Box<T>` |
| `&[T]` | `&Vec<T>` |
| `&str` | `&String` |
| `Option<&T>` | `&Option<T>` |

Noncontiguous text structures such as ropes may require their own interfaces.

**Bad:**

```rust
fn total_bytes(names: &Vec<String>) -> usize {
    names.iter().map(String::len).sum()
}
```

**Good:**

```rust
fn total_bytes(names: &[String]) -> usize {
    names.iter().map(String::len).sum()
}
```

## Iterator parameters

Highly reusable libraries should accept an iterator or `impl IntoIterator` when
they only need to iterate. Application code may accept a slice when it is
simpler and broader flexibility has no clear benefit.

**Bad — a reusable API needlessly consumes a particular collection:**

```rust
fn total(values: Vec<u64>) -> u64 {
    values.into_iter().sum()
}
```

**Good — callers choose their collection or iterator:**

```rust
fn total(values: impl IntoIterator<Item = u64>) -> u64 {
    values.into_iter().sum()
}
```

## Iterator return values

Prefer returning a concrete iterator or `impl Iterator` over allocating a
vector when this improves reusability or avoids unnecessary work.

**Bad — forcing allocation when callers may only iterate:**

```rust
fn nonempty(lines: &[String]) -> Vec<&str> {
    lines.iter().map(String::as_str).filter(|line| !line.is_empty()).collect()
}
```

**Good:**

```rust
fn nonempty(lines: &[String]) -> impl Iterator<Item = &str> {
    lines
        .iter()
        .map(String::as_str)
        .filter(|line| !line.is_empty())
}
```

A vector is appropriate when most callers need one, the implementation
naturally constructs one, callers need random access, a loop is clearer and
allocation is insignificant, creating an owned collection is the purpose, or
the vector already exists. Application code may return a vector for simplicity
when iterator flexibility or allocation savings are not clearly useful.

## `Into` parameters

Consider `impl Into<T>` for library parameters when accepting several input
types materially improves call-site ergonomics. Balance that benefit against
generic API complexity, inference difficulty, and generated code size.
Application code should almost never introduce it merely to save `.into()` at
the call site.

**Bad — an application helper generalizes an input it only reads:**

```rust
fn print_name(name: impl Into<String>) {
    println!("{}", name.into());
}
```

**Good:**

```rust
fn print_name(name: &str) {
    println!("{name}");
}
```

For a reusable constructor that owns its input, accepting `impl Into<String>`
may instead be a worthwhile ergonomic choice.

## Prefer meaningful enums to unclear booleans

**Bad — the call does not explain what `true` means:**

```rust
set_visibility(true);
```

**Good:**

```rust
enum Visibility {
    Visible,
    Hidden,
}

set_visibility(Visibility::Visible);
```

Use an enum when an argument's purpose would otherwise be unclear. If a boolean
is appropriate, an inline argument comment can clarify its meaning.

## Choose the right error contract

- Use `Option` for simple absence or anticipated failure whose cause is obvious
  and where more detail would not help, such as checked arithmetic overflow or
  an expected parsing mismatch.
- Use `Result` for recoverable failures where context helps the caller respond,
  especially external failures such as I/O. Preserve useful distinctions such
  as a missing file versus permission denied.
- Use `panic!` for bugs or violated invariants: repair the missing check or
  broken precondition instead of treating the defect as an ordinary outcome.
  A codebase may require `Result` even for invariant failures.

**Bad — ordinary I/O failure is turned into a panic:**

```rust
use std::fs;
use std::path::Path;

fn load(path: &Path) -> String {
    fs::read_to_string(path).expect("could not read input")
}
```

**Good — the caller receives the recoverable failure:**

```rust
use std::fs;
use std::io;
use std::path::Path;

fn load(path: &Path) -> io::Result<String> {
    fs::read_to_string(path)
}
```

**Good — there is no useful additional error detail:**

```rust
fn checked_sum(left: u32, right: u32) -> Option<u32> {
    left.checked_add(right)
}
```

Developer-facing binaries may use panics for non-fatal errors, and tests may
panic for assertions. Recovery from panics has a narrow, separate exception;
see [implementation](implementation.md#panic-recovery).
