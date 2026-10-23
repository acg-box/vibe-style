---
type: Runbook
title: Benchmark Tracking
description: Select release or semantic benchmark evidence, account for commit-anchored source snapshots, collect pre-commit timings, and use workflow artifacts for project tracking.
tags: [runbook, benchmarks, performance]
openwiki:
  roles: [operations, testing]
  change_kinds: [benchmark, performance]
  source_paths: [scripts/benchmark-common.bash, scripts/bench-release-vstyle.sh, scripts/bench-semantic-vstyle.sh, Makefile.toml, .github/workflows/benchmark.yml]
  symbols: [bench-release-vstyle, bench-semantic-vstyle]
  validation_commands: [cargo make bench-release-vstyle]
verified:
  - by: openwiki/0.7.2
    at: 2026-10-23T01:52:10.637Z
sources:
  - id: openwiki-source-61a12770dd6258a4227672cd
    resource: repo://.github/workflows/benchmark.yml
  - id: openwiki-source-111747efc627ea82ca555afb
    resource: repo://scripts/bench-release-vstyle.sh
  - id: openwiki-source-59a609e749db71d05073a492
    resource: repo://scripts/bench-semantic-vstyle.sh
  - id: openwiki-source-198fe29b6b95e264cfe6cc70
    resource: repo://scripts/benchmark-common.bash
generated: { by: "codex", at: "2026-10-23T01:52:10.637Z" }
---

# Benchmark Tracking

**Goal:** Choose reproducible performance evidence for a `vstyle` change.

**Read this when:** changing `src/style/*` (including the edit application in `src/style/fixes.rs`), benchmark scripts, or benchmark workflow policy.

**Inputs:** changed paths and whether the worktree contains uncommitted self-host edits. **Depends on:** `Makefile.toml`, benchmark scripts, and `.github/workflows/benchmark.yml`. **Verification:** selected lane matches the touched behavior and results are kept outside the generated OpenWiki tree.

## Select a lane

- Use `cargo make bench-release-vstyle` for workspace scan, imports, modules, spacing, quality, and fix-engine changes.
- Use `cargo make bench-semantic-vstyle` for `src/style/semantic.rs`, compiler-validation invalidation, or recovery changes.
- Run both when ordinary scanning and semantic-positive workloads can change.

## Local versus commit-anchored evidence

Build with `cargo build --profile final-release --locked --bins`. Direct `target/final-release/vstyle` timings observe every selected-language, non-ignored file in the current worktree and are appropriate pre-commit evidence. If self-host coverage expanded, curate first and repair drift.

The release harness builds the local binary but runs it in an owned temporary source snapshot made with `git archive HEAD`, so uncommitted source files in the primary checkout are absent from the scenario. The snapshot has its own Git repository for ignore-aware discovery. Treat it as authoritative release-path evidence only after relevant changes are committed. It runs `curate --language rust --workspace` and `tune --language rust --workspace --verbose`; output is under `target/vstyle-bench/`.

The semantic harness builds a temporary Cargo fixture with safe and unsafe `let mut` cases, runs cold and warm `tune --language rust --verbose`, records timing and process exit status under `target/vstyle-bench-semantic/`, and removes the fixture. Both runs invoke Cargo; its own incremental state makes the second run warm. A no-op release self-tune does not exercise compiler validation. The harnesses share profile parsing, locked builds, metadata, and timing in `scripts/benchmark-common.bash`. Each removes only its owned temporary source directory on exit.

## Project tracking

The scheduled/push/manual `Benchmarks` workflow runs release and semantic steps in one job, publishes separate summaries, and uploads successful artifacts with 14-day retention. A final gate requires both outcomes to succeed. Use those artifacts for cross-run comparison; do not make the workflow a PR gate without an understood noise and alert policy.
