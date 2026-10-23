#!/usr/bin/env bash

set -euo pipefail

source "$(dirname "${BASH_SOURCE[0]}")/benchmark-common.bash"
benchmark_init vstyle-bench-semantic scripts/bench-semantic-vstyle.sh "$@"

fixture_root="$(mktemp -d "${TMPDIR:-/tmp}/vstyle-semantic-bench.XXXXXX")"
trap 'rm -rf "$fixture_root"' EXIT

write_fixture_sources() {
	mkdir -p "$fixture_root/src"

	cat > "$fixture_root/Cargo.toml" <<'EOF'
[package]
name = "vstyle-let-mut-reorder-fixture"
version = "0.1.0"
edition = "2021"
EOF
	printf '/target\n' > "$fixture_root/.gitignore"

	cat > "$fixture_root/src/main.rs" <<'EOF'
mod safe;
mod r#unsafe;

fn main() {}
EOF

	cat > "$fixture_root/src/safe.rs" <<'EOF'
pub fn safe_case() -> usize {
	let mut mutable_value = 1usize;
	let immutable_value = 2usize;
	mutable_value + immutable_value
}
EOF

	cat > "$fixture_root/src/unsafe.rs" <<'EOF'
pub fn closure_carries_binding() {
	let mut value = String::from("value");
	let _trace = format!("{}\n", value);
	let deferred = || value;
	let _ = deferred();
}
EOF
}

benchmark_build

printf 'Preparing semantic benchmark fixture at %s...\n' "$fixture_root"
write_fixture_sources
git -C "$fixture_root" init --quiet
(cd "$fixture_root" && cargo generate-lockfile >/dev/null)

summary_file="${log_dir}/summary.txt"
{
	benchmark_header
	printf 'BENCH_KIND=semantic-positive-let-mut-reorder\n'
	printf 'FIXTURE_ROOT=%s\n' "$fixture_root"
	benchmark_measure "$fixture_root" cold_tune "$binary_path" tune --language rust --verbose
	write_fixture_sources
	benchmark_measure "$fixture_root" warm_tune "$binary_path" tune --language rust --verbose
} | tee "$summary_file"

printf 'Benchmark summary saved to %s\n' "$summary_file"
