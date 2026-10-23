---
type: Normative Specification
title: Import and Derive Interaction
description: Exact interaction and fix sequencing for RUST-STYLE-IMPORT-008, RUST-STYLE-IMPORT-009, and RUST-STYLE-IMPORT-011 on derive attributes.
tags: [specification, imports, derives, fixes]
openwiki:
  roles: [domain, testing]
  change_kinds: [fixes, ordering]
  source_paths: [src/style/imports.rs, src/style/imports/derives.rs, src/style/imports/shortening.rs, src/style/imports/qualification.rs, src/style/engine.rs]
  symbols: [RUST-STYLE-IMPORT-008, RUST-STYLE-IMPORT-009, RUST-STYLE-IMPORT-011]
  test_paths: [src/style/tests/derives.rs, src/style/tests/shortening.rs, src/style/tests/interactions.rs, tests/pub_use_self_group.rs]
  invariants: [Path rewrites precede derive ordering; overlapping edits are not emitted in the same collection round; a later pass rereads rewritten source.]
verified:
  - by: openwiki/0.7.2
    at: 2026-10-23T01:52:10.637Z
sources:
  - id: openwiki-source-f9df7723e0c608a7d6d83f07
    resource: repo://src/style/imports.rs
  - id: openwiki-source-f1c91c675c1949978a65b5f4
    resource: repo://src/style/imports/derives.rs
  - id: openwiki-source-3ff0d32085848fe3db454f15
    resource: repo://src/style/tests/shortening.rs
generated: { by: "codex", at: "2026-10-23T01:52:10.637Z" }
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

## Implementation and verification

`imports.rs` coordinates path rules before `derives::apply_import011_derive_order_rule`. `derives.rs` owns overlap detection and ordering; `shortening.rs` and `qualification.rs` own path policy. `engine.rs` rereads source between bounded fix passes. The focused convergence case is `import011_converges_after_import008_shortens_derive_paths` in `src/style/tests/shortening.rs`; derive ordering and unresolved-entry cases are in `src/style/tests/derives.rs`.
