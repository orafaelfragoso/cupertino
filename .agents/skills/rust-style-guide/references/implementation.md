# Implementation

## Make inferred types understandable

Add enough explicit types for someone familiar with the codebase to follow the
inference. Prefer annotations on variable declarations over turbofish syntax.
When the compiler requires a turbofish, `_` inference markers are acceptable.
Especially consider annotations for generic return types such as `.into()` or
`.parse()` and the results of complex iterator chains.

**Bad — the result's type is left implicit across statements:**

```rust
let port = text.parse().expect("configured port must be numeric");
configure_port(port);
```

**Good:**

```rust
let port: u16 = text.parse().expect("configured port must be numeric");
configure_port(port);
```

## Match instead of checking and unwrapping

Use pattern matching to prove the branch's precondition to the compiler instead
of relying on an earlier check to justify a potentially panicking operation.

**Bad:**

```rust
if option.is_some() {
    use_value(option.unwrap());
}
```

**Good:**

```rust
if let Some(value) = option {
    use_value(value);
}
```

## Prefer `expect` for necessary invariant panics

When an invariant cannot be expressed in the type system and a panic is
necessary, prefer `expect` with the reason the invariant holds. Ordinary
recoverable failures should still use their error contract rather than panic.

**Bad:**

```rust
let newest = history.last().unwrap();
```

**Good — this data structure requires an initial entry:**

```rust
let newest = history
    .last()
    .expect("history is initialized with an initial entry");
```

## Match non-exhaustive enums deliberately

Prefer listing all known variants when matching a non-exhaustive enum. Name
known variants that need no action and retain the wildcard for future variants,
so readers can see which cases were considered.

**Bad — an ignored known variant is invisible to a reviewer:**

```rust
match shape {
    Shape::Circle => draw_circle(),
    Shape::Square => draw_square(),
    _ => {}
}
```

**Good — all currently known variants are accounted for:**

```rust
match shape {
    Shape::Circle => draw_circle(),
    Shape::Square => draw_square(),
    Shape::Triangle => {}
    _ => {}
}
```

When only selected variants matter, prefer `if let` or `let ... else`:

```rust
if let Shape::Circle = shape {
    draw_circle();
}
```

A `match` is still acceptable when it expresses that selective check more
clearly. This rule does not require verbose matches for every single-variant
test.

## Assertion context

Consider context strings, especially when a reason would otherwise need a
comment, users may see the failure, values help explain it, or the message
should distinguish caller misuse from an implementation bug.

**Bad — the message only repeats the condition:**

```rust
assert!(values.is_sorted(), "values should be sorted");
```

**Good — explain why the condition should hold:**

```rust
assert!(
    values.is_sorted(),
    "values are populated from an ordered queue"
);
```

**Good — expose the value needed to diagnose a violation:**

```rust
assert!(refund > 0, "refund must be positive, got {refund}");
```

Prefer the message to a nearby explanation comment so the reason appears in
failure logs. A self-explanatory assertion does not need a redundant message.

## Prefer clear iteration

Avoid long or complex combinator chains, particularly when they mutate external
state or conceal control flow.

**Bad:**

```rust
let mut error_count = 0;
let values: Vec<u32> = inputs
    .iter()
    .map(|text| text.parse::<u32>())
    .map(|result| {
        if result.is_err() {
            error_count += 1;
        }
        result
    })
    .filter_map(Result::ok)
    .collect();
```

**Good:**

```rust
let mut error_count = 0;
let mut values = Vec::with_capacity(inputs.len());

for text in inputs {
    match text.parse::<u32>() {
        Ok(value) => values.push(value),
        Err(_) => error_count += 1,
    }
}
```

Do not build a combinator chain in a `for` header. Assign it to a descriptive
variable before consuming it.

**Bad:**

```rust
for value in values.iter().copied().filter(|value| *value > limit) {
    process(value);
}
```

**Good:**

```rust
let over_limit = values.iter().copied().filter(|value| *value > limit);
for value in over_limit {
    process(value);
}
```

Normally use `for` instead of `Iterator::for_each`; choose `for_each` only with a
clear reason. Short combinators are preferable when they express useful logic
directly rather than rebuilding it in a longer loop.

**Bad — manually recreating a short predicate combinator:**

```rust
let mut exceeded = false;
for &value in values {
    if value > limit {
        exceeded = true;
        break;
    }
}
```

**Good:**

```rust
let exceeded = values.iter().any(|&value| value > limit);
```

## Panic recovery

Do not call `panic::catch_unwind` in first-party code except as a supplementary
defense at critical entry points in a codebase already doing what it can to
avoid panics. Correctness and safety must never depend on that recovery.

The exception requires an environment that can run with `panic=unwind`.
`catch_unwind` cannot recover panics in a `panic=abort` build. Explain why the
exception is justified during review.

**Bad — using a panic boundary as normal I/O error handling:**

```rust
use std::fs;
use std::panic;

let contents = panic::catch_unwind(|| fs::read_to_string(path).unwrap());
```

**Good — use the operation's recoverable error contract:**

```rust
use std::fs;
use std::io;
use std::path::Path;

fn load(path: &Path) -> io::Result<String> {
    fs::read_to_string(path)
}
```

## String construction

Prefer `format!` or `write!` when concatenating more than two strings or
substituting values into a literal.

**Bad:**

```rust
let label = prefix.to_owned() + ": " + name;
let greeting = "Hello, NAME!".replace("NAME", name);
```

**Good:**

```rust
let label = format!("{prefix}: {name}");
let greeting = format!("Hello, {name}!");
```

Use `write!` when writing into an existing destination is appropriate. Prefer
standard format strings to repeated concatenation allocations or an invented
literal-substitution convention.
