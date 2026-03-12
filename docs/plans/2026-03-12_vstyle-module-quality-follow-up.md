# vstyle Module and Quality Runtime Follow-up Plan

## Goal

Land a narrowly scoped `XY-97` checkpoint that reduces repeated module/quality analysis work in the
release self-host benchmark without reopening the already-closed broad scan-fusion experiment.

## Scope

- Rebaseline the checked-in self-host `final-release` benchmark after the semantic follow-up lane.
- Narrow `XY-97` to the highest-confidence low-risk hotspots still visible in `src/style/module.rs`
  and `src/style/quality.rs`.
- Keep Linear aligned around `XY-97` and its parent runtime stream in `XY-90`.

## Non-goals

- Re-running the previously reverted broad module/quality scan-fusion experiment.
- Reopening the semantic-specific lane in `docs/plans/2026-03-12_vstyle-semantic-runtime-follow-up.md`.
- Claiming a runtime win if the narrowed checkpoint does not beat the fresh post-`XY-95` baseline.

## Constraints

- Acceptance remains the checked-in self-host `final-release` benchmark from
  `scripts/bench-release-vstyle.sh`.
- Rule behavior must stay unchanged; this lane can only remove repeated work or repeated regex
  compilation.
- If the narrowed patch is runtime-neutral on repeated release runs, revert or stop without forcing
  a keepable commit.

## Open Questions

- None.

## Execution State

- Last Updated: 2026-03-12
- Next Checkpoint: None.
- Blockers: None.

## Decision Notes

- The prior Task 5 experiment already proved that broad module/quality scan fusion was too wide to
  attribute cleanly and did not beat the release band. This follow-up lane only allows narrow,
  independently explainable changes.
- Current code still shows two repeated-work patterns that were not isolated as their own
  checkpoint: per-call regex compilation in `quality.rs` / `module.rs`, and repeated
  `item_list.syntax().descendants()` scans in nested-module macro hoist checks.
- 2026-03-12: Task 1 refreshed the post-`XY-95` self-host release baseline at `curate 1.45s` and
  `tune 2.88s`, so this lane compares only against that fresh band rather than the earlier reverted
  broad experiment.
- 2026-03-12: The first narrowed candidate (regex hoist plus nested-scope macro-call reuse) showed
  `2.85s` and `2.82s` on early reruns but regressed to `2.93s` and `2.97s` on post-gate reruns, so
  it was reverted instead of forced into a noisy checkpoint.
- 2026-03-12: The current Task 2 checkpoint moved `check_expect_unwrap` name filtering ahead of
  `method_call_in_test_context`, targeting the large gap between workspace method-call density and
  actual `unwrap` / `expect` usage. Early reruns on this patch measured `2.88s` and `2.83s`, and
  the post-pre-commit rerun settled at `2.85s`, so this narrower candidate is worth keeping as a
  modest runtime improvement.

## Implementation Outline

Start by refreshing the self-host release benchmark after the semantic lane closeout so `XY-97`
does not compare against a stale pre-`55d2af4` runtime band. That fresh run becomes the only
acceptance baseline for this follow-up.

Then keep the code checkpoint narrow and evidence-driven. If one candidate does not survive repeated
reruns, revert it and pivot to the next smallest hotspot instead of widening the experiment.
The current narrowed checkpoint targets the `check_expect_unwrap` front-end loop, where most method
calls are irrelevant to the unwrap/expect rule but still paid for test-context ancestor scans.

## Task 1: Refresh release baseline and isolate the narrowed checkpoint (`XY-97`)

**Owner**

Executor

**Status**

done

**Outcome**

The lane has a fresh post-`XY-95` self-host release baseline and a narrow checkpoint definition
that can be accepted or rejected cleanly.

**Files**

- Create: `docs/benchmarks/2026-03-12_vstyle-module-quality-follow-up.md`
- Review: `docs/benchmarks/2026-03-12_vstyle-release-runtime-baseline.md`
- Review: `src/style/module.rs`
- Review: `src/style/quality.rs`

**Changes**

1. Run the checked-in self-host release benchmark on the current post-`XY-95` code and record the
   fresh baseline in a dedicated benchmark doc.
2. Confirm the narrow `XY-97` checkpoint boundaries from current code evidence before editing.
3. Update Linear `XY-97` with the fresh baseline and narrowed execution shape.

**Verification**

- `cargo make bench-release-vstyle`
- `git status --short`

**Dependencies**

- None.

## Task 2: Hoist reusable regexes and precompute nested-scope macro-call names (`XY-97`)

**Owner**

Executor

**Status**

done

**Outcome**

The narrowed module/quality checkpoint removes repeated front-end work from the unwrap/expect rule
without changing behavior in other rule families.

**Files**

- Modify: `src/style/quality.rs`
- Review: `src/style/module.rs`

**Changes**

1. Filter `MethodCallExpr` nodes by method name before calling `method_call_in_test_context` or
   computing line numbers, so unrelated method calls do not pay for ancestor and attribute scans.
2. Keep the patch narrow: avoid reintroducing the reverted regex/macro candidate or broader scan
   fusion.
3. Verify that unwrap/expect rule behavior is unchanged and measure the self-host release path again.

**Verification**

- `cargo make fmt-check`
- `cargo make lint-rust`
- `cargo make test-rust`
- `cargo make bench-release-vstyle`

**Dependencies**

- Task 1.

## Task 3: Keep or re-queue the narrowed checkpoint

**Owner**

Executor

**Status**

done

**Outcome**

The narrowed `XY-97` lane ends at a clean benchmark-backed decision boundary with docs and Linear
aligned.

**Files**

- Modify: `docs/plans/2026-03-12_vstyle-module-quality-follow-up.md`
- Modify: `docs/benchmarks/2026-03-12_vstyle-module-quality-follow-up.md`
- Review: `docs/benchmarks/2026-03-12_vstyle-release-runtime-baseline.md`

**Changes**

1. Record the narrowed checkpoint result and whether it is kept or re-queued.
2. Sync `XY-97` and `XY-90` with the measured before/after evidence.
3. Stop at a commit boundary only if the narrowed checkpoint is worth keeping.

**Verification**

- `git status --short`
- `cargo make bench-release-vstyle`

**Dependencies**

- Task 2.

## Suggested Execution

- Sequential: Task 1 defines the only valid acceptance baseline, Task 2 is the narrow code change,
  and Task 3 is the keep-vs-requeue decision.
- Parallelizable: None. The benchmark evidence and code checkpoint share one acceptance path.
