# Macros and code generation

## Choose the simplest suitable mechanism

Prefer, in order:

1. Ordinary functions, types, traits, objects, and modules.
2. `macro_rules!` macros.
3. Other generation strategies, such as build-time generation rules.
4. Procedural macros.

**Bad — introducing a macro where an ordinary function is sufficient:**

```rust
macro_rules! is_empty_name {
    ($name:expr) => {
        $name.trim().is_empty()
    };
}

let empty = is_empty_name!(name);
```

**Good — the input contract is explicit and ordinary tooling works:**

```rust
fn is_empty_name(name: &str) -> bool {
    name.trim().is_empty()
}

let empty = is_empty_name(name);
```

## Weigh the cost of metaprogramming

Use a macro or generated code when its benefit justifies weaker IDE and source
search support, more difficult automated refactoring, and the cost of teaching
readers a local language dialect. Do not invent metaprogramming when ordinary
Rust expresses the abstraction adequately.

For simple syntax generation, `macro_rules!` can be less fragile than textual
generation because it operates on syntax trees. This preference does not turn
macros into the default for ordinary functions or data types.
