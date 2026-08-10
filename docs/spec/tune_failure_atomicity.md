# Tune Failure Atomicity

Purpose: Define when `vstyle tune` may keep edits and when it must restore source files.

Status: normative

Read this when:

- You change fix-round writes, semantic validation, fallback behavior, or tune error handling.
- You need to determine whether a failed tune command may leave source edits on disk.

Not this document:

- This document does not define style rule semantics.
- This document does not guarantee recovery after process termination, power loss, or storage
  failure.

Defines:

- The semantic commit condition for automatic fixes.
- The rollback boundary for errors returned by `vstyle tune`.

## Commit condition

- Before a planned fix batch writes files, its Cargo scope must produce a successful
  `build-finished` result.
- After edits and any semantic import recovery, the Cargo scope must produce a successful
  `build-finished` result.
- A failed Cargo command without file-scoped compiler diagnostics is still a semantic validation
  failure.

## Rollback boundary

- `vstyle tune` snapshots all selected source files at the start of the command.
- If the command returns an internal error after a source write, it restores every selected source
  file to its start-of-command contents.
- The command returns the original error after a successful rollback.
- If rollback also fails, the command reports both the original error and the rollback error.
- Package-scoped fix rounds may use narrower fallback edits before final validation. The command
  commits those edits only when the final semantic result succeeds.

The whole-command boundary is required because one edit can cause a compiler error in another
file. A compiler diagnostic's primary file is not sufficient proof of which edit caused the error.

## Non-error strict results

A completed `tune --strict` run can return a non-zero exit code because style violations remain.
This result is not an internal tune error. Semantically validated edits remain on disk.
