---
type: repository guide
title: vibe-style Wiki Quickstart
description: Canonical map for the vibe-style Rust and Swift style checker, its rule contracts, fix engine, integrations, release workflows, and validation routes.
tags: [vibe-style, navigation, rust, swift]
---

# vibe-style Wiki Quickstart

`vibe-style` is a Rust executable and Cargo subcommand that checks Rust and first-batch Swift source, reports stable rule IDs, and applies conservative deterministic fixes. Source and tests remain authoritative; this wiki compresses the route from an engineering intent to owning symbols, focused tests, and validation.

## Map

- [Architecture overview](architecture/overview.md) explains composition and end-to-end flow.
- [CLI](architecture/cli.md) covers `vstyle`, `cargo-vstyle`, `curate`, `tune`, and `coverage`.
- [Engine](architecture/engine.md) covers discovery, diagnostics, fix rounds, concurrency, and convergence.
- [Rules](architecture/rules.md) is the rule-family and change-surface overview; the complete 43-ID classification is canonical in [style rule backends](specifications/style-rule-backends.md).
- [Semantic validation](architecture/semantic-validation.md) covers Cargo checks, cache lifecycle, and rollback.
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
| Change file selection | [Engine](architecture/engine.md) | `shared::resolve_files` | Swift and workspace fixtures | `cargo test --test swift_curate` |
| Change auto-fix convergence | [Engine](architecture/engine.md) | `run_fix`, `run_fix_round` | `tests/let_mut_reorder.rs`, `type_alias_rename.rs` | `cargo test --test let_mut_reorder --test type_alias_rename` |
| Change compiler-backed behavior or tune rollback | [Semantic validation](architecture/semantic-validation.md) and [tune failure atomicity](specifications/tune-failure-atomicity.md) | `src/style/semantic.rs`, `src/style.rs` | `tests/let_mut_reorder.rs`, snapshot tests | `cargo test --test let_mut_reorder` |
| Change Swift support | [Swift](architecture/swift.md) and [Swift applicability](specifications/swift-style-rule-applicability.md) | `src/style/swift.rs` | `tests/swift_curate.rs` | `cargo test --test swift_curate` |
| Change import or derive ordering | [Rules](architecture/rules.md) and [derive interaction](specifications/style-import-derive-interactions.md) | `src/style/imports.rs` | import/grouping tests | `cargo test --all-targets` |
| Change release or Action behavior | [Automation and release](architecture/testing-and-automation.md) | `action.yml`, `.github/workflows/release.yml` | workflow review | `cargo build --profile final-release --bins` |
| Choose benchmark evidence | [Benchmark runbook](runbooks/benchmark-tracking.md) | `scripts/bench-*.sh` | current-worktree and workflow artifacts | selected benchmark command |
| Change documentation | [Agent context](agent-context.md) and [Documentation map](reference/documentation-map.md) | `/openwiki`, `.github/workflows/openwiki-update.yml` | link and coverage review | `openwiki code --update --print` |

## Install and use

Build with `cargo build --release`; the binaries are `target/release/vstyle` and `target/release/cargo-vstyle`. The public commands are:

```sh
vstyle curate --language rust
vstyle tune --language rust --strict
vstyle coverage
```

`curate` exits `1` when violations exist. `tune` exits `0` with remaining violations unless `--strict` is supplied. `--language` is mandatory for `curate` and `tune`; selection supports `--workspace`, `-p/--package`, `--features`, `--all-features`, and `--no-default-features`.

## Backlog

No source-grounded documentation area is deferred. Historical benchmark artifacts are intentionally not retained; use fresh runs or non-blocking `Benchmarks` workflow artifacts.
