# Style Rule Backend Map

Purpose: Document which style rules are primarily AST-backed versus layout-backed, and where semantic (compiler) signals are involved.

This document is normative for documentation purposes only. It does not change rule semantics.

## Definitions

- AST-backed: The rule's primary signal comes from `ra_ap_syntax` (`SourceFile` and `ast::*` nodes).
- Layout-backed: The rule's primary signal comes from raw source text (`FileContext.text`, `FileContext.lines`), token stream heuristics, or regex.
- Semantic-backed: The rule depends on compiler output (`cargo check --message-format=json`) or compiler-error diffs as part of validation.

## Rule Backend Classification

AST-backed rules (some may also use limited text for replacement formatting):

- `RUST-STYLE-FILE-001`
- `RUST-STYLE-SERDE-001`
- `RUST-STYLE-IMPORT-001`
- `RUST-STYLE-IMPORT-002`
- `RUST-STYLE-IMPORT-003`
- `RUST-STYLE-IMPORT-004`
- `RUST-STYLE-IMPORT-005`
- `RUST-STYLE-IMPORT-007`
- `RUST-STYLE-IMPORT-008`
- `RUST-STYLE-IMPORT-009`
- `RUST-STYLE-IMPORT-010`
- `RUST-STYLE-GENERICS-001`
- `RUST-STYLE-GENERICS-002`
- `RUST-STYLE-GENERICS-003`
- `RUST-STYLE-TYPE-001`
- `RUST-STYLE-LOG-002`
- `RUST-STYLE-RUNTIME-001`
- `RUST-STYLE-RUNTIME-002`
- `RUST-STYLE-NUM-001`
- `RUST-STYLE-NUM-002`
- `RUST-STYLE-READ-002`
- `RUST-STYLE-TEST-001`
- `RUST-STYLE-TEST-002`
- `RUST-STYLE-MOD-007`

Layout-backed rules (source-text driven by design):

- `RUST-STYLE-SPACE-003`
- `RUST-STYLE-SPACE-004`

Hybrid AST-backed + layout-backed rules (AST classification plus line-aware spacing or reorder planning):

- `RUST-STYLE-MOD-001`
- `RUST-STYLE-MOD-002`
- `RUST-STYLE-MOD-003`
- `RUST-STYLE-MOD-005`
- `RUST-STYLE-IMPL-001`
- `RUST-STYLE-IMPL-003`

Semantic-backed rules:

- `RUST-STYLE-LET-001` (AST edit generation with compiler-error diff validation during `tune`)

