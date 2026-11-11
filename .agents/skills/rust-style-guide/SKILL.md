---
name: rust-style-guide
description: Write, refactor, and review Rust using a self-contained style guide covering API design, ownership, modules, traits, concurrency, naming, documentation, and implementation.
---

# Rust style guide

Use the bundled topic references to write, refactor, or review Rust. They contain
rules, rationale, exceptions, and labeled bad/good examples. Everything needed
is local; no external style guide or download is required.

Read the relevant references before applying a rule. In reviews, explain the
rule's practical effect and consider its exceptions. Keep edits within the
requested scope.

## Shared principles

- Prioritize readability and long-term maintainability over ease of writing.
  Favor consistency, conventional Rust, and code that tools can understand and
  refactor. Rules should provide a concrete benefit or settle recurring style
  disagreements without prolonged debate.
- Follow standard Rust formatting and API conventions where these rules do not
  prescribe a different choice. Let the codebase's formatter handle ordinary
  formatting.
- Distinguish reusable libraries from application code. API design rules are
  binding for reusable libraries and strong recommendations for applications.
  Application code should favor simplicity and immediate needs over premature
  generalization.
- Preserve requirements, preferences, and exceptions as written. Apply build
  and platform policies only in environments that use those integration models.
  Examples illustrate their stated context, not universal prohibitions.

## Topic references

Read only the references relevant to the change. For a complete style review,
cover every topic.

| When working on | Read |
| --- | --- |
| Modules, file layout, imports, and re-exports | [Code organization](references/code-organization.md) |
| Formatter settings, literals, and formatting opt-outs | [Formatting](references/formatting.md) |
| Ownership, borrowing, cloning, and shared pointers | [Ownership and borrowing](references/ownership-and-borrowing.md) |
| Parameters, iterators, conversions, booleans, and errors | [Function interfaces](references/function-interfaces.md) |
| Newtypes, aliases, tuples, and named fields | [Types](references/types.md) |
| Trait selection, derives, dispatch, extensions, and defaults | [Traits](references/traits.md) |
| Mutex poison and host concurrency choices | [Concurrency](references/concurrency.md) |
| Macros, ordinary abstractions, and code generation | [Macros](references/macros.md) |
| Method names, lifetimes, constructors, getters, and setters | [Naming](references/naming.md) |
| Public contracts, examples, panic cases, and safety docs | [Documentation](references/documentation.md) |
| Unsafe, stable features, API evolution, attributes, and initialization | [Language features and APIs](references/language-features-and-apis.md) |
| Inference, matching, assertions, iteration, panics, and strings | [Implementation](references/implementation.md) |
| Integrated builds, platform policies, and test harnesses | [Platform integration](references/platform-integration.md) |
