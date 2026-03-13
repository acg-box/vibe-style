# vstyle Spacing and Quality Regex Hoist Follow-up Plan

## Goal

Preserve the drafted `XY-101` lane and narrow any future work to the remaining
quality-side regex-hoist after the spacing-side work already landed elsewhere.

## Scope

- Preserve the original `388f09e` baseline for this broader regex-hoist idea.
- Record that the spacing-side regex-hoist was satisfied by `bb4fae5`.
- Record the resumed `2026-03-13` evaluation of the remaining
  `src/style/quality.rs` fixed-regex helper.
- Keep Linear aligned around `XY-101` as an evaluated lane that closes without
  a retained code delta.

## Non-goals

- Reopening the closed `XY-90` runtime-acceleration wave as one monolithic lane.
- Re-running the already-landed spacing regex-hoist under a second issue.
- Claiming a quality-side win before a fresh post-`bb4fae5` baseline exists.

## Constraints

- Acceptance remains the checked-in self-host `final-release` benchmark from
  `scripts/bench-release-vstyle.sh`.
- Any resumed `XY-101` work must stay behavior-preserving and focus only on
  remaining `quality.rs` fixed-regex helpers.
- Because the recorded benchmark here predates `bb4fae5`, a resumed lane must
  refresh its baseline before code changes begin.

## Open Questions

- None.

## Execution State

- Last Updated: 2026-03-13
- Next Checkpoint: None.
- Blockers: None.

## Decision Notes

- `XY-101` was drafted as a broader spacing-plus-quality follow-up while the
  spacing regex-hoist was still under evaluation.
- The spacing portion is now already covered by `bb4fae5` / `XY-99`.
- On `2026-03-13`, the lane was resumed on current `main` with a fresh
  `curate 0.73s` / `tune 1.42s` baseline and a local `quality.rs`
  `has_structured_fields()` regex-hoist candidate.
- That candidate measured slower than the fresh baseline (`tune 1.47s` and
  `1.46s` on two reruns), so the code change was reverted and the lane closes as
  not worth keeping.

## Implementation Outline

Keep the baseline, resumed measurements, and decision notes as durable history.
The only remaining direct quality-side regex-hoist candidate was measured and
rejected, so no further execution remains in this lane.

## Task 1: Record baseline and isolate the checkpoint (`XY-101`)

**Owner**

Executor

**Status**

done

**Outcome**

The lane has a preserved draft baseline and scope definition in repo history.

**Files**

- Create: `docs/benchmarks/2026-03-12_vstyle-spacing-quality-regex-hoist-follow-up.md`
- Review: `src/style/spacing.rs`
- Review: `src/style/quality.rs`

**Changes**

1. Preserve the pre-`bb4fae5` baseline note.
2. Keep the original broader follow-up scope discoverable in git history.
3. Record that the spacing-side half is no longer pending.

**Verification**

- `cargo make bench-release-vstyle`
- `git status --short`

**Dependencies**

- None.

## Task 2: Evaluate remaining quality-side fixed regexes (`XY-101`)

**Owner**

Executor

**Status**

done

**Outcome**

`XY-101` evaluated the remaining `quality.rs` fixed-regex helper against a
fresh current-main baseline and found no keepable win, so the code change was
reverted.

**Files**

- Modify: `src/style/quality.rs`
- Review: `docs/benchmarks/2026-03-12_vstyle-spacing-quality-regex-hoist-follow-up.md`

**Changes**

1. Rebaselined from current `main` before editing.
2. Limited the candidate change to `quality.rs::has_structured_fields()`.
3. Reverted the code change after two post-patch reruns failed to beat the
   fresh baseline.

**Verification**

- `cargo make fmt-check`
- `cargo make lint-rust`
- `cargo make test-rust`
- `cargo make bench-release-vstyle`

**Dependencies**

- Task 1.

## Task 3: Benchmark, decide, and sync the lane

**Owner**

Executor

**Status**

done

**Outcome**

The lane now ends at a clean "do not keep" boundary with docs, Linear, and git
state aligned.

**Files**

- Modify: `docs/plans/2026-03-12_vstyle-spacing-quality-regex-hoist-follow-up.md`
- Modify: `docs/benchmarks/2026-03-12_vstyle-spacing-quality-regex-hoist-follow-up.md`

**Changes**

1. Re-ran the self-host release benchmark against fresh current-main baseline
   and post-patch code.
2. Recorded that no `quality.rs` checkpoint is kept.
3. Synced `XY-101` to a closed non-retained outcome.

**Verification**

- `cargo make bench-release-vstyle`
- `git status --short`

**Dependencies**

- Task 2.

## Suggested Execution

- Sequential: None. The lane is closed.
- Parallelizable: None.
