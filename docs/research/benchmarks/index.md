# Benchmark Research Index

Purpose: Route agents to benchmark checkpoint records and performance-history evidence.

Question this index answers: "which benchmark record should I compare against?"

## Use this index when

- You need historical release or semantic benchmark output.
- You need the benchmark evidence behind a performance decision.
- `docs/runbook/benchmark_tracking.md` tells you to record or compare checkpoint
  results.

## Do not use this index when

- You need to choose which benchmark to run; read `docs/runbook/benchmark_tracking.md`.
- You need repo task names; read `Makefile.toml`.
- You need behavior contracts; read `docs/spec/index.md`.

## Current benchmark records

- `docs/research/benchmarks/2026-03-12_vstyle-release-runtime-baseline.md`:
  release-path baseline and checkpoint history.
- `docs/research/benchmarks/2026-03-12_vstyle-semantic-runtime-baseline.md`:
  semantic-positive benchmark baseline and follow-up history.
- `docs/research/benchmarks/2026-03-12_vstyle-spacing-regex-hoist.md`:
  spacing regex-hoist checkpoint evidence.
- `docs/research/benchmarks/2026-03-12_vstyle-spacing-regex-follow-up.md`:
  spacing regex follow-up evidence.
- `docs/research/benchmarks/2026-03-12_vstyle-module-quality-follow-up.md`:
  module and quality runtime follow-up evidence.
- `docs/research/benchmarks/2026-03-12_vstyle-spacing-quality-regex-hoist-follow-up.md`:
  spacing and quality regex-hoist follow-up evidence.
- `docs/research/benchmarks/2026-03-13_vstyle-pubfi-noop-tune-fast-path.md`:
  downstream no-op tune fast-path evidence.
- `docs/research/benchmarks/2026-03-14_vstyle-benchmark-tracking-bootstrap.md`:
  benchmark workflow bootstrap evidence.

## Structure policy

- Keep benchmark records chronological and topic-specific.
- Put selection procedures in `docs/runbook/benchmark_tracking.md`, not in benchmark
  records.
- Link to benchmark records instead of copying result tables into specs or runbooks.
