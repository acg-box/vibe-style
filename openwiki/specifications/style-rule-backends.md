---
type: Normative Specification
title: Style Rule Backend Classification
description: Complete primary evidence backend classification for all 42 registered Rust and Swift style rule IDs.
tags: [specification, rules, backends]
openwiki:
  roles: [domain, architecture]
  change_kinds: [rules, backend]
  source_paths: [src/style/shared.rs, src/style/swift.rs, src/style/semantic.rs]
  symbols: [STYLE_RULE_IDS]
  invariants: [Each registered ID has one documented primary backend classification.]
verified:
  - by: openwiki/0.7.2
    at: 2026-10-23T01:52:10.637Z
sources:
  - id: openwiki-source-8bafbb1f268d9f7b6c45513e
    resource: repo://src/style/quality.rs
  - id: openwiki-source-adcb383dccd0ff7b6608b41c
    resource: repo://src/style/semantic.rs
  - id: openwiki-source-05232c26a674118ba025697f
    resource: repo://src/style/shared.rs
  - id: openwiki-source-c03e631bb26c1bde452d6532
    resource: repo://src/style/swift.rs
  - id: openwiki-source-ddd1dd2ef043c2cd534ba33e
    resource: repo://src/style/tests/rules.rs
generated: { by: "codex", at: "2026-10-23T01:52:10.637Z" }
---

# Style Rule Backend Classification

**Purpose:** Document the primary evidence backend for every style rule registered in `src/style/shared.rs`.

**Status:** normative.

**Read this when:** changing execution, benchmarking, or review logic that depends on backend type. This document does not redefine rule semantics.

## Definitions

- **AST-backed:** primary signal from `ra_ap_syntax` `SourceFile` nodes and tokens.
- **Layout-backed:** primary signal from `FileContext.text`, `FileContext.lines`, token heuristics, or regex.
- **Path-backed:** primary signal from the checked file path.
- **Semantic-backed:** compiler output from `cargo check --message-format=json` or compiler-error diffs participates in validation.
- **Swift source-text-backed:** stable Swift source-text scanning without SwiftSyntax or Swift compiler semantic output.

## Complete classification

**Path-backed:** `RUST-STYLE-FILE-001`.

**AST-backed:** `RUST-STYLE-MOD-004`, `RUST-STYLE-MOD-007`, `RUST-STYLE-IMPORT-001`, `RUST-STYLE-IMPORT-002`, `RUST-STYLE-IMPORT-003`, `RUST-STYLE-IMPORT-004`, `RUST-STYLE-IMPORT-005`, `RUST-STYLE-IMPORT-006`, `RUST-STYLE-IMPORT-007`, `RUST-STYLE-IMPORT-008`, `RUST-STYLE-IMPORT-009`, `RUST-STYLE-IMPORT-010`, `RUST-STYLE-IMPORT-011`, `RUST-STYLE-IMPORT-012`, `RUST-STYLE-GENERICS-001`, `RUST-STYLE-GENERICS-002`, `RUST-STYLE-GENERICS-003`, `RUST-STYLE-TYPE-001`, `RUST-STYLE-LOG-002`, `RUST-STYLE-RUNTIME-001`, `RUST-STYLE-RUNTIME-002`, `RUST-STYLE-NUM-001`, `RUST-STYLE-NUM-002`, `RUST-STYLE-READ-002`, `RUST-STYLE-TEST-001`, `RUST-STYLE-TEST-002`.

**Layout-backed:** `RUST-STYLE-SPACE-003`, `RUST-STYLE-SPACE-004`.

**Hybrid AST-backed + layout-backed:** `RUST-STYLE-MOD-001`, `RUST-STYLE-MOD-002`, `RUST-STYLE-MOD-003`, `RUST-STYLE-MOD-005`, `RUST-STYLE-IMPL-001`, `RUST-STYLE-IMPL-003`.

**Semantic-backed:** `RUST-STYLE-LET-001` (AST edit generation with compiler-error diff validation during `tune`).

**Swift source-text-backed:** `SWIFT-STYLE-FILE-001`, `SWIFT-STYLE-IMPORT-004`, `SWIFT-STYLE-TYPE-001`, `SWIFT-STYLE-RUNTIME-001`, `SWIFT-STYLE-NUM-002`, `SWIFT-STYLE-READ-002`.

Update this list whenever `STYLE_RULE_IDS` or a rule's primary evidence source changes. Validate with the focused family tests and the shared all-target test when registration or dispatch changes.

`rules.rs` owns Rust dispatch. Numeric rules use a single syntax-token pass for ordinary literals and macro token trees. The registry has 36 Rust IDs and six Swift IDs. Serde default attributes are preserved and have no dedicated active rule. Import policy is distributed across `src/style/imports/`, while compiler validation remains in `semantic.rs`.
