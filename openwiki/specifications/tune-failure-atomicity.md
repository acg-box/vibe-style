---
type: Normative Specification
title: Tune Failure Atomicity
description: Commit and rollback contract for vstyle tune, including compiler validation gates, whole-command snapshots, fallback edits, and strict-mode results.
tags: [specification, tune, atomicity, rollback, semantic]
openwiki:
  roles: [domain, architecture, testing]
  change_kinds: [lifecycle, semantic-validation, fixes]
  source_paths: [src/style.rs, src/style/semantic.rs]
  symbols: [run_fix, run_fix_inner, validate_pre_edit_semantic_baseline, apply_semantic_validation, collect_file_snapshots, restore_file_snapshots, semantic_check_succeeded]
  test_paths: [tests/let_mut_reorder.rs, src/style.rs]
  invariants: [A tune internal error after writes restores every selected source file to its start-of-command contents.]
  validation_commands: [cargo test --test let_mut_reorder tune_rolls_back_the_run_when_semantic_validation_fails]
---

# Tune Failure Atomicity

**Purpose:** Define when `vstyle tune` may keep edits and when it must restore source files.

**Status:** normative.

**Read this when:** changing fix-round writes, semantic validation, fallback behavior, or tune error handling; or when deciding whether a failed tune command may leave edits on disk.

**Not this document:** This specification does not define style-rule semantics and does not guarantee recovery after process termination, power loss, or storage failure.

**Defines:** the semantic commit condition, the fallback boundary, the whole-command rollback boundary, and strict-mode exit behavior.

## Commit condition

Each fix scope validates its pre-edit semantic baseline before writing fixes. The Cargo invocation must provide a successful `build-finished` result; a failed Cargo command without file-scoped compiler diagnostics is still semantic validation failure and no fixes are written for that scope.

After ordinary edits and any semantic import recovery, the scope validates the edited files again. If recovery was needed or the post-edit check failed, a final semantic check must succeed before the scope completes. The [semantic validation boundary](../architecture/semantic-validation.md) owns compiler output parsing and cache behavior.

Package-scoped rounds may use narrower fallback edits for import shortening, `RUST-STYLE-TYPE-001`, or `RUST-STYLE-LET-001`, but those edits are committed only if final semantic validation succeeds.

## Rollback boundary

```mermaid
sequenceDiagram
  participant Tune as run_fix
  participant Snapshot as selected-file snapshots
  participant Round as run_fix_inner
  participant Cargo as semantic validation
  Tune->>Snapshot: snapshot every selected source file
  Tune->>Round: run initial scan and bounded fix rounds
  Round->>Cargo: validate baseline and edited files
  alt all internal steps succeed
    Round-->>Tune: summary; keep semantically validated edits
  else internal error after a write
    Tune->>Snapshot: compare selected files
    Tune->>Snapshot: restore changed files to command-start contents
    Snapshot-->>Tune: original error, or original plus rollback error
  end
```

`run_fix` snapshots all selected source files at command start. If `run_fix_inner` returns an internal error after any source write, it restores every selected file to its start-of-command contents, not merely the compiler diagnostic's primary file. This whole-command boundary is required because one edit can cause a compiler error in another file. A rollback failure is reported together with the original error.

If an error occurs before any selected file changed, the original error is returned without a restore attempt. The guarantee covers files selected for the command; it does not claim transactional recovery for external changes made concurrently.

## Strict results are not internal errors

A completed `vstyle tune --strict` may return a non-zero exit code when style violations remain. Semantically validated edits remain on disk in that case. The strict result is a policy outcome, not a failed tune transaction, so it does not trigger the internal-error rollback boundary.

## Focused evidence

`tests/let_mut_reorder.rs` covers compiler validation, cache reuse, telemetry, and rollback after semantic validation failure. `src/style.rs` unit tests cover snapshot delta detection and skipping semantic validation when a scope has no changes. Use `cargo test --test let_mut_reorder tune_rolls_back_the_run_when_semantic_validation_fails` for the narrow rollback check; use `cargo test --test let_mut_reorder` for the full semantic fixture lane.
