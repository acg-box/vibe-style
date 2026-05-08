# vstyle Benchmark Tracking Bootstrap

## Scope

Bootstrap project-level benchmark tracking after adding the non-blocking `Benchmarks` workflow and
after extending `IMPORT-008` to shorten qualified value-receiver paths inside macro token trees.

## Workload

- Release pre-commit snapshot: direct current-worktree runs of
  `target/final-release/vstyle curate --workspace` and
  `target/final-release/vstyle tune --workspace --verbose`
- Semantic benchmark: `cargo make bench-semantic-vstyle`
- Commit-anchored release-harness diagnostic: `cargo make bench-release-vstyle`

## Benchmark Run

- Benchmark date (UTC): `2026-03-14T08:18:04Z`
- Commit anchor: `980edabd6ec394ee18b00bdda87006d45e7e8741` (`980edab`)
- Binary: `/Users/xavier/code/trusted/y/hack-ink/vibe-style/target/final-release/vstyle`
- Version: `vibe-style 0.1.16-980edab-aarch64-apple-darwin`
- Semantic log directory:
  `/Users/xavier/code/trusted/y/hack-ink/vibe-style/target/vstyle-bench-semantic/20260314T081804Z-final-release`
- Release-harness diagnostic log directory:
  `/Users/xavier/code/trusted/y/hack-ink/vibe-style/target/vstyle-bench/20260314T081621Z-final-release`

## Results

### Current-worktree release snapshot

These timings come from the local current working tree after self-hosting the new `IMPORT-008`
shortening opportunities that the updated rule exposed inside `vibe-style` itself.

| Command | Exit | Real (s) | User (s) | Sys (s) |
| --- | --- | ---: | ---: | ---: |
| `target/final-release/vstyle curate --workspace` | `0` | `1.07` | `1.61` | `0.05` |
| `target/final-release/vstyle tune --workspace --verbose` | `0` | `0.73` | `1.60` | `0.03` |

### Semantic benchmark snapshot

| Run | Exit | Real (s) | User (s) | Sys (s) | Cache Hits | Cache Misses |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| Cold `vstyle tune --verbose` | `0` | `0.17` | `0.11` | `0.06` | `0` | `1` |
| Warm `vstyle tune --verbose` | `0` | `0.08` | `0.05` | `0.02` | `1` | `0` |

### Commit-anchored release harness diagnostic

The checked-in release harness still fails before commit because it benchmarks a detached `HEAD`
worktree instead of the current uncommitted working tree.

| Command | Exit | Real (s) | User (s) | Sys (s) |
| --- | --- | ---: | ---: | ---: |
| `cargo make bench-release-vstyle` | `1` | `0.76` | `1.54` | `0.05` |

- Failure point: `CURATE_EXIT=1`
- Cause: detached `HEAD` still lacks the self-host import-shortening rewrites now present in the
  current working tree, so the harness sees pre-fix repository sources until these edits are
  committed.

## Verification

- `cargo build --profile final-release --bins`
- `target/final-release/vstyle curate --workspace`
- `target/final-release/vstyle tune --workspace --verbose`
- `cargo make bench-semantic-vstyle`
- `cargo make bench-release-vstyle`

## Next Step

Use the current-worktree release snapshot as local pre-commit evidence only. After these source
changes are committed to `main`, rerun the `Benchmarks` workflow to capture the first
artifact-backed release baseline for the updated self-hosted repository state.
