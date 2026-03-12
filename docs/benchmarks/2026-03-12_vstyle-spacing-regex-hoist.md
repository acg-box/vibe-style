# vstyle Spacing Regex Hoist

## Goal

Preserve the benchmark evidence for the narrowed `XY-99` spacing regex-hoist checkpoint.

## Record Scope

This record captures the fresh baseline and kept reruns for the owning spacing regex-hoist plan.

## Assumptions

- These measurements are point-in-time host-dependent evidence, not normative behavior.
- The owning plan is `docs/plans/2026-03-12_vstyle-spacing-regex-hoist.md`.

## Steps

1. Use the recorded harness and verification commands to reproduce or compare this checkpoint.
2. Read this file together with the owning plan instead of treating it as a primary routing entrypoint.

## Status

- Archived supporting record for the landed `XY-99` checkpoint.

## Scope

Task 1 baseline for the narrowed `XY-99` follow-up lane. This records the fresh
post-`388f09e` self-host release benchmark before any spacing-regex hoist
checkpoint is applied.

## Workload

- Harness: `scripts/bench-release-vstyle.sh`
- Acceptance profile: `final-release`
- Workload shape: self-host workspace benchmark inside a disposable Git worktree
  at the current commit
- Benchmark policy: keep `XY-99` only if repeated release runs beat or at least
  hold this fresh baseline

## Benchmark Run

- Benchmark date (UTC): `2026-03-12T11:56:05Z`
- Benchmark workload commit: `388f09ea445f23603448b078f28404f2eef42f5d` (`388f09e`)
- Binary: `/Users/xavier/code/trusted/y/hack-ink/vibe-style/.worktrees/vstyle-release-runtime-acceleration/target/final-release/vstyle`
- Version: `vibe-style 0.1.15-e658577-aarch64-apple-darwin`
- Log directory: `/Users/xavier/code/trusted/y/hack-ink/vibe-style/.worktrees/vstyle-release-runtime-acceleration/target/vstyle-bench/20260312T115605Z-final-release`

## Results

| Command | Exit | Real (s) | User (s) | Sys (s) |
| --- | --- | ---: | ---: | ---: |
| `vstyle curate --workspace` | `0` | `1.44` | `4.39` | `0.06` |
| `vstyle tune --workspace --verbose` | `0` | `2.85` | `8.58` | `0.08` |

## Verification

- `cargo make bench-release-vstyle`
- `git status --short`

## Candidate A: spacing.rs Static Regex Hoist

This checkpoint hoists the remaining fixed regexes used by the spacing-rule
statement classifier and related helpers into `LazyLock<Regex>` statics near the
top of `src/style/spacing.rs`. The change is intentionally behavior-preserving:
it only removes repeated regex construction from hot-path helpers.

### `final-release` rerun 1

- Benchmark date (UTC): `2026-03-12T11:58:14Z`
- Log directory: `/Users/xavier/code/trusted/y/hack-ink/vibe-style/.worktrees/vstyle-release-runtime-acceleration/target/vstyle-bench/20260312T115814Z-final-release`

| Command | Exit | Real (s) | User (s) | Sys (s) |
| --- | --- | ---: | ---: | ---: |
| `vstyle curate --workspace` | `0` | `0.76` | `1.55` | `0.04` |
| `vstyle tune --workspace --verbose` | `0` | `1.48` | `3.09` | `0.04` |

### `final-release` rerun 2

- Benchmark date (UTC): `2026-03-12T11:58:53Z`
- Log directory: `/Users/xavier/code/trusted/y/hack-ink/vibe-style/.worktrees/vstyle-release-runtime-acceleration/target/vstyle-bench/20260312T115853Z-final-release`

| Command | Exit | Real (s) | User (s) | Sys (s) |
| --- | --- | ---: | ---: | ---: |
| `vstyle curate --workspace` | `0` | `0.72` | `1.47` | `0.04` |
| `vstyle tune --workspace --verbose` | `0` | `1.43` | `2.95` | `0.04` |

### `final-release` rerun 3

- Benchmark date (UTC): `2026-03-12T11:59:05Z`
- Log directory: `/Users/xavier/code/trusted/y/hack-ink/vibe-style/.worktrees/vstyle-release-runtime-acceleration/target/vstyle-bench/20260312T115905Z-final-release`

| Command | Exit | Real (s) | User (s) | Sys (s) |
| --- | --- | ---: | ---: | ---: |
| `vstyle curate --workspace` | `0` | `0.73` | `1.51` | `0.04` |
| `vstyle tune --workspace --verbose` | `0` | `1.40` | `2.89` | `0.04` |

### `final-release` rerun 4 (post-pre-commit state)

- Benchmark date (UTC): `2026-03-12T12:02:34Z`
- Log directory: `/Users/xavier/code/trusted/y/hack-ink/vibe-style/.worktrees/vstyle-release-runtime-acceleration/target/vstyle-bench/20260312T120234Z-final-release`

| Command | Exit | Real (s) | User (s) | Sys (s) |
| --- | --- | ---: | ---: | ---: |
| `vstyle curate --workspace` | `0` | `0.72` | `1.51` | `0.04` |
| `vstyle tune --workspace --verbose` | `0` | `1.43` | `2.99` | `0.04` |

## Current Decision

Keep candidate A. Against the fresh baseline (`curate 1.44s`, `tune 2.85s`),
the retained reruns are materially lower on both commands (`0.76/1.48`,
`0.72/1.43`, `0.73/1.40`, `0.72/1.43`) while `cargo make fmt-check`,
`cargo make lint-rust`, `cargo make test-rust`, `cargo make lint-fix`,
`cargo make fmt`, and `cargo make test` all stayed green.
