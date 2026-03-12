# vstyle Spacing Regex Runtime Follow-up

## Goal

Preserve the prototype benchmark evidence for the drafted `XY-100` spacing follow-up lane.

## Record Scope

This record captures prototype runs that remained historical evidence after the actual spacing
checkpoint landed elsewhere.

## Assumptions

- These measurements are point-in-time host-dependent evidence, not normative behavior.
- The owning plan is `docs/plans/2026-03-12_vstyle-spacing-regex-follow-up.md`.

## Steps

1. Use this file only to understand the drafted lane's historical evidence.
2. Route current execution to the already-landed `XY-99` plan and benchmark instead.

## Status

- Archived supporting record for a closed draft lane.

## Scope

Archived prototype notes for the drafted `XY-100` spacing follow-up. These runs
were collected while the spacing regex-hoist was still only in the working tree.

## Workload

- Harness: `scripts/bench-release-vstyle.sh`
- Acceptance profile: `final-release`
- Workload shape: self-host workspace benchmark inside a disposable Git worktree
  at the current commit
- Benchmark policy: do not treat these runs as the clean baseline for the
  landed spacing checkpoint

## Prototype Run 1

- Benchmark date (UTC): `2026-03-12T11:57:35Z`
- Benchmark workload commit: `388f09ea445f23603448b078f28404f2eef42f5d` (`388f09e`)
- Binary: `/Users/xavier/code/trusted/y/hack-ink/vibe-style/.worktrees/vstyle-release-runtime-acceleration/target/final-release/vstyle`
- Version: `vibe-style 0.1.15-388f09e-aarch64-apple-darwin`
- Log directory: `/Users/xavier/code/trusted/y/hack-ink/vibe-style/.worktrees/vstyle-release-runtime-acceleration/target/vstyle-bench/20260312T115735Z-final-release`

| Command | Exit | Real (s) | User (s) | Sys (s) |
| --- | --- | ---: | ---: | ---: |
| `vstyle curate --workspace` | `0` | `0.77` | `1.60` | `0.04` |
| `vstyle tune --workspace --verbose` | `0` | `1.44` | `3.06` | `0.06` |

## Prototype Run 2

- Benchmark date (UTC): `2026-03-12T11:58:15Z`
- Log directory: `/Users/xavier/code/trusted/y/hack-ink/vibe-style/.worktrees/vstyle-release-runtime-acceleration/target/vstyle-bench/20260312T115815Z-final-release`

| Command | Exit | Real (s) | User (s) | Sys (s) |
| --- | --- | ---: | ---: | ---: |
| `vstyle curate --workspace` | `0` | `0.75` | `1.54` | `0.03` |
| `vstyle tune --workspace --verbose` | `0` | `1.44` | `2.98` | `0.04` |

## Observations

- These runs were captured on an uncommitted working-tree prototype while
  `HEAD` still reported `388f09e`, so they are not a clean commit-level
  baseline.
- The actual committed spacing checkpoint is `bb4fae5`.
- The source-of-truth benchmark note for the landed spacing change is
  `docs/benchmarks/2026-03-12_vstyle-spacing-regex-hoist.md`.

## Verification

- `cargo make bench-release-vstyle`
- `git status --short`

## Current Decision

Archived only. `XY-100` was covered by `bb4fae5` / `XY-99`, so no separate
`XY-100` code delta remains.
