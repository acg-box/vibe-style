# Rust Development Style Guide

This document defines mandatory Rust style rules for this repository.

## Scope

- Applies to all Rust crates, binaries, and tooling in this repository.
- Does not apply to non-Rust projects.
- Repository language and tone rules are defined in `AGENTS.md` and still apply.

## Enforcement Model

- For rules with `RUST-STYLE-*` IDs, the checker implementation is authoritative:
  - `src/style_checker/shared.rs` (ID registry)
  - `src/style_checker/*.rs` (rule behavior and fix behavior)
- This document defines intent and stable rule semantics.
- Keep rule IDs stable.

## Workflow

When claiming Rust work is complete:

1. `cargo make fmt-rust`
2. `cargo make lint-rust`
3. `cargo make test-rust` when behavior changed.

## Tooling and Platform Constraints

- Rust toolchain is pinned. Do not modify `rust-toolchain.toml`, `.cargo/config.toml`, or `.rustfmt.toml`.
- Do not install, update, or override system/toolchain packages.
- Use `cargo make` tasks when applicable.

## Non-ID Rules (Still Mandatory)

### Time and TLS

- Use `time` crate types for date/time. Do not add `chrono`.
- Prefer rustls. Use native-tls only when rustls is unsupported.

### Error Handling

- Use `color_eyre::eyre::Result` for fallible APIs. Do not introduce `anyhow`.
- Add boundary context while preserving the source error.
- Boundary includes public APIs, entrypoints, and cross-module helpers.
- Use `#[error(transparent)]` only for true thin wrappers.
- Use short, action-oriented error messages.
- Use `ok_or_else` for `Option` to `Result` conversion with context.

### Readability and Ownership

- Keep happy path linear.
- Extract helpers when functions become hard to follow.
- Avoid unnecessary `.clone()`.
- Prefer borrowing (`&`) when equivalent.
- Use explicit `drop` only when early release is required.
- Use `let _ = value;` only to end a borrow on references when needed.

## Style Rule IDs (Checker Mapping)

### File Structure

- `RUST-STYLE-FILE-001`: Do not use `mod.rs`; use flat module files.

### Module Layout

- `RUST-STYLE-MOD-001`: Top-level item order is `mod`, `use`, `macro_rules!`, `type`, `const`, `static`, `trait`, `enum`, `struct`, `impl`, `fn`.
- `RUST-STYLE-MOD-002`: Place `pub` items before non-`pub` items within a group.
- `RUST-STYLE-MOD-003`: Place non-`async` functions before `async` functions at the same visibility.
- `RUST-STYLE-MOD-005`: Keep each type adjacent to its related `impl` blocks, with no blank line between the type and first `impl`.
- `RUST-STYLE-MOD-007`: In `#[cfg(test)] mod tests`, include `#[allow(unused_imports)] use super::*;` unless it is a keep-alive module.

### Serde

- `RUST-STYLE-SERDE-001`: Do not use `#[serde(default)]` on `Option<T>` fields.

### Imports and Paths

- `RUST-STYLE-IMPORT-001`: Group imports by origin in order: standard library, third-party, self/workspace.
- `RUST-STYLE-IMPORT-002`: Use exactly one blank line between groups; do not use import-group header comments. Normalize `use a::{b, b::c}` to `use a::{b::{self, c}}`.
- `RUST-STYLE-IMPORT-003`: Do not alias imports, except `as _` in keep-alive test modules.
- `RUST-STYLE-IMPORT-004`: Do not import free functions/macros into scope; use qualified paths. If imported symbols are ambiguous (same symbol name from multiple paths), do not import them and use fully qualified paths.
- `RUST-STYLE-IMPORT-005`: In `error.rs`, do not add `use` imports.
- `RUST-STYLE-IMPORT-006`: Do not qualify standard macros with `std::`.
- `RUST-STYLE-IMPORT-007`: Avoid redundant `crate::...` imports when `crate::prelude::*` is imported.

### Types and Generics

- `RUST-STYLE-IMPL-001`: Use `Self` instead of concrete type names in `impl` method signatures.
- `RUST-STYLE-IMPL-003`: Keep `impl` blocks contiguous and ordered as inherent, standard-library traits, third-party traits, workspace-member traits.
- `RUST-STYLE-GENERICS-001`: Move trait bounds to `where`; do not use inline bounds.

### Logging

- `RUST-STYLE-LOG-002`: Use structured logging fields and complete-sentence messages.

### Runtime Safety

- `RUST-STYLE-RUNTIME-001`: Do not use `unwrap()` in non-test code.
- `RUST-STYLE-RUNTIME-002`: `expect()` must use a clear, user-actionable string literal message.

### Numeric Literals

- `RUST-STYLE-NUM-001`: Separate numeric literal suffixes with an underscore.
- `RUST-STYLE-NUM-002`: Use underscore grouping for integers with more than three digits.

### Readability

- `RUST-STYLE-READ-002`: Keep functions at or under 120 lines.

### Vertical Spacing

- `RUST-STYLE-SPACE-003`: No blank lines within the same statement type; exactly one blank line between different statement types.
- `RUST-STYLE-SPACE-004`: Exactly one blank line before each `return`, and before final tail expressions unless the body is a single expression.

For statement-type classification details (including turbofish, UFCS, method grouping, and recursive nested-block checks), the checker implementation is authoritative.

### Tests

- `RUST-STYLE-TEST-001`: Use descriptive `snake_case` test names.
- `RUST-STYLE-TEST-002`: Reserve `#[cfg(test)] mod _test` for keep-alive imports only.
