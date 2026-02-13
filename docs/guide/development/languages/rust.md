# Rust Development and Style Guide

These rules apply to Rust code and Rust development workflows in this repository.
All comments and messages must also follow the Global Language Rules in `AGENTS.md`.

## Scope

These rules apply to Rust crates, binaries, and tooling in this repository. They do not apply to non-Rust projects.

All rules in this guide are mandatory.

## Agent Checklist

Before you start a Rust change:

- Identify which sections apply (Imports and Paths, Error Handling, Logging, Vertical Spacing).
- Ensure your change can follow the Completion Checklist tasks.

Before you claim a Rust change is complete:

- Follow the Completion Checklist section.
- Ensure errors use `color_eyre::eyre::Result` and add boundary context with `WrapErr`.
- Ensure logs use `tracing::...!` with structured fields.
- Ensure function bodies follow the Vertical Spacing statement-type rules.

## Decision Priorities

Use this priority order when trade-offs appear:

1. Correctness and safety.
2. Deterministic behavior and reproducibility.
3. LLM readability and auditability.
4. Simplicity of implementation.
5. Performance.

## Tooling and Workflow

- The Rust toolchain is pinned. Do not modify `rust-toolchain.toml`, `.cargo/config.toml`, or `.rustfmt.toml`.
- Do not install, update, or override toolchains.
- Do not invoke system package managers.
- Use `cargo make` tasks when they are a good fit for formatting, linting, and testing.

## Runtime Safety

- Do not use `unwrap()` in non-test code.
- `expect()` requires a clear, user-actionable message.

## Time and TLS

- Use the `time` crate for all date and time types. Do not add `chrono`.
- Use rustls for TLS. Use native-tls only when rustls is not supported.

## Formatting and Layout

- `rustfmt` output is the final authority for formatting.
- Use tabs (`\t`) for indentation.

### File Structure

- Use a flat module structure. Do not create or keep `mod.rs`. If `mod.rs` exists, flatten it into `a.rs` and `a/xxx.rs` style files.

## Error Handling

- Use `color_eyre::eyre::Result` for fallible APIs. Do not introduce `anyhow`.
- Add context at crate or module boundaries and keep the original error as the source.
- Boundaries include public APIs, entrypoints, and module-level helpers that are consumed outside the module.
- Use `#[error(transparent)]` only for thin wrappers where this crate adds no context and the upstream message is already sufficient for developers.
- Use short, action-oriented error messages that include the source error.
- Use `ok_or_else` to convert `Option` to `Result` with context.

Example (use):

```rust
use color_eyre::eyre::WrapErr;

fn load_config(path: &std::path::Path) -> color_eyre::eyre::Result<Config> {
	let bytes = std::fs::read(path)
		.wrap_err_with(|| format!("Failed to read config file at {path:?}."))?;

	parse_config(&bytes).wrap_err("Failed to parse config file.")
}
```

Example (avoid):

```rust
fn load_config(path: &std::path::Path) -> color_eyre::eyre::Result<Config> {
	let bytes = std::fs::read(path)?;

	parse_config(&bytes)
}
```

## Logging

- Use fully qualified tracing macros, such as `tracing::info!`.
- Do not import tracing macros.
- Always use structured fields for dynamic values such as identifiers, names, counts, and errors.
- Use short, action-oriented messages as complete sentences.

Example (use):

```rust
tracing::info!(user_id = %user_id, "Created session.");
```

Example (avoid):

```rust
tracing::info!("Created session for user {user_id}.");
```

## Borrowing and Ownership

- Use borrowing with `&` over `.as_*()` conversions when both are applicable.
- Avoid `.clone()` unless it is required by ownership or lifetimes, or it clearly improves clarity.
- Use `into_iter()` when intentionally consuming collections.
- Do not use scope blocks solely to end a borrow.
- When an early release is required, use an explicit `drop`.
- When the value is a reference and you need to end a borrow without a drop warning, use `let _ = value;`.

## Comments and Documentation

- Comments must be full sentences with proper punctuation.
- Use comments only when intent is not clear from names and types.
- Public items should have doc comments when the intent is not obvious.

## Tests

- Use descriptive test names in `snake_case` that encode the behavior and expected outcome.
- Tests must be deterministic to keep LLM reasoning and CI outcomes stable.
- Integration tests that require external services must be marked `#[ignore]` with a clear message about required dependencies.
- `#[cfg(test)] mod _test` is reserved for dev-dependency keep-alive imports such as `use some_crate as _;`. Do not place behavior tests in `_test`.

## LLM Readability Checklist

Before finalizing a Rust change, ensure the following:

- Functions follow the Readability Rules section.
- Error boundaries are explicit.
- Logging uses structured fields.
- Names convey intent without relying on comments.
- Imports and call sites follow the rules in the Imports and Paths section.

## Completion Checklist

When you claim a Rust change is complete, run the following tasks:

1. `cargo make fmt-rust`
2. `cargo make lint-rust`
3. `cargo make test-rust` when the change affects behavior, not just formatting or comments.
