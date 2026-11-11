# Code organization

## Modules and source files

Name a leaf module and its file after its primary user-defined item, such as
`Widget` in `widget.rs`. A module without one primary abstraction may use another
descriptive name.

Keep non-leaf modules primarily to module declarations and re-exports. A small
amount of code tying things together is acceptable, but substantial APIs belong
in leaf modules. Inline test modules and cross-language binding modules do not
prevent their containing implementation module from being a leaf.

**Bad — substantial implementation mixed with module organization:**

```rust
// lib.rs
mod settings;

pub fn normalize_name(name: &str) -> String {
    name.trim().to_owned()
}
```

**Good — the root exposes a leaf module's implementation:**

```rust
// lib.rs
mod name;
mod settings;

pub use name::normalize_name;
```

```rust
// name.rs
pub fn normalize_name(name: &str) -> String {
    name.trim().to_owned()
}
```

This makes both the public surface and the location of its implementation easy
to find. Small-crate and flat-layout requirements are conditional integration
policies; see [platform integration](platform-integration.md).

## Glob imports

Avoid glob imports and avoid defining preludes. Explicit imports reveal where a
name comes from and make automated refactoring easier.

**Bad:**

```rust
use std::collections::*;
```

**Good:**

```rust
use std::collections::{HashMap, HashSet};
```

Permitted exceptions:

- `use super::*` inside an inline module.
- Importing enum variants in that enum's methods or functions primarily working
  with that enum.
- `pub use submodule::*` in a non-leaf module.
- Privately merging C++ namespaces that are spread across Rust crates. Use the
  technique only for this merge and keep the resulting module private.
- A third-party prelude when its reviewer judges the import appropriate.

## One source module per import

Each `use` statement should import items from one module. Do not put additional
`::` paths inside its braces. This corresponds to
`imports_granularity = "Module"` where the formatter supports it.

**Bad:**

```rust
use foo::{Foo, bar::Bar};
```

**Good:**

```rust
use foo::bar::{Bar, Baz};
use foo::Foo;
```

## Trait import scope

Import a trait inside the function using its methods unless those methods are
used extensively throughout the file.

**Bad — a trait imported for one local use:**

```rust
use std::str::FromStr;

fn parse_byte(text: &str) -> Result<u8, std::num::ParseIntError> {
    u8::from_str(text)
}
```

**Good:**

```rust
fn parse_byte(text: &str) -> Result<u8, std::num::ParseIntError> {
    use std::str::FromStr;

    u8::from_str(text)
}
```

## Free functions and type names

Normally qualify external free functions with their module. Import types and
traits for unqualified use.

**Bad:**

```rust
use std::fs::read_to_string;

fn load(path: &std::path::Path) -> std::io::Result<String> {
    read_to_string(path)
}
```

**Good:**

```rust
use std::fs;
use std::io;
use std::path::Path;

fn load(path: &Path) -> io::Result<String> {
    fs::read_to_string(path)
}
```

Free functions may be imported directly when their names are sufficiently
distinctive, when module names collide (for example, shared C++ namespaces), or
under an integrated build's small, single-purpose crate policy.

Qualified or aliased type names are appropriate for ambiguous names such as
`io::Error`, large related type families, macros or generated code where imports
are counterproductive, and dependency-heavy files where a large import block
would add little benefit.
