# Types

## Newtypes versus aliases

Use a newtype rather than an alias when a type has different semantics, needs
its own API, should hide a representation that may change, or must not be
accidentally interchanged with similar values.

**Bad — aliases do not prevent mixing different identifiers:**

```rust
type UserId = u64;
type GroupId = u64;

fn add_member(user: UserId, group: GroupId) {
    // Both arguments are indistinguishable to the type system.
}

let user: UserId = 12;
let group: GroupId = 34;
add_member(group, user); // Compiles despite the reversed arguments.
```

**Good — distinct meanings have distinct types:**

```rust
struct UserId(u64);
struct GroupId(u64);

fn add_member(user: UserId, group: GroupId) {
    // The compiler prevents interchanging these arguments.
}

let user = UserId(12);
let group = GroupId(34);
add_member(user, group);
```

An alias is still appropriate when the name is merely a convenient spelling
for the same type and no semantic distinction or separate API is intended.

## Named fields

Prefer named fields over tuples or multi-field tuple structs when positional
access would be confusing.

**Bad:**

```rust
struct Dimensions(u32, u32);

fn area(size: &Dimensions) -> u32 {
    size.0 * size.1
}
```

**Good:**

```rust
struct Dimensions {
    width: u32,
    height: u32,
}

fn area(size: &Dimensions) -> u32 {
    size.width * size.height
}
```

Single-field newtypes and very local, unambiguous tuples are exceptions. Do not
replace a straightforward local pair with a new public abstraction solely to
avoid tuple syntax.
