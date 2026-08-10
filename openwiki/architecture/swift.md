---
type: Swift Backend
title: Swift Source-Text Backend
description: Conservative read-only Swift checks, masking rules, test-file exception, applicability boundary, and focused integration coverage.
tags: [swift, source-text, rules]
openwiki:
  roles: [architecture, domain, testing]
  change_kinds: [language-support, rules]
  source_paths: [src/style/swift.rs, src/style/shared.rs]
  symbols: [collect_violations_from_file, collect_violations_from_text, mask_swift_code_lines]
  test_paths: [tests/swift_curate.rs]
  invariants: [The first Swift batch reports violations without automatic fixes; force operators are rejected only in non-test Swift files; comments and strings are masked before token checks.]
  validation_commands: [cargo test --test swift_curate]
---

# Swift Backend

`src/style/swift.rs` reads Swift files as text, masks comments and strings, and runs six conservative checks: `SWIFT-STYLE-FILE-001`, `SWIFT-STYLE-IMPORT-004`, `SWIFT-STYLE-TYPE-001`, `SWIFT-STYLE-RUNTIME-001`, `SWIFT-STYLE-NUM-002`, and `SWIFT-STYLE-READ-002`. The backend does not require SwiftSyntax or Swift compiler output and emits no automatic edits in this batch.

`SWIFT-STYLE-RUNTIME-001` ignores Swift test files, identified by path conventions in `is_swift_test_file`; other checks operate on masked lines. Function length tracks brace depth and reports functions over 120 lines at their starting line. Decimal grouping applies to non-zero integer tokens over three digits without underscores.

The shared Rust shell still owns language parsing, workspace selection, diagnostics, coverage, and exit behavior. The complete direct, Swift-shaped, semantic-gated, and Rust-only boundary is normative in [Swift applicability](../specifications/swift-style-rule-applicability.md).

## Change navigation

Start at `collect_violations_from_text` for a new check, add a stable ID to `STYLE_RULE_IDS`, and add fixture coverage in `tests/swift_curate.rs`. Validate with `cargo test --test swift_curate`; broader CLI or shared-discovery changes require the corresponding [CLI](cli.md) or [engine](engine.md) checks.
