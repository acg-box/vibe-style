# vstyle Release Runtime Acceleration Plan

## Goal

Improve the final release-binary runtime of `vstyle curate --workspace` and `vstyle tune --workspace --verbose` without changing rule behavior, weakening correctness, or conflating debug-path diagnostics with shipped performance.

## Scope

- Establish a reproducible release-only benchmark path for `vstyle` that measures the locally built binary, not an installed subcommand.
- Optimize the highest-leverage runtime hot paths in `src/style.rs`, `src/style/imports.rs`, `src/style/module.rs`, `src/style/quality.rs`, `src/style/shared.rs`, and `src/style/semantic.rs`.
- Keep the current Linear stream aligned with execution checkpoints: `XY-90` through `XY-97`.

## Non-goals

- Optimizing debug/profile-development timings as an end in itself.
- Changing style-rule semantics, reducing rule coverage, or dropping semantic validation for speed.
- Reworking unrelated repository structure or creating a permanent monolithic per-repo tracking lane outside the existing bounded Linear project.

## Constraints

- Acceptance is based on release-binary runtime only. Debug timings are diagnostic only.
- `Makefile.toml` is the source of truth for existing repo tasks, but direct release-binary commands are required for performance acceptance because `cargo make lint-vstyle` routes through `cargo vstyle curate --workspace`.
- `vstyle` file discovery depends on `git ls-files` in `src/style/shared.rs`, so benchmarks must run inside a real Git checkout rather than a plain copied directory.
- `vstyle tune` mutates files; all release benchmarking for `tune` should run in a disposable Git worktree or equivalent isolated checkout.
- Execution should start from a clean `main` worktree and follow the isolated-workspace flow before code changes begin.

## Open Questions

- What release-performance target should close `XY-90`: percentage reduction, absolute seconds, or both?
- Should the benchmark artifact persist as a checked-in script plus checked-in baseline doc, or only as a checked-in script with Linear issue comments carrying the baseline results?

## Execution State

- Last Updated: 2026-03-12
- Next Checkpoint: None. This execution wave is closed; remaining follow-ups stay queued in Linear.
- Blockers: None.

## Decision Notes

- Acceptance and closeout for this stream are release-binary only. Debug-path measurements remain useful for hotspot discovery but do not count as success criteria.
- The benchmark harness should use a disposable Git worktree at the current commit because `src/style/shared.rs` resolves files through `git ls-files` and `tune` can rewrite tracked files.
- Performance verification should call the locally built release binary directly (`target/release/vstyle` or the selected final shipping profile), not `cargo make lint-vstyle`, to avoid accidentally benchmarking an installed `cargo-vstyle` binary outside the repo build.
- 2026-03-12: Task 1 defaulted benchmark acceptance to `final-release`, with `release` retained as a diagnostic override and compatibility check.
- 2026-03-12: Current live runtime executes directly with optional read-only sidecars; Task 1 resumed in the existing isolated worktree after stale Builder-only routing proved inapplicable to the live environment.
- 2026-03-12: Task 2 landed the low-risk lazy-fallback and reduced-string-churn cleanup. On this host, two immediate `final-release` reruns stayed near baseline (`4.38s` and `4.32s` tune vs `4.35s` initial baseline), so Task 3 remains the next higher-leverage checkpoint.
- 2026-03-12: Task 3 replaced intermediate full-workspace rechecks with incremental per-file refreshes plus a final full pass. On this host, repeated `final-release` reruns dropped `tune` from the `4.35s` baseline to `2.89s`, `2.93s`, and `2.87s`, so the fix engine checkpoint is materially successful and Task 4 is now the next runtime target.
- 2026-03-12: Task 4 collapsed repeated import-rule analysis into shared per-file state across import rules and cfg-test follow-up scans. On this host, `final-release` reruns landed at `2.98s`, `2.88s`, and `2.92s` tune, which stays inside the prior `2.87s` to `2.93s` band, so the checkpoint is structurally complete but runtime-neutral and Task 5 is now the next target.
- 2026-03-12: Task 5 module/quality scan-fusion and regex-hoist experiments were benchmarked but not retained. The reverted fresh baseline measured `2.99s` tune, and the exploratory runs stayed in the `2.95s` to `3.07s` range, so that checkpoint remains queued rather than landed.
- 2026-03-12: Fresh no-op release benchmarks still do not enter semantic validation (`Semantic cache: 0 hit(s), 0 miss(es)`), so Task 6 narrowed to discovery-path reuse only. Caching workspace metadata and tracked-file discovery, then reusing grouped package scopes, moved `final-release tune` to `2.84s`, `2.93s`, `2.93s`, and `2.95s` versus the fresh post-revert `2.99s` baseline, which is a modest improvement without justifying semantic-path work yet.
- 2026-03-12: Task 7 closed the current execution wave. README guidance now explains how to interpret the self-host no-op benchmark versus semantic-path workloads, the benchmark doc records the before/after release history, `XY-92` is done, and `XY-95`/`XY-97` remain queued as measured follow-ups.

