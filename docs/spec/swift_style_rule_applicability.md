# Swift Style Rule Applicability

Purpose: Define which existing Rust style rules may be applied to Swift, which must be
rewritten for Swift semantics, and which are Rust-only.

Status: normative

Read this when:

- You are adding or reviewing Swift support in `vstyle`.
- You need to decide whether a `RUST-STYLE-*` rule can become a `SWIFT-STYLE-*`
  rule.
- You are deciding which Swift checks require SwiftSyntax, SourceKit, or compiler
  validation.

Not this document:

- This document is not a Swift implementation runbook.
- This document is not a complete Swift community style guide.
- This document does not redefine the current Rust rule semantics.

Defines:

- Swift applicability classes for the existing Rust rule catalog.
- The first supported Swift rule batch.
- Backend expectations for future Swift checks.

## Backend Policy

The shared `vstyle` product shell remains implemented in Rust: command-line
parsing, file selection, diagnostic formatting, rule coverage, and exit behavior
stay in one host. This is an implementation boundary, not a priority order between
Rust, Swift, or future language lanes.

Swift rules that require Swift AST fidelity should use a SwiftSyntax-backed backend.
The first Swift batch may use source-text checks only for rules whose syntax shape is
stable enough that no Swift semantic interpretation is required.

Swift compiler output is validation evidence, not a stable primary rule source. Do not
depend on `swiftc -dump-ast` text or JSON as the canonical rule backend.

## Applicability Classes

- Direct: The rule maps to Swift with the same intent and no material semantic
  change.
- Swift-shaped: The rule intent applies, but names, syntax, or ordering must be
  redefined for Swift.
- Semantic-gated: The rule may apply, but automatic fixes or strong diagnostics need
  Swift type checking, SourceKit, or compiler validation.
- Rust-only: The rule depends on Rust-specific syntax, crates, modules, attributes, or
  compiler behavior and must not be applied to Swift.

## First Supported Swift Batch

The first Swift implementation batch is intentionally conservative and reports
read-only violations only.

- `SWIFT-STYLE-FILE-001`: Do not use `mod.swift`; use flat module entry files.
- `SWIFT-STYLE-IMPORT-004`: Do not import individual symbols with `import func`,
  `import struct`, `import class`, `import enum`, `import protocol`, `import var`,
  `import let`, or `import typealias`; import modules instead.
- `SWIFT-STYLE-TYPE-001`: Do not add `typealias` declarations that are only pure
  renames.
- `SWIFT-STYLE-RUNTIME-001`: Do not use force unwraps, force casts, or `try!` in
  non-test Swift files.
- `SWIFT-STYLE-NUM-002`: Use underscore grouping for decimal numeric literals with
  more than three digits.
- `SWIFT-STYLE-READ-002`: Keep function bodies at or under 120 lines.

## Rust Rule Applicability Map

