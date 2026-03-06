# Style Import Derive Interactions

Purpose: Define the normative interaction between `RUST-STYLE-IMPORT-008`, `RUST-STYLE-IMPORT-009`, and `RUST-STYLE-IMPORT-011` when the same `#[derive(...)]` attribute is eligible for more than one rule.

Audience: This document is written for implementers and reviewers of `vstyle`.

## Scope

- This document only covers `#[derive(...)]` attributes.
- This document does not redefine the standalone meaning of `RUST-STYLE-IMPORT-008`, `RUST-STYLE-IMPORT-009`, or `RUST-STYLE-IMPORT-011`.
- This document defines how those rules compose during auto-fix collection and across repeated fix passes.

## Rule roles

- `RUST-STYLE-IMPORT-008` shortens qualified derive paths to imported short names when the short name is unambiguous.
- `RUST-STYLE-IMPORT-009` rewrites imported short derive names back to qualified paths when qualified-path consistency requires that rewrite.
- `RUST-STYLE-IMPORT-011` orders derive entries like imports: `std`/`core`/`alloc` first, then third-party derives, then workspace derives; each group is alphabetized.

## Interaction contract

- A single `#[derive(...)]` attribute may satisfy more than one rule predicate at the same time.
- `RUST-STYLE-IMPORT-008` and `RUST-STYLE-IMPORT-009` change the path form of a derive entry.
- `RUST-STYLE-IMPORT-011` changes only the ordering of derive entries.
- `RUST-STYLE-IMPORT-011` ordering is defined over the derive entry text after any required `RUST-STYLE-IMPORT-008` or `RUST-STYLE-IMPORT-009` rewrite has been applied.

## Auto-fix sequencing

- Within one fix-collection round, overlapping edits for the same derive attribute must not be emitted by both `RUST-STYLE-IMPORT-008` or `RUST-STYLE-IMPORT-009` and `RUST-STYLE-IMPORT-011`.
- If `RUST-STYLE-IMPORT-008` or `RUST-STYLE-IMPORT-009` emits an edit whose range overlaps a `#[derive(...)]` attribute, `RUST-STYLE-IMPORT-011` must defer that attribute for the current round.
- A later fix pass must re-read the rewritten source and may then apply `RUST-STYLE-IMPORT-011` to that same derive attribute.
- This sequencing rule exists to prevent conflicting overlapping edits while still allowing the source to converge to the final ordered form.

## Example

Input:

```rust
#[derive(sqlx::FromRow, Debug, Clone)]
struct Row;
```

Required convergence:

```rust
use sqlx::FromRow;

#[derive(Clone, Debug, FromRow)]
struct Row;
```

Explanation:

- `RUST-STYLE-IMPORT-008` first rewrites `sqlx::FromRow` to `FromRow` and inserts the required `use`.
- `RUST-STYLE-IMPORT-011` then reorders the derive entries on a later fix pass.

## Non-goals

- This document does not require all diagnostics to be emitted in the same collection round when one rule's edit is deferred.
- This document does not define interaction with non-derive attributes.
