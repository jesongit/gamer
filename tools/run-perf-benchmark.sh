#!/usr/bin/env sh
# PERF offline benchmark entry point for Linux/Docker.
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
ITERATIONS=${GAMER_PERF_ITERS:-20}
WARMUP=${GAMER_PERF_WARMUP:-3}
FRESHNESS_MS=${GAMER_DECODE_FRESHNESS_MS:-75}
RELEASE=1
FULL_SCREEN=0

usage() {
    echo "usage: tools/run-perf-benchmark.sh [-i iterations] [-w warmup] [-f 50..100] [--debug] [--full-screen]" >&2
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        -i|--iterations) ITERATIONS=$2; shift 2 ;;
        -w|--warmup) WARMUP=$2; shift 2 ;;
        -f|--freshness-ms) FRESHNESS_MS=$2; shift 2 ;;
        --debug) RELEASE=0; shift ;;
        --full-screen) FULL_SCREEN=1; shift ;;
        -h|--help) usage; exit 0 ;;
        *) usage; exit 2 ;;
    esac
done

case "$FRESHNESS_MS" in
    50|51|52|53|54|55|56|57|58|59|6[0-9]|7[0-9]|8[0-9]|9[0-9]|100) ;;
    *) echo "freshness must be between 50 and 100 ms" >&2; exit 2 ;;
esac

export GAMER_PERF_ITERS=$ITERATIONS
export GAMER_PERF_WARMUP=$WARMUP
export GAMER_DECODE_FRESHNESS_MS=$FRESHNESS_MS
if [ "$FULL_SCREEN" -eq 1 ]; then
    export GAMER_PERF_FULL_SCREEN=1
else
    unset GAMER_PERF_FULL_SCREEN 2>/dev/null || true
fi

cd "$ROOT/server"
if [ "$RELEASE" -eq 1 ]; then
    cargo test --release matcher::tests::fixed_fixture_benchmark_p50_p95 -- --ignored --nocapture
else
    cargo test matcher::tests::fixed_fixture_benchmark_p50_p95 -- --ignored --nocapture
fi
