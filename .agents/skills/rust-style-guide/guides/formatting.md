# Formatting and aesthetics

## Use the formatter

Use the codebase's formatter and configuration. Preferred choices are:

- At most 100 columns: `max_width = 100`.
- Lowercase hexadecimal digits: `hex_literal_case = "Lower"`.
- Formatted macro matchers: `format_macro_matchers = true`.
- Escapes for invisible characters in literals, such as `\u{200D}`.

A codebase may omit settings unavailable on its supported stable formatter. Do
not add a nightly dependency merely to enable these settings.

**Bad:**

```rust
fn combine(a:u32,b:u32)->u32{a|b|0xAB00}
```

**Good:**

```rust
fn combine(a: u32, b: u32) -> u32 {
    a | b | 0xab00
}
```

An invisible character should be visible in the source representation:

```rust
let joined = "left\u{200D}right";
```

## Formatting opt-outs

Use `#[rustfmt::skip]` only for generated code or situation-specific manual
alignment that improves readability, such as a matrix or repeated aligned
expressions. Prefer generating generated code through a build rule where
practical.

**Bad — opting out for personal formatting preferences:**

```rust
#[rustfmt::skip]
fn add(a:i32,b:i32)->i32 { a+b }
```

**Good — ordinary code follows the formatter:**

```rust
fn add(a: i32, b: i32) -> i32 {
    a + b
}
```

**Good — a narrowly justified alignment exception:**

```rust
// Keep corresponding entries aligned to make the pattern visible.
#[rustfmt::skip]
const PATTERN: [[u8; 3]; 3] = [
    [1, 0, 1],
    [0, 1, 0],
    [1, 0, 1],
];
```

Apply an opt-out to the smallest useful item rather than suppressing formatting
across unrelated code.
