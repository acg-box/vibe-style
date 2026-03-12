# vstyle Release Runtime Baseline

## Scope

Task 1 baseline for the release-runtime acceleration plan. This records the first reproducible
release-only benchmark run from the checked-in harness.

## Profile Policy

- Acceptance benchmark profile: `final-release`
- Diagnostic override: `release`
- Rationale: `Cargo.toml` defines `[profile.final-release]` as the shipping profile, so benchmark
  acceptance should use that path. `cargo build --release --bins` remains a compatibility check for
  the original Task 1 verification bundle.

## Benchmark Run

- Benchmark date (UTC): `2026-03-12T07:53:43Z`
- Commit: `e6585772e848aa0743117839070d8bf719f3328d` (`e658577`)
- Binary: `/Users/xavier/code/trusted/y/hack-ink/vibe-style/.worktrees/vstyle-release-runtime-acceleration/target/final-release/vstyle`
- Version: `vibe-style 0.1.15-e658577-aarch64-apple-darwin`
- Log directory: `/Users/xavier/code/trusted/y/hack-ink/vibe-style/.worktrees/vstyle-release-runtime-acceleration/target/vstyle-bench/20260312T075343Z-final-release`

## Results

### Acceptance baseline (`final-release`)

| Command | Exit | Real (s) | User (s) | Sys (s) |
| --- | --- | ---: | ---: | ---: |
| `vstyle curate --workspace` | `0` | `1.44` | `4.29` | `0.08` |
| `vstyle tune --workspace --verbose` | `0` | `4.35` | `12.75` | `0.19` |

### Diagnostic comparison (`release`)

- Benchmark date (UTC): `2026-03-12T07:56:11Z`
- Log directory: `/Users/xavier/code/trusted/y/hack-ink/vibe-style/.worktrees/vstyle-release-runtime-acceleration/target/vstyle-bench/20260312T075611Z-release`

| Command | Exit | Real (s) | User (s) | Sys (s) |
| --- | --- | ---: | ---: | ---: |
| `vstyle curate --workspace` | `0` | `1.36` | `4.02` | `0.07` |
| `vstyle tune --workspace --verbose` | `0` | `4.06` | `11.70` | `0.16` |

## Verification

- `bash -n scripts/bench-release-vstyle.sh`
- `cargo make fmt-toml-check`
- `cargo build --release --bins`
- `./scripts/bench-release-vstyle.sh`
- `VSTYLE_BENCH_PROFILE=release ./scripts/bench-release-vstyle.sh`

## Notes

- The harness builds the local binary and benchmarks inside a disposable Git worktree so `tune`
  does not rewrite the primary checkout.
- Benchmark logs persist under `target/vstyle-bench/...` even though the disposable benchmark
  worktree is removed during cleanup.
- On this host, the diagnostic `release` profile measured slightly faster than `final-release`, but
  acceptance remains pinned to `final-release` because it is the configured shipping profile.

## Post-Task-2 Checkpoint

Task 2 removed eager import-fallback generation on the optimistic path and reduced some
`read_file_context_from_text` string churn in the fix engine. End-to-end runtime on this host stayed
roughly flat across two immediate reruns, so the structural cleanup is landed but Task 3 is still
needed for a material benchmark shift.

### `final-release` rerun 1

- Benchmark date (UTC): `2026-03-12T08:03:00Z`
- Log directory: `/Users/xavier/code/trusted/y/hack-ink/vibe-style/.worktrees/vstyle-release-runtime-acceleration/target/vstyle-bench/20260312T080300Z-final-release`

| Command | Exit | Real (s) | User (s) | Sys (s) |
| --- | --- | ---: | ---: | ---: |
| `vstyle curate --workspace` | `0` | `1.41` | `4.31` | `0.07` |
| `vstyle tune --workspace --verbose` | `0` | `4.38` | `12.90` | `0.20` |

### `final-release` rerun 2

- Benchmark date (UTC): `2026-03-12T08:03:50Z`
- Log directory: `/Users/xavier/code/trusted/y/hack-ink/vibe-style/.worktrees/vstyle-release-runtime-acceleration/target/vstyle-bench/20260312T080350Z-final-release`

| Command | Exit | Real (s) | User (s) | Sys (s) |
| --- | --- | ---: | ---: | ---: |
| `vstyle curate --workspace` | `0` | `1.41` | `4.41` | `0.07` |
| `vstyle tune --workspace --verbose` | `0` | `4.32` | `12.72` | `0.20` |

## Post-Task-3 Checkpoint

Task 3 replaced the intermediate full-workspace `run_check` passes in `tune` with incremental
per-file state refreshes, while keeping the final full pass for end-of-run reporting. On this host,
that change produced the first clear release-path drop in `tune`, from the original `4.35s`
baseline to repeated `2.89s` and `2.93s` runs.

### `final-release` rerun 1

- Benchmark date (UTC): `2026-03-12T08:09:28Z`
- Log directory: `/Users/xavier/code/trusted/y/hack-ink/vibe-style/.worktrees/vstyle-release-runtime-acceleration/target/vstyle-bench/20260312T080928Z-final-release`

| Command | Exit | Real (s) | User (s) | Sys (s) |
| --- | --- | ---: | ---: | ---: |
| `vstyle curate --workspace` | `0` | `1.43` | `4.27` | `0.07` |
| `vstyle tune --workspace --verbose` | `0` | `2.89` | `8.48` | `0.14` |

### `final-release` rerun 2

- Benchmark date (UTC): `2026-03-12T08:10:04Z`
- Log directory: `/Users/xavier/code/trusted/y/hack-ink/vibe-style/.worktrees/vstyle-release-runtime-acceleration/target/vstyle-bench/20260312T081004Z-final-release`

| Command | Exit | Real (s) | User (s) | Sys (s) |
| --- | --- | ---: | ---: | ---: |
| `vstyle curate --workspace` | `0` | `1.39` | `4.22` | `0.07` |
| `vstyle tune --workspace --verbose` | `0` | `2.93` | `8.69` | `0.14` |

### `final-release` rerun 3

- Benchmark date (UTC): `2026-03-12T09:24:58Z`
- Log directory: `/Users/xavier/code/trusted/y/hack-ink/vibe-style/.worktrees/vstyle-release-runtime-acceleration/target/vstyle-bench/20260312T092458Z-final-release`

| Command | Exit | Real (s) | User (s) | Sys (s) |
| --- | --- | ---: | ---: | ---: |
| `vstyle curate --workspace` | `0` | `1.40` | `4.20` | `0.06` |
| `vstyle tune --workspace --verbose` | `0` | `2.87` | `8.47` | `0.11` |