| Rust rule | Swift class | Swift disposition |
| --- | --- | --- |
| `RUST-STYLE-FILE-001` | Direct | Implement as `SWIFT-STYLE-FILE-001` for `mod.swift`. |
| `RUST-STYLE-MOD-001` | Swift-shaped | Define a Swift top-level declaration order before implementation. |
| `RUST-STYLE-MOD-002` | Swift-shaped | Redefine visibility ordering for `open`, `public`, `package`, implicit `internal`, `fileprivate`, and `private`. |
| `RUST-STYLE-MOD-003` | Direct | May become a Swift rule that places non-`async` functions before `async` functions within the same scope and visibility. |
| `RUST-STYLE-MOD-004` | Rust-only | Rust `mod` documentation placement has no Swift equivalent. |
| `RUST-STYLE-MOD-005` | Swift-shaped | May become a type/`extension` adjacency rule. |
| `RUST-STYLE-MOD-007` | Rust-only | Rust `#[cfg(test)] mod tests` keep-alive imports have no Swift equivalent. |
| `RUST-STYLE-SERDE-001` | Rust-only | Serde attributes are Rust-specific. |
| `RUST-STYLE-IMPORT-001` | Swift-shaped | Swift import grouping needs a Swift module classifier before implementation. |
| `RUST-STYLE-IMPORT-002` | Swift-shaped | Blank-line grouping can apply; Rust use-tree normalization cannot. |
| `RUST-STYLE-IMPORT-003` | Rust-only | Rust `use ... as ...` aliasing has no direct Swift import equivalent. |
| `RUST-STYLE-IMPORT-004` | Direct | Implement as `SWIFT-STYLE-IMPORT-004` for Swift symbol imports. |
| `RUST-STYLE-IMPORT-005` | Rust-only | `error.rs` is a Rust file convention. |
| `RUST-STYLE-IMPORT-006` | Rust-only | Swift imports are already file-scope declarations. |
| `RUST-STYLE-IMPORT-007` | Rust-only | Swift has no glob import syntax. |
| `RUST-STYLE-IMPORT-008` | Semantic-gated | Qualified-vs-imported symbol style needs Swift semantic evidence. |
| `RUST-STYLE-IMPORT-009` | Semantic-gated | Consistent qualified path use needs Swift semantic evidence. |
| `RUST-STYLE-IMPORT-010` | Rust-only | Rust `self` and `super` import prefixes have no Swift import equivalent. |
| `RUST-STYLE-IMPORT-011` | Rust-only | Rust `derive` ordering has no Swift equivalent. A separate Swift attribute ordering rule may be designed later. |
| `RUST-STYLE-IMPORT-012` | Rust-only | Crate keep-alive imports are Rust-specific. |
| `RUST-STYLE-IMPL-001` | Semantic-gated | Swift `Self` does not exactly match Rust `Self`; do not auto-port. |
| `RUST-STYLE-IMPL-003` | Swift-shaped | May become an `extension` contiguity and ordering rule. |
| `RUST-STYLE-GENERICS-001` | Swift-shaped | Swift `where` preference can apply after Swift generic syntax is defined. |
| `RUST-STYLE-GENERICS-002` | Rust-only | Swift has no turbofish syntax. |
| `RUST-STYLE-GENERICS-003` | Rust-only | Swift has no turbofish canonical form. |
| `RUST-STYLE-TYPE-001` | Direct | Implement as `SWIFT-STYLE-TYPE-001` for pure `typealias` renames. |
| `RUST-STYLE-LET-001` | Semantic-gated | `let` before `var` may apply, but reordering needs Swift compiler validation. |
| `RUST-STYLE-LOG-002` | Swift-shaped | Swift logging APIs need a separate structured logging contract. |
| `RUST-STYLE-RUNTIME-001` | Swift-shaped | Implement as `SWIFT-STYLE-RUNTIME-001` for force unwraps, force casts, and `try!`. |
| `RUST-STYLE-RUNTIME-002` | Swift-shaped | May become a clear-message rule for `fatalError` and precondition failures. |
| `RUST-STYLE-NUM-001` | Rust-only | Swift has no Rust-style numeric suffixes. |
| `RUST-STYLE-NUM-002` | Direct | Implement as `SWIFT-STYLE-NUM-002` for large decimal numeric literals. |
| `RUST-STYLE-READ-002` | Direct | Implement as `SWIFT-STYLE-READ-002` for function body length. |
| `RUST-STYLE-SPACE-003` | Swift-shaped | Statement spacing can apply after Swift block and statement classification is defined. |
| `RUST-STYLE-SPACE-004` | Swift-shaped | `return` spacing may apply; tail-expression handling must be Swift-specific. |
| `RUST-STYLE-TEST-001` | Swift-shaped | Swift Testing and XCTest naming conventions differ and need separate handling. |
| `RUST-STYLE-TEST-002` | Rust-only | Rust keep-alive test modules have no Swift equivalent. |
