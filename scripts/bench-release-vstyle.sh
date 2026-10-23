#!/usr/bin/env bash

set -euo pipefail

source "$(dirname "${BASH_SOURCE[0]}")/benchmark-common.bash"
benchmark_init vstyle-bench scripts/bench-release-vstyle.sh "$@"

fixture_root="$(mktemp -d "${TMPDIR:-/tmp}/vstyle-release-bench.XXXXXX")"
trap 'rm -rf "$fixture_root"' EXIT

benchmark_build

printf 'Preparing source snapshot at %s...\n' "$fixture_root"
git -C "$repo_root" archive "$commit" | tar -x -C "$fixture_root"
git -C "$fixture_root" init --quiet

summary_file="${log_dir}/summary.txt"
{
	benchmark_header
	printf 'FIXTURE_ROOT=%s\n' "$fixture_root"
	benchmark_measure "$fixture_root" curate "$binary_path" curate --language rust --workspace
	benchmark_measure "$fixture_root" tune "$binary_path" tune --language rust --workspace --verbose
} | tee "$summary_file"

printf 'Benchmark summary saved to %s\n' "$summary_file"
