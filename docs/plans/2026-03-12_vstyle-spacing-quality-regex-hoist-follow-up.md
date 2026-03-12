# vstyle Spacing and Quality Regex Hoist Follow-up Plan

## Goal

Preserve the drafted `XY-101` lane and narrow any future work to the remaining
quality-side regex-hoist after the spacing-side work already landed elsewhere.

## Scope

- Preserve the original `388f09e` baseline for this broader regex-hoist idea.
- Record that the spacing-side regex-hoist was satisfied by `bb4fae5`.
- If resumed, limit code changes to remaining `src/style/quality.rs` fixed-regex
  helpers and refresh the release baseline from current `main` first.
- Keep Linear aligned around `XY-101` as a submitted but not yet executed
  follow-up lane.

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

- After the spacing-side win in `bb4fae5`, does the remaining
  `quality.rs::has_structured_fields` regex-hoist still show enough release-path
  cost to justify a separate checkpoint?

## Execution State

- Last Updated: 2026-03-12
- Next Checkpoint: Task 2.
- Blockers: None.

## Decision Notes

- `XY-101` was drafted as a broader spacing-plus-quality follow-up while the
  spacing regex-hoist was still under evaluation.
- The spacing portion is now already covered by `bb4fae5` / `XY-99`.
- This plan therefore remains useful only as a durable note for the still-open
  quality-side question.

## Implementation Outline

Keep the baseline and scope notes, but narrow any resumed execution to
`src/style/quality.rs`. If this lane resumes, start with a fresh release
baseline from current `main`, then decide whether the remaining fixed-regex
setup in `quality.rs` is still worth a dedicated checkpoint.

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

pending

**Outcome**

If resumed, `XY-101` will evaluate only the remaining `quality.rs` fixed-regex
helpers against a fresh post-`bb4fae5` release baseline.

**Files**

- Modify: `src/style/quality.rs`
- Review: `docs/benchmarks/2026-03-12_vstyle-spacing-quality-regex-hoist-follow-up.md`

**Changes**

1. Rebaseline from current `main` before editing.
2. Limit candidate changes to remaining fixed-regex helpers in `quality.rs`.
3. Keep spacing-side code out of scope because it is already landed.

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

pending

**Outcome**

The lane will end at a clean keep-vs-requeue boundary with docs, Linear, and git
state aligned.

**Files**

- Modify: `docs/plans/2026-03-12_vstyle-spacing-quality-regex-hoist-follow-up.md`
- Modify: `docs/benchmarks/2026-03-12_vstyle-spacing-quality-regex-hoist-follow-up.md`

**Changes**

1. Re-run the self-host release benchmark against a fresh post-`bb4fae5`
   baseline.
2. Record whether any `quality.rs` checkpoint is kept or re-queued.
3. Sync `XY-101` with the measured outcome.

**Verification**

- `cargo make bench-release-vstyle`
- `git status --short`

**Dependencies**

- Task 2.

## Suggested Execution

- Sequential: Task 1 is already archived, Task 2 becomes the next decision
  point if this lane resumes, and Task 3 closes the lane.
- Parallelizable: None. Any resumed code checkpoint still shares one release
  benchmark acceptance path.
