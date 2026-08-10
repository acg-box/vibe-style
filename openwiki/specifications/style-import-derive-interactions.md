---
type: Normative Specification
title: Import and Derive Interaction
description: Exact interaction and fix sequencing for RUST-STYLE-IMPORT-008, RUST-STYLE-IMPORT-009, and RUST-STYLE-IMPORT-011 on derive attributes.
tags: [specification, imports, derives, fixes]
openwiki:
  roles: [domain, testing]
  change_kinds: [fixes, ordering]
  source_paths: [src/style/imports.rs, src/style.rs]
  symbols: [RUST-STYLE-IMPORT-008, RUST-STYLE-IMPORT-009, RUST-STYLE-IMPORT-011]
  test_paths: [tests/pub_use_self_group.rs]
  invariants: [Path rewrites precede derive ordering; overlapping edits are not emitted in the same collection round; a later pass rereads rewritten source.]
---

# Import and Derive Interaction

**Purpose:** Define how `RUST-STYLE-IMPORT-008`, `RUST-STYLE-IMPORT-009`, and `RUST-STYLE-IMPORT-011` compose for `#[derive(...)]` attributes. **Status:** normative.

`IMPORT-008` shortens qualified derive paths when the short name is unambiguous. `IMPORT-009` rewrites imported short names back to qualified paths when consistency requires it. `IMPORT-011` orders derive entries as `std`/`core`/`alloc`, then third-party, then workspace derives, alphabetically within each group.

A derive attribute can satisfy multiple predicates. Path rules change entry text; `IMPORT-011` changes only order, and its ordering operates on the entry text after required path rewrites.

## Fix sequencing

Within one fix-collection round, overlapping edits for the same derive attribute must not be emitted by a path rule and `IMPORT-011`. If `IMPORT-008` or `IMPORT-009` emits an edit overlapping the attribute, `IMPORT-011` defers that attribute. A later fix pass rereads the rewritten source and may order it. This avoids conflicting ranges while allowing convergence.

```rust
// Input
#[derive(sqlx::FromRow, Debug, Clone)]
struct Row;

// Required converged form
use sqlx::FromRow;

#[derive(Clone, Debug, FromRow)]
struct Row;
```

The path rewrite and import insertion occur first; ordering occurs on a later pass. The contract does not require all diagnostics to appear in one collection round and does not cover non-derive attributes.
