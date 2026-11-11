# Traits

## Implement traits when they are useful

Low-level libraries should anticipate useful trait implementations; application
code should add them as needed.

Implement a standard-library trait when its purpose and contract match an
operation. An inherent method may coexist with the trait implementation for
discoverability.

**Bad — duplicating a standard parsing contract with an inherent method only:**

```rust
struct Port(u16);

impl Port {
    fn from_str(text: &str) -> Result<Self, std::num::ParseIntError> {
        text.parse().map(Self)
    }
}
```

**Good — callers can also use ordinary parsing APIs:**

```rust
use std::num::ParseIntError;
use std::str::FromStr;

struct Port(u16);

impl FromStr for Port {
    type Err = ParseIntError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        text.parse().map(Self)
    }
}
```

## Derive equivalent implementations

Prefer `#[derive(...)]` over handwritten implementations with the same behavior.

**Bad:**

```rust
struct Sequence(u64);

impl Clone for Sequence {
    fn clone(&self) -> Self {
        Self(self.0)
    }
}
```

**Good:**

```rust
#[derive(Clone)]
struct Sequence(u64);
```

## Generics versus dynamic dispatch

Library parameters should favor generics (`T: Trait` or `impl Trait`) over
`dyn Trait`, except when code size is a significant concern. Generics avoid
virtual dispatch and allow optimization; not every trait is dynamically
dispatchable.

**Bad — unnecessary dynamic dispatch in a reusable helper:**

```rust
use std::fmt::Display;

fn render(value: &dyn Display) -> String {
    format!("{value}")
}
```

**Good:**

```rust
use std::fmt::Display;

fn render(value: impl Display) -> String {
    format!("{value}")
}
```

## Traits versus inherent implementations

Define a trait to enable generic programming, accept or return `impl Trait` or
`dyn Trait`, or provide a justified extension trait. Otherwise use an inherent
implementation. Do not create a trait merely to organize methods.

**Bad — a trait with no abstraction or extension purpose:**

```rust
struct Counter(u64);

trait CounterHelpers {
    fn is_zero(&self) -> bool;
}

impl CounterHelpers for Counter {
    fn is_zero(&self) -> bool {
        self.0 == 0
    }
}
```

**Good:**

```rust
struct Counter(u64);

impl Counter {
    fn is_zero(&self) -> bool {
        self.0 == 0
    }
}
```

## Extension traits

An extension trait adds methods to a foreign type or to every implementor of a
foreign trait. Its purpose is not generic bounds or dynamic dispatch. Give its
name an `Ext` suffix.

**Bad:**

```rust
trait ForeignTypeHelpers {
    fn additional_method(&self);
}
```

**Good:**

```rust
trait ForeignTypeExt {
    fn additional_method(&self);
}
```

The team owning the original type should generally own its extension traits to
avoid method-name conflicts. Widely useful extensions in foundational libraries
or frameworks are an exception. A generic input interface implemented for
foreign types is not an extension trait merely because those types are foreign.

## Common trait contracts

- For a type with public fields, consider traits implemented by every field.
- Implement `Copy` when the type can reasonably be expected to remain `Copy`.
  Error types should be `Copy` whenever their contents permit it; an owned
  `String`, for example, prevents this.
- Most types should implement `Clone` unless they represent an external resource.
- Implement `Default` for a meaningful default state or a zero-argument `new()`.
- Use `PartialEq` for equality and add `Eq` only when its stronger requirements,
  including reflexivity, hold.
- Use `PartialOrd` for ordering and add `Ord` only when a total ordering is
  valid. Do not promise stronger contracts without reason.
- Implement `Hash` and `Eq` for likely map keys or cached keys. Add `Ord` and its
  prerequisites when a clear total ordering also exists.

## Constructors and defaults

Pair an important `From` implementation used for primary construction with an
explicit inherent constructor such as `new()`, `new_from_foo()`, or `from_foo()`.
This helps callers discover how to construct the type.

An all-public-fields options struct should implement `Default`, allowing callers
to specify only the options they need.

**Bad — every caller must spell out unchanged defaults:**

```rust
pub struct Options {
    pub verbose: bool,
    pub limit: usize,
}

let options = Options { verbose: true, limit: 0 };
```

**Good — zero and false are meaningful defaults for this API:**

```rust
#[derive(Default)]
pub struct Options {
    pub verbose: bool,
    pub limit: usize,
}

let options = Options {
    verbose: true,
    ..Default::default()
};
```

Choose actual semantic defaults rather than deriving inappropriate ones. A
struct-update API is an exception to some non-exhaustive-struct guidance; see
[language features and APIs](language-features-and-apis.md#non-exhaustive-public-types).
