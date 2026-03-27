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

INSTALL_DIR="$HOME/.cargo/bin"
mkdir -p "${INSTALL_DIR}"
install -m 0755 "vibe-style-${TARGET}-${VERSION}/vstyle" "${INSTALL_DIR}/vstyle"
install -m 0755 "vibe-style-${TARGET}-${VERSION}/cargo-vstyle" "${INSTALL_DIR}/cargo-vstyle"
```

#### Windows (PowerShell)

```powershell
$Repo = "hack-ink/vibe-style"
$Version = (Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases/latest").tag_name
$Target = "x86_64-pc-windows-msvc"
$Asset = "vibe-style-$Target-$Version.zip"

Invoke-WebRequest -Uri "https://github.com/$Repo/releases/download/$Version/$Asset" -OutFile $Asset
Expand-Archive -Path $Asset -DestinationPath .

$InstallDir = "$env:USERPROFILE\.cargo\bin"
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

# Enable verbose output.
vstyle tune --verbose

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

### CI policy

CI runs `vstyle curate` (read-only verification) to keep feedback fast and deterministic.
Use `vstyle tune` locally when you want to apply safe automatic fixes (for example, via `cargo make lint-fix`).

### Release benchmark

Release-performance acceptance is based on the locally built `vstyle` binary, not on an installed
`cargo-vstyle` subcommand and not on debug-profile timings.

Use the checked-in harness:

```sh
cargo make bench-release-vstyle
```

By default the harness builds the shipping `final-release` profile from `Cargo.toml` and runs both
`vstyle curate --workspace` and `vstyle tune --workspace --verbose` inside a disposable Git
worktree at the current commit. This keeps `tune` from rewriting the primary checkout while still
preserving `git ls-files` semantics for file discovery.

Treat the checked-in self-host benchmark as a release-path regression guard, not as a universal
microbenchmark for every hotspot. On the current workspace it is usually a no-op `tune`; if
`--verbose` reports `Semantic cache: 0 hit(s), 0 miss(es)`, that run did not enter semantic
validation and should not be used to judge semantic-path changes in `src/style/semantic.rs`.
Use a semantic-positive workload before drawing conclusions about semantic validation performance.

The current baseline and checkpoint history live in
`docs/benchmarks/2026-03-12_vstyle-release-runtime-baseline.md`.

To compare the plain `release` profile diagnostically:

```sh
VSTYLE_BENCH_PROFILE=release cargo make bench-release-vstyle
```

`cargo make lint-vstyle` remains the repo-native style gate, but it is not the release benchmark
source of truth because it routes through `cargo vstyle curate --workspace` and can resolve to an
installed subcommand outside the locally built binary under test.

### Semantic benchmark

Use the semantic-specific harness when a change targets `src/style/semantic.rs` or semantic
validation fallback behavior:

```sh
cargo make bench-semantic-vstyle
```

This harness builds the local release binary once, creates a disposable Git-tracked fixture crate
based on the `tests/let_mut_reorder.rs` semantic-validation shape, generates a local `Cargo.lock`,
and runs `vstyle tune --verbose` twice:

- a cold run after clearing `target/vstyle-cache/semantic`
- a warm rerun after restoring the original fixture sources while keeping the cache directory

Use this semantic benchmark to judge `XY-95`-style work; do not compare semantic-path changes only
against the self-host no-op benchmark above.

The current semantic-path baseline and follow-up history live in
`docs/benchmarks/2026-03-12_vstyle-semantic-runtime-baseline.md`.

### Benchmark tracking

The repository also tracks both benchmark harnesses through a non-blocking GitHub Actions workflow.
Use the `Benchmarks` workflow for periodic project-level tracking, scheduled baseline refreshes, and
manual reruns when you want artifact-backed evidence without turning performance into a PR gate.

Use direct current-worktree timings first when a local rule change makes the repository's own
sources newly fixable. The checked-in `bench-release-vstyle` harness builds the current binary but
benchmarks a detached `HEAD` worktree, so self-host drift in uncommitted files must be fixed and
committed before the harness becomes authoritative again.

Use the release benchmark for general workspace-scan, fix-engine, import, module, spacing, or
quality-path changes. Use the semantic benchmark for `src/style/semantic.rs`, semantic cache key
changes, or semantic-validation fallback changes. Run both when a change touches both lanes.

The operational runbook for selecting the right benchmark evidence lives in
`docs/guide/benchmark_tracking.md`.

## Configuration

There is currently no user configuration file.
Rules are built into the checker.

### Environment variables

- `VSTYLE_MAX_IMPORT_SUGGESTION_ROUNDS`
  - Controls how many semantic missing-import suggestion rounds `vstyle tune` will perform.
  - Default: `2`.
  - Increasing this may fix more missing-import cases but will run additional `cargo check --message-format=json` rounds.

### Semantic cache

- `--verbose` prints semantic cache hit/miss statistics for each command.
- Cache files are written under `target/vstyle-cache/semantic/` and keyed by:
  - vstyle version metadata,
  - `rustc -Vv` output,
  - `Cargo.lock` hash,
  - selected cargo options,
  - tracked `*.rs` file fingerprints.

## Rule Catalog

### File structure

- `RUST-STYLE-FILE-001`: Do not use `mod.rs`; use flat module files.

### Module layout

- `RUST-STYLE-MOD-001`: Keep top-level item order as `mod`, `use`, `macro_rules!`, `type`, `const`, `static`, `trait`, `enum`, `struct`, `impl`, `fn`.
- `RUST-STYLE-MOD-002`: Place `pub` items before non-`pub` items within the same kind. Visibility boundaries define separate batches and must be separated by exactly one blank line.
- `RUST-STYLE-MOD-003`: Place non-`async` functions before `async` functions at the same visibility.
- `RUST-STYLE-MOD-004`: Do not document modules with outer doc comments on the `mod` declaration; place module docs inside the module with `//!`.
- `RUST-STYLE-MOD-005`: Keep each type adjacent to related `impl` blocks, with no blank line between the type and its first `impl`.
- `RUST-STYLE-MOD-007`: In `#[cfg(test)] mod tests`, remove unused `use super::*;` keep-alive imports during `tune`.

### Serde

- `RUST-STYLE-SERDE-001`: Do not use `#[serde(default)]` on `Option<T>` fields.

### Imports and paths

- `RUST-STYLE-IMPORT-001`: Group imports in this order: standard library, third-party, self/workspace/local-module roots.
- `RUST-STYLE-IMPORT-002`: Use exactly one blank line between groups; do not use import-group header comments; normalize `use a::{b, b::c}` to `use a::{b::{self, c}}`.
- `RUST-STYLE-IMPORT-003`: Do not alias imports, except `as _` keep-alive imports. Trait imports used only for method resolution must use `as _`.
- `RUST-STYLE-IMPORT-004`: Do not import free functions or macros into scope; use qualified paths. If imported symbols are ambiguous, use fully qualified paths.
- `RUST-STYLE-IMPORT-005`: In `error.rs`, do not add `use` imports.
- `RUST-STYLE-IMPORT-007`: Do not use glob imports (`use ...::*` or equivalent). Use explicit imports only.
- `RUST-STYLE-IMPORT-008`: For non-function, non-macro symbols in type paths and `#[derive(...)]` attributes, prefer unqualified usage with `use` imports when unambiguous; keep fully qualified paths when ambiguous.
- `RUST-STYLE-IMPORT-009`: If a symbol is both imported and also used via other qualified type paths, stop importing that symbol and use fully qualified paths consistently.
- `RUST-STYLE-IMPORT-010`: Do not use `super` or `self` import prefixes. Rewrite `super` imports to crate-absolute imports (`use crate::...`) when module depth allows it, and rewrite `self::...` imports to direct module paths.
- `RUST-STYLE-IMPORT-011`: Order `#[derive(...)]` entries like imports: `std`/`core`/`alloc` first, then third-party derives, then workspace derives; alphabetize within each group.
- `RUST-STYLE-IMPORT-012`: Do not add crate keep-alive imports `use dep as _;` unless another path in the same package uses that crate.

### Types and generics

- `RUST-STYLE-IMPL-001`: Use `Self` instead of concrete type names in `impl` method signatures.
- `RUST-STYLE-IMPL-003`: Keep `impl` blocks contiguous and ordered as inherent, standard-library traits, third-party traits, then workspace-member traits.
- `RUST-STYLE-GENERICS-001`: Move trait bounds to `where`; do not use inline bounds.
- `RUST-STYLE-GENERICS-002`: Remove unnecessary turbofish when the type is already explicit in a `let` binding.
- `RUST-STYLE-GENERICS-003`: Canonicalize turbofish paths to `Type::<Args>::Assoc` form.
- `RUST-STYLE-TYPE-001`: Do not add type aliases that are only pure renames.
- `RUST-STYLE-LET-001`: Place immutable `let` bindings before mutable ones when the reorder is semantically safe.

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

# Lint (clippy + vibe-style).
cargo make lint

# Rust-only clippy.
cargo make lint-rust

# vibe-style only.
cargo make lint-vstyle

# Rust tests.
cargo make test-rust

# Full checks.
cargo make checks
```

## License

Licensed under [GPL-3.0](LICENSE).