## Implementation Outline

Start with `XY-91` and make performance measurement reproducible before touching optimizer code. The harness should build the local release binary once, create a disposable Git worktree anchored to the current commit, run `curate` and `tune` against that isolated checkout, and record enough metadata to keep cache state, profile choice, and commit identity explicit. This protects the main checkout from `tune` rewrites while keeping `git ls-files` semantics intact for `vstyle`.

Once the release benchmark exists, attack the highest-confidence runtime multipliers in `src/style.rs` first. Current code shows that `run_fix` triggers an initial `run_check`, then re-runs `run_check` after each tune round, while `collect_fix_outcomes` and `apply_fix_passes` repeatedly read, clone, parse, and sometimes precompute fallback variants even before semantic validation proves they are necessary. These are the first checkpoints because they affect the no-op `tune` path before semantic work dominates.

After the core tune loop is reduced, move into rule-level hotspots and only then spend effort on `src/style/semantic.rs` and `src/style/shared.rs` if release benchmarks still show meaningful residual cost there. Use `cargo make fmt-check`, `cargo make lint-rust`, and `cargo make test-rust` as the repo-native code-quality gates, but pair every checkpoint with the release benchmark harness because the built-in `lint-vstyle` path is not the performance source of truth for the locally modified binary.

## Task 1: Establish release-only benchmark harness

**Owner**

Executor

**Status**

done

**Outcome**

A reproducible, release-only benchmark path exists for `curate` and `tune`, and the first baseline is captured in a way that later checkpoints can compare directly.

**Files**

- Modify: `Makefile.toml`
- Modify: `README.md`
- Create: `scripts/bench-release-vstyle.sh`
- Create: `docs/benchmarks/2026-03-12_vstyle-release-runtime-baseline.md`
- Review: `Cargo.toml`

**Changes**

1. Create a benchmark script that builds the local release binary, creates a disposable Git worktree at the current commit, and runs the selected-profile `vstyle curate --workspace` plus `vstyle tune --workspace --verbose` inside that isolated checkout.
2. Add a repo-native entrypoint in `Makefile.toml` for the benchmark script so executors do not need to rediscover the invocation pattern.
3. Document the benchmark policy in `README.md`, including that debug timings are diagnostic only and that `tune` benchmarks must not run in the primary checkout.
4. Capture the initial baseline in `docs/benchmarks/2026-03-12_vstyle-release-runtime-baseline.md`.

**Verification**

- `bash -n scripts/bench-release-vstyle.sh`
- `cargo make fmt-toml-check`
- `cargo build --release --bins`
- `./scripts/bench-release-vstyle.sh`

**Dependencies**

- None.

## Task 2: Reduce no-op tune overhead in the fix engine (`XY-94`)

**Owner**

Executor

**Status**

done

**Outcome**

The no-op `tune` path does less repeated parsing and avoids computing fallback variants on the optimistic success path.

**Files**

- Modify: `src/style.rs`
- Review: `src/style/imports.rs`
- Review: `src/style/shared.rs`

**Changes**

1. Refactor `collect_fix_outcomes` so fallback variants are not computed eagerly for files unless semantic validation later proves they are needed.
2. Tighten `apply_fix_passes` to stop earlier when a file has converged and to avoid avoidable `text.clone()` plus `read_file_context_from_text` churn where possible.
3. Preserve changed-file detection and fix counting semantics so a no-op `tune` remains behaviorally identical apart from runtime.

**Verification**

- `cargo make fmt-check`
- `cargo make lint-rust`
- `cargo make test-rust`
- `./scripts/bench-release-vstyle.sh`

**Dependencies**

- Task 1.

## Task 3: Remove intermediate full-workspace rechecks from `tune` (`XY-93`)

**Owner**

Executor

**Status**

done

**Outcome**

Intermediate `tune` rounds no longer pay for a full `run_check` over unchanged files, while final reporting remains correct.

**Files**

- Modify: `src/style.rs`
- Review: `src/style/shared.rs`
- Review: `README.md`

**Changes**

1. Replace the unconditional `checked = run_check(cargo_options)?` inside `run_fix` with scoped or incremental recomputation tied to the files that actually changed in the preceding round.
2. Keep a final full pass only if it is still needed for the final summary and violation ordering contract.
3. Add or adjust targeted tests around violation counts, round stopping, and final summary stability.

**Verification**

- `cargo make fmt-check`
- `cargo make lint-rust`
- `cargo make test-rust`
- `./scripts/bench-release-vstyle.sh`

**Dependencies**

- Task 1.
- Task 2.

## Task 4: Collapse repeated import-rule analysis into shared per-file state (`XY-96`)

**Owner**

Executor

**Status**

done

**Outcome**

Import-rule evaluation traverses each file’s syntax tree fewer times and reuses shared analysis structures instead of rebuilding them for each rule family.

