---
type: Runbook
title: Benchmark Tracking
description: Select release or semantic benchmark evidence, account for detached-worktree behavior, collect pre-commit timings, and use workflow artifacts for project tracking.
tags: [runbook, benchmarks, performance]
openwiki:
  roles: [operations, testing]
  change_kinds: [benchmark, performance]
  source_paths: [scripts/bench-release-vstyle.sh, scripts/bench-semantic-vstyle.sh, Makefile.toml, .github/workflows/benchmark.yml]
  symbols: [bench-release-vstyle, bench-semantic-vstyle]
  validation_commands: [cargo make bench-release-vstyle]
---

# Benchmark Tracking

**Goal:** Choose reproducible performance evidence for a `vstyle` change.

**Read this when:** changing `src/style/*` (including the edit application in `src/style/fixes.rs`), benchmark scripts, or benchmark workflow policy.

**Inputs:** changed paths and whether the worktree contains uncommitted self-host edits. **Depends on:** `Makefile.toml`, benchmark scripts, and `.github/workflows/benchmark.yml`. **Verification:** selected lane matches the touched behavior and results are kept outside the generated OpenWiki tree.

## Select a lane

- Use `cargo make bench-release-vstyle` for workspace scan, imports, modules, spacing, quality, and fix-engine changes.
- Use `cargo make bench-semantic-vstyle` for `src/style/semantic.rs`, semantic cache keys, or compiler fallback changes.
- Run both when ordinary scanning and semantic-positive workloads can change.

## Local versus commit-anchored evidence

Build with `cargo build --profile final-release --bins`. Direct `target/final-release/vstyle` timings observe every selected-language, non-ignored file in the current worktree and are appropriate pre-commit evidence. If self-host coverage expanded, curate first and repair drift.

The release harness builds the local binary but runs it in a detached worktree at `HEAD`, so uncommitted files in the primary checkout are absent. Treat it as authoritative release-path evidence only after relevant changes are committed. It runs `curate --language rust --workspace` and `tune --language rust --workspace --verbose`; output is under `target/vstyle-bench/`.

The semantic harness builds a temporary Cargo fixture with safe and unsafe `let mut` cases, runs cold and warm `tune --language rust --verbose`, records timing and semantic cache counts under `target/vstyle-bench-semantic/`, and removes the fixture. A release run showing `Semantic cache: 0 hit(s), 0 miss(es)` did not exercise semantic validation.

## Project tracking

The scheduled/push/manual `Benchmarks` workflow runs release and semantic matrix jobs, publishes summaries, and uploads artifacts with 14-day retention. Use those artifacts for cross-run comparison; do not make the workflow a PR gate without an understood noise and alert policy.
