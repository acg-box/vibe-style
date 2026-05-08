# vstyle Spacing and Quality Regex Hoist Follow-up

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

The spacing half of this idea was later satisfied by `bb4fae5` / `XY-99`. On
2026-03-13, the lane was resumed long enough to measure the remaining
`quality.rs::has_structured_fields` regex-hoist question from current `main`.
That resumed evaluation did not produce a keepable runtime checkpoint, so the
candidate was reverted and the lane now closes without retained code changes.

## Resumed Evaluation From Current `main`

### Fresh post-`XY-99` baseline

- Benchmark date (UTC): `2026-03-13T05:33:45Z`
- Benchmark workload commit: `81ca390c66cb6237e395083086c8d01918429e92`
  (`81ca390`)
- Binary: `/Users/xavier/code/trusted/y/hack-ink/vibe-style/target/final-release/vstyle`
- Version: `vibe-style 0.1.15-81ca390-aarch64-apple-darwin`
- Log directory: `/Users/xavier/code/trusted/y/hack-ink/vibe-style/target/vstyle-bench/20260313T053345Z-final-release`

| Command | Exit | Real (s) | User (s) | Sys (s) |
| --- | --- | ---: | ---: | ---: |
| `vstyle curate --workspace` | `0` | `0.73` | `1.51` | `0.04` |
| `vstyle tune --workspace --verbose` | `0` | `1.42` | `2.93` | `0.04` |

### Candidate: hoist `has_structured_fields` regexes to `LazyLock`

The only remaining direct regex-hoist candidate in `src/style/quality.rs` was
the pair of `Regex::new(...)` calls inside `has_structured_fields()`. That
candidate was implemented in a local working tree, then measured twice from the
same commit before deciding whether to keep it.

#### Post-patch rerun 1

- Benchmark date (UTC): `2026-03-13T05:35:12Z`
- Log directory: `/Users/xavier/code/trusted/y/hack-ink/vibe-style/target/vstyle-bench/20260313T053512Z-final-release`

| Command | Exit | Real (s) | User (s) | Sys (s) |
| --- | --- | ---: | ---: | ---: |
| `vstyle curate --workspace` | `0` | `0.74` | `1.54` | `0.04` |
| `vstyle tune --workspace --verbose` | `0` | `1.47` | `3.00` | `0.04` |

#### Post-patch rerun 2

- Benchmark date (UTC): `2026-03-13T05:35:45Z`
- Log directory: `/Users/xavier/code/trusted/y/hack-ink/vibe-style/target/vstyle-bench/20260313T053545Z-final-release`

| Command | Exit | Real (s) | User (s) | Sys (s) |
| --- | --- | ---: | ---: | ---: |
| `vstyle curate --workspace` | `0` | `0.74` | `1.54` | `0.04` |
| `vstyle tune --workspace --verbose` | `0` | `1.46` | `3.03` | `0.05` |

### Resumed-lane Verification

- `cargo make fmt-check`
- `cargo make lint-rust`
- `cargo make test-rust`
- `cargo make bench-release-vstyle`

### Final Decision

Do not retain the `quality.rs` regex-hoist candidate. The fresh current-main
baseline already sits at `curate 0.73s` / `tune 1.42s`, and the measured
`has_structured_fields()` hoist reruns came back slower (`1.47s` and `1.46s`
for `tune`). `XY-101` should therefore be treated as evaluated and closed
without a surviving code delta.
