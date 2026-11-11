# Naming and accessors

## Method and conversion names

Follow standard-library names and receiver conventions:

- `as_` for cheap borrowed views.
- `to_` for conversions that retain the source and may do work.
- `into_` for conversions that consume the source.

**Bad — the name promises a consuming conversion but the method clones:**

```rust
struct Packet(Vec<u8>);

impl Packet {
    fn into_bytes(&self) -> Vec<u8> {
        self.0.clone()
    }
}
```

**Good — the receiver and operation match the name:**

```rust
struct Packet(Vec<u8>);

impl Packet {
    fn into_bytes(self) -> Vec<u8> {
        self.0
    }
}
```

If retaining the source and making a copy is the intended contract, a name such
as `to_bytes` is appropriate instead.

## Lifetimes

Give lifetimes descriptive names, especially on types. Anonymous `'_` is
appropriate where it works. Extremely generic abstractions may use short names;
up to two local single-letter lifetimes are fine for input/output relationships
or higher-ranked bounds. Prefer descriptive names when more are needed.

**Bad — three unrelated type lifetimes are hard to follow:**

```rust
struct ParseContext<'a, 'b, 'c> {
    input: &'a str,
    tokens: &'b [String],
    source_name: &'c str,
}
```

**Good:**

```rust
struct ParseContext<'input, 'tokens, 'source> {
    input: &'input str,
    tokens: &'tokens [String],
    source_name: &'source str,
}
```

## Generic parameters

Choose short parameter names related to what they represent, ideally their
principal trait bound. Single-letter names are fine when informative. `T` and
`U` fit broadly generic, relatively unconstrained APIs such as collections.

**Bad — an arbitrary name obscures a specific role:**

```rust
fn read_all<X: std::io::Read>(mut source: X) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    source.read_to_end(&mut bytes)?;
    Ok(bytes)
}
```

**Good:**

```rust
use std::io;
use std::io::Read;

fn read_all<R: Read>(mut source: R) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    source.read_to_end(&mut bytes)?;
    Ok(bytes)
}
```

## Constructors, conversions, and extension traits

- Name constructors `new()` or `new_with_foo()` when distinguishing variants.
- Use `try_new()` for a constructor returning `Result`.
- Use `from_foo()` for value-to-value conversions, with a corresponding `From`
  implementation when its contract fits.
- Pair an important `From` implementation used for primary construction with
  an explicit inherent constructor for discoverability.
- Give extension traits an `Ext` suffix, such as `ForeignTypeExt`.

**Bad — a fallible constructor is not named as one:**

```rust
struct Port(u16);

impl Port {
    fn new(text: &str) -> Result<Self, std::num::ParseIntError> {
        text.parse().map(Self)
    }
}
```

**Good:**

```rust
use std::num::ParseIntError;

struct Port(u16);

impl Port {
    fn try_new(text: &str) -> Result<Self, ParseIntError> {
        text.parse().map(Self)
    }
}
```

## Abbreviations

Avoid unfamiliar abbreviations, acronyms, initialisms, and spellings formed by
deleting letters. Common publicly established abbreviations and names that an
outside reader understands, such as `IpAddr`, are acceptable. Prefer the full
word when uncertain.

**Bad:**

```rust
let usr_cfg = load_config();
```

**Good:**

```rust
let user_config = load_config();
```

## Getter and setter conventions

| Operation | Convention |
| --- | --- |
| Borrow or copy a `Copy` value | `foo()` returning `&T` or `T` |
| Mutably borrow a value | `foo_mut()` returning `&mut T` |
| Remove a value, leaving `None` or another default | `take_foo()` returning the old value |
| Set a value | `set_foo(value: T)` |
| Set a value and return the old one | `replace_foo(value: T)` |
| Access within a closure | `with_foo(...)` or `with_mut_foo(...)` |

Closure accessors are useful when interior mutability must limit the borrow's
scope, or mutation needs follow-up validation and replacing the whole value is
expensive or impossible. Prefer them to exposing an implementation-specific
guard in these situations.

**Bad — the public accessor exposes the locking implementation:**

```rust
use std::sync::{Mutex, MutexGuard};

struct Counter {
    value: Mutex<u64>,
}

impl Counter {
    pub fn value(&self) -> MutexGuard<'_, u64> {
        self.value.lock().expect("counter state must not be poisoned")
    }
}
```

**Good — access stays scoped and the closure's result is forwarded:**

```rust
use std::sync::Mutex;

struct Counter {
    value: Mutex<u64>,
}

impl Counter {
    pub fn with_value<R>(&self, operation: impl FnOnce(&u64) -> R) -> R {
        let value = self
            .value
            .lock()
            .expect("counter state must not be poisoned");
        operation(&value)
    }
}
```

Allow the closure to return a value when useful. Use `with_foo` and
`with_mut_foo` when providing both read-only and mutable versions. A sole mutable
accessor may be named `with_foo`.
