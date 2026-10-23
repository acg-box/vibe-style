---
type: Semantic Validation
title: Semantic Validation and Recovery
description: Cargo checks, compiler-error comparison, bounded import suggestions, recovery, rollback, and cold or warm validation benchmarks.
tags: [semantic, cargo, recovery, validation]
openwiki:
  roles: [architecture, testing, operations]
  change_kinds: [semantic-validation, recovery]
  source_paths: [src/style/semantic.rs, src/style/engine.rs, scripts/bench-semantic-vstyle.sh]
  symbols: [apply_semantic_fixes, collect_compiler_error_files_with_output, max_import_suggestion_rounds, run_semantic_cargo_check]
  test_paths: [tests/let_mut_reorder.rs]
  invariants: [Semantic checks use cargo check --all-targets --message-format=json; import suggestion rounds default to 2 and are capped at 16; each validation invokes Cargo and uses its incremental dependency tracking.]
  validation_commands: [cargo make bench-semantic-vstyle]
verified:
  - by: openwiki/0.7.2
    at: 2026-10-23T01:52:10.637Z
sources:
  - id: openwiki-source-59a609e749db71d05073a492
    resource: repo://scripts/bench-semantic-vstyle.sh
  - id: openwiki-source-adcb383dccd0ff7b6608b41c
    resource: repo://src/style/semantic.rs
  - id: openwiki-source-51261a6dd57286265897d7e9
    resource: repo://tests/let_mut_reorder.rs
generated: { by: "codex", at: "2026-10-23T01:52:10.637Z" }
---

# Semantic Validation

`src/style/semantic.rs` is the compiler boundary. It invokes `cargo check --all-targets --message-format=json`, extracts compiler-error files and missing-import suggestions, and validates changes that can alter binding semantics. `src/style/engine.rs` supplies baseline and post-edit error sets to its fallback logic, while the whole-command restore boundary is specified in [tune failure atomicity](../specifications/tune-failure-atomicity.md).

## Lifecycle

```mermaid
sequenceDiagram
  participant Engine as engine.rs
  participant Semantic as semantic.rs
  participant Cargo
  Engine->>Semantic: pre-edit compiler check
  Semantic->>Cargo: cargo check --all-targets --message-format=json
  Cargo-->>Semantic: compiler JSON and build status
  Semantic-->>Engine: baseline error files
  Engine->>Engine: apply candidate source edits
  Engine->>Semantic: post-edit compiler check
  Semantic->>Cargo: check current source again
  Cargo-->>Semantic: compiler JSON and build status
  Semantic-->>Engine: error files and import suggestions
  Engine->>Engine: accept, recover, or restore snapshots
```

`VSTYLE_MAX_IMPORT_SUGGESTION_ROUNDS` is read once: missing uses `2`, invalid text or `0` uses `2`, values above `16` use `16`, and valid values from `1..=16` are honored. Each check starts a Cargo subprocess. Cargo owns incremental reuse and invalidation for companion sources, manifests, dependencies, and build scripts. vstyle does not persist compiler output or report cache-hit counters. The changed-companion regression in `tests/let_mut_reorder.rs` ensures a later invalid source cannot be hidden by an earlier successful validation.

Compiler output is scoped back to selected style files. Import suggestions are applied only to those files and are bounded by the round setting. A semantic-positive change must test both the baseline-error and post-edit paths; a no-op self-tune does not exercise semantic validation. A missing build-finished status is an execution error, not a successful empty diagnostic set.

## Validation

Use `cargo make bench-semantic-vstyle` for semantic changes. The harness builds `final-release` by default, creates a temporary fixture with safe and unsafe `let mut` cases, records cold and warm tune timings with actual Cargo validation on both runs under `target/vstyle-bench-semantic/`, and removes the fixture. Use `cargo test --test let_mut_reorder` for focused behavioral evidence. Release-wide evidence is separate in [benchmark tracking](../runbooks/benchmark-tracking.md).
