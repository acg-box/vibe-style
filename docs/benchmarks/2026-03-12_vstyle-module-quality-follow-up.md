# vstyle Module and Quality Runtime Follow-up

## Goal

Preserve the benchmark evidence for the narrowed `XY-97` module/quality follow-up lane.

## Record Scope

This record captures the fresh post-`XY-95` self-host release baseline and the evaluated narrowed
candidate for the owning follow-up plan.

## Assumptions

- These measurements are point-in-time host-dependent evidence, not normative behavior.
- The owning plan is `docs/plans/2026-03-12_vstyle-module-quality-follow-up.md`.

## Steps

1. Use the harness and verification commands recorded below to reproduce or compare this lane.
2. Read this file together with the owning plan instead of treating it as a primary routing entrypoint.

## Status

- Archived supporting record for the owning follow-up plan.

## Scope

Task 1 baseline for the narrowed `XY-97` follow-up lane. This records the fresh post-`XY-95`
self-host release benchmark before any new module/quality checkpoint is applied.

## Workload

- Harness: `scripts/bench-release-vstyle.sh`
- Acceptance profile: `final-release`
- Workload shape: self-host workspace benchmark inside a disposable Git worktree at the current
  commit
- Benchmark policy: compare any narrowed `XY-97` checkpoint only against this post-`55d2af4`
  baseline, not against the broader reverted Task 5 experiment

## Benchmark Run

- Benchmark date (UTC): `2026-03-12T11:06:47Z`
- Benchmark workload commit: `55d2af49090cb195ddb25a6d9abd287270555079` (`55d2af4`)
- Binary: `/Users/xavier/code/trusted/y/hack-ink/vibe-style/.worktrees/vstyle-release-runtime-acceleration/target/final-release/vstyle`
- Version: `vibe-style 0.1.15-e658577-aarch64-apple-darwin`
- Log directory: `/Users/xavier/code/trusted/y/hack-ink/vibe-style/.worktrees/vstyle-release-runtime-acceleration/target/vstyle-bench/20260312T110647Z-final-release`

## Results

| Command | Exit | Real (s) | User (s) | Sys (s) |
| --- | --- | ---: | ---: | ---: |
| `vstyle curate --workspace` | `0` | `1.45` | `4.36` | `0.07` |
| `vstyle tune --workspace --verbose` | `0` | `2.88` | `8.53` | `0.09` |

## Verification

- `cargo make bench-release-vstyle`
- `git status --short`

## Candidate A: Regex Hoist and Macro-Call Reuse (Reverted)

The first narrowed candidate stayed intentionally small:

1. hoist the structured-logging and `macro_rules!` name regex compilation out of per-call helper
   paths
2. precompute nested-scope macro-call names once per `ItemList` instead of rescanning the same
   descendant tree for every `macro_rules!` item

It looked promising on the first two reruns, but it did not survive later post-gate reruns cleanly
enough to justify a commit.

### `final-release` rerun 1

- Benchmark date (UTC): `2026-03-12T11:09:16Z`
- Log directory: `/Users/xavier/code/trusted/y/hack-ink/vibe-style/.worktrees/vstyle-release-runtime-acceleration/target/vstyle-bench/20260312T110916Z-final-release`

| Command | Exit | Real (s) | User (s) | Sys (s) |
| --- | --- | ---: | ---: | ---: |
| `vstyle curate --workspace` | `0` | `1.44` | `4.35` | `0.06` |
| `vstyle tune --workspace --verbose` | `0` | `2.85` | `8.42` | `0.08` |

### `final-release` rerun 2

- Benchmark date (UTC): `2026-03-12T11:09:49Z`
- Log directory: `/Users/xavier/code/trusted/y/hack-ink/vibe-style/.worktrees/vstyle-release-runtime-acceleration/target/vstyle-bench/20260312T110949Z-final-release`

| Command | Exit | Real (s) | User (s) | Sys (s) |
| --- | --- | ---: | ---: | ---: |
| `vstyle curate --workspace` | `0` | `1.43` | `4.25` | `0.06` |
| `vstyle tune --workspace --verbose` | `0` | `2.82` | `8.43` | `0.08` |

