# vstyle Spacing Regex Hoist Plan

## Goal

Land a narrow `XY-99` checkpoint that removes repeated regex compilation from
`src/style/spacing.rs` hot paths and keeps the release self-host benchmark honest
with a fresh before/after comparison.

## Scope

- Rebaseline the checked-in self-host `final-release` benchmark on the current
  post-`388f09e` code.
- Hoist static spacing-rule regexes to `LazyLock<Regex>` definitions in
  `src/style/spacing.rs`.
- Reuse the hoisted regexes in hot-path helpers without changing rule behavior.
- Keep Linear aligned around `XY-99` and the existing runtime-acceleration
  project.

## Non-goals

- Changing spacing-rule semantics or expanding the lane into broader spacing-rule
  rewrites.
- Reopening already-closed `XY-90` child lanes as part of this checkpoint.
- Keeping the patch if repeated release runs do not beat or at least hold the
  fresh baseline.

## Constraints

- Acceptance remains the checked-in self-host `final-release` benchmark from
  `scripts/bench-release-vstyle.sh`.
- Rule correctness must stay unchanged; this lane only changes regex lifecycle
  and reuse.
- The patch should stay reviewable in one file plus benchmark/plan documentation.

## Open Questions

- None.

## Execution State

- Last Updated: 2026-03-12
- Next Checkpoint: None.
- Blockers: None.

## Decision Notes

- `src/style/spacing.rs` already uses `LazyLock<Regex>` for some shared regexes,
  but several statement-classification helpers still call `Regex::new(...)`
  inline on every invocation.
- The most promising repeated-work sites are `classify_statement_type`,
  `is_return_or_tail_statement`, `is_explicit_return_statement`,
  `is_item_like_statement`, `is_const_group_statement`, and
  `parse_ufcs_target_call`.
- This lane stays narrow on purpose: if the hoist is runtime-neutral, stop or
  revert instead of widening into unrelated spacing cleanup.
- `2026-03-12`: Task 1 refreshed the `XY-99` self-host release baseline at
  `curate 1.44s` and `tune 2.85s` on commit `388f09e`. This lane now compares
  only against that fresh band rather than earlier module/quality follow-up
  logs.
- `2026-03-12`: The retained `spacing.rs` regex-hoist checkpoint reran at
  `curate 0.76s/0.72s/0.73s/0.72s` and `tune 1.48s/1.43s/1.40s/1.43s`, which is
  materially below the fresh baseline while keeping formatter, lint, and test
  gates green.

## Implementation Outline

Start by capturing a fresh self-host release benchmark on the current
post-`388f09e` state so `XY-99` has its own baseline rather than comparing
against the earlier module/quality lane.

Then hoist only the static regexes that are clearly reused in `spacing.rs` and
swap the helper functions over to those statics. Once the code is in place, rerun
the repo-native formatting, lint, test, and release benchmark gates, then record
whether the measured result is worth keeping.

## Task 1: Refresh the `XY-99` release baseline

**Owner**

Executor

**Status**

done

**Outcome**

The lane has a fresh release self-host baseline and a dedicated benchmark note
for `XY-99`.

**Files**

- Create: `docs/benchmarks/2026-03-12_vstyle-spacing-regex-hoist.md`
- Review: `scripts/bench-release-vstyle.sh`
- Review: `docs/benchmarks/2026-03-12_vstyle-module-quality-follow-up.md`

**Changes**

1. Run the checked-in self-host release benchmark on the current code.
2. Record the baseline under a dedicated `XY-99` benchmark note.
3. Update Linear `XY-99` with the baseline and intended narrow execution shape.

**Verification**

- `cargo make bench-release-vstyle`
- `git status --short`

**Dependencies**

- None.

## Task 2: Hoist spacing regexes and keep behavior stable

**Owner**

Executor

**Status**

done

**Outcome**

`spacing.rs` reuses static regex instances in its hot-path helpers without
changing spacing-rule behavior.

**Files**

- Modify: `src/style/spacing.rs`

**Changes**

1. Hoist the relevant static regexes to `LazyLock<Regex>` definitions near the
   top of `spacing.rs`.
2. Replace inline `Regex::new(...)` calls in the chosen helper functions with the
   shared statics.
3. Keep the patch limited to regex lifecycle changes and behavior-preserving
   reuse.

**Verification**

- `cargo make fmt-check`
- `cargo make lint-rust`
- `cargo make test-rust`

**Dependencies**

- Task 1.

## Task 3: Measure, keep, or revert the checkpoint

**Owner**

Executor

**Status**

done

**Outcome**

`XY-99` ends at a benchmark-backed keep/revert decision with docs and Linear
aligned.

**Files**

- Modify: `docs/plans/2026-03-12_vstyle-spacing-regex-hoist.md`
- Modify: `docs/benchmarks/2026-03-12_vstyle-spacing-regex-hoist.md`

**Changes**

1. Rerun the self-host release benchmark on the patched code.
2. Record the before/after evidence and the keep/revert decision.
3. Sync `XY-99` in Linear with the measured outcome.

**Verification**

- `cargo make bench-release-vstyle`
- `git status --short`

**Dependencies**

- Task 2.

## Suggested Execution

- Sequential: Task 1 sets the baseline, Task 2 applies the narrow code change,
  and Task 3 decides whether the result is worth keeping.
- Parallelizable: None. This lane has one shared benchmark acceptance path.
