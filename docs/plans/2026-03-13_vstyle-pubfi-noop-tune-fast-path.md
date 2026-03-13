# vstyle pubfi-mono No-op Tune Fast Path Plan

## Goal

Remove the remaining no-op `tune` overhead on a clean large downstream
workspace without reopening the full runtime-acceleration project.

## Scope

- Use `pubfi-mono` as the benchmark workload for `XY-103`.
- Keep the change narrowly scoped to the `tune` control flow in `src/style.rs`.
- Preserve rule correctness and reporting behavior.
- Record the benchmark evidence in repo docs.

## Non-goals

- Reopening the completed `XY-90` runtime wave as a broad new project.
- Optimizing `cargo vstyle ...` dispatch overhead.
- Changing downstream `pubfi-mono` source as part of the runtime fix.

## Constraints

- Acceptance is direct `final-release` binary runtime, not Cargo subcommand
  timings.
- The fast path must trigger only when the initial scan proves there are zero
  fixable violations.
- A clean-workspace fast path is only acceptable if it preserves the same
  output summary as the initial `run_check_with_state(...)`.

## Open Questions

- None.

## Execution State

- Last Updated: 2026-03-13
- Next Checkpoint: None
- Blockers: None

## Decision Notes

- Fresh `pubfi-mono` direct-binary measurement showed a clean-workspace no-op
  `tune` at `2.36s-2.37s`, still noticeably above `curate` at `2.07s`.
- The current `run_fix(...)` flow always entered at least one fix round after
  the initial scan, even when `check_state.fixable_count()` was already `0`.
- In that case the extra round only repeated scope resolution, type-alias plan
  collection, and per-file fix-pass collection before stopping with
  `Applied 0 fix(es).`
- The correct narrow fix is an early return from `run_fix(...)` when the
  initial scan already proves no fixable violations exist.

## Implementation Outline

1. Add a no-op fast path in `run_fix(...)` keyed off the initial
   `fixable_count`.
2. Add focused unit coverage for the new skip condition.
3. Rebuild the `final-release` binary and rerun the `pubfi-mono` workload.
4. Record the before/after benchmark in repo docs.

## Task 1: Isolate the clean-workspace fast path (`XY-103`)

**Owner**

Executor

**Status**

done

**Outcome**

`run_fix(...)` now exits immediately after the initial scan when there are no
fixable violations, avoiding a redundant clean-workspace fix round.

**Files**

- Modify: `src/style.rs`

**Changes**

1. Added an early return after the initial `run_check_with_state(...)`.
2. Kept the fast path gated strictly on `fixable_count == 0`.
3. Left the rest of the `tune` round logic unchanged for non-clean workspaces.

**Verification**

- `cargo test --bin vstyle skip_tune_rounds`

**Dependencies**

- None.

## Task 2: Re-measure the downstream workload

**Owner**

Executor

**Status**

done

**Outcome**

Direct `final-release` no-op `tune` on `pubfi-mono` dropped from `2.36s-2.37s`
to `1.54s-1.56s`.

**Files**

- Review: `src/style.rs`
- Create: `docs/benchmarks/2026-03-13_vstyle-pubfi-noop-tune-fast-path.md`

**Changes**

1. Rebuilt a parent-source baseline from `81ca390`.
2. Re-ran the same downstream workload with the patched local binary.
3. Recorded the before/after direct-binary benchmark evidence.

**Verification**

- `cargo build --profile final-release --bins`
- Direct detached-worktree benchmark against `pubfi-mono`

**Dependencies**

- Task 1.

## Task 3: Close the lane with docs and tracker sync

**Owner**

Executor

**Status**

done

**Outcome**

The lane now has code, benchmark artifacts, and repo-native verification
evidence aligned around the clean-workspace fast path.

**Files**

- Create: `docs/plans/2026-03-13_vstyle-pubfi-noop-tune-fast-path.md`
- Create: `docs/benchmarks/2026-03-13_vstyle-pubfi-noop-tune-fast-path.md`

**Changes**

1. Captured the optimization rationale and benchmark evidence.
2. Kept the lane independent from the completed `vstyle Runtime Acceleration`
   project.
3. Closed the technical lane with verification-backed docs.

**Verification**

- `cargo make fmt-check`
- `cargo make lint-rust`
- `cargo make test-rust`
- `cargo make lint-fix`
- `cargo make fmt`
- `cargo make test`

**Dependencies**

- Task 2.
