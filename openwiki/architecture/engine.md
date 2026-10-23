---
type: Runtime Engine
title: Check and Fix Engine
description: File discovery, parallel checking, bounded tune rounds, edit application, state refresh, and convergence behavior implemented by the engine, workspace, rule-dispatch, and edit owners.
tags: [engine, fixes, lifecycle, concurrency]
openwiki:
  roles: [architecture, workflow, testing]
  change_kinds: [lifecycle, fixes, concurrency]
  source_paths: [src/style/engine.rs, src/style/workspace.rs, src/style/rules.rs, src/style/fixes.rs, src/style/shared.rs, src/style/types/renames.rs]
  symbols: [run_check, run_fix, run_fix_rounds_for_scopes, resolve_fix_round_scopes, CheckState, MAX_TUNE_ROUNDS]
  test_paths: [tests/let_mut_reorder.rs, tests/type_alias_rename.rs, tests/pub_use_self_group.rs]
  invariants: [Fix rounds are bounded; changed files are rechecked before deciding whether another round is needed; a final scan runs only after edits were applied.]
  validation_commands: [cargo test --test let_mut_reorder --test type_alias_rename]
sources:
  - id: openwiki-source-5a343b6d360ed17beec1a852
    resource: repo://src/style/engine.rs
  - id: openwiki-source-09edad079b5428ffc6f317a4
    resource: repo://src/style/fixes.rs
  - id: openwiki-source-05232c26a674118ba025697f
    resource: repo://src/style/shared.rs
  - id: openwiki-source-18ddc34d07fdcb8d65c2689c
    resource: repo://src/style/types/renames.rs
  - id: openwiki-source-c7b7f1d0eae5d93ce0817638
    resource: repo://tests/mod007_super_import.rs
  - id: openwiki-source-525a4a0622f393e8828a2378
    resource: repo://tests/support/fixture.rs
  - id: openwiki-source-e645da4f9c4a0035b94904d8
    resource: repo://tests/type_alias_rename.rs
generated: { by: "codex", at: "2026-10-23T02:37:02.452Z" }
verified:
  - by: openwiki/0.7.2
    at: 2026-10-23T02:37:02.452Z
---

# Check and Fix Engine

`src/style/engine.rs` owns the lifecycle. The facade in `src/style.rs` exposes its entrypoints to the CLI. `run_check` resolves files through `workspace::resolve_files`, collects rule outcomes, and returns a `RunSummary`. `run_fix` snapshots every selected source file, performs an initial scan, applies up to `MAX_TUNE_ROUNDS` rounds, refreshes state for changed files, and performs a final scan when at least one edit was applied. If an internal error occurs after writes, the command restores the command-start snapshot; the complete contract is [tune failure atomicity](../specifications/tune-failure-atomicity.md).

```mermaid
stateDiagram-v2
  [*] --> InitialScan
  InitialScan --> NoFixable: no fixable violations
  InitialScan --> FixRound: fixable violations
  FixRound --> RefreshState: files changed
  FixRound --> Stop: no edits or convergence stop
  RefreshState --> FixRound: follow-up required and round budget remains
  RefreshState --> FinalScan: edits complete
  FinalScan --> [*]
  NoFixable --> [*]
  Stop --> FinalScan: edits occurred
  Stop --> [*]: no edits
```

The engine groups work into file/package scopes. Checking can use Rayon; fix scopes run in parallel only when more than one scope exists and their selected file sets are disjoint. `CheckState` keeps per-file counts and updates aggregate violation and manual-fix counts by subtracting the prior file entry before adding the refreshed one.

Fix collection produces `Edit` ranges. `src/style/fixes.rs` applies compatible edits, while rule-specific metadata records import fallback, `let mut` reorder, and type-alias rename follow-up work. Overlapping edits are deferred rather than blindly composed; [derive interaction](../specifications/style-import-derive-interactions.md) specifies the important derive case.

## Edit and binding boundaries

`types/renames.rs` owns `TypeAliasRenamePlan`. It plans definition and reference edits from module-qualified type bindings before the engine applies per-file passes. The engine owns snapshots, compiler validation, and recovery; alias name resolution stays with the type policy.

`shared::item_text_range` extends a syntax span to its physical lines only when both the leading and trailing text are whitespace. Otherwise, removal and reorder operations use the exact syntax span. This keeps adjacent declarations and enclosing braces when multiple items share one line. `fixes.rs` finds protected string, byte, C-string, and character ranges from parser tokens, so literal prefixes are protected and Unicode lifetimes remain code.

Cargo validation covers the selected build configuration. The integration regressions compare original and tuned program output for binding-sensitive changes and verify that test discovery retains inline test functions. Additional configurations and runtime contracts need their own tests.

## Ownership and test scope

`workspace.rs` owns package and feature selection, Git-ignore discovery, and process-local discovery caches. `rules.rs` dispatches Rust checks; `shared.rs` owns source contexts and diagnostics. The engine keeps transaction records and recovery state together. It permits four tune rounds and eight per-file fix passes. A scan with no fixable violations returns before fix or compiler-validation rounds.

The regression modules in `src/style/tests/` are private test children of the engine, so they can verify internal transactions without exposing helpers to production callers. Integration targets in `tests/` verify real binary behavior and share temporary-directory ownership in `tests/support/fixture.rs`.

## Change recipes

For a new fix, add the detector/edit producer in its rule family, ensure the edit is represented in the file outcome and any required semantic fallback, then add a fixture test that asserts both source result and remaining diagnostics. For lifecycle changes, update the stop conditions and tests around initial, unchanged, multi-round, and failed-validation paths. Narrow checks include `cargo test --test let_mut_reorder --test type_alias_rename` and `cargo test --test pub_use_self_group`; use the full all-target test only when shared orchestration changes broadly.
