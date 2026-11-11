# Documentation

## Document useful contracts

Document public APIs with information a reader cannot infer from the code.
Consider `missing_docs` to find gaps. Use capitalization, punctuation, and
Markdown. Do not explain basic Rust syntax or repeat a declaration in prose.

**Bad — the comment only repeats the name:**

```rust
/// Removes an element.
pub fn remove_element(items: &mut Vec<String>, index: usize) -> String {
    items.remove(index)
}
```

**Good — the comment explains behavior and the panic precondition:**

```rust
/// Removes and returns the element at `index`, shifting later elements left.
///
/// # Panics
///
/// Panics if `index` is outside the vector.
pub fn remove_element(items: &mut Vec<String>, index: usize) -> String {
    items.remove(index)
}
```

If there is nothing insightful to say about a trivial API, omit the comment and
allow the lint as appropriate instead of adding redundant prose.
Self-explanatory enum variants need not each have documentation.

## Examples and invariants

- Provide enough examples somewhere in the crate to show how to use its
  functionality. Every individual function does not need an example.
- Always document panic cases caused by malformed input.
- Clearly document the invariants required by unsafe functions and traits.
  Explain the caller's obligations, rather than merely saying that an API is
  unsafe.

**Bad:**

```rust
/// Reads a byte from an unsafe pointer.
pub unsafe fn read_byte(pointer: *const u8) -> u8 {
    unsafe { *pointer }
}
```

**Good — a low-level boundary states the safety obligations:**

```rust
/// Copies the byte at `pointer`.
///
/// # Safety
///
/// `pointer` must be non-null, properly aligned, and valid to read one
/// initialized byte. Access must not race with a concurrent write.
pub unsafe fn read_byte(pointer: *const u8) -> u8 {
    // SAFETY: The caller guarantees a valid, initialized, race-free byte.
    unsafe { *pointer }
}
```

This example illustrates documenting an existing low-level boundary, not a
reason to introduce unsafe code. Necessary unsafe code also needs the technical
justification and isolation described in
[language features and APIs](language-features-and-apis.md#unsafe-code).

## Use the right comment syntax

- Use `///` immediately before the item it documents.
- Use `//!` at the beginning of a module or library to document that module or
  library itself.
- Use ordinary `//` comments for implementation details, internal TODOs, and
  other comments not intended as public API documentation.
- Prefer directly written documentation over macro-generated documentation so
  it remains readable in source and visible to documentation tools.

**Bad — an internal TODO is published as part of the API contract:**

```rust
/// Finds the first matching item.
/// TODO: Replace the temporary linear scan.
pub fn find_item(items: &[String], name: &str) -> Option<usize> {
    items.iter().position(|item| item == name)
}
```

**Good — public documentation and an internal note coexist:**

```rust
/// Returns the index of the first item equal to `name`, or `None` if absent.
// TODO: Replace the temporary linear scan when lookup performance matters.
pub fn find_item(items: &[String], name: &str) -> Option<usize> {
    items.iter().position(|item| item == name)
}
```
