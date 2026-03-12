# vstyle Spacing Regex Runtime Follow-up Plan

## Goal

Preserve the drafted `XY-100` lane as a submitted artifact and close it cleanly
now that the spacing regex-hoist checkpoint already landed elsewhere.

## Scope

- Keep the original `XY-100` draft intent and prototype measurements in repo
  history.
- Record that the actual spacing regex-hoist implementation landed in
  `bb4fae5`.
- Remove any ambiguous "still pending" execution state from the drafted
  `XY-100` lane.

## Assumptions

- The actual kept code change for this topic is `bb4fae5` / `XY-99`, not a separate `XY-100` checkpoint.
- The linked benchmark notes remain archived evidence for the drafted lane only.

## Steps

1. Preserve the draft lane and prototype measurements for traceability.
2. Point future readers to the already-landed `XY-99` source of truth.
3. Keep this plan closed unless a genuinely new spacing delta appears later.

## Status

- Archived execution record. The drafted lane is closed as covered by the landed `XY-99` work.

## Non-goals

- Re-implementing the spacing regex-hoist under a second issue.
- Reopening the broader module/quality follow-up that was already narrowed away.
- Claiming a distinct code delta exists under `XY-100` when the landed change is
  already part of `bb4fae5`.

## Constraints

- This document must reflect actual git history and measured evidence.
- The committed source of truth for the landed spacing change is
  `bb4fae5` together with
  `docs/plans/2026-03-12_vstyle-spacing-regex-hoist.md` and
  `docs/benchmarks/2026-03-12_vstyle-spacing-regex-hoist.md`.
- `XY-100` remains a tracking/documentation lane only unless a new code delta is
  proposed later.

## Open Questions

- None.

## Execution State

- Last Updated: 2026-03-12
- Next Checkpoint: None.
- Blockers: None.

## Decision Notes

- `XY-100` was drafted while the spacing regex-hoist was still being evaluated
  on top of `388f09e`.
- The actual spacing regex-hoist landed as `bb4fae5` with `refs:["XY-99"]`, so
  `XY-100` does not carry a separate code checkpoint.
- This plan is still submitted to preserve the drafted lane and prototype
  evidence, but execution is closed as covered by the already-landed `XY-99`
  checkpoint.

## Implementation Outline

The intended `XY-100` scope was satisfied by the already-landed spacing
checkpoint, so there is no separate implementation left to execute here. Keep
the draft baseline notes for traceability, and direct future readers to the
committed `XY-99` source of truth rather than duplicating the code lane.

## Task 1: Refresh release baseline and isolate the spacing checkpoint (`XY-100`)

**Owner**

Executor

**Status**

done

**Outcome**

The drafted lane preserved its prototype measurements and no longer leaves an
ambiguous pending checkpoint behind.

**Files**

- Review: `docs/benchmarks/2026-03-12_vstyle-spacing-regex-follow-up.md`
- Review: `docs/benchmarks/2026-03-12_vstyle-spacing-regex-hoist.md`
- Review: `src/style/spacing.rs`

**Changes**

1. Keep the original prototype note under the `XY-100` filename.
2. Mark the lane as archival because the landed checkpoint already exists in
   `bb4fae5`.
3. Point readers to the committed `XY-99` plan and benchmark as the execution
   source of truth.

**Verification**

- `git show --stat bb4fae5 -- src/style/spacing.rs docs/plans/2026-03-12_vstyle-spacing-regex-hoist.md docs/benchmarks/2026-03-12_vstyle-spacing-regex-hoist.md`
- `git status --short`

**Dependencies**

- None.

## Task 2: Hoist spacing classifier regexes into shared statics (`XY-100`)

**Owner**

Executor

**Status**

done

**Outcome**

The intended spacing regex-hoist scope is already satisfied by `bb4fae5`, so no
second code delta is needed under `XY-100`.

**Files**

- Review: `src/style/spacing.rs`
- Review: `docs/plans/2026-03-12_vstyle-spacing-regex-hoist.md`
- Review: `docs/benchmarks/2026-03-12_vstyle-spacing-regex-hoist.md`

**Changes**

1. Treat `bb4fae5` as the satisfying spacing checkpoint for this drafted lane.
2. Do not add redundant code under `XY-100`.
3. Keep execution history explicit by recording that the landed code references
   `XY-99`, not `XY-100`.

**Verification**

- `git show --stat bb4fae5 -- src/style/spacing.rs`
- `cargo make bench-release-vstyle`

**Dependencies**

- Task 1.

## Task 3: Keep or re-queue the spacing checkpoint

**Owner**

Executor

**Status**

done

**Outcome**

`XY-100` ends as a closed archival/documentation lane with no missing code
content.

**Files**

- Modify: `docs/plans/2026-03-12_vstyle-spacing-regex-follow-up.md`
- Modify: `docs/benchmarks/2026-03-12_vstyle-spacing-regex-follow-up.md`

**Changes**

1. Record that the drafted lane was covered by `bb4fae5`.
2. Leave the prototype measurements in history, but mark them as non-source of
   truth.
3. Align the issue/document trail so merge cleanup does not drop the draft.

**Verification**

- `git status --short`

**Dependencies**

- Task 2.

## Suggested Execution

- Sequential: None. This lane is closed as covered by an already-landed
  checkpoint.
- Parallelizable: None.
