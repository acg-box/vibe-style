---
name: vstyle
description: Use when Codex needs to enforce vibe-style development rules with `vstyle` or `cargo vstyle` across supported language lanes, including today's Rust and Swift lanes and future lanes when they exist. Use for style validation, safe automatic fixes where supported, language selection, workspace/package/feature scoping, CI-style handoff checks, or troubleshooting vibe-style command usage in repositories that adopt vibe-style.
---

# Vstyle

Use `vstyle` as a language-aware style gate. Do not treat one language lane as more
important than another. Choose the lane from the user's request, changed files,
repository wrappers, and current `vstyle` help.

Current CLI fact: when `--language` is omitted, `vstyle curate` and `vstyle tune`
use the default language lane. Today that default is Rust. Select Swift explicitly
with `--language swift`. For future language lanes, inspect the repository wrapper
or `vstyle <command> --help` instead of hard-coding assumptions.

## Quick Workflow

1. Discover repository authority.
   - Prefer checked-in wrappers from `Makefile.toml`, `justfile`, `Makefile`,
     package scripts, CI workflows, or repository docs.
   - Wrapper names may be generic (`style`, `style-check`, `fmt`, `fmt-check`,
     `lint`, `checks`) or language-specific (`lint-vstyle`, `lint-vstyle-swift`).
   - Use direct `vstyle` commands only when no relevant repository wrapper exists.
2. Determine language lanes.
   - Rust lane: changed `*.rs` files or a Rust-focused wrapper.
   - Swift lane: changed `*.swift` files or a Swift-focused wrapper.
   - Mixed-language changes: run each relevant lane or the repository's combined
     wrapper.
   - Unknown or future lane: inspect `vstyle <command> --help` and prefer the
     repository wrapper if it already encodes the language choice.
3. Probe tool availability before suggesting installation.
   - Check `command -v vstyle` for direct `vstyle` commands.
   - Use `cargo vstyle ...` when the repository wrapper or installation exposes the
     Cargo subcommand.
   - Do not install `vibe-style` unless the user or task allows tool installation.
4. Choose the smallest correct scope.
   - Use repository wrappers when they own the validation scope.
   - Use package/workspace/feature flags only when they are valid for the selected
     language lane and command.
   - For Rust Cargo workspaces, use `-p` / `--package`, `--workspace`, `--features`,
     `--all-features`, and `--no-default-features` to mirror the target build surface.
5. Run the repository's normal formatter for the selected language when one exists.
6. Run read-only validation with `curate`.
7. Run `tune` only when the selected language lane supports safe fixes and you own the
   affected files or the user asks for fixes.
8. Before final handoff, rerun the repository wrapper or a matching read-only `curate`
   command for every affected language lane.

## Command Recipes

Current direct-command default lane validation. Today this default is Rust:

```sh
vstyle curate
vstyle curate --workspace
```

Swift lane validation:

```sh
vstyle curate --workspace --language swift
```

Rust Cargo package and feature scoped validation:

```sh
vstyle curate -p api --features serde,tracing
vstyle curate -p api --all-features --no-default-features
```

Safe fixes where the selected lane supports them:

```sh
vstyle tune
vstyle tune --strict
vstyle tune -p api --all-features --strict
vstyle tune --workspace --all-features --strict
```

Rule discovery:

```sh
vstyle coverage
```

Use these recipes as examples, not as a replacement for checked-in wrappers.
For new language lanes, inspect `vstyle curate --help` and the repository wrapper
before assuming the right `--language` value or scope flags.

## Command Semantics

- `curate` checks style and reports violations. It exits `0` when clean and `1` when
  violations are found.
- `tune` applies safe fixes, then re-checks. It exits `0` even if unresolved
  violations remain unless `--strict` is set.
- `coverage` prints implemented rule IDs for supported language lanes, such as
  `RUST-STYLE-*` and `SWIFT-STYLE-*`.
- `curate` and `tune` use Cargo-like scope flags. They do not accept positional file
  paths such as `vstyle curate src/lib.rs`.
- File discovery scans selected style files that are not Git-ignored. Git tracking
  state is not the filter, so non-ignored untracked files can be checked.
- Today, `--workspace` selects Rust files from Cargo workspace package roots, and
  selects Swift files from the Cargo workspace root when `--language swift` is used.

## Guardrails

- Inspect `git status --short` before `tune`. Avoid broad fixes when unrelated local
  changes could be rewritten.
- Do not claim style is clean from `tune` alone. Use `curate`, `tune --strict`, or a
  strict repository wrapper as the final gate.
- Do not invent configuration files, rule exceptions, language lanes, or local policy
  overrides. vibe-style rules are built into the checker.
- Treat checker behavior as the source of truth for parser and AST edge cases.
- Do not replace the repository's broader lint, test, or check workflow with `vstyle`;
  `vstyle` is the style gate only.

## Reporting

Report:

- the wrapper or direct command used,
- whether the command was read-only or fixing,
- selected language lane or lanes,
- package/workspace and feature scope when applicable,
- whether remaining violations require manual edits.
