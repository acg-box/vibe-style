# vstyle Spacing and Quality Regex Hoist Follow-up

## Goal

Preserve the historical benchmark evidence for the drafted `XY-101` spacing-and-quality follow-up lane.

## Record Scope

This record captures the archived broader regex-hoist baseline that now survives only as supporting
evidence for the owning follow-up plan.

## Assumptions

- These measurements are point-in-time host-dependent evidence, not normative behavior.
- The owning plan is `docs/plans/2026-03-12_vstyle-spacing-quality-regex-hoist-follow-up.md`.

## Steps

1. Use this file only as historical evidence for the owning plan.
2. Refresh the baseline from current `main` before resuming any future execution.

## Status

- Archived supporting record for a queued follow-up note.

## Scope

Archived baseline for the drafted `XY-101` broader regex-hoist follow-up,
captured before the spacing-only checkpoint landed.

## Workload

- Harness: `scripts/bench-release-vstyle.sh`
- Acceptance profile: `final-release`
- Workload shape: self-host workspace benchmark inside a disposable Git worktree
  at the current commit
- Benchmark policy: use this note as historical reference only; refresh the
  baseline from current `main` before resuming `XY-101`

## Benchmark Run

- Benchmark date (UTC): `2026-03-12T11:55:32Z`
- Benchmark workload commit: `388f09ea445f23603448b078f28404f2eef42f5d` (`388f09e`)
- Binary: `/Users/xavier/code/trusted/y/hack-ink/vibe-style/.worktrees/vstyle-release-runtime-acceleration/target/final-release/vstyle`
- Version: `vibe-style 0.1.15-e658577-aarch64-apple-darwin`
- Log directory: `/Users/xavier/code/trusted/y/hack-ink/vibe-style/.worktrees/vstyle-release-runtime-acceleration/target/vstyle-bench/20260312T115532Z-final-release`

## Results

| Command | Exit | Real (s) | User (s) | Sys (s) |
| --- | --- | ---: | ---: | ---: |
| `vstyle curate --workspace` | `0` | `1.46` | `4.43` | `0.06` |
| `vstyle tune --workspace --verbose` | `0` | `2.92` | `8.45` | `0.08` |

## Verification

- `cargo make bench-release-vstyle`
- `git status --short`

## Current Decision

Pending as a future lane only. The spacing half of this idea was later
satisfied by `bb4fae5` / `XY-99`; any resumed `XY-101` work should refresh the
baseline from current `main` and focus on remaining quality-side regex helpers.
