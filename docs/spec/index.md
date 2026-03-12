# Spec Index

Purpose: Route agents to normative documents that define what must be true in this repository.

Question this index answers: "what must be true?"

## Use this index when

- You need the authoritative contract, invariant, required behavior, or classification for a
  repository concept.
- You are changing implementation behavior and need to confirm the governing truth first.
- You need the document that other guides or plans should defer to instead of restating.

## Do not use this index when

- You need a runbook, migration sequence, validation flow, or troubleshooting procedure.
- You need a temporary execution plan or in-flight draft.
- You need a dated benchmark record or execution artifact; follow the owning plan to the linked
  supporting record instead.

## What belongs in `docs/spec/`

- Contracts, invariants, and required behavior.
- Normative classifications that other code or docs must agree with.
- Interaction rules between repository concepts when those interactions affect correctness.
- Stable repository truth that should outlive one execution lane or benchmark run.

## Spec document contract

Start each spec with a compact routing header:

- `Purpose`
- `Status: normative`
- `Read this when`
- `Not this document`
- `Defines`

Then write the body for verification:

- State the required truth directly.
- Keep scope boundaries explicit.
- Link to guides or plans for procedure instead of embedding runbooks here.
- Use examples only when they clarify the contract.

## Structure policy

- Keep one authoritative topic per spec file.
- Prefer descriptive `snake_case` names that reflect the topic instead of rigid prefix schemes.
- Add links from `docs/index.md` or `docs/guide/index.md` when a procedure depends on the spec.
- Benchmark notes and plan history do not belong in `docs/spec/` even when they describe measured
  behavior.

## Specs

- `docs/spec/system_style_rule_backends.md` for the normative backend classification of style
  rules.
- `docs/spec/system_style_import_derive_interactions.md` for the normative interaction between
  `IMPORT-008`, `IMPORT-009`, and `IMPORT-011` on `#[derive(...)]`.
