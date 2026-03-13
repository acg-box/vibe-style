# vstyle pubfi-mono No-op Tune Fast Path

## Scope

`XY-103` follow-up benchmark for a clean downstream workspace where `tune`
still paid non-trivial extra runtime despite applying zero fixes.

## Workload

- Benchmark target: `/Users/xavier/code/trusted/helixbox/pubfi-mono`
- Workload shape: detached Git worktree on `pubfi-mono` `main`
- Acceptance binary: direct `target/final-release/vstyle`
- Benchmark policy: release-binary runtime only; `cargo vstyle ...` timings are
  diagnostic because they include Cargo subcommand dispatch overhead
- Workspace state: clean before and after each run

## Optimization

`run_fix(...)` now returns immediately after the initial `run_check_with_state`
when the initial scan reports `fixable_count == 0`. This skips the otherwise
redundant clean-workspace fix round, including:

- `resolve_fix_round_scopes(...)`
- `collect_type_alias_rename_plan(...)`
- `collect_fix_outcomes(...)`

## Benchmark Run

### Pre-patch baseline

- Benchmark date (UTC): `2026-03-13T06:26:10Z`
- Source commit: `81ca390c66cb6237e395083086c8d01918429e92` (`81ca390`)
- Binary: `/var/folders/c0/wqgh59fj3j7b6hjphk6mpfgw0000gn/T//vstyle-xy103-compare-src.lkV0Ep/target/final-release/vstyle`

| Command | Exit | Real (s) | User (s) | Sys (s) |
| --- | --- | ---: | ---: | ---: |
| `vstyle curate --workspace --all-features` | `0` | `2.07` | `8.13` | `0.12` |
| `vstyle tune --workspace --all-features --strict` (run 1) | `0` | `2.37` | `15.64` | `0.21` |
| `vstyle tune --workspace --all-features --strict` (run 2) | `0` | `2.36` | `14.70` | `0.19` |

### Post-patch rerun

- Benchmark date (UTC): `2026-03-13T06:26:17Z`
- Source base commit: `9521fef821ecff6b9a3a02ff59e3a2988dfc75bf` (`9521fef`)
- Source state: local working tree on top of `9521fef`
- Binary: `/Users/xavier/code/trusted/y/hack-ink/vibe-style/target/final-release/vstyle`

| Command | Exit | Real (s) | User (s) | Sys (s) |
| --- | --- | ---: | ---: | ---: |
| `vstyle curate --workspace --all-features` | `0` | `1.58` | `8.27` | `0.10` |
| `vstyle tune --workspace --all-features --strict` (run 1) | `0` | `1.54` | `7.41` | `0.11` |
| `vstyle tune --workspace --all-features --strict` (run 2) | `0` | `1.56` | `7.33` | `0.08` |

## Result

The no-op `tune` overhead on this downstream workload dropped from
`2.36s-2.37s` to `1.54s-1.56s`, a reduction of about `0.80s-0.83s`
(`34%`-`35%`). `tune` now sits inside the same range as `curate`, which is the
expected shape for a clean workspace with zero fixable violations.

## Verification

- `cargo test --bin vstyle skip_tune_rounds`
- `cargo build --profile final-release --bins`
- `cargo make fmt-check`
- `cargo make lint-rust`
- `cargo make test-rust`
- `cargo make lint-fix`
- `cargo make fmt`
- `cargo make test`
- Rebuilt the parent-source baseline from `81ca390`
- Re-ran direct `target/final-release/vstyle` benchmarks against detached
  `pubfi-mono` worktrees
