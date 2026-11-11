# Ownership and borrowing

## Prefer unique ownership

Prefer unique ownership and borrows over `Rc` or `Arc`. Shared pointers add
runtime overhead and make ownership relationships harder to understand when
they are unnecessary.

**Bad — reference counting without any shared ownership need:**

```rust
use std::rc::Rc;

struct Label {
    text: Rc<String>,
}
```

**Good — the label owns its text:**

```rust
struct Label {
    text: String,
}
```

Use `Rc` or `Arc` when avoiding shared ownership would make the design
unreasonably difficult to write, understand, or maintain. Discuss that tradeoff
in review; do not replace straightforward sharing with tangled lifetimes just
to avoid a shared pointer.

## Borrow rather than make unnecessary deep copies

Use `.clone()` when a deep copy is needed, rather than as a substitute for a
natural borrowing design. Consider the code's performance needs.

**Bad — cloning solely to keep using the original afterward:**

```rust
fn length(text: String) -> usize {
    text.len()
}

let name = String::from("Ada");
let count = length(name.clone());
println!("{name}: {count}");
```

**Good — the callee borrows what it reads:**

```rust
fn length(text: &str) -> usize {
    text.len()
}

let name = String::from("Ada");
let count = length(&name);
println!("{name}: {count}");
```

Cloning can be pragmatic when borrow relationships become excessively complex.
Preserve that exception rather than introducing an absolute ban on `.clone()`.

## Shared-pointer clones are different

Cloning an `Rc` or `Arc` is expected shared-pointer use, not a deep copy. Do not
flag it as copying the underlying allocation.

**Bad — copying the data when another shared owner is wanted:**

```rust
use std::sync::Arc;

let original = Arc::new(String::from("shared configuration"));
let another_owner = Arc::new((*original).clone());
```

**Good — share the existing allocation:**

```rust
use std::sync::Arc;

let original = Arc::new(String::from("shared configuration"));
let another_owner = Arc::clone(&original);
```
