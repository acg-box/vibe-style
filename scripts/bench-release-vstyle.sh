#!/usr/bin/env bash

set -euo pipefail

usage() {
	cat <<'EOF'
Usage: scripts/bench-release-vstyle.sh [--profile release|final-release]

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
log_dir="${repo_root}/target/vstyle-bench/${artifact_stamp}-${profile_dir}"
bench_worktree="$(mktemp -d "${TMPDIR:-/tmp}/vstyle-release-bench.XXXXXX")"

cleanup() {
	git -C "$repo_root" worktree remove --force "$bench_worktree" >/dev/null 2>&1 || true
	rm -rf "$bench_worktree"
}

trap cleanup EXIT

mkdir -p "$log_dir"

printf 'Building local binary with profile %s...\n' "$profile"
(cd "$repo_root" && cargo build "${build_args[@]}")

binary_path="${repo_root}/target/${profile_dir}/vstyle"
if [[ ! -x "$binary_path" ]]; then
	printf 'Missing benchmark binary: %s\n' "$binary_path" >&2
	exit 1
fi

printf 'Creating disposable benchmark worktree at %s...\n' "$bench_worktree"
git -C "$repo_root" worktree add --detach "$bench_worktree" "$commit" >/dev/null

run_bench() {
	local label="$1"
	shift

	local time_file="${log_dir}/${label}.time"
	local stdout_file="${log_dir}/${label}.stdout"
	local stderr_file="${log_dir}/${label}.stderr"
	local status

	set +e
	(
		cd "$bench_worktree" || exit 1
		/usr/bin/time -p -o "$time_file" "$@" >"$stdout_file" 2>"$stderr_file"
	)
	status=$?
	set -e

	local real_s
	local user_s
	local sys_s
	real_s="$(awk '/^real / {print $2}' "$time_file")"
	user_s="$(awk '/^user / {print $2}' "$time_file")"
	sys_s="$(awk '/^sys / {print $2}' "$time_file")"

	printf '%s_EXIT=%s\n' "${label^^}" "$status"
	printf '%s_REAL_S=%s\n' "${label^^}" "$real_s"
	printf '%s_USER_S=%s\n' "${label^^}" "$user_s"
	printf '%s_SYS_S=%s\n' "${label^^}" "$sys_s"

	return "$status"
}

summary_file="${log_dir}/summary.txt"
{
	printf 'BENCHMARK_DATE_UTC=%s\n' "$timestamp_utc"
	printf 'COMMIT=%s\n' "$commit"
	printf 'COMMIT_SHORT=%s\n' "$commit_short"
	printf 'PROFILE=%s\n' "$profile"
	printf 'BINARY=%s\n' "$binary_path"
	printf 'WORKTREE=%s\n' "$bench_worktree"
	printf 'LOG_DIR=%s\n' "$log_dir"
	printf 'VSTYLE_VERSION=%s\n' "$("$binary_path" --version)"
	run_bench curate "$binary_path" curate --workspace
	run_bench tune "$binary_path" tune --workspace --verbose
} | tee "$summary_file"

printf 'Benchmark summary saved to %s\n' "$summary_file"
