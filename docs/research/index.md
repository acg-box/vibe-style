# Research Index

Purpose: Route agents to supporting evidence that informs repository work but is not the
primary authority for contracts, procedures, or current structure.

Question this index answers: "what evidence supports this?"

## Use this index when

- You need benchmark records, checkpoint history, investigation notes, or evidence that
  supports a runbook, spec, or decision.
- You are comparing current measurements with prior recorded runs.
- A runbook points to historical evidence and you need the underlying artifact.

## Do not use this index when

- You need the authoritative behavior contract; read `docs/spec/index.md`.
- You need the sequence to execute; read `docs/runbook/index.md`.
- You need a planning-tool artifact or saved execution plan under `docs/plans/`.

## Current research

- `docs/research/benchmarks/index.md`: benchmark checkpoint records and
  performance-history evidence.

## Research document contract

Research artifacts should make the evidence boundary explicit:

- Scope or question.
- Workload, source, or data being evaluated.
- Results or observations.
- Verification commands or evidence anchors.
- Current decision, note, or follow-up when the artifact affects future work.

## Structure policy

- Keep research as supporting evidence, not primary policy.
- Link from runbooks, specs, and decisions to research artifacts instead of copying
  benchmark output or investigation notes into authoritative documents.
- Add subfolders only for stable evidence families such as `benchmarks/`.
