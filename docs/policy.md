# Documentation Policy

Purpose: Define how agent-facing documentation is organized, updated, and kept consistent
across this repository.

Audience: All documentation under `docs/` is written for AI agents and LLM workflows.
The split between lanes is by question type, not by reader type.

## Principles

- Optimize for retrieval, routing, and execution.
- Keep one authoritative document per topic.
- Separate normative truth from procedural steps.
- Prefer explicit section labels and stable links over prose-heavy narrative.
- Use the standard docs lanes unless a checked-in tool requires a compatibility exception.

## Document classes

| Class | Location | Answers | Source of truth for | Update trigger |
| --- | --- | --- | --- | --- |
| Spec | `docs/spec/` | What must be true? | Contracts, schemas, invariants, required behavior | Any behavior or schema change |
| Runbook | `docs/runbook/` | What should I do? | Runbooks, migrations, validation, troubleshooting | Any procedure or operational change |
| Reference | `docs/reference/` | How is it organized now? | Current structure, ownership boundaries, surface maps | Any structure or ownership change |
| Decision | `docs/decisions/` | Why is it shaped this way? | Durable rationale, tradeoffs, consequences | Any durable architecture or policy decision |
| Research | `docs/research/` | What evidence supports this? | Benchmark records and supporting investigation artifacts | Any evidence refresh or archived investigation |
| Plan artifacts | `docs/plans/` | Which saved plan artifact should a planning tool or execution workflow use? | Tool-managed planning outputs | As emitted or updated by the relevant tool |

## Placement rules

- If a document defines correctness, it belongs in `docs/spec/`.
- If a document defines actions, it belongs in `docs/runbook/`.
- If a document maps current organization, ownership, or implementation surfaces, it
  belongs in `docs/reference/`.
- If a document records durable rationale and consequences, it belongs in
  `docs/decisions/`.
- If a document records supporting evidence, benchmark output, or investigation history,
  it belongs in `docs/research/`.
- Benchmark selection and validation procedure belongs in `docs/runbook/`; benchmark
  records belong in `docs/research/benchmarks/`.
- Do not treat `docs/plans/` as a general-purpose docs bucket.
- Use `docs/plans/` only for artifacts produced or consumed by planning tools or
  workflows that explicitly depend on saved plan files.
- Do not duplicate the same authoritative content across documents. Link to the source
  of truth instead.
- A runbook may summarize why a step exists, but normative statements still live in the
  governing spec.

## Document contracts

Every document should start with a short routing header.

Spec header:

- `Purpose`
- `Status: normative`
- `Read this when`
- `Not this document`
- `Defines`

Runbook header:

- `Goal`
- `Read this when`
- `Inputs` or `Preconditions`
- `Depends on`
- `Outputs` or `Verification`

Reference header:

- `Purpose`
- `Read this when`
- `Not this document`
- `Covers`

Decision header:

- `Status`
- `Date`
- `Question`
- `Decision`
- `Consequences`

Research records should make scope, workload or evidence source, results, and verification
explicit. Benchmark records may keep the established benchmark-note shape when that is
clearer than a generic research template.

## Structure rules

- Prefer shallow paths by default within each lane.
- Add subfolders only when they mirror stable system boundaries or improve retrieval.
- Use descriptive `snake_case` file names.
- Do not require fixed filename prefixes unless a real ambiguity appears.
- Keep one `index.md` per standard lane so the router can state whether that lane has
  current topic documents.
- Do not create additional empty subfolders or placeholder topic documents.

## Canonical entry points

- Unified documentation router: `docs/index.md`
- Documentation policy: `docs/policy.md`
- Normative router: `docs/spec/index.md`
- Procedural router: `docs/runbook/index.md`
- Current-state router: `docs/reference/index.md`
- Decision router: `docs/decisions/index.md`
- Supporting-evidence router: `docs/research/index.md`
- Repo task and automation entrypoints: `Makefile.toml`

## LLM reading guidance

When answering a repository question:

1. Read `docs/index.md` for routing.
2. Route by question type:
   - "What must be true?" -> `docs/spec/index.md`
   - "What should I do?" -> `docs/runbook/index.md`
   - "How is it organized now?" -> `docs/reference/index.md`
   - "Why is it shaped this way?" -> `docs/decisions/index.md`
   - "What evidence supports this?" -> `docs/research/index.md`
3. Read `Makefile.toml` when the task depends on repository automation or named tasks.
4. Use `docs/plans/` only when the task explicitly concerns a saved plan artifact used by
   a planning tool or execution workflow.

## Update workflow

- Behavior or schema change: update the relevant spec.
- Procedure change: update the relevant runbook.
- Current organization change: update the relevant reference document.
- Durable rationale change: update the relevant decision document.
- Benchmark or investigation evidence refresh: update the relevant research artifact.
- If a change touches both truth and procedure, update both documents and keep their
  boundary explicit.
- When a runbook starts carrying normative content, move that content into spec and link
  to it.
- Do not impose local document-header requirements on files under `docs/plans/`; those
  files are owned by the planning tool or workflow that created them.
