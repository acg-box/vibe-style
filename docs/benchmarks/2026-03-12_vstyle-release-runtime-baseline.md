# vstyle Release Runtime Baseline

## Goal

Preserve the first reproducible release-only benchmark evidence for the release-runtime acceleration lane.

## Record Scope

This record captures the checked-in harness baseline and later checkpoint reruns for the owning
release-runtime plan.

## Assumptions

- These measurements are point-in-time host-dependent evidence, not normative behavior.
- The owning plan is `docs/plans/2026-03-11_vstyle-release-runtime-acceleration.md`.

## Steps

1. Use the harness and verification commands recorded below to reproduce or compare this lane.
2. Read this file together with the owning plan instead of treating it as a primary routing entrypoint.

## Status

- Archived supporting record for the owning release-runtime plan.

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

## Post-Task-4 Checkpoint

Task 4 introduced shared per-file import analysis so IMPORT-004/008/009/011 and cfg-test
follow-up checks can reuse `use` runs, symbol maps, and qualified path maps instead of rebuilding
the same AST-derived state repeatedly. On this host, end-to-end `tune` stayed within the existing
Task 3 noise band, so this checkpoint is best treated as structural groundwork rather than a new
runtime win.

- Binary source state: local Task 4 working tree on top of `9b2d59e`
- Benchmark workload commit: `9b2d59ef5e980791555c19f18f8e5a5c70f96a8d` (`9b2d59e`)

### `final-release` rerun 1

- Benchmark date (UTC): `2026-03-12T09:34:59Z`
- Log directory: `/Users/xavier/code/trusted/y/hack-ink/vibe-style/.worktrees/vstyle-release-runtime-acceleration/target/vstyle-bench/20260312T093459Z-final-release`

| Command | Exit | Real (s) | User (s) | Sys (s) |
| --- | --- | ---: | ---: | ---: |
| `vstyle curate --workspace` | `0` | `1.40` | `4.09` | `0.05` |
| `vstyle tune --workspace --verbose` | `0` | `2.98` | `8.51` | `0.11` |

### `final-release` rerun 2

- Benchmark date (UTC): `2026-03-12T09:36:40Z`
- Log directory: `/Users/xavier/code/trusted/y/hack-ink/vibe-style/.worktrees/vstyle-release-runtime-acceleration/target/vstyle-bench/20260312T093640Z-final-release`

| Command | Exit | Real (s) | User (s) | Sys (s) |
| --- | --- | ---: | ---: | ---: |
| `vstyle curate --workspace` | `0` | `1.40` | `4.19` | `0.06` |
| `vstyle tune --workspace --verbose` | `0` | `2.88` | `8.32` | `0.10` |

### `final-release` rerun 3

- Benchmark date (UTC): `2026-03-12T09:39:24Z`
- Log directory: `/Users/xavier/code/trusted/y/hack-ink/vibe-style/.worktrees/vstyle-release-runtime-acceleration/target/vstyle-bench/20260312T093924Z-final-release`

| Command | Exit | Real (s) | User (s) | Sys (s) |
| --- | --- | ---: | ---: | ---: |
| `vstyle curate --workspace` | `0` | `1.44` | `4.19` | `0.05` |
| `vstyle tune --workspace --verbose` | `0` | `2.92` | `8.48` | `0.11` |

## Post-Task-5 Decision

Task 5 explored module/quality scan fusion and regex-hoist candidates, but the fresh release runs did
not beat the existing Task 3/4 band reliably enough to keep. Those experiments were reverted before
continuing, and the post-revert rerun became the fresh baseline for Task 6.

- Experimental `final-release tune` range before revert: `2.95s` to `3.07s`
- Decision: do not retain the Task 5 patch set; keep module/quality follow-ups queued

### Post-revert `final-release` baseline

- Benchmark date (UTC): `2026-03-12T10:06:12Z`
- Log directory: `/Users/xavier/code/trusted/y/hack-ink/vibe-style/.worktrees/vstyle-release-runtime-acceleration/target/vstyle-bench/20260312T100612Z-final-release`

| Command | Exit | Real (s) | User (s) | Sys (s) |
| --- | --- | ---: | ---: | ---: |
| `vstyle curate --workspace` | `0` | `1.46` | `4.29` | `0.06` |
| `vstyle tune --workspace --verbose` | `0` | `2.99` | `8.55` | `0.12` |

## Post-Task-6 Checkpoint

Task 6 stayed evidence-gated. The current self-host benchmark is still a no-op `tune`
(`Checked 21 file(s). Applied 0 fix(es). Semantic cache: 0 hit(s), 0 miss(es).`), so semantic
follow-ups remain queued. The landed checkpoint only targets discovery-path reuse by caching
workspace metadata and tracked-file discovery, then grouping workspace files into per-package
scopes directly instead of rediscovering each package scope separately.

On this host, that narrowed checkpoint moved `final-release tune` from the fresh `2.99s`
post-revert baseline to one faster rerun at `2.84s`, two follow-up reruns at `2.93s`, and a final
post-gate rerun at `2.95s`. That is best treated as a modest discovery-path improvement rather than
a new major step-change.

### `final-release` rerun 1

- Benchmark date (UTC): `2026-03-12T10:14:53Z`
- Log directory: `/Users/xavier/code/trusted/y/hack-ink/vibe-style/.worktrees/vstyle-release-runtime-acceleration/target/vstyle-bench/20260312T101453Z-final-release`

| Command | Exit | Real (s) | User (s) | Sys (s) |
| --- | --- | ---: | ---: | ---: |
| `vstyle curate --workspace` | `0` | `1.48` | `4.37` | `0.07` |
| `vstyle tune --workspace --verbose` | `0` | `2.84` | `8.41` | `0.09` |

### `final-release` rerun 2

- Benchmark date (UTC): `2026-03-12T10:15:31Z`
- Log directory: `/Users/xavier/code/trusted/y/hack-ink/vibe-style/.worktrees/vstyle-release-runtime-acceleration/target/vstyle-bench/20260312T101531Z-final-release`

| Command | Exit | Real (s) | User (s) | Sys (s) |
| --- | --- | ---: | ---: | ---: |
| `vstyle curate --workspace` | `0` | `1.46` | `4.31` | `0.06` |
| `vstyle tune --workspace --verbose` | `0` | `2.93` | `8.49` | `0.09` |

### `final-release` rerun 3

- Benchmark date (UTC): `2026-03-12T10:15:46Z`
- Log directory: `/Users/xavier/code/trusted/y/hack-ink/vibe-style/.worktrees/vstyle-release-runtime-acceleration/target/vstyle-bench/20260312T101546Z-final-release`

| Command | Exit | Real (s) | User (s) | Sys (s) |
| --- | --- | ---: | ---: | ---: |
| `vstyle curate --workspace` | `0` | `1.50` | `4.34` | `0.06` |
| `vstyle tune --workspace --verbose` | `0` | `2.93` | `8.63` | `0.10` |

### `final-release` rerun 4 (post-pre-commit state)

- Benchmark date (UTC): `2026-03-12T10:19:13Z`
- Log directory: `/Users/xavier/code/trusted/y/hack-ink/vibe-style/.worktrees/vstyle-release-runtime-acceleration/target/vstyle-bench/20260312T101913Z-final-release`

| Command | Exit | Real (s) | User (s) | Sys (s) |
| --- | --- | ---: | ---: | ---: |
| `vstyle curate --workspace` | `0` | `1.47` | `4.36` | `0.06` |
| `vstyle tune --workspace --verbose` | `0` | `2.95` | `8.66` | `0.10` |
