# vstyle Semantic Runtime Baseline

## Goal

Preserve the first semantic-positive benchmark evidence for the semantic runtime follow-up lane.

## Record Scope

This record captures the checked-in semantic benchmark baseline and later retained reruns for the
owning semantic follow-up plan.

## Assumptions

- These measurements are point-in-time host-dependent evidence, not normative behavior.
- The owning plan is `docs/plans/2026-03-12_vstyle-semantic-runtime-follow-up.md`.

## Steps

1. Use the harness and verification commands recorded below to reproduce or compare this lane.
2. Read this file together with the owning plan instead of treating it as a primary routing entrypoint.

## Status

- Archived supporting record for the owning semantic follow-up plan.

## Scope

Task 1 baseline for the semantic runtime follow-up plan. This records the first reproducible
semantic-positive benchmark run from the checked-in harness.

## Workload

- Fixture source: `tests/let_mut_reorder.rs`
- Workload shape: a disposable Git-tracked Cargo crate with one semantically safe `let mut`
  reorder candidate and one unsafe closure-capture case that must remain unchanged.
- Cold run policy: clear `target/vstyle-cache/semantic` before the first `tune --verbose` run.
- Warm run policy: restore the original fixture sources and rerun without clearing the cache.

## Profile Policy

- Acceptance benchmark profile: `final-release`
- Diagnostic override: `release`
- Rationale: semantic-path acceptance should match the shipping profile used by the existing
  release-runtime lane, while keeping a `release` override available for local diagnosis.

## Benchmark Run

- Benchmark date (UTC): `2026-03-12T10:45:06Z`
- Benchmark workload commit: `da03b17b1cc73f0afe72eb4a8c81496ab542ac39` (`da03b17`)
- Binary: `/Users/xavier/code/trusted/y/hack-ink/vibe-style/.worktrees/vstyle-release-runtime-acceleration/target/final-release/vstyle`
- Version: `vibe-style 0.1.15-e658577-aarch64-apple-darwin`
- Log directory: `/Users/xavier/code/trusted/y/hack-ink/vibe-style/.worktrees/vstyle-release-runtime-acceleration/target/vstyle-bench-semantic/20260312T104506Z-final-release`

## Results

| Run | Exit | Real (s) | User (s) | Sys (s) | Cache Hits | Cache Misses |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| Cold `vstyle tune --verbose` | `0` | `0.26` | `0.17` | `0.09` | `2` | `1` |
| Warm `vstyle tune --verbose` | `0` | `0.15` | `0.10` | `0.04` | `3` | `0` |

## Verification

- `bash -n scripts/bench-semantic-vstyle.sh`
- `cargo make fmt-toml-check`
- `cargo build --profile final-release --bins`
- `cargo make bench-semantic-vstyle`

## Notes

- The cold run already shows intra-invocation semantic cache reuse (`2` hits, `1` miss), so Task 2
  should distinguish repeated semantic `cargo check` work from cache-key overhead rather than treat
  semantic validation as purely uncached.
- The warm rerun drops to `0.15s` with `3` hits and `0` misses after restoring the original source
  texts, which proves the current semantic cache is reusable across invocations when tracked-file
  fingerprints return to the same state.
- This checkpoint only adds the benchmark harness and docs, so the binary version string still
  reflects the last Rust-code build metadata refresh. Use the benchmark workload commit and log
  directory as the authoritative Task 1 identity.

## Post-Task-2 Checkpoint

Task 2 reused the baseline semantic output for import-suggestion handling and skipped the
post-validation semantic rerun when no semantic edits occurred. On the current semantic-positive
fixture, that removes two redundant semantic-path cache lookups from the cold path while preserving
the warm-cache behavior.

- Binary source state: local Task 2 working tree on top of `77fb038`
- Acceptance signal: cold semantic cache stats moved from `2` hits / `1` miss to `0` hits / `1`
  miss, while the warm rerun now reports `1` hit / `0` misses.

### `final-release` rerun 1

- Benchmark date (UTC): `2026-03-12T10:51:22Z`
- Log directory: `/Users/xavier/code/trusted/y/hack-ink/vibe-style/.worktrees/vstyle-release-runtime-acceleration/target/vstyle-bench-semantic/20260312T105122Z-final-release`

| Run | Exit | Real (s) | User (s) | Sys (s) | Cache Hits | Cache Misses |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| Cold `vstyle tune --verbose` | `0` | `0.18` | `0.13` | `0.07` | `0` | `1` |
| Warm `vstyle tune --verbose` | `0` | `0.08` | `0.06` | `0.02` | `1` | `0` |

### `final-release` rerun 2

- Benchmark date (UTC): `2026-03-12T10:52:38Z`
- Log directory: `/Users/xavier/code/trusted/y/hack-ink/vibe-style/.worktrees/vstyle-release-runtime-acceleration/target/vstyle-bench-semantic/20260312T105238Z-final-release`

| Run | Exit | Real (s) | User (s) | Sys (s) | Cache Hits | Cache Misses |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| Cold `vstyle tune --verbose` | `0` | `0.19` | `0.12` | `0.07` | `0` | `1` |
| Warm `vstyle tune --verbose` | `0` | `0.08` | `0.06` | `0.02` | `1` | `0` |

### `final-release` rerun 3 (post-pre-commit state)

- Benchmark date (UTC): `2026-03-12T10:55:22Z`
- Log directory: `/Users/xavier/code/trusted/y/hack-ink/vibe-style/.worktrees/vstyle-release-runtime-acceleration/target/vstyle-bench-semantic/20260312T105522Z-final-release`

| Run | Exit | Real (s) | User (s) | Sys (s) | Cache Hits | Cache Misses |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| Cold `vstyle tune --verbose` | `0` | `0.19` | `0.12` | `0.07` | `0` | `1` |
| Warm `vstyle tune --verbose` | `0` | `0.08` | `0.06` | `0.02` | `1` | `0` |

## Closeout

`XY-95` now has a semantic-positive benchmark, a commit-backed optimization, and an end-to-end test
that locks the cold/warm semantic cache counts on the `let_mut_reorder` fixture. This lane can
close as done unless a future semantic workload reveals a different hotspot than duplicate
semantic-check reuse.