### `final-release` rerun 3 (post-gate state)

- Benchmark date (UTC): `2026-03-12T11:11:40Z`
- Log directory: `/Users/xavier/code/trusted/y/hack-ink/vibe-style/.worktrees/vstyle-release-runtime-acceleration/target/vstyle-bench/20260312T111140Z-final-release`

| Command | Exit | Real (s) | User (s) | Sys (s) |
| --- | --- | ---: | ---: | ---: |
| `vstyle curate --workspace` | `0` | `1.48` | `4.37` | `0.06` |
| `vstyle tune --workspace --verbose` | `0` | `2.93` | `8.48` | `0.10` |

### `final-release` rerun 4 (confirmation)

- Benchmark date (UTC): `2026-03-12T11:12:23Z`
- Log directory: `/Users/xavier/code/trusted/y/hack-ink/vibe-style/.worktrees/vstyle-release-runtime-acceleration/target/vstyle-bench/20260312T111223Z-final-release`

| Command | Exit | Real (s) | User (s) | Sys (s) |
| --- | --- | ---: | ---: | ---: |
| `vstyle curate --workspace` | `0` | `1.46` | `4.28` | `0.06` |
| `vstyle tune --workspace --verbose` | `0` | `2.97` | `8.82` | `0.09` |

Decision: revert candidate A. The early wins were not stable once the patch was measured again from
the final post-gate state.

## Candidate B: Expect/Unwrap Front-End Filtering

The current narrowed checkpoint targets `check_expect_unwrap` instead of regex/macro helpers.
Inside the workspace, a simple grep finds about `4078` method-call sites but only about `683`
`.unwrap(` / `.expect(` occurrences, so the unwrap/expect rule was paying for a large amount of
irrelevant `method_call_in_test_context` work.

This candidate only changes one thing: filter `MethodCallExpr` nodes by method name before calling
`method_call_in_test_context` or computing line numbers.

### `final-release` rerun 1

- Benchmark date (UTC): `2026-03-12T11:13:56Z`
- Log directory: `/Users/xavier/code/trusted/y/hack-ink/vibe-style/.worktrees/vstyle-release-runtime-acceleration/target/vstyle-bench/20260312T111356Z-final-release`

| Command | Exit | Real (s) | User (s) | Sys (s) |
| --- | --- | ---: | ---: | ---: |
| `vstyle curate --workspace` | `0` | `1.46` | `4.41` | `0.06` |
| `vstyle tune --workspace --verbose` | `0` | `2.88` | `8.58` | `0.08` |

### `final-release` rerun 2

- Benchmark date (UTC): `2026-03-12T11:14:29Z`
- Log directory: `/Users/xavier/code/trusted/y/hack-ink/vibe-style/.worktrees/vstyle-release-runtime-acceleration/target/vstyle-bench/20260312T111429Z-final-release`

| Command | Exit | Real (s) | User (s) | Sys (s) |
| --- | --- | ---: | ---: | ---: |
| `vstyle curate --workspace` | `0` | `1.45` | `4.36` | `0.06` |
| `vstyle tune --workspace --verbose` | `0` | `2.83` | `8.46` | `0.07` |

### `final-release` rerun 3 (post-pre-commit state)

- Benchmark date (UTC): `2026-03-12T11:16:44Z`
- Log directory: `/Users/xavier/code/trusted/y/hack-ink/vibe-style/.worktrees/vstyle-release-runtime-acceleration/target/vstyle-bench/20260312T111644Z-final-release`

| Command | Exit | Real (s) | User (s) | Sys (s) |
| --- | --- | ---: | ---: | ---: |
| `vstyle curate --workspace` | `0` | `1.45` | `4.31` | `0.06` |
| `vstyle tune --workspace --verbose` | `0` | `2.85` | `8.60` | `0.08` |

## Current Decision

Candidate B is still modest, but it is narrower than candidate A and all retained measurements are
at or below the fresh `2.88s` baseline (`2.88s`, `2.83s`, `2.85s`) instead of drifting above it.
That is enough to keep as the `XY-97` checkpoint for this lane.
