---
type: Rule Family Overview
title: Style Rule Families and Change Surface
description: Registered stable rule IDs, family ownership, backend classification, fixability boundaries, interactions, and test navigation for Rust and Swift checks.
tags: [rules, rust, swift, diagnostics]
openwiki:
  roles: [domain, architecture, testing]
  change_kinds: [rules, public-api, fixes]
  source_paths: [src/style/shared.rs, src/style/rules.rs, src/style/file.rs, src/style/module.rs, src/style/imports.rs, src/style/impls.rs, src/style/generics.rs, src/style/types.rs, src/style/bindings.rs, src/style/quality.rs, src/style/spacing.rs, src/style/test_modules.rs, src/style/swift.rs]
  symbols: [STYLE_RULE_IDS, Violation, Edit, push_violation]
  test_paths: [tests/pub_use_self_group.rs, tests/let_mut_reorder.rs, tests/type_alias_rename.rs, tests/mod007_super_import.rs, tests/swift_curate.rs]
  invariants: [Every registered ID is emitted through the shared diagnostic contract; rule IDs are stable and coverage prints the registry; overlapping edits must be resolved before application.]
  validation_commands: [cargo test --all-targets --all-features]
sources:
  - id: openwiki-source-41e559bbb87cd2adb8c59921
    resource: repo://src/style.rs
  - id: openwiki-source-f9df7723e0c608a7d6d83f07
    resource: repo://src/style/imports.rs
  - id: openwiki-source-b7a6ae5f72d1d14fc3a92e74
    resource: repo://src/style/rules.rs
  - id: openwiki-source-05232c26a674118ba025697f
    resource: repo://src/style/shared.rs
generated: { by: "codex", at: "2026-10-23T01:52:10.637Z" }
verified:
  - by: openwiki/0.7.2
    at: 2026-10-23T01:59:51.049Z
---

# Style Rule Families and Change Surface

This page is a rule-family and change-surface overview, not the complete ID catalog. `src/style/shared.rs` is the registry and diagnostic contract. `STYLE_RULE_IDS` contains 42 stable IDs: 36 Rust IDs and six first-batch Swift IDs. A `Violation` carries file, line, rule, message, and fixability; an `Edit` carries byte range, replacement, and owning rule. The exact complete ID classification is canonical in [style rule backends](../specifications/style-rule-backends.md).

## Family ownership

| Family | Implementation | Representative evidence |
| --- | --- | --- |
| File | `src/style/file.rs` | `RUST-STYLE-FILE-001` |
| Modules | `src/style/module.rs` | `RUST-STYLE-MOD-001` through `MOD-007` |
| Imports and derives | `src/style/imports.rs` and `src/style/imports/` | `RUST-STYLE-IMPORT-001` through `IMPORT-012` |
| Impl and generics | `src/style/impls.rs`, `src/style/generics.rs` | `RUST-STYLE-IMPL-*`, `RUST-STYLE-GENERICS-*` |
| Types and bindings | `src/style/types.rs`, `src/style/bindings.rs` | `RUST-STYLE-TYPE-001`, `RUST-STYLE-LET-001` |
| Quality/runtime/numbers/readability | `src/style/quality.rs` | `RUST-STYLE-LOG-002`, `RUST-STYLE-RUNTIME-*`, `RUST-STYLE-NUM-*`, `RUST-STYLE-READ-002` |
| Spacing and test modules | `src/style/spacing.rs`, `src/style/test_modules.rs` | `RUST-STYLE-SPACE-*`, `RUST-STYLE-TEST-*` |
| Swift | `src/style/swift.rs` | `SWIFT-STYLE-*` |

`src/style/rules.rs` owns Rust dispatch. Import policy modules share path parsing, symbol inventories, reference rewriting, grouping, and plan sequencing. Serde default attributes are preserved; there is no active Serde-specific rule. The engine regression groups live in `src/style/tests/`, while the integration targets verify binary consumers.

The exact backend classification for every ID is canonical in [style rule backends](../specifications/style-rule-backends.md), and the exact Swift applicability table is canonical in [Swift applicability](../specifications/swift-style-rule-applicability.md). Do not duplicate those full classifications here.

## Extension recipe

1. Define detection and optional edit collection in the owning family module.
2. Register the stable ID in `STYLE_RULE_IDS` and ensure the rule is dispatched by the appropriate collector.
3. Preserve diagnostic formatting and fixability semantics in `Violation`/`Edit`.
4. If the rule crosses derive, import, compiler, or language boundaries, update the relevant normative specification and semantic fallback behavior.
5. Add a focused fixture test that exercises the violation, fix result, and remaining diagnostics; add consumer-facing CLI coverage only when the public command contract changes.

Run `cargo test --all-targets --all-features` for registry or shared dispatch changes. Use the narrow family integration test first for local rule edits.
