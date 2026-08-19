#!/usr/bin/env bash
set -euo pipefail

IMAGE="grpc-php-rs-test"
CONTAINER="grpc-rs-zts-test"
DOCKERFILE="tests/Dockerfile"
COMPOSE_INTEGRATION="docker-compose.integration.yml"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
NC='\033[0m'

info()  { echo -e "${CYAN}===${NC} $*"; }
ok()    { echo -e "${GREEN}  ✓${NC} $*"; }
fail()  { echo -e "${RED}  ✗${NC} $*"; }
warn()  { echo -e "${YELLOW}  !${NC} $*"; }

usage() {
    cat <<EOF
Usage: ./test.sh [command]

Commands:
  all         Build + run all core suites (default; excludes zts/parity/bench/integration)
  smoke       Run PHP smoke test (API surface, no network)
  compat      Run grpc/grpc library compatibility test (Issue #4)
  grpc-gcp    Run google/grpc-gcp channel pool compatibility test
  firestore   Run Firestore client compatibility test
  leak        Run memory leak test with local gRPC test server
  streaming   Run server streaming test with local gRPC test server
  trailer     Run binary trailing metadata test with local gRPC test server
  unary       Run split-batch unary test with local gRPC test server
  promises    Run gax-style promise concurrency test (grpc/grpc + guzzle)
  ledger      Run ext-grpc parity ledger (known divergences, strict-xfail)
  bench       Benchmark grpc-php-rs vs official ext-grpc, side by side
  parity      Diff observable behavior against official ext-grpc (pecl)
  zts         Run ZTS stress test with FrankenPHP + concurrent requests
  temporal    Run Temporal SDK integration test (starts temporalio/auto-setup)
  ecosystem   Run google/cloud-{pubsub,spanner,firestore} against emulators + lib checks
  otel        Run OpenTelemetry integration test (starts otel-collector)
  integration Run both temporal + otel integration tests
  shell       Drop into PHP CLI with extension loaded
EOF
}

# --- Helpers ---

# Build one or more Docker targets. Docker caches shared stages (builder, test-base)
# so only the first build is slow.
build_target() {
    local target="$1"
    local tag="${IMAGE}:${target}"
    info "Building ${target}"
    DOCKER_BUILDKIT=1 docker build \
        --target "$target" \
        -t "$tag" \
        -f "$DOCKERFILE" . \
        --quiet > /dev/null
}

run_target() {
    local target="$1"
    docker run --rm "${IMAGE}:${target}"
}

# --- Commands ---

cmd_smoke() {
    build_target test-smoke
    info "Running smoke test"
    run_target test-smoke
    ok "Smoke test passed"
}

cmd_compat() {
    build_target test-compat
    info "Running grpc/grpc compatibility test"
    run_target test-compat
    ok "Compatibility test passed"
}

cmd_grpc_gcp() {
    build_target test-grpc-gcp
    info "Running google/grpc-gcp compatibility test"
    run_target test-grpc-gcp
    ok "grpc-gcp test passed"
}

cmd_firestore() {
    build_target test-firestore
    info "Running Firestore compatibility test"
    run_target test-firestore
    ok "Firestore test passed"
}

cmd_leak() {
    build_target test-leak
    info "Running memory leak tests"
    run_target test-leak
    ok "Memory leak test passed"
}

cmd_streaming() {
    build_target test-streaming
    info "Running server streaming test"
    run_target test-streaming
    ok "Server streaming test passed"
}

cmd_trailer() {
    build_target test-binary-trailer
    info "Running binary trailing metadata test"
    run_target test-binary-trailer
    ok "Binary trailer test passed"
}

cmd_unary() {
    build_target test-unary
    info "Running split-batch unary test"
    run_target test-unary
    ok "Split-batch unary test passed"
}

cmd_promises() {
    build_target test-promises
    info "Running gax-style promise concurrency test"
    run_target test-promises
    ok "Promise concurrency test passed"
}

cmd_ledger() {
    build_target test-parity-ledger
    info "Running ext-grpc parity ledger"
    run_target test-parity-ledger
    ok "Parity ledger consistent"
}

cmd_bench() {
    build_target test-bench
    build_target test-parity-extgrpc

    info "Benchmarking grpc-php-rs"
    local ours ext
    ours=$(docker run --rm "${IMAGE}:test-bench")
    info "Benchmarking official ext-grpc"
    ext=$(docker run --rm "${IMAGE}:test-parity-extgrpc" sh -c "grpc-test-server 2>/dev/null & sleep 1 && php tests/bench.php")

    info "Results (median µs; ratio >1 means grpc-php-rs is slower)"
    python3 - "$ours" "$ext" <<'PYEOF'
import sys
def parse(text):
    out = {}
    for line in text.strip().splitlines():
        parts = line.split('|')
        if len(parts) == 3:
            out[parts[0]] = (float(parts[1]), float(parts[2]))
    return out
ours, ext = parse(sys.argv[1]), parse(sys.argv[2])
if not ours or set(ours) != set(ext):
    missing = set(ours) ^ set(ext)
    print(f"benchmark incomplete: scenario mismatch {sorted(missing) or '(both empty)'}")
    sys.exit(1)
print(f"{'scenario':<24}{'grpc-php-rs':>14}{'ext-grpc':>14}{'ratio':>8}")
for name in ours:
    o, e = ours[name][0], ext[name][0]
    flag = '  <-- slower' if o > e * 1.15 else ''
    print(f"{name:<24}{o:>12.1f}µs{e:>12.1f}µs{o/e:>8.2f}{flag}")
PYEOF
    ok "Benchmark complete"
}

cmd_parity() {
    build_target test-parity-ours
    build_target test-parity-extgrpc

    info "Running probe under grpc-php-rs"
    local ours ext
    ours=$(docker run --rm "${IMAGE}:test-parity-ours")
    info "Running probe under official ext-grpc"
    ext=$(docker run --rm "${IMAGE}:test-parity-extgrpc")

    if diff <(echo "$ext") <(echo "$ours") > /tmp/grpc_parity.diff; then
        ok "Parity: identical observable behavior (ext-grpc vs grpc-php-rs)"
    else
        fail "Parity divergence (< ext-grpc, > grpc-php-rs):"
        cat /tmp/grpc_parity.diff
        exit 1
    fi
}

cmd_zts() {
    build_target test-zts

    info "Starting FrankenPHP (ZTS, threaded classic mode)"
    docker rm -f "$CONTAINER" 2>/dev/null || true
    docker run -d --name "$CONTAINER" \
        -p 8099:8080 \
        "${IMAGE}:test-zts"

    info "Waiting for FrankenPHP to start"
    local retries=30
    while ! docker exec "$CONTAINER" php -r "echo 'ok';" &>/dev/null; do
        retries=$((retries - 1))
        if [ "$retries" -le 0 ]; then
            fail "Container failed to start within 15s"
            docker logs "$CONTAINER"
            docker rm -f "$CONTAINER" 2>/dev/null || true
            exit 1
        fi
        sleep 0.5
    done

    info "Waiting for the HTTP listener (readiness = a real request succeeds, not just php -r)"
    retries=60
    until curl -sf -o /dev/null http://localhost:8099/test_zts_stress.php; do
        retries=$((retries - 1))
        if [ "$retries" -le 0 ]; then
            fail "HTTP listener did not become ready within 30s"
            docker logs "$CONTAINER" | tail -20
            docker rm -f "$CONTAINER" 2>/dev/null || true
            exit 1
        fi
        sleep 0.5
    done

    info "Verifying extension loaded in ZTS container"
    docker exec "$CONTAINER" php -r "exit(extension_loaded('grpc') ? 0 : 1);" || {
        fail "grpc extension not loaded in ZTS container"
        docker rm -f "$CONTAINER" 2>/dev/null || true
        exit 1
    }

    info "Running smoke test inside ZTS container"
    docker exec "$CONTAINER" php /app/tests/test_smoke.php

    info "Concurrent stress test (200 requests, 10 concurrent)"
    local stress_out
    stress_out=$(mktemp -d)
    local curl_failed=0
    for i in $(seq 1 10); do
        local pids=()
        for j in $(seq 1 20); do
            curl -sf http://localhost:8099/test_zts_stress.php > "$stress_out/$i.$j" &
            pids+=($!)
        done
        local p
        for p in "${pids[@]}"; do
            wait "$p" || curl_failed=$((curl_failed + 1))
        done
        echo "--- Batch $i/10 complete ---"
    done
    local bad_bodies
    # grep -L exits 1 when every file matches (the GOOD case) — guard it or
    # set -e kills the script precisely when all 200 requests succeeded.
    bad_bodies=$( (grep -L '^OK' "$stress_out"/*.* 2>/dev/null || true) | wc -l | tr -d ' ')
    if [ "$curl_failed" -ne 0 ] || [ "$bad_bodies" -ne 0 ]; then
        fail "Stress requests failed: $curl_failed curl errors, $bad_bodies non-OK bodies"
        (grep -L '^OK' "$stress_out"/*.* 2>/dev/null || true) | head -3 | xargs -I{} sh -c 'echo "--- {}"; cat {}'
        docker rm -f "$CONTAINER" 2>/dev/null || true
        rm -rf "$stress_out"
        exit 1
    fi
    ok "All 200 requests returned OK bodies"
    rm -rf "$stress_out"

    echo ""
    info "Checking container still alive"
    local failed=0
    if docker exec "$CONTAINER" php -r "echo 'alive';"; then
        echo ""
        ok "Container survived 200 concurrent gRPC+TLS requests under ZTS!"
    else
        echo ""
        fail "Container crashed — check: docker logs $CONTAINER"
        failed=1
    fi

    docker rm -f "$CONTAINER" 2>/dev/null || true
    [ "$failed" -eq 0 ] || exit 1
}

cmd_ecosystem() {
    info "Running Google Cloud ecosystem tests (emulators + real clients)"
    warn "First run pulls the cloud-sdk emulators image (~1.3GB)"
    local COMPOSE="docker compose -f $COMPOSE_INTEGRATION"
    trap "$COMPOSE down --volumes 2>/dev/null || true" EXIT
    DOCKER_BUILDKIT=1 $COMPOSE build test-ecosystem
    $COMPOSE run --rm test-ecosystem php /integration/test_pubsub.php
    ok "Pub/Sub emulator test passed"
    $COMPOSE run --rm test-ecosystem php /integration/test_spanner.php
    ok "Spanner emulator test passed"
    $COMPOSE run --rm test-ecosystem php /integration/test_firestore_emulator.php
    ok "Firestore emulator test passed"
    $COMPOSE run --rm test-ecosystem php /integration/test_bigtable.php
    ok "Bigtable emulator test passed"
    $COMPOSE run --rm test-ecosystem php /integration/test_datastore.php
    ok "Datastore emulator test passed"
    $COMPOSE run --rm test-ecosystem php /integration/test_etcd.php
    ok "etcd test passed"
    $COMPOSE run --rm test-ecosystem php /integration/test_google_libs.php
    ok "Library construction checks passed"
    $COMPOSE down --volumes
    trap - EXIT
    ok "Ecosystem tests passed"
}

cmd_temporal() {
    info "Running Temporal SDK integration test"
    warn "This starts temporalio/auto-setup — may take ~30s on first run"
    local COMPOSE="docker compose -f $COMPOSE_INTEGRATION"
    trap "$COMPOSE down --volumes 2>/dev/null || true" EXIT
    DOCKER_BUILDKIT=1 $COMPOSE build test-temporal
    $COMPOSE run --rm test-temporal
    $COMPOSE down --volumes
    trap - EXIT
    ok "Temporal integration test passed"
}

cmd_otel() {
    info "Running OpenTelemetry integration test"
    local COMPOSE="docker compose -f $COMPOSE_INTEGRATION"
    trap "$COMPOSE down --volumes 2>/dev/null || true" EXIT
    DOCKER_BUILDKIT=1 $COMPOSE build test-otel
    $COMPOSE run --rm test-otel
    $COMPOSE down --volumes
    trap - EXIT
    ok "OpenTelemetry integration test passed"
}

cmd_integration() {
    info "Running all integration tests (Temporal + OpenTelemetry)"
    local COMPOSE="docker compose -f $COMPOSE_INTEGRATION"
    trap "$COMPOSE down --volumes 2>/dev/null || true" EXIT
    DOCKER_BUILDKIT=1 $COMPOSE build test-temporal test-otel
    $COMPOSE run --rm test-temporal
    ok "Temporal test passed"
    $COMPOSE run --rm test-otel
    ok "OpenTelemetry test passed"
    $COMPOSE down --volumes
    trap - EXIT
    ok "All integration tests passed"
}

cmd_shell() {
    build_target test-base
    info "Dropping into PHP CLI with extension loaded"
    docker run --rm -it "${IMAGE}:test-base" bash
}

cmd_all() {
    info "Building and running all core tests"
    echo ""
    cmd_smoke
    echo ""
    cmd_compat
    echo ""
    cmd_grpc_gcp
    echo ""
    cmd_streaming
    echo ""
    cmd_trailer
    echo ""
    cmd_firestore
    echo ""
    cmd_unary
    echo ""
    cmd_promises
    echo ""
    cmd_ledger
    echo ""
    cmd_leak
    echo ""
    ok "Core suites passed (zts/parity/bench/integration/ecosystem run separately)"
}

# --- Main ---

command="${1:-all}"

case "$command" in
    all)         cmd_all ;;
    smoke)       cmd_smoke ;;
    compat)      cmd_compat ;;
    grpc-gcp)    cmd_grpc_gcp ;;
    firestore)   cmd_firestore ;;
    leak)        cmd_leak ;;
    streaming)   cmd_streaming ;;
    trailer)     cmd_trailer ;;
    unary)       cmd_unary ;;
    promises)    cmd_promises ;;
    ledger)      cmd_ledger ;;
    bench)       cmd_bench ;;
    parity)      cmd_parity ;;
    zts)         cmd_zts ;;
    temporal)    cmd_temporal ;;
    ecosystem)   cmd_ecosystem ;;
    otel)        cmd_otel ;;
    integration) cmd_integration ;;
    shell)       cmd_shell ;;
    -h|--help|help) usage ;;
    *)
        echo "Unknown command: $command"
        usage
        exit 1
        ;;
esac