**Files**

- Modify: `src/style/imports.rs`
- Review: `src/style.rs`
- Review: `src/style/shared.rs`

**Changes**

1. Introduce a shared import-analysis structure for `check_import_rules` that collects `use` runs, symbol maps, qualified path maps, and related reusable context once per file.
2. Thread that shared structure through the import rule helpers instead of rebuilding overlapping AST-derived collections.
3. Remove nested reparsing where offsets or parent-context analysis can preserve the same semantics.

**Verification**

- `cargo make fmt-check`
- `cargo make lint-rust`
- `cargo make test-rust`
- `./scripts/bench-release-vstyle.sh`

**Dependencies**

- Task 1.

## Task 5: Fuse module and quality scans, hoist reusable regexes (`XY-97`)

**Owner**

Executor

**Status**

done

**Outcome**

Fresh module/quality optimization experiments did not produce a keepable release-runtime checkpoint, so broader scan fusion remains queued and no Task 5 code changes were retained.

**Files**

- Modify: `src/style/module.rs`
- Modify: `src/style/quality.rs`
- Review: `src/style/shared.rs`

**Changes**

1. Benchmark module/quality scan-fusion and regex-hoist candidates against the existing Task 3/4 `final-release tune` band.
2. Retain only semantic-preserving changes that beat the current no-op release benchmark; otherwise revert and record the result.
3. Leave broader module/quality follow-ups queued if they do not beat the current release band.

**Verification**

- `cargo make fmt-check`
- `cargo make lint-rust`
- `cargo make test-rust`
- `./scripts/bench-release-vstyle.sh`

**Dependencies**

- Task 1.

## Task 6: Reassess semantic and file-discovery follow-ups (`XY-95`, `XY-92`)

**Owner**

Executor

**Status**

done

**Outcome**

Release-benchmark evidence showed that the current no-op `tune` workload still benefits from file/package discovery reuse, while semantic validation remains out of the measured path and stays queued.

**Files**

- Modify: `src/style/shared.rs`
- Modify: `src/style.rs`
- Review: `src/style/semantic.rs`
- Review: `docs/benchmarks/2026-03-12_vstyle-release-runtime-baseline.md`

**Changes**

1. Use fresh post-Task-5 release benchmarks to determine whether semantic validation or discovery work still contributes enough runtime to justify immediate action.
2. Keep semantic-path work queued when the benchmark does not enter semantic validation, instead of broadening the checkpoint without evidence.
3. Land a narrow discovery-path reuse checkpoint by caching workspace metadata and tracked-file discovery, then grouping workspace files by package directly instead of rediscovering each package scope separately.

**Verification**

- `cargo make fmt-check`
- `cargo make lint-rust`
- `cargo make test-rust`
- `./scripts/bench-release-vstyle.sh`

**Dependencies**

- Task 1.
- Task 2.
- Task 3.
- Task 4.
- Task 5.

## Task 7: Close out release benchmarks, docs, and tracking state

**Owner**

Executor

**Status**

done

**Outcome**

The repository and Linear stream now show the current release-performance evidence, the execution path is reproducible, and the next session can resume from queued follow-ups without re-triage.

**Files**

- Modify: `README.md`
- Modify: `docs/benchmarks/2026-03-12_vstyle-release-runtime-baseline.md`
- Review: `docs/plans/2026-03-11_vstyle-release-runtime-acceleration.md`

**Changes**

1. Update the checked-in benchmark doc with before/after release results, profile choice, and any remaining caveats.
2. Document any durable operator guidance in `README.md`, especially how to run release benchmarks safely and how to interpret them.
3. Post the final release measurements and residual follow-up decisions back into `XY-90` through `XY-97` so the Linear project remains the external execution record.

**Verification**

- `cargo make fmt-check`
- `./scripts/bench-release-vstyle.sh`

**Dependencies**

- Task 6.

## Rollout Notes

- Start execution in an isolated worktree created from clean `main`; do not run `tune` benchmarks in the primary checkout.
- Treat Task 1 as the gating checkpoint for the rest of the stream. Do not claim optimization wins until the release benchmark harness is stable.
- Use the `verification-before-completion` skill before closing any implementation slice, because repo-native gates alone do not prove release-binary performance changes.

## Suggested Execution

- Sequential: Tasks 1, 2, and 3 should run in order because they establish the benchmark harness and then reduce the highest-confidence tune-loop multipliers in `src/style.rs`.
- Parallelizable: After Task 1, Tasks 4 and 5 can be split into separate isolated-workspace lanes because they primarily touch `src/style/imports.rs` versus `src/style/module.rs` and `src/style/quality.rs`.
- Decision boundary: Task 6 should only start after fresh release benchmarks exist from Tasks 2 through 5; it is intentionally evidence-gated.
- Handoff: Once this plan is saved, execution should continue from this task-specific worktree rather than restarting from the primary checkout.
