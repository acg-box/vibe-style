#!/usr/bin/env bash

# Shared setup and measurements for the two benchmark entrypoints.
benchmark_init() {
	local lane="$1"
	local program="$2"
	shift 2

	profile="${VSTYLE_BENCH_PROFILE:-final-release}"
	while [[ $# -gt 0 ]]; do
		case "$1" in
			--profile)
				if [[ $# -lt 2 ]]; then
					printf 'Missing value for --profile.\n' >&2
					return 1
				fi
				profile="$2"
				shift 2
				;;
			-h|--help)
				printf 'Usage: %s [--profile release|final-release]\n\nEnvironment:\n  VSTYLE_BENCH_PROFILE   Override the benchmark build profile.\n' "$program"
				exit 0
				;;
			*)
				printf 'Unknown argument: %s\n' "$1" >&2
				return 1
				;;
		esac
	done

	case "$profile" in
		release) build_args=(--release --locked --bins); profile_dir="release" ;;
		final-release) build_args=(--profile final-release --locked --bins); profile_dir="final-release" ;;
		*) printf 'Unsupported profile: %s\n' "$profile" >&2; return 1 ;;
	esac

	repo_root="$(git rev-parse --show-toplevel)"
	commit="$(git rev-parse HEAD)"
	commit_short="$(git rev-parse --short HEAD)"
	timestamp_utc="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"
	log_dir="${repo_root}/target/${lane}/$(date -u +"%Y%m%dT%H%M%SZ")-${profile_dir}"
	binary_path="${repo_root}/target/${profile_dir}/vstyle"
	mkdir -p "$log_dir"
}

benchmark_build() {
	printf 'Building local binary with profile %s...\n' "$profile"
	(cd "$repo_root" && cargo build "${build_args[@]}")
	test -x "$binary_path"
}

benchmark_header() {
	printf 'BENCHMARK_DATE_UTC=%s\nCOMMIT=%s\nCOMMIT_SHORT=%s\nPROFILE=%s\nBINARY=%s\nLOG_DIR=%s\n' \
		"$timestamp_utc" "$commit" "$commit_short" "$profile" "$binary_path" "$log_dir"
	printf 'VSTYLE_VERSION=%s\n' "$("$binary_path" --version)"
}

benchmark_measure() {
	local directory="$1"
	local label="$2"
	shift 2

	local time_file="${log_dir}/${label}.time"
	local status
	local metric_label

	metric_label="$(printf '%s' "$label" | tr '[:lower:]' '[:upper:]')"

	set +e
	(cd "$directory" && /usr/bin/time -p -o "$time_file" "$@" >"${log_dir}/${label}.stdout" 2>"${log_dir}/${label}.stderr")
	status=$?
	set -e

	printf '%s_EXIT=%s\n' "$metric_label" "$status"
	awk -v label="$metric_label" '/^real / { print label "_REAL_S=" $2 } /^user / { print label "_USER_S=" $2 } /^sys / { print label "_SYS_S=" $2 }' "$time_file"
	return "$status"
}
