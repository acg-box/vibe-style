# vstyle Semantic Runtime Follow-up Plan

## Goal

Establish a semantic-positive release benchmark for `vstyle tune --verbose`, then use that workload
to reduce semantic validation overhead in `src/style/semantic.rs` only if the measured path justifies
the change.

## Scope

- Add a checked-in semantic benchmark harness that exercises the compiler-validation path on a
  disposable fixture crate instead of the current self-host no-op workload.
- Capture the first semantic-path baseline in a dedicated benchmark document with cold and warm cache
  evidence.
- Keep the Linear execution record aligned around `XY-95` and its parent performance stream in
  `XY-90`.

## Non-goals

- Reopening the closed self-host release-runtime wave in
  `docs/plans/2026-03-11_vstyle-release-runtime-acceleration.md`.
- Reworking unrelated rule families such as the previously deferred `XY-97` module/quality scans.
- Claiming semantic runtime wins before a semantic-positive benchmark proves the path is hot.

## Constraints

- Acceptance for this lane is still based on the locally built `final-release` binary, not an
  installed `cargo-vstyle` subcommand and not debug/profile-development timings.
- `vstyle` file discovery still depends on `git ls-files`, so the benchmark workload must run inside
  a real Git checkout.
- `vstyle tune` mutates files; semantic benchmarking must use a disposable fixture checkout and keep
  the main worktree untouched.
- A meaningful `XY-95` checkpoint must preserve semantic-validation behavior and cache semantics; it
  cannot skip compiler-backed verification for speed.

## Open Questions

- Should `XY-95` close on cold semantic-path wins only, or require both cold and warm reruns to
  improve?
- If semantic cache-key generation remains dominant after cold-path tuning, should that follow-up
  stay inside `XY-95` or split into a narrower issue?

## Execution State

- Last Updated: 2026-03-12
- Next Checkpoint: Task 2
- Blockers: None.

## Decision Notes

- The prior self-host release harness is intentionally retained as a no-op regression guard; this
  new lane exists because that workload does not usually enter semantic validation.
- `tests/let_mut_reorder.rs` provides the current best semantic-positive fixture shape because it
  triggers compiler-backed reorder validation and preserves one safe and one unsafe case in a tiny
  crate.
- 2026-03-12: Task 1 landed `scripts/bench-semantic-vstyle.sh`, `cargo make bench-semantic-vstyle`,
  README guidance, and the first semantic baseline. On this host, the initial `final-release` run
  measured `0.26s` cold (`2` hits, `1` miss) and `0.15s` warm (`3` hits, `0` misses), so Task 2
  should focus on reducing duplicated semantic work inside the cold path.

## Implementation Outline

Start by creating a semantic-specific benchmark harness instead of broadening the existing
self-host script. The semantic lane needs a different workload shape: a tiny disposable Cargo
project, tracked by Git, with a reorder candidate that forces `tune` into semantic validation and a
cache-preserving warm rerun after restoring the original sources.

Once that harness exists, baseline the cold and warm `final-release` timings and cache-hit/miss
counts before touching `src/style/semantic.rs`. That baseline decides whether the next checkpoint
should focus on repeated `cargo check` work, semantic cache-key generation, or stay queued if the
results do not justify code churn.

## Task 1: Establish semantic-positive benchmark harness (`XY-95`)

**Owner**

Executor

**Status**

done

**Outcome**

A reproducible semantic benchmark path exists for the release binary, and the first cold/warm
baseline is captured for future `XY-95` checkpoints.

**Files**

- Modify: `Makefile.toml`
- Modify: `README.md`
- Create: `scripts/bench-semantic-vstyle.sh`
- Create: `docs/benchmarks/2026-03-12_vstyle-semantic-runtime-baseline.md`
- Review: `tests/let_mut_reorder.rs`
- Review: `src/style/semantic.rs`

**Changes**

1. Add a benchmark script that builds the local release binary, creates a disposable fixture crate
   based on the `let_mut_reorder` semantic-validation test shape, and records cold plus warm
   `vstyle tune --verbose` runs.
2. Add a repo-native `cargo make` entrypoint and document when semantic-path changes should use this
   harness instead of the self-host benchmark.
3. Capture the initial semantic-path baseline in a dedicated benchmark document, including semantic
   cache hit/miss counts.

**Verification**

- `bash -n scripts/bench-semantic-vstyle.sh`
- `cargo make fmt-toml-check`
- `cargo build --profile final-release --bins`
- `cargo make bench-semantic-vstyle`

**Dependencies**

- None.

## Task 2: Narrow the semantic hot path (`XY-95`)

**Owner**

Executor

**Status**

pending

**Outcome**

The semantic benchmark identifies the dominant runtime cost, and the next code change targets that
specific cost instead of guessing from the self-host workload.

**Files**

- Modify: `src/style/semantic.rs`
- Review: `src/style.rs`
- Review: `docs/benchmarks/2026-03-12_vstyle-semantic-runtime-baseline.md`

**Changes**

1. Use the Task 1 cold/warm benchmark output to decide whether the next checkpoint should reduce
   duplicate semantic `cargo check` calls, cache-key overhead, or both.
2. Implement only the measured semantic-path optimization, keeping compiler-backed validation and
   existing fallback behavior intact.
3. Update the semantic benchmark doc and Linear issue with the post-change result before deciding
   whether `XY-95` is done or needs another narrower pass.

**Verification**

- `cargo make fmt-check`
- `cargo make lint-rust`
- `cargo make test-rust`
- `cargo make bench-semantic-vstyle`

**Dependencies**

- Task 1.

## Task 3: Close out or re-queue the semantic lane

**Owner**

Executor

**Status**

pending

**Outcome**

The semantic lane ends in a clean checkpoint with docs and Linear aligned, whether the result is a
landed optimization or a measured re-queue decision.

**Files**

- Modify: `docs/plans/2026-03-12_vstyle-semantic-runtime-follow-up.md`
- Modify: `docs/benchmarks/2026-03-12_vstyle-semantic-runtime-baseline.md`
- Review: `README.md`

**Changes**

1. Record the landed semantic runtime result or the measured reason for re-queuing.
2. Sync `XY-95` and `XY-90` with the checkpoint evidence and next-step decision.
3. Stop the lane at a commit boundary once the benchmark, docs, and tracker state agree.

**Verification**

- `git status --short`
- `cargo make bench-semantic-vstyle`

**Dependencies**

- Task 2.

## Suggested Execution

- Sequential: Task 1 establishes the only trustworthy semantic-path baseline; Task 2 depends on that
  evidence, and Task 3 is pure closeout.
- Parallelizable: None. The benchmark workload, semantic optimization, and closeout all share the
  same execution evidence.
