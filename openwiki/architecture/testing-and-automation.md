---
type: Automation Reference
title: Testing, CI, Release, and OpenWiki Automation
description: Repository task entrypoints, focused and broad validation, composite Action behavior, release publication, benchmark workflow routing, and scheduled OpenWiki updates.
tags: [testing, ci, release, automation]
openwiki:
  roles: [testing, delivery, operations]
  change_kinds: [ci, release, documentation]
  source_paths: [Makefile.toml, action.yml, .github/workflows/language.yml, .github/workflows/release.yml, .github/workflows/benchmark.yml, .github/workflows/openwiki-update.yml]
  symbols: [check, lint, test, bench-release-vstyle, bench-semantic-vstyle]
  test_paths: [tests/swift_curate.rs, tests/let_mut_reorder.rs, tests/type_alias_rename.rs]
  invariants: [The Action performs read-only curate; local lint uses tune --strict; release publishes both vstyle and cargo-vstyle binaries.]
  validation_commands: [cargo test --all-targets --all-features]
---

# Testing and Automation

`Makefile.toml` separates formatting, Rust clippy checks, vstyle checks, tests, and two benchmark tasks. `cargo make test-rust` runs nextest across workspace/all targets/all features. The broad `cargo make check` composes formatting, clippy, vstyle, and tests; use it only when a cross-cutting validation is justified.

The language workflow uses the local composite `action.yml` with `version: checkout` for Rust curate, then runs clippy and tests. The Action supports `rust` and `swift`, defaults `workspace: true`, accepts extra whitespace-split `args`, and installs either a release archive or both local binaries via `cargo install --path`.

Release workflow behavior is owned by `.github/workflows/release.yml`: tag pushes build and publish binary artifacts, run the released composite Action against the exact tag, then publish to crates.io. A manual dispatch accepts an existing signed release tag, verifies it points at the checked-out commit, verifies the released Action, removes downloaded release binaries, requires a clean checkout, and republishes with `cargo publish --locked`; inspect the workflow for the exact target matrix, archive naming, checksums, and permissions before changing release behavior. The benchmark workflow runs independent release and semantic jobs, writes summaries, and uploads artifacts for 14 days; see [benchmark tracking](../runbooks/benchmark-tracking.md).

The scheduled OpenWiki workflow checks out full history, installs OpenWiki plus Mermaid validation dependencies, runs `openwiki code --update --print`, and opens an update PR containing `openwiki`, agent files, and the workflow. Generated wiki pages remain under `openwiki/`; source and tests remain authoritative.

## Validation routing

- CLI-only: `cargo test cli`.
- Swift-only: `cargo test --test swift_curate`.
- Fix behavior: the relevant fixture test, such as `cargo test --test let_mut_reorder --test type_alias_rename`.
- Shared registry/orchestration: `cargo test --all-targets --all-features`.
- Release packaging: `cargo build --profile final-release --bins` plus release workflow review.
