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
---

# Agent Context

Source code and tests are authoritative for behavior. The generated `openwiki/` documentation tree is the repository's only current documentation surface and is optional just-in-time context, not mandatory startup reading. Prefer the narrowest quiet validation that proves a change and preserve full failure diagnostics.

## Documentation ownership

Use [quickstart](quickstart.md) as the entrypoint. Route correctness contracts to [specifications](specifications/style-rule-backends.md), procedures to [benchmark tracking](runbooks/benchmark-tracking.md), current implementation to architecture pages, and rationale to the decisions lane. Keep one authoritative page per topic and link rather than duplicate. `index.md` files are reserved for deterministic generation; `_plan.md` is temporary.

Former documentation contracts are preserved in [documentation map](reference/documentation-map.md): specs state purpose, normative status, read boundary, and definitions; runbooks state goal, inputs, dependencies, and verification; references state purpose, read boundary, exclusions, and coverage.

## Safe-change rules

Do not read or document secrets. Do not modify source code during wiki maintenance. Preserve accurate unrelated wiki content. For code changes, start with the affected entrypoint and symbol, follow imports and tests across boundaries, update the owning concept page, and validate the smallest relevant test. Public or runtime changes need consumer-facing validation, not only a defining-module check.

The scheduled workflow in `.github/workflows/openwiki-update.yml` checks out full history and runs `openwiki code --update --print`; it creates a PR for generated wiki and selected agent/workflow files. Repository agent files point agents to source/test authority for behavior changes and to the generated `openwiki/` tree for documentation ownership and navigation.
cumentation ownership and navigation.
