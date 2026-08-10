---
type: Normative Specification
title: Swift Style Rule Applicability
description: Complete applicability classes, first supported Swift rule batch, backend policy, and Rust applicability table for all registered Rust style rules.
tags: [specification, swift, applicability]
openwiki:
  roles: [domain, architecture]
  change_kinds: [language-support, rules]
  source_paths: [src/style/swift.rs, src/style/shared.rs]
  symbols: [STYLE_RULE_IDS, collect_violations_from_text]
  test_paths: [tests/swift_curate.rs]
  invariants: [The first Swift batch is read-only source-text checking; Swift compiler output is validation evidence rather than a stable primary backend.]
---

# Swift Style Rule Applicability

**Purpose:** Define which Rust rules map to Swift, require Swift-specific semantics, or remain Rust-only. **Status:** normative.

## Backend policy and classes

The Rust shell keeps CLI parsing, selection, diagnostic formatting, coverage, and exits in one host. SwiftSyntax is required when Swift AST fidelity is needed; source text is acceptable only for stable syntax shapes. Swift compiler output is validation evidence, not a canonical rule backend; do not depend on `swiftc -dump-ast` text or JSON.

- **Direct:** same intent without material semantic change.
- **Swift-shaped:** intent applies but names, syntax, or ordering must be redefined.
- **Semantic-gated:** application or fixes need Swift type checking, SourceKit, or compiler validation.
- **Rust-only:** depends on Rust syntax, crates, modules, attributes, or compiler behavior.

## First supported Swift batch

All six are read-only: `SWIFT-STYLE-FILE-001` rejects `mod.swift`; `SWIFT-STYLE-IMPORT-004` rejects `import func`, `struct`, `class`, `enum`, `protocol`, `var`, `let`, and `typealias` symbol imports; `SWIFT-STYLE-TYPE-001` rejects pure-renaming `typealias`; `SWIFT-STYLE-RUNTIME-001` rejects force unwraps, force casts, and `try!` in non-test files; `SWIFT-STYLE-NUM-002` requires underscore grouping for decimal integer literals over three digits; `SWIFT-STYLE-READ-002` caps function bodies at 120 lines.

## Complete Rust applicability table

| Rust rule | Swift class | Swift disposition |
| --- | --- | --- |
| `RUST-STYLE-FILE-001` | Direct | `SWIFT-STYLE-FILE-001` for `mod.swift`. |
| `RUST-STYLE-MOD-001` | Swift-shaped | Define Swift top-level declaration order first. |
| `RUST-STYLE-MOD-002` | Swift-shaped | Redefine visibility for `open`, `public`, `package`, implicit `internal`, `fileprivate`, `private`. |
| `RUST-STYLE-MOD-003` | Direct | Non-`async` functions before `async` within scope and visibility. |
| `RUST-STYLE-MOD-004` | Rust-only | Rust `mod` documentation placement. |
| `RUST-STYLE-MOD-005` | Swift-shaped | Type/`extension` adjacency candidate. |
| `RUST-STYLE-MOD-007` | Rust-only | `#[cfg(test)] mod tests` keep-alive imports. |
| `RUST-STYLE-SERDE-001` | Rust-only | Serde attributes. |
| `RUST-STYLE-IMPORT-001` | Swift-shaped | Needs Swift module classifier. |
| `RUST-STYLE-IMPORT-002` | Swift-shaped | Blank-line grouping may apply; Rust use-tree normalization cannot. |
| `RUST-STYLE-IMPORT-003` | Rust-only | Rust `use ... as ...` aliasing. |
| `RUST-STYLE-IMPORT-004` | Direct | `SWIFT-STYLE-IMPORT-004` symbol imports. |
| `RUST-STYLE-IMPORT-005` | Rust-only | `error.rs` convention. |
| `RUST-STYLE-IMPORT-006` | Rust-only | Swift imports are already file-scope declarations. |
| `RUST-STYLE-IMPORT-007` | Rust-only | Swift has no glob import syntax. |
| `RUST-STYLE-IMPORT-008` | Semantic-gated | Qualified/imported style needs Swift semantic evidence. |
| `RUST-STYLE-IMPORT-009` | Semantic-gated | Consistent qualified paths need Swift semantic evidence. |
| `RUST-STYLE-IMPORT-010` | Rust-only | Rust `self`/`super` prefixes. |
| `RUST-STYLE-IMPORT-011` | Rust-only | Rust derive ordering; separate Swift attribute rule may be designed later. |
| `RUST-STYLE-IMPORT-012` | Rust-only | Crate keep-alive imports. |
| `RUST-STYLE-IMPL-001` | Semantic-gated | Swift `Self` does not exactly match Rust `Self`; do not auto-port. |
| `RUST-STYLE-IMPL-003` | Swift-shaped | `extension` contiguity and ordering candidate. |
| `RUST-STYLE-GENERICS-001` | Swift-shaped | Swift `where` preference after syntax is defined. |
| `RUST-STYLE-GENERICS-002` | Rust-only | No Swift turbofish. |
| `RUST-STYLE-GENERICS-003` | Rust-only | No Swift turbofish canonical form. |
| `RUST-STYLE-TYPE-001` | Direct | `SWIFT-STYLE-TYPE-001` pure `typealias` renames. |
| `RUST-STYLE-LET-001` | Semantic-gated | `let` before `var` may apply with compiler validation. |
| `RUST-STYLE-LOG-002` | Swift-shaped | Needs separate structured logging contract. |
| `RUST-STYLE-RUNTIME-001` | Swift-shaped | `SWIFT-STYLE-RUNTIME-001` force operators and `try!`. |
| `RUST-STYLE-RUNTIME-002` | Swift-shaped | Clear-message rule for `fatalError`/preconditions candidate. |
| `RUST-STYLE-NUM-001` | Rust-only | No Rust numeric suffixes. |
| `RUST-STYLE-NUM-002` | Direct | `SWIFT-STYLE-NUM-002` large decimal literals. |
| `RUST-STYLE-READ-002` | Direct | `SWIFT-STYLE-READ-002` function body length. |
| `RUST-STYLE-SPACE-003` | Swift-shaped | Needs Swift block/statement classification. |
| `RUST-STYLE-SPACE-004` | Swift-shaped | Return spacing requires Swift-specific tail-expression handling. |
| `RUST-STYLE-TEST-001` | Swift-shaped | Swift Testing/XCTest naming differs. |
| `RUST-STYLE-TEST-002` | Rust-only | Rust keep-alive test modules. |
