<div align="center">

# vibe-style

AST-based style checker and formatter for deterministic, rule-driven code layout.

[![License](https://img.shields.io/badge/License-GPLv3-blue.svg)](https://www.gnu.org/licenses/gpl-3.0)
[![Docs](https://img.shields.io/docsrs/vibe-style)](https://docs.rs/vibe-style)
[![Language Checks](https://github.com/hack-ink/vibe-style/actions/workflows/language.yml/badge.svg?branch=main)](https://github.com/hack-ink/vibe-style/actions/workflows/language.yml)
[![Release](https://github.com/hack-ink/vibe-style/actions/workflows/release.yml/badge.svg)](https://github.com/hack-ink/vibe-style/actions/workflows/release.yml)
[![GitHub tag (latest by date)](https://img.shields.io/github/v/tag/hack-ink/vibe-style)](https://github.com/hack-ink/vibe-style/tags)
[![GitHub last commit](https://img.shields.io/github/last-commit/hack-ink/vibe-style?color=red&style=plastic)](https://github.com/hack-ink/vibe-style)
[![GitHub code lines](https://tokei.rs/b1/github/hack-ink/vibe-style)](https://github.com/hack-ink/vibe-style)

</div>

## Feature Highlights

### TODO

TODO

## Status

TODO

## Usage

### Installation

#### Build from Source

```sh
# Clone the repository.
git clone https://github.com/hack-ink/vibe-style
cd vibe-style

# To install Rust on macOS and Unix, run the following command.
#
# To install Rust on Windows, download and run the installer from `https://rustup.rs`.
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- --default-toolchain stable

# Install the necessary dependencies. (Unix only)
# Using Ubuntu as an example, this really depends on your distribution.
sudo apt-get update
sudo apt-get install <DEPENDENCIES>

# Build the project, and the binary will be available at `target/release/vstyle`.
cargo build --release

# If you are a macOS user and want to have a `vstyle.app`, run the following command.
# Install `cargo-bundle` to pack the binary into an app.
cargo install cargo-bundle
# Pack the app, and the it will be available at `target/release/bundle/osx/vstyle.app`.
cargo bundle --release
```

#### Download Pre-built Binary

- **macOS**
  - Download the latest pre-built binary from [GitHub Releases](https://github.com/hack-ink/vibe-style/releases/latest).
- **Windows**
  - TODO
- **Unix**
  - TODO

### Configuration

#### TODO

TODO

### Interaction

#### Rust Style Checker

This project provides a Rust style checker with check and fix workflows.
The style rules are defined in this section of `README.md`.

To enable `cargo vstyle ...`, install the binary once:

```sh
cargo install --path .
```

Why this exists:

- Provide one stable, reusable style contract (`RUST-STYLE-*` IDs) for Rust codebases.
- Avoid duplicating language-specific style prompts in every repository.
- Let downstream repositories consume the tool directly and keep their agent guidance minimal.

Commands:

```sh
# Run style checks. Exit code is 0 when no violations are found.
cargo run -- curate

# Run style checks for specific files.
cargo run -- curate src/main.rs src/cli.rs

# Apply safe automatic fixes, then re-check.
cargo run -- tune

# Apply fixes and fail when unresolved violations remain.
cargo run -- tune --strict

# Scope checks/fixes like cargo clippy.
cargo run -- curate --workspace
cargo run -- tune -p api -p db-service
cargo run -- tune -p api --features serde,tracing
cargo run -- tune -p api --all-features --no-default-features

# Print implemented rule IDs.
cargo run -- coverage

# After installing as a cargo subcommand, use:
# cargo vstyle curate
# cargo vstyle tune --strict
```

Behavior:

- `curate` reports all violations and exits with code `1` when violations exist.
- `tune` applies safe automatic edits and reports remaining violations but exits with code `0` by default.
- `tune --strict` exits with code `1` when violations remain after fixes.
- Violation lines marked with `(fixable)` can be auto-fixed by `tune`.
- `curate` and `tune` support cargo-targeting flags: `--workspace`, `-p/--package`, `--features`, `--all-features`, `--no-default-features`.
- You can run this tool as a cargo subcommand (`cargo vstyle ...`) when `cargo-vstyle` is available in `PATH`.

Rule catalog:

File structure:

- `RUST-STYLE-FILE-001`: Do not use `mod.rs`; use flat module files.

Module layout:

- `RUST-STYLE-MOD-001`: Keep top-level item order as `mod`, `use`, `macro_rules!`, `type`, `const`, `static`, `trait`, `enum`, `struct`, `impl`, `fn`.
- `RUST-STYLE-MOD-002`: Place `pub` items before non-`pub` items within the same kind. Visibility boundaries define separate batches and must be separated by exactly one blank line.
- `RUST-STYLE-MOD-003`: Place non-`async` functions before `async` functions at the same visibility.
- `RUST-STYLE-MOD-005`: Keep each type adjacent to related `impl` blocks, with no blank line between the type and its first `impl`.
- `RUST-STYLE-MOD-007`: In `#[cfg(test)] mod tests`, require `use super::*;` (with `#[allow(unused_imports)]` inserted when needed) unless it is a keep-alive module.

Serde:

- `RUST-STYLE-SERDE-001`: Do not use `#[serde(default)]` on `Option<T>` fields.

Imports and paths:

- `RUST-STYLE-IMPORT-001`: Group imports in this order: standard library, third-party, self/workspace/local-module roots.
- `RUST-STYLE-IMPORT-002`: Use exactly one blank line between groups; do not use import-group header comments; normalize `use a::{b, b::c}` to `use a::{b::{self, c}}`.
- `RUST-STYLE-IMPORT-003`: Do not alias imports, except `as _` in keep-alive test modules.
- `RUST-STYLE-IMPORT-004`: Do not import free functions or macros into scope; use qualified paths. If imported symbols are ambiguous, use fully qualified paths.
- `RUST-STYLE-IMPORT-005`: In `error.rs`, do not add `use` imports.
- `RUST-STYLE-IMPORT-006`: Do not qualify standard macros with `std::`.
- `RUST-STYLE-IMPORT-007`: Avoid redundant `crate::...` imports when `crate::prelude::*` is already imported.
- `RUST-STYLE-IMPORT-008`: For non-function, non-macro symbols in type paths, prefer unqualified usage with `use` imports when unambiguous; keep fully qualified paths when ambiguous.
- `RUST-STYLE-IMPORT-009`: If a symbol is both imported and also used via other qualified type paths, stop importing that symbol and use fully qualified paths consistently.

Types and generics:

- `RUST-STYLE-IMPL-001`: Use `Self` instead of concrete type names in `impl` method signatures.
- `RUST-STYLE-IMPL-003`: Keep `impl` blocks contiguous and ordered as inherent, standard-library traits, third-party traits, then workspace-member traits.
- `RUST-STYLE-GENERICS-001`: Move trait bounds to `where`; do not use inline bounds.

Logging and runtime safety:

- `RUST-STYLE-LOG-002`: Use structured logging fields and complete-sentence log messages.
- `RUST-STYLE-RUNTIME-001`: Do not use `unwrap()` in non-test code.
- `RUST-STYLE-RUNTIME-002`: `expect()` must use a clear, user-actionable string literal message.

Numeric literals:

- `RUST-STYLE-NUM-001`: Separate numeric literal suffixes with an underscore (for example `10_f32`).
- `RUST-STYLE-NUM-002`: Use underscore grouping for integers with more than three digits.

Readability:

- `RUST-STYLE-READ-002`: Keep functions at or under 120 lines.

Spacing:

- `RUST-STYLE-SPACE-003`: Do not insert blank lines within the same statement type. Use exactly one blank line between different statement types. Keep constant declaration groups compact only within the same visibility batch.
- `RUST-STYLE-SPACE-004`: Insert exactly one blank line before each `return` and before final tail expressions unless the body is a single expression.

Tests:

- `RUST-STYLE-TEST-001`: Use descriptive `snake_case` test names.
- `RUST-STYLE-TEST-002`: Reserve `#[cfg(test)] mod _test` for keep-alive imports only.

The checker implementation is authoritative for parser- and AST-level edge cases.

### Update

TODO

## Development

### Architecture

TODO

## Support Me

If you find this project helpful and would like to support its development, you can buy me a coffee!

Your support is greatly appreciated and motivates me to keep improving this project.

- **Fiat**
  - [Ko-fi](https://ko-fi.com/hack_ink)
  - [爱发电](https://afdian.com/a/hack_ink)
- **Crypto**
  - **Bitcoin**
    - `bc1pedlrf67ss52md29qqkzr2avma6ghyrt4jx9ecp9457qsl75x247sqcp43c`
  - **Ethereum**
    - `0x3e25247CfF03F99a7D83b28F207112234feE73a6`
  - **Polkadot**
    - `156HGo9setPcU2qhFMVWLkcmtCEGySLwNqa3DaEiYSWtte4Y`

Thank you for your support!

## Appreciation

We would like to extend our heartfelt gratitude to the following projects and contributors:

- The Rust community for their continuous support and development of the Rust ecosystem.

## Additional Acknowledgements

- TODO

<div align="right">

### License

<sup>Licensed under [GPL-3.0](LICENSE).</sup>

</div>
