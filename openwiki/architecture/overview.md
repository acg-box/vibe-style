---
type: Architecture Overview
title: vibe-style Architecture Overview
description: Runtime composition and ownership boundaries for the Rust vstyle and cargo-vstyle binaries, Rust and Swift style lanes, diagnostics, fixes, and semantic validation.
tags: [architecture, rust, swift, runtime]
openwiki:
  roles: [architecture, repository]
  change_kinds: [runtime, public-api]
  source_paths: [Cargo.toml, src/main.rs, src/style.rs, src/style/engine.rs, src/style/workspace.rs, src/style/rules.rs, src/style/shared.rs, src/style/syntax.rs, src/style/module_tree.rs, src/style/types/renames.rs]
  symbols: [main, Cli, run_check, run_fix, resolve_files, STYLE_RULE_IDS]
  test_paths: [src/main.rs, src/cli.rs]
  invariants: [The vstyle and cargo-vstyle binaries share src/main.rs and normalize the cargo subcommand argument shape.]
  validation_commands: [cargo test --all-targets --all-features]
sources:
  - id: openwiki-source-651d1fb6c9e49916a916ab51
    resource: repo://Cargo.toml
  - id: openwiki-source-41e559bbb87cd2adb8c59921
    resource: repo://src/style.rs
  - id: openwiki-source-5a343b6d360ed17beec1a852
    resource: repo://src/style/engine.rs
  - id: openwiki-source-f9df7723e0c608a7d6d83f07
    resource: repo://src/style/imports.rs
  - id: openwiki-source-ac49b78710cf5ad65f8667ff
    resource: repo://src/style/imports/local.rs
  - id: openwiki-source-c37d2b23d52a989dc6442e19
    resource: repo://src/style/imports/planning.rs
  - id: openwiki-source-c1681fd0f4c6f3def015c057
    resource: repo://src/style/module_tree.rs
  - id: openwiki-source-8bafbb1f268d9f7b6c45513e
    resource: repo://src/style/quality.rs
  - id: openwiki-source-b7a6ae5f72d1d14fc3a92e74
    resource: repo://src/style/rules.rs
  - id: openwiki-source-8361c261bed96b2c06638161
    resource: repo://src/style/syntax.rs
  - id: openwiki-source-3bd071445b58e68edb6523d5
    resource: repo://src/style/types.rs
  - id: openwiki-source-18ddc34d07fdcb8d65c2689c
    resource: repo://src/style/types/renames.rs
  - id: openwiki-source-60fd4a2aff5978a4368f5dd1
    resource: repo://src/style/workspace.rs
generated: { by: "codex", at: "2026-10-23T02:37:02.452Z" }
verified:
  - by: openwiki/0.7.2
    at: 2026-10-23T02:37:02.452Z
---

# Architecture Overview

`vibe-style` is one Cargo package with two binaries, `vstyle` and `cargo-vstyle`, both built from `src/main.rs`. The executable owns CLI parsing and delegates to the `style` module; the style module selects files, runs language-specific detectors, formats stable rule diagnostics, and optionally applies deterministic edits.

## Composition

```mermaid
flowchart LR
  CLI["main.rs + cli.rs"] --> Facade["style.rs facade"]
  Facade --> Engine["style/engine.rs"]
  Engine --> Workspace["workspace.rs: Cargo + Git discovery"]
  Engine --> Dispatch["rules.rs: Rust dispatch"]
  Engine --> Swift["swift.rs: read-only checks"]
  Engine --> Semantic["semantic.rs: Cargo validation"]
  Engine --> Fixes["fixes.rs: edit application"]
  Dispatch --> RustRules["Rust rule families"]
```

`src/style.rs` is a small CLI facade. `engine.rs` owns scans, fix rounds, state refresh, and transaction recovery. `workspace.rs` owns Cargo metadata, package selection, Git-ignore discovery, and process-local discovery caches. `shared.rs` owns source contexts, diagnostics, edit records, and the 42-entry rule registry: 36 Rust IDs and six Swift IDs. `syntax.rs` owns common token and path helpers, AST import bindings, and generic-parameter names. `module_tree.rs` owns declared file-module identity, including `#[path]` declarations and test-only contexts; import policies and type-alias planning share it. The complete ID-to-backend classification is canonical in [style rule backends](../specifications/style-rule-backends.md).

Rule families are split by responsibility: `file.rs`, `module.rs`, `imports.rs`, `impls.rs`, `generics.rs`, `types.rs`, `bindings.rs`, `quality.rs`, `spacing.rs`, and `test_modules.rs`; Swift has its separate source-text backend in `swift.rs`. `rules.rs` dispatches Rust families. `imports.rs` coordinates policies through child modules: `paths`, `references`, `scopes`, `groups`, and `analysis` own mechanisms; `aliases`, `qualification`, `shortening`, and `derives` own policies; `planning` shares their order with `local` module processing. Private plan records remain in the import coordinator. Compiler-backed validation is isolated in `semantic.rs` and invokes Cargo for each check.

`types/renames.rs` owns the type-alias rewrite plan. It matches module-qualified type identities, respects generic shadows, and preserves private target paths. It leaves bare values, enum variants, and macro names in their own namespaces. Ambiguous conditional definitions and unsupported import-owner changes remain manual cases; selected-configuration Cargo checks remain the engine's validation gate.

## Change navigation

- Change command parsing or exits in [CLI](cli.md), then run the focused CLI tests.
- Change discovery, edit conflict handling, rounds, or convergence in [Engine](engine.md).
- Change a rule contract or registration in [Rules](rules.md) and the normative [backend specification](../specifications/style-rule-backends.md).
- Change compiler validation or recovery in [Semantic validation](semantic-validation.md).
- Change Swift checks only within [Swift](swift.md) and its [applicability specification](../specifications/swift-style-rule-applicability.md).

The package has no separate public library entrypoint: consumers use the binaries or the Cargo subcommand. Release and Action packaging are documented in [testing and automation](testing-and-automation.md).
