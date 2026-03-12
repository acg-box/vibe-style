#!/usr/bin/env bash

set -euo pipefail

usage() {
	cat <<'EOF'
Usage: scripts/bench-semantic-vstyle.sh [--profile release|final-release]

Environment:
  VSTYLE_BENCH_PROFILE   Override the benchmark build profile.
EOF
}

profile="${VSTYLE_BENCH_PROFILE:-final-release}"

while [[ $# -gt 0 ]]; do
	case "$1" in
		--profile)
			profile="${2:-}"
			shift 2
			;;
		-h|--help)
			usage
			exit 0
			;;
		*)
			printf 'Unknown argument: %s\n' "$1" >&2
			usage >&2
			exit 1
			;;
	esac
done

case "$profile" in
	release)
		build_args=(--release --bins)
		profile_dir="release"
		;;
	final-release)
		build_args=(--profile final-release --bins)
		profile_dir="final-release"
		;;
	*)
		printf 'Unsupported profile: %s\n' "$profile" >&2
		exit 1
		;;
esac

repo_root="$(git rev-parse --show-toplevel)"
commit="$(git rev-parse HEAD)"
commit_short="$(git rev-parse --short HEAD)"
timestamp_utc="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"
artifact_stamp="$(date -u +"%Y%m%dT%H%M%SZ")"
log_dir="${repo_root}/target/vstyle-bench-semantic/${artifact_stamp}-${profile_dir}"
fixture_root="$(mktemp -d "${TMPDIR:-/tmp}/vstyle-semantic-bench.XXXXXX")"

cleanup() {
	rm -rf "$fixture_root"
}

trap cleanup EXIT

mkdir -p "$log_dir"

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

extract_cache_hits() {
	local stdout_file="$1"
	local stderr_file="$2"

	cat "$stdout_file" "$stderr_file" \
		| sed -nE 's/.*Semantic cache: ([0-9]+) hit\(s\), ([0-9]+) miss\(es\)\..*/\1/p' \
		| tail -n 1
}

extract_cache_misses() {
	local stdout_file="$1"
	local stderr_file="$2"

	cat "$stdout_file" "$stderr_file" \
		| sed -nE 's/.*Semantic cache: ([0-9]+) hit\(s\), ([0-9]+) miss\(es\)\..*/\2/p' \
		| tail -n 1
}

run_bench() {
	local label="$1"
	shift

	local time_file="${log_dir}/${label}.time"
	local stdout_file="${log_dir}/${label}.stdout"
	local stderr_file="${log_dir}/${label}.stderr"
	local status

	set +e
	(
		cd "$fixture_root" || exit 1
		/usr/bin/time -p -o "$time_file" "$@" >"$stdout_file" 2>"$stderr_file"
	)
	status=$?
	set -e

	local real_s
	local user_s
	local sys_s
	local cache_hits
	local cache_misses
	real_s="$(awk '/^real / {print $2}' "$time_file")"
	user_s="$(awk '/^user / {print $2}' "$time_file")"
	sys_s="$(awk '/^sys / {print $2}' "$time_file")"
	cache_hits="$(extract_cache_hits "$stdout_file" "$stderr_file")"
	cache_misses="$(extract_cache_misses "$stdout_file" "$stderr_file")"

	printf '%s_EXIT=%s\n' "${label^^}" "$status"
	printf '%s_REAL_S=%s\n' "${label^^}" "$real_s"
	printf '%s_USER_S=%s\n' "${label^^}" "$user_s"
	printf '%s_SYS_S=%s\n' "${label^^}" "$sys_s"
	printf '%s_CACHE_HITS=%s\n' "${label^^}" "${cache_hits:-unknown}"
	printf '%s_CACHE_MISSES=%s\n' "${label^^}" "${cache_misses:-unknown}"

	return "$status"
}

printf 'Building local binary with profile %s...\n' "$profile"
(cd "$repo_root" && cargo build "${build_args[@]}")

binary_path="${repo_root}/target/${profile_dir}/vstyle"
if [[ ! -x "$binary_path" ]]; then
	printf 'Missing benchmark binary: %s\n' "$binary_path" >&2
	exit 1
fi

printf 'Preparing semantic benchmark fixture at %s...\n' "$fixture_root"
write_fixture_sources
git -C "$fixture_root" init >/dev/null
git -C "$fixture_root" add Cargo.toml src/main.rs src/safe.rs src/unsafe.rs .gitignore >/dev/null
(cd "$fixture_root" && cargo generate-lockfile >/dev/null)

rm -rf "$fixture_root/target/vstyle-cache/semantic"

summary_file="${log_dir}/summary.txt"
{
	printf 'BENCHMARK_DATE_UTC=%s\n' "$timestamp_utc"
	printf 'COMMIT=%s\n' "$commit"
	printf 'COMMIT_SHORT=%s\n' "$commit_short"
	printf 'PROFILE=%s\n' "$profile"
	printf 'BENCH_KIND=semantic-positive-let-mut-reorder\n'
	printf 'BINARY=%s\n' "$binary_path"
	printf 'FIXTURE_ROOT=%s\n' "$fixture_root"
	printf 'LOG_DIR=%s\n' "$log_dir"
	printf 'VSTYLE_VERSION=%s\n' "$("$binary_path" --version)"
	run_bench cold_tune "$binary_path" tune --verbose
	write_fixture_sources
	run_bench warm_tune "$binary_path" tune --verbose
} | tee "$summary_file"

printf 'Benchmark summary saved to %s\n' "$summary_file"
