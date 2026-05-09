# Benchmark Tracking

Goal: choose the correct benchmark evidence for a `vstyle` change and keep project-level
performance tracking reproducible.

Read this when: you changed `src/style/*`, `src/fix_engine.rs`, benchmark harnesses, or benchmark
workflow policy and need to decide which performance evidence to collect.

Inputs:

- the changed paths
- whether the working tree includes uncommitted self-host source edits

Depends on:

- `README.md`
- `Makefile.toml`
- `.github/workflows/benchmark.yml`

Verification:

- the selected benchmark command(s) match the touched lane
- local evidence is recorded outside the docs tree when a fresh baseline matters
- project-level evidence is available through the `Benchmarks` workflow after commit or merge

## Pick the benchmark lane

- Run `cargo make bench-release-vstyle` for general workspace-scan changes, including import,
  module, spacing, quality, and fix-engine paths.
- Run `cargo make bench-semantic-vstyle` for `src/style/semantic.rs`, semantic cache-key changes,
  or semantic-validation fallback behavior.
- Run both when a change can affect both the ordinary workspace scan and semantic-positive
  workloads.

## Pre-commit local evidence

- Build the current shipping binary with `cargo build --profile final-release --bins`.
- File discovery follows Git ignore rules only: every non-ignored style file for the selected
  language is scanned, and tracking state does not affect local discovery.
- If the change expands self-host style coverage, run
  `target/final-release/vstyle curate --language rust --workspace` first and fix any newly
  reported repository drift before treating timing results as meaningful.
- For an uncommitted current-worktree snapshot, time the local binary directly:
  - `target/final-release/vstyle curate --language rust --workspace`
  - `target/final-release/vstyle tune --language rust --workspace --verbose`
- Treat those direct timings as pre-commit local evidence only.

## Commit-anchored release harness

- `cargo make bench-release-vstyle` builds the current binary but runs the workload inside a
  detached Git worktree at `HEAD`.
- The detached workload does not include uncommitted files from the primary checkout, even though
  direct local `vstyle` runs include every selected-language, non-ignored style file in the current
  working tree.
- If the current working tree contains uncommitted self-host rewrites, the harness can still fail
  on the detached workload even after the local working tree is clean.
- Use this harness as the authoritative release-path benchmark only after the relevant source
  changes are committed.

## Project-level tracking

- Use the non-blocking `Benchmarks` workflow on `main` for periodic baseline refreshes and
  artifact-backed history.
- Keep meaningful checkpoint results in workflow artifacts or task-local notes when later work needs a
  stable comparison point.
- Do not turn the benchmark workflow into a mandatory PR gate unless runner noise and alert policy
  are already understood.
