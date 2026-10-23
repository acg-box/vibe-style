<div align="center">

# vibe-style

Check Rust style with syntax and semantic analysis, run read-only Swift checks, and apply validated Rust fixes.

[![License](https://img.shields.io/badge/License-GPLv3%20only-blue.svg)](https://spdx.org/licenses/GPL-3.0-only.html)
[![Docs](https://img.shields.io/badge/docs-OpenWiki-blue)](openwiki/quickstart.md)
[![Release](https://github.com/acg-box/vibe-style/actions/workflows/release.yml/badge.svg)](https://github.com/acg-box/vibe-style/actions/workflows/release.yml)
[![GitHub tag (latest by date)](https://img.shields.io/github/v/tag/acg-box/vibe-style)](https://github.com/acg-box/vibe-style/tags)
[![GitHub last commit](https://img.shields.io/github/last-commit/acg-box/vibe-style?color=red&style=plastic)](https://github.com/acg-box/vibe-style)

</div>

## Feature Highlights

### Rust and Swift Style Checks

- Check Rust module layout, imports, generics, types, logging, spacing, and tests with
  36 stable `RUST-STYLE-*` rule IDs.
- Run six read-only Swift checks with stable `SWIFT-STYLE-*` rule IDs.
- Use `curate` to report violations, `tune` to apply supported Rust fixes, and
  `coverage` to list the implemented rules.
- Select Cargo packages, workspace scope, and feature flags. File discovery follows
  Git ignore rules and includes untracked source files in the selected scope.
- Validate Rust binding changes with Cargo checks. If a fix or semantic-validation
  error occurs, `tune` restores all selected source files to their initial contents.
- Run checks in CI with the composite GitHub Action.

<details>
<summary>Rule catalog</summary>

**File structure**

- `RUST-STYLE-FILE-001`: Do not use `mod.rs`; use flat module files.

**Module layout**

- `RUST-STYLE-MOD-001`: Keep top-level item order as `mod`, `use`, `macro_rules!`, `type`, `const`, `static`, `trait`, `enum`, `struct`, `impl`, `fn`.
- `RUST-STYLE-MOD-002`: Place `pub` items before non-`pub` items within the same kind. Visibility boundaries define separate batches and must be separated by exactly one blank line.
- `RUST-STYLE-MOD-003`: Place non-`async` functions before `async` functions at the same visibility.
- `RUST-STYLE-MOD-004`: Do not document modules with outer doc comments on the `mod` declaration; place module docs inside the module with `//!`.
- `RUST-STYLE-MOD-005`: Keep each type adjacent to related `impl` blocks, with no blank line between the type and its first `impl`.
- `RUST-STYLE-MOD-007`: In `#[cfg(test)] mod tests`, remove unused `use super::*;` keep-alive imports during `tune`.

**Imports and paths**

- `RUST-STYLE-IMPORT-001`: Group imports in this order: standard library, third-party, self/workspace/local-module roots.
- `RUST-STYLE-IMPORT-002`: Use exactly one blank line between groups; do not use import-group header comments; normalize `use a::{b, b::c}` to `use a::{b::{self, c}}`.
- `RUST-STYLE-IMPORT-003`: Do not alias imports, except `as _` keep-alive imports. Trait imports used only for method resolution must use `as _`.
- `RUST-STYLE-IMPORT-004`: Do not import free functions or macros into scope; use qualified paths. If imported symbols are ambiguous, use fully qualified paths.
- `RUST-STYLE-IMPORT-005`: In `error.rs`, do not add `use` imports.
- `RUST-STYLE-IMPORT-006`: Keep `use` items only at file top level or module top level.
- `RUST-STYLE-IMPORT-007`: Do not use glob imports (`use ...::*` or equivalent). Use explicit imports only.
- `RUST-STYLE-IMPORT-008`: For non-function, non-macro symbols in type paths and `#[derive(...)]` attributes, prefer unqualified usage with `use` imports when unambiguous; keep fully qualified paths when ambiguous.
- `RUST-STYLE-IMPORT-009`: If a symbol is both imported and also used via other qualified type paths, stop importing that symbol and use fully qualified paths consistently.
- `RUST-STYLE-IMPORT-010`: Do not use `super` or `self` import prefixes. Rewrite `super` imports to crate-absolute imports (`use crate::...`) when module depth allows it, and rewrite `self::...` imports to direct module paths.
- `RUST-STYLE-IMPORT-011`: Order `#[derive(...)]` entries like imports: `std`/`core`/`alloc` first, then third-party derives, then workspace derives; alphabetize within each group.
- `RUST-STYLE-IMPORT-012`: Do not add crate keep-alive imports `use dep as _;` unless another path in the same package uses that crate.

**Types and generics**

- `RUST-STYLE-IMPL-001`: Use `Self` instead of concrete type names in `impl` method signatures.
- `RUST-STYLE-IMPL-003`: Keep `impl` blocks contiguous and ordered as inherent, standard-library traits, third-party traits, then workspace-member traits.
- `RUST-STYLE-GENERICS-001`: Move trait bounds to `where`; do not use inline bounds.
- `RUST-STYLE-GENERICS-002`: Remove unnecessary turbofish when the type is already explicit in a `let` binding.
- `RUST-STYLE-GENERICS-003`: Canonicalize turbofish paths to `Type::<Args>::Assoc` form.
- `RUST-STYLE-TYPE-001`: Do not add type aliases that are only pure renames.
- `RUST-STYLE-LET-001`: Place immutable `let` bindings before mutable ones when the reorder is semantically safe.

**Logging and runtime safety**

- `RUST-STYLE-LOG-002`: Use structured logging fields and complete-sentence log messages.
- `RUST-STYLE-RUNTIME-001`: Do not use `unwrap()` in non-test code.
- `RUST-STYLE-RUNTIME-002`: `expect()` must use a clear, user-actionable string literal message.

**Numeric literals**

- `RUST-STYLE-NUM-001`: Separate numeric literal suffixes with an underscore (for example, `10_f32`).
- `RUST-STYLE-NUM-002`: Use underscore grouping for integers with more than three digits.

**Readability and spacing**

- `RUST-STYLE-READ-002`: Keep functions at or under 120 lines.
- `RUST-STYLE-SPACE-003`: Do not insert blank lines within the same statement type. Use exactly one blank line between different statement types. Keep constant declaration groups compact only within the same visibility batch.
- `RUST-STYLE-SPACE-004`: Insert exactly one blank line before each `return` and before final tail expressions unless the body is a single expression.

**Tests**

- `RUST-STYLE-TEST-001`: Use descriptive `snake_case` test names.
- `RUST-STYLE-TEST-002`: Reserve `#[cfg(test)] mod _test` for keep-alive imports only.

**Swift first batch**

- `SWIFT-STYLE-FILE-001`: Do not use `mod.swift`; use flat Swift entry files.
- `SWIFT-STYLE-IMPORT-004`: Do not import individual Swift symbols; import modules instead.
- `SWIFT-STYLE-TYPE-001`: Do not add `typealias` declarations that are only pure renames.
- `SWIFT-STYLE-RUNTIME-001`: Do not use force unwraps, force casts, or `try!` in non-test Swift code.
- `SWIFT-STYLE-NUM-002`: Use underscore grouping for integers with more than three digits.
- `SWIFT-STYLE-READ-002`: Keep functions at or under 120 lines.

The governing Swift applicability map lives in `openwiki/specifications/swift-style-rule-applicability.md`.

</details>

## Status

The Rust checker supports syntax analysis, semantic validation, and automatic fixes.
Swift support covers an initial set of read-only checks. Swift fixes are not implemented.

Rule behavior is defined by the checker source and its tests. Run `vstyle coverage`
for the active rule list. Serde default attributes are preserved, including defaults
on `Option<T>` fields.

## Workspace Posture

- The root `Cargo.toml` owns the `vibe-style` package, dependencies, and release profiles.
- `src/main.rs` and `src/cli.rs` provide the `vstyle` and `cargo-vstyle` commands.
- `src/style.rs` and `src/style/` own rule checks, file selection, fixes, and semantic validation.
- `tests/` owns integration tests and regression fixtures.
- `scripts/` owns the release and semantic benchmark harnesses.
- `action.yml` owns the composite GitHub Action.
- `Makefile.toml` and `.github/workflows/` own local checks, CI, and release jobs.
- `openwiki/` contains generated documentation. Use source code and tests to verify behavior.

## Usage

### Installation

#### Build from Source

Use the stable Rust toolchain selected by `rust-toolchain.toml`.

```sh
# Clone the repository.
git clone https://github.com/acg-box/vibe-style
cd vibe-style

# Build both binaries with the shipping release profile.
cargo build --profile final-release --locked --bins

# Install both binaries from the local package.
cargo install --path . --locked
```

The build writes `vstyle` and `cargo-vstyle` to `target/final-release/`.
After installation, use either `vstyle ...` or `cargo vstyle ...`.

To compile and install the published crate instead, run:

```sh
cargo install vibe-style --locked
```

#### Download Pre-built Binary

Download an archive from [GitHub Releases](https://github.com/acg-box/vibe-style/releases/latest).

- **macOS**: `vibe-style-aarch64-apple-darwin-v<VERSION>.tgz` for Apple Silicon.
- **Windows**: `vibe-style-x86_64-pc-windows-msvc-v<VERSION>.zip` for x86-64 MSVC.
- **Unix**: `vibe-style-x86_64-unknown-linux-gnu-v<VERSION>.tgz` for x86-64 GNU/Linux.

Extract the archive. Copy both binaries to a directory on `PATH`, such as
`~/.cargo/bin` on Unix or `%USERPROFILE%\.cargo\bin` on Windows.

If `cargo-binstall` is available, it can install the prebuilt binaries:

```sh
cargo binstall vibe-style
```

### Configuration

#### Runtime Settings

Rules are built into the checker. There is no user configuration file.
`curate` and `tune` require `--language rust` or `--language swift`.

Set `VSTYLE_MAX_IMPORT_SUGGESTION_ROUNDS` to control the number of missing-import
suggestion rounds during Rust fixes. The default is `2`. A larger value can require
more `cargo check --message-format=json` runs.

Each semantic validation invokes Cargo. Cargo owns the build cache and checks source,
manifest, dependency, and build-script changes before it reuses compiler results.
vstyle does not store compiler output in a separate persistent cache.

### Interaction

```sh
# Check Rust style across the workspace.
vstyle curate --language rust --workspace

# Check Swift style without changing files.
vstyle curate --language swift --workspace

# Apply supported Rust fixes, then fail if violations remain.
vstyle tune --language rust --workspace --strict

# Select packages and features.
vstyle tune --language rust -p api -p db-service --features serde,tracing
vstyle tune --language rust -p api --all-features --no-default-features

# Include semantic recovery details.
vstyle tune --language rust --verbose

# List implemented rule IDs.
vstyle coverage
```

`tune` writes progress to stderr, including when stderr is redirected. It reports
scan phases, fix rounds, scoped fix batches, and semantic validation.

| Command | Exit behavior |
| --- | --- |
| `curate` | Exit `0` when no violations remain; exit `1` when violations are found. |
| `tune` | Exit `0` after a completed run, including when violations remain. |
| `tune --strict` | Exit `1` when violations remain. |
| Internal fix or semantic-validation error | Restore selected source files and return a failure. |

Use the GitHub Action to install a release and run a read-only check:

```yaml
- uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
- uses: acg-box/vibe-style@d4ae213399e27ba087d741e4c52d5b382664ffe7 # v0.2.7
  with:
    language: rust
    workspace: true
    args: --all-features
    version: v0.2.7
```

Set `language: swift` to check Swift files. `version` defaults to `latest`.
Use an explicit release tag for a fixed binary version. Use `version: checkout`
when the action must build the source in its own checkout.

### Update

Update an installation from crates.io:

```sh
cargo install vibe-style --locked --force
```

For a prebuilt installation, download the new release archive and replace both
binaries. For a source installation, pull the new source and repeat the local install
command. Update both the action ref and its `version` input when CI pins a release.

## Development

`rust-toolchain.toml` selects stable Rust. Formatting uses nightly rustfmt.
The local tasks require `cargo-make`, `taplo`, and `cargo-nextest`.
Use the host-managed developer tools and the repository toolchain settings.

```sh
# Correct Rust and TOML formatting.
cargo make fmt

# Check formatting without changing files.
cargo make fmt-check

# Run formatting, Clippy, style checks, and tests.
cargo make check

# Run individual Rust checks or tests.
cargo make check-rust
cargo make test-rust

# Apply Clippy and style fixes.
cargo make lint
```

Local tasks and CI use `cargo make check-vstyle` to check the current source with a
local build. This task runs:

```sh
cargo run --locked --all-features --bin vstyle -- \
  curate --language rust --workspace --all-features
```

Use the benchmark harnesses when a change affects scan, fix, or semantic-validation performance:

```sh
cargo make bench-release-vstyle
cargo make bench-semantic-vstyle
```

The release harness builds the local `final-release` binary and measures a temporary
source snapshot of `HEAD`. Commit source changes before using it to measure those changes.
The semantic harness uses a fixture that requires semantic validation and measures
cold and warm Cargo builds. Both runs invoke Cargo. A no-op self-check does not measure
semantic performance. The harnesses share setup and timing code in
`scripts/benchmark-common.bash` and remove their temporary source directories on exit.
Set `VSTYLE_BENCH_PROFILE=release` to compare the standard release profile.

The `Benchmarks` workflow stores run artifacts. Use fresh results for comparisons;
benchmark baselines are not stored in the repository. See the
[benchmark runbook](openwiki/runbooks/benchmark-tracking.md) for the measurement procedure.

### Architecture

- `src/cli.rs` maps commands, language selection, and Cargo options to the style engine.
- `src/style.rs` exposes the style engine and rule coverage to the CLI.
- `src/style/engine.rs` owns check state, fix rounds, semantic recovery, and rollback.
- `src/style/rules.rs` dispatches Rust rules. `shared.rs` owns source contexts and diagnostics;
  `syntax.rs` owns common Rust syntax helpers.
- `src/style/workspace.rs` owns Cargo package selection and Git ignore file discovery.
- `src/style/imports.rs` coordinates import plans. Its child modules own aliases, scopes,
  use paths, grouping, local imports, qualification, derives, and shortening. Shared
  analysis, reference rewrites, and rule sequencing have separate owners.
- The other `src/style/` modules own rule groups, edit application, and semantic checks.
- `src/style/tests/` groups engine regression tests by responsibility. `tests/` verifies
  command behavior and source transformations, with shared temporary-directory ownership.
- The release workflow builds three platform archives, publishes the GitHub Release,
  checks the released action, and publishes the crate to crates.io.
- Generated documentation starts at [OpenWiki quickstart](openwiki/quickstart.md).
  Documentation routing is described in [agent context](openwiki/agent-context.md).

## Support Me

If you find this project helpful and would like to support its development, you can buy me a coffee!

Your support is greatly appreciated and motivates me to keep improving this project.

- **Fiat**
    - [Ko-fi](https://ko-fi.com/hack_ink)
    - [Afdian](https://afdian.com/a/hack_ink)
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

---

<div align="right">

### License

<sup>Licensed under [GPL-3.0-only](LICENSE).</sup>

</div>
