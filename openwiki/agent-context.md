---
type: Agent Context
title: Agent Context and Documentation Ownership
description: Safe-change boundaries, source-of-truth rules, OpenWiki update workflow, documentation contracts, and validation routing for future repository agents.
tags: [agents, documentation, policy]
openwiki:
  roles: [repository, workflow]
  change_kinds: [documentation, maintenance]
  source_paths: [AGENTS.md, CLAUDE.md, .github/workflows/openwiki-update.yml, Makefile.toml]
  symbols: [openwiki code --update --print]
  validation_commands: [openwiki code --update --print]
verified:
  - by: openwiki/0.7.2
    at: 2026-10-23T01:59:51.049Z
sources:
  - id: openwiki-source-6d4b4e707b8d60b6ccfa3425
    resource: repo://.github/workflows/openwiki-update.yml
  - id: openwiki-source-8037e2358a2c4f9b2c722a11
    resource: repo://AGENTS.md
  - id: openwiki-source-a2371d6362e5db4bc834ad03
    resource: repo://CLAUDE.md
  - id: openwiki-source-c8b1a2a9f2113ec43d4066da
    resource: repo://Makefile.toml
generated: { by: "codex", at: "2026-10-23T01:52:10.637Z" }
---

# Agent Context

Source code and tests are authoritative for behavior. The generated `openwiki/` tree is an evidence index alongside `README.md` and the usage skill in `skills/vstyle/`. It is optional just-in-time context. Use retrieval when the task or a source uncertainty needs it. Prefer the narrowest quiet validation that proves a change and preserve full failure diagnostics.

## Documentation ownership

Use [quickstart](quickstart.md) as the entrypoint. Route correctness contracts to [specifications](specifications/style-rule-backends.md), procedures to [benchmark tracking](runbooks/benchmark-tracking.md), current implementation to architecture pages, and rationale to the decisions lane. Keep one authoritative page per topic and link rather than duplicate. `index.md` files are reserved for deterministic generation; `_plan.md` is temporary.

Former documentation contracts are preserved in [documentation map](reference/documentation-map.md): specs state purpose, normative status, read boundary, and definitions; runbooks state goal, inputs, dependencies, and verification; references state purpose, read boundary, exclusions, and coverage.

## Safe-change rules

Do not read or document secrets. Do not modify source code during wiki maintenance. Preserve accurate unrelated wiki content. For code changes, start with the affected entrypoint and symbol, follow imports and tests across boundaries, update the owning concept page, and validate the smallest relevant test. Public or runtime changes need consumer-facing validation, not only a defining-module check.

The scheduled workflow in `.github/workflows/openwiki-update.yml` checks out full history and runs `openwiki code --update --print`; it creates a PR for generated wiki and selected agent/workflow files. Repository agent files point agents to source/test authority for behavior changes and to the generated `openwiki/` tree for documentation ownership and navigation.

## Validation ownership

`Makefile.toml` owns local validation commands. `cargo make check-vstyle` builds and checks the current source; `cargo make lint-vstyle` applies Rust fixes with `--strict`. Use the full `cargo make check` for changes that cross engine, rule, and discovery boundaries. Keep the host-managed tools and repository toolchain settings.
