# Language features and public APIs

## Unsafe code

Use safe code whenever it can implement the required functionality. Necessary
unsafe code must be encapsulated in a safe abstraction and isolated from
business logic, ideally in a separate crate with few dependencies beyond the
standard library.

Every unsafe operation needs a `SAFETY:` comment explaining why it is sound.
Also provide a technical justification explaining why equivalent safe code is
not used. That justification may cover one block or an entire file or build
target. A performance justification needs benchmark evidence.

**Bad — unsafe code without any need for it:**

```rust
fn first(values: &[u8]) -> Option<u8> {
    if values.is_empty() {
        None
    } else {
        // SAFETY: The nonempty slice contains an initialized first byte.
        Some(unsafe { *values.as_ptr() })
    }
}
```

**Good — the same operation uses safe Rust:**

```rust
fn first(values: &[u8]) -> Option<u8> {
    values.first().copied()
}
```

A correct safety comment alone does not establish that using unsafe code was
necessary. Document caller obligations for unsafe APIs as described in
[documentation](documentation.md#examples-and-invariants).

## Stable features

Do not use unstable features by default. Narrow exceptions may follow the
environment's established review process for features essential to embedded
artifacts, an explicitly approved feature list with a clear stabilization path,
or the platform's specific unstable-feature policy. Convenience alone is not
an exception.

**Bad — enabling an unstable feature for an operation available on stable Rust:**

```rust
#![feature(test)]

extern crate test;

fn consume(value: u64) {
    test::black_box(value);
}
```

**Good — use the stable equivalent:**

```rust
use std::hint;

fn consume(value: u64) {
    hint::black_box(value);
}
```

## Complete API sets

Provide `is_empty()` together with `len()`.

**Bad:**

```rust
pub struct Buffer(Vec<u8>);

impl Buffer {
    pub fn len(&self) -> usize {
        self.0.len()
    }
}
```

**Good:**

```rust
pub struct Buffer(Vec<u8>);

impl Buffer {
    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}
```

Getter, setter, and closure-accessor contracts are covered in
[naming](naming.md#getter-and-setter-conventions).

## Hidden items, const promises, and operators

- Treat foreign `#[doc(hidden)]` items as private. Do not access them unless you
  own that code; public visibility alone does not make them supported APIs.
- Mark a public function `const` only if it can remain so, or if you commit to
  helping callers migrate if that promise changes. Removing `const` breaks
  callers that evaluate it in a const context.
- Overload operators for their mathematical or logical meaning, not merely to
  shorten domain-specific method calls.

**Bad — using addition as an unrelated query-building command:**

```rust
let query = table + filter;
```

**Good — a named operation states the relationship:**

```rust
let query = table.filter(filter);
```

Mathematical values and logical operations are appropriate operator-overload
uses; the rule is about preserving the operator's meaning.

## Non-exhaustive public types

Mark public enums used across team boundaries `#[non_exhaustive]` so adding a
variant does not require every caller to change.

**Bad — an evolving public enum promises a closed set of variants:**

```rust
pub enum OutputFormat {
    Text,
    Json,
}
```

**Good:**

```rust
#[non_exhaustive]
pub enum OutputFormat {
    Text,
    Json,
}
```

Exceptions include a fixed ontology, a two-state enum equivalent to a boolean,
or a commitment to fix all affected callers in a coordinated change. Small,
locally used enums may omit the attribute when the breakage scope is limited.

Likewise mark public structs with public fields `#[non_exhaustive]` when they
are used across team boundaries and their fields may evolve.

**Bad — future fields would break external struct literals:**

```rust
pub struct ServerInfo {
    pub name: String,
}
```

**Good — construction can evolve behind the public API:**

```rust
#[non_exhaustive]
pub struct ServerInfo {
    pub name: String,
}
```

Struct exceptions include effectively fixed fields, a commitment to update all
affected callers, an intentional struct-update API such as
`Config { field, ..Default::default() }`, or a small scope of usage.
Non-exhaustive structs cannot be constructed with struct literals outside their
crate; account for the resulting need for another construction API.

## Must-use results

Apply `#[must_use]` to functions when ignoring their return value makes using
them incorrect. This includes pure functions, errors, and other critical
information.

**Bad — an easily ignored return value is the entire effect:**

```rust
pub fn normalized(name: &str) -> String {
    name.trim().to_owned()
}
```

**Good:**

```rust
#[must_use]
pub fn normalized(name: &str) -> String {
    name.trim().to_owned()
}
```

Do not duplicate the annotation on a function whose return type already carries
it, such as `Result`. Put `#[must_use]` on types returned for these purposes,
including error-like types and lazy computations that have no effect until
their results are used.

## Initialization

Do not arrange for user code to run before `main()`, including through static
constructor mechanisms. Initialize through ordinary explicit control flow.

**Bad — a dependency's constructor macro runs initialization before entry:**

```rust
#[ctor::ctor]
fn initialize() {
    setup_logging();
}
```

**Good:**

```rust
fn main() {
    setup_logging();
    run_application();
}
```
