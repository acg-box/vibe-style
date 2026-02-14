<div align="center">

# vibe-style

Rust style checker with syntax and semantic analysis, plus a safe auto-fixer for deterministic, rule-driven code layout.

[![License](https://img.shields.io/badge/License-GPLv3-blue.svg)](https://www.gnu.org/licenses/gpl-3.0)
[![Docs](https://img.shields.io/docsrs/vibe-style)](https://docs.rs/vibe-style)
[![Language Checks](https://github.com/hack-ink/vibe-style/actions/workflows/language.yml/badge.svg?branch=main)](https://github.com/hack-ink/vibe-style/actions/workflows/language.yml)
[![Release](https://github.com/hack-ink/vibe-style/actions/workflows/release.yml/badge.svg)](https://github.com/hack-ink/vibe-style/actions/workflows/release.yml)
[![GitHub tag (latest by date)](https://img.shields.io/github/v/tag/hack-ink/vibe-style)](https://github.com/hack-ink/vibe-style/tags)
[![GitHub last commit](https://img.shields.io/github/last-commit/hack-ink/vibe-style?color=red&style=plastic)](https://github.com/hack-ink/vibe-style)
[![GitHub code lines](https://tokei.rs/b1/github/hack-ink/vibe-style)](https://github.com/hack-ink/vibe-style)

</div>

## Overview

`vibe-style` enforces a strict Rust style contract with stable rule IDs (`RUST-STYLE-*`).
It supports:

- `curate`: check and report violations.
- `tune`: apply safe automatic fixes, then re-check.
- `coverage`: print implemented rule IDs.

The checker implementation is the source of truth for parser- and AST-level edge cases.

## Installation

Methods are listed from easiest to most advanced.

### Install prebuilt binaries (curl)

#### Unix (Linux/macOS)

```sh
VERSION="$(curl -fsSL https://api.github.com/repos/hack-ink/vibe-style/releases/latest | grep -oE '"tag_name": "v[^"]+"' | cut -d'"' -f4)"
OS="$(uname -s)"
ARCH="$(uname -m)"

case "${OS}:${ARCH}" in
	Linux:x86_64) TARGET="x86_64-unknown-linux-gnu" ;;
	Darwin:arm64) TARGET="aarch64-apple-darwin" ;;
	*) echo "Unsupported platform: ${OS}/${ARCH}" >&2; exit 1 ;;
esac

ASSET="vibe-style-${TARGET}-${VERSION}.tgz"
curl -fsSLO "https://github.com/hack-ink/vibe-style/releases/download/${VERSION}/${ASSET}"
tar -xzf "${ASSET}"

sudo install -m 0755 "vibe-style-${TARGET}-${VERSION}/vstyle" /usr/local/bin/vstyle
sudo install -m 0755 "vibe-style-${TARGET}-${VERSION}/cargo-vstyle" /usr/local/bin/cargo-vstyle
```

#### Windows (PowerShell)

```powershell
$Repo = "hack-ink/vibe-style"
$Version = (Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases/latest").tag_name
$Target = "x86_64-pc-windows-msvc"
$Asset = "vibe-style-$Target-$Version.zip"

Invoke-WebRequest -Uri "https://github.com/$Repo/releases/download/$Version/$Asset" -OutFile $Asset
Expand-Archive -Path $Asset -DestinationPath .

$InstallDir = "$env:USERPROFILE\.local\bin"
New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null

Copy-Item "vibe-style-$Target-$Version\vstyle.exe" "$InstallDir\vstyle.exe" -Force
Copy-Item "vibe-style-$Target-$Version\cargo-vstyle.exe" "$InstallDir\cargo-vstyle.exe" -Force
setx PATH "$env:PATH;$InstallDir"
```

Open a new terminal after running `setx`.

Supported prebuilt targets:

- `x86_64-unknown-linux-gnu`
- `aarch64-apple-darwin`
- `x86_64-pc-windows-msvc`

### Install from crates.io (requires Rust/Cargo)

```sh
# Install both binaries (`vstyle` and `cargo-vstyle`).
cargo install vibe-style
```

After installation, you can use both `vstyle ...` and `cargo vstyle ...`.

### Install prebuilt binaries (cargo-binstall, requires Rust/Cargo)

```sh
# Optional: install cargo-binstall once.
cargo install cargo-binstall

# Then install prebuilt binaries for this crate.
cargo binstall vibe-style
```

### Build from source

```sh
git clone https://github.com/hack-ink/vibe-style
cd vibe-style
cargo build --release
```

Binaries:

- `target/release/vstyle`
- `target/release/cargo-vstyle`

### Install as a cargo subcommand (local source)

```sh
cargo install --path . --bin cargo-vstyle
```

After installation, you can run `cargo vstyle ...`.

## Usage

### Basic commands

```sh
# Check style.
vstyle curate

# Apply safe fixes, then re-check.
vstyle tune

# Same as tune, but fail if violations remain.
vstyle tune --strict

# Print implemented rule IDs.
vstyle coverage
```

### Cargo-like target selection

```sh
# Workspace-wide.
vstyle curate --workspace

# Selected packages.
vstyle tune -p api -p db-service

# Feature flags.
vstyle tune -p api --features serde,tracing
vstyle tune -p api --all-features --no-default-features
```

### Exit behavior

- `curate`
  - Exit `0`: no violations.
  - Exit `1`: violations found.
- `tune`
  - Exit `0`: even if unresolved violations remain.
  - Exit `1`: unresolved violations remain and `--strict` is used.

By default, `curate` and `tune` follow cargo default package selection and scan git-tracked `*.rs`
files inside that package scope.

## Configuration

There is currently no user configuration file.
Rules are built into the checker.

## Rule Catalog

### File structure

- `RUST-STYLE-FILE-001`: Do not use `mod.rs`; use flat module files.

### Module layout

- `RUST-STYLE-MOD-001`: Keep top-level item order as `mod`, `use`, `macro_rules!`, `type`, `const`, `static`, `trait`, `enum`, `struct`, `impl`, `fn`.
- `RUST-STYLE-MOD-002`: Place `pub` items before non-`pub` items within the same kind. Visibility boundaries define separate batches and must be separated by exactly one blank line.
- `RUST-STYLE-MOD-003`: Place non-`async` functions before `async` functions at the same visibility.
- `RUST-STYLE-MOD-005`: Keep each type adjacent to related `impl` blocks, with no blank line between the type and its first `impl`.

### Serde

- `RUST-STYLE-SERDE-001`: Do not use `#[serde(default)]` on `Option<T>` fields.

### Imports and paths

- `RUST-STYLE-IMPORT-001`: Group imports in this order: standard library, third-party, self/workspace/local-module roots.
- `RUST-STYLE-IMPORT-002`: Use exactly one blank line between groups; do not use import-group header comments; normalize `use a::{b, b::c}` to `use a::{b::{self, c}}`.
- `RUST-STYLE-IMPORT-003`: Do not alias imports, except `as _` keep-alive imports. Trait imports used only for method resolution must use `as _`.
- `RUST-STYLE-IMPORT-004`: Do not import free functions or macros into scope; use qualified paths. If imported symbols are ambiguous, use fully qualified paths.
- `RUST-STYLE-IMPORT-005`: In `error.rs`, do not add `use` imports.
- `RUST-STYLE-IMPORT-008`: For non-function, non-macro symbols in type paths, prefer unqualified usage with `use` imports when unambiguous; keep fully qualified paths when ambiguous.
- `RUST-STYLE-IMPORT-009`: If a symbol is both imported and also used via other qualified type paths, stop importing that symbol and use fully qualified paths consistently.
- `RUST-STYLE-IMPORT-007`: Do not use glob imports (`use ...::*` or equivalent). Use explicit imports only.
- `RUST-STYLE-IMPORT-010`: Do not use `super` imports; rewrite to crate-absolute imports (`use crate::...`) when module depth allows it.

### Types and generics

- `RUST-STYLE-IMPL-001`: Use `Self` instead of concrete type names in `impl` method signatures.
- `RUST-STYLE-IMPL-003`: Keep `impl` blocks contiguous and ordered as inherent, standard-library traits, third-party traits, then workspace-member traits.
- `RUST-STYLE-GENERICS-001`: Move trait bounds to `where`; do not use inline bounds.

### Logging and runtime safety

- `RUST-STYLE-LOG-002`: Use structured logging fields and complete-sentence log messages.
- `RUST-STYLE-RUNTIME-001`: Do not use `unwrap()` in non-test code.
- `RUST-STYLE-RUNTIME-002`: `expect()` must use a clear, user-actionable string literal message.

### Numeric literals

- `RUST-STYLE-NUM-001`: Separate numeric literal suffixes with an underscore (for example, `10_f32`).
- `RUST-STYLE-NUM-002`: Use underscore grouping for integers with more than three digits.

### Readability and spacing

- `RUST-STYLE-READ-002`: Keep functions at or under 120 lines.
- `RUST-STYLE-SPACE-003`: Do not insert blank lines within the same statement type. Use exactly one blank line between different statement types. Keep constant declaration groups compact only within the same visibility batch.
- `RUST-STYLE-SPACE-004`: Insert exactly one blank line before each `return` and before final tail expressions unless the body is a single expression.

### Tests

- `RUST-STYLE-TEST-001`: Use descriptive `snake_case` test names.
- `RUST-STYLE-TEST-002`: Reserve `#[cfg(test)] mod _test` for keep-alive imports only.

## Development

This repository uses `cargo make` tasks from `Makefile.toml`.

```sh
# Format.
cargo make fmt
cargo make fmt-check

# Rust lint.
cargo make lint-rust

# Rust tests.
cargo make test-rust

# Full checks.
cargo make checks
```

## License

Licensed under [GPL-3.0](LICENSE).
