---
type: repository guide
title: vibe-style Wiki Quickstart
description: Canonical map for the vibe-style Rust and Swift style checker, its rule contracts, fix engine, integrations, release workflows, and validation routes.
tags: [vibe-style, navigation, rust, swift]
verified:
  - by: openwiki/0.7.2
    at: 2026-10-23T01:52:10.637Z
sources:
  - id: openwiki-source-651d1fb6c9e49916a916ab51
    resource: repo://Cargo.toml
  - id: openwiki-source-c8b1a2a9f2113ec43d4066da
    resource: repo://Makefile.toml
  - id: openwiki-source-111747efc627ea82ca555afb
    resource: repo://scripts/bench-release-vstyle.sh
  - id: openwiki-source-59a609e749db71d05073a492
    resource: repo://scripts/bench-semantic-vstyle.sh
  - id: openwiki-source-c38906bbfa9e9c69417b11b5
    resource: repo://src/cli.rs
  - id: openwiki-source-5a343b6d360ed17beec1a852
    resource: repo://src/style/engine.rs
  - id: openwiki-source-b7a6ae5f72d1d14fc3a92e74
    resource: repo://src/style/rules.rs
  - id: openwiki-source-05232c26a674118ba025697f
    resource: repo://src/style/shared.rs
  - id: openwiki-source-60fd4a2aff5978a4368f5dd1
    resource: repo://src/style/workspace.rs
generated: { by: "codex", at: "2026-10-23T01:52:10.637Z" }
---

# vibe-style Wiki Quickstart

`vibe-style` is a Rust executable and Cargo subcommand that checks Rust and first-batch Swift source, reports stable rule IDs, and applies conservative deterministic Rust fixes. Source and tests remain authoritative; this wiki compresses the route from an engineering intent to owning symbols, focused tests, and validation.

## Map

- [Architecture overview](architecture/overview.md) explains composition and end-to-end flow.
- [CLI](architecture/cli.md) covers `vstyle`, `cargo-vstyle`, `curate`, `tune`, and `coverage`.
- [Engine](architecture/engine.md) covers discovery, diagnostics, fix rounds, concurrency, and convergence.
- [Rules](architecture/rules.md) is the rule-family and change-surface overview; the complete 42-ID classification is canonical in [style rule backends](specifications/style-rule-backends.md).
- [Semantic validation](architecture/semantic-validation.md) covers real Cargo checks, bounded recovery, and rollback.
- [Swift](architecture/swift.md) covers the source-text lane and applicability boundary.
- [Automation and release](architecture/testing-and-automation.md) covers Make, CI, Action, release, and OpenWiki.
- [Backend specification](specifications/style-rule-backends.md), [derive interaction](specifications/style-import-derive-interactions.md), [Swift applicability](specifications/swift-style-rule-applicability.md), and [tune failure atomicity](specifications/tune-failure-atomicity.md) are normative.
- [Benchmark runbook](runbooks/benchmark-tracking.md) routes performance evidence.
- [Documentation map](reference/documentation-map.md) records the full former `docs/` migration and document contracts.
- [Agent context](agent-context.md) records documentation ownership and safe-change boundaries.
- [Decisions](decisions/index.md) is the rationale lane; it is currently empty because no durable decision topic existed.

## Task routing

| Intent | Canonical page | Entrypoints and symbols | Focused evidence | Minimal validation |
| --- | --- | --- | --- | --- |
| Add or change a rule | [Rules](architecture/rules.md) | `src/style/shared.rs`, family checker, `STYLE_RULE_IDS` | matching integration test or family unit tests | `cargo test --all-targets --all-features` |
| Change CLI behavior | [CLI](architecture/cli.md) | `src/cli.rs`, `src/main.rs` | CLI parser tests | `cargo test cli` |
| Change file selection | [Engine](architecture/engine.md) | `workspace::resolve_files` in `src/style/workspace.rs` | Swift and workspace fixtures | `cargo test --test swift_curate` |
| Change auto-fix convergence | [Engine](architecture/engine.md) | `src/style/engine.rs`, `run_fix`, `run_fix_round` | `tests/let_mut_reorder.rs`, `type_alias_rename.rs` | `cargo test --test let_mut_reorder --test type_alias_rename` |
| Change compiler-backed behavior or tune rollback | [Semantic validation](architecture/semantic-validation.md) and [tune failure atomicity](specifications/tune-failure-atomicity.md) | `src/style/semantic.rs`, `src/style/engine.rs` | `tests/let_mut_reorder.rs`, snapshot tests | `cargo test --test let_mut_reorder` |
| Change Swift support | [Swift](architecture/swift.md) and [Swift applicability](specifications/swift-style-rule-applicability.md) | `src/style/swift.rs` | `tests/swift_curate.rs` | `cargo test --test swift_curate` |
| Change import or derive ordering | [Rules](architecture/rules.md) and [derive interaction](specifications/style-import-derive-interactions.md) | `src/style/imports.rs`, `src/style/imports/` | `src/style/tests/` import groups | `cargo test --all-targets` |
| Change release or Action behavior | [Automation and release](architecture/testing-and-automation.md) | `action.yml`, `.github/workflows/release.yml` | workflow review | `cargo build --profile final-release --locked --bins` |
| Choose benchmark evidence | [Benchmark runbook](runbooks/benchmark-tracking.md) | `scripts/bench-*.sh` | committed source snapshots and workflow artifacts | selected benchmark command |
| Change documentation | [Agent context](agent-context.md) and [Documentation map](reference/documentation-map.md) | `/openwiki`, `.github/workflows/openwiki-update.yml` | link and coverage review | `openwiki code --update --print` |

## Install and use

Use the stable toolchain selected by `rust-toolchain.toml`. Build with `cargo build --profile final-release --locked --bins`; the binaries are `target/final-release/vstyle` and `target/final-release/cargo-vstyle`. The public commands are:

```sh
vstyle curate --language rust
vstyle tune --language rust --strict
vstyle coverage
```

`curate` exits `1` when violations exist. `tune` exits `0` with remaining violations unless `--strict` is supplied. `--language` is mandatory for `curate` and `tune`; selection supports `--workspace`, `-p/--package`, `--features`, `--all-features`, and `--no-default-features`.

Use `cargo make check-vstyle` to check the current source and `cargo make check` for cross-cutting validation. The broad task includes format checks, strict Clippy, style checks, and nextest.

## Backlog

The map covers current CLI, discovery, rule, fix, compiler-validation, and delivery owners. Historical benchmark artifacts are intentionally not retained in the wiki; use fresh runs or `Benchmarks` workflow artifacts.
