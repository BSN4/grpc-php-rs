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
  all         Build + run smoke + compat tests (default)
  smoke       Run PHP smoke test (API surface, no network)
  compat      Run grpc/grpc library compatibility test (Issue #4)
  grpc-gcp    Run google/grpc-gcp channel pool compatibility test
  firestore   Run Firestore client compatibility test
  leak        Run memory leak test with local gRPC test server
  streaming   Run server streaming test with local gRPC test server
  zts         Run ZTS stress test with FrankenPHP + concurrent requests
  temporal    Run Temporal SDK integration test (starts temporalio/auto-setup)
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

cmd_zts() {
    build_target test-zts

    info "Starting FrankenPHP with 4 workers"
    docker rm -f "$CONTAINER" 2>/dev/null || true
    docker run -d --name "$CONTAINER" \
        -p 8099:8080 \
        -e FRANKENPHP_WORKERS="/app/tests/test_zts_stress.php=4" \
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

    info "Verifying extension loaded in ZTS container"
    docker exec "$CONTAINER" php -r "echo 'grpc loaded: ' . (extension_loaded('grpc') ? 'yes' : 'no') . PHP_EOL;"

    info "Running smoke test inside ZTS container"
    docker exec "$CONTAINER" php /app/tests/test_smoke.php

    info "Concurrent stress test (200 requests, 10 concurrent)"
    for i in $(seq 1 10); do
        for j in $(seq 1 20); do
            curl -sf http://localhost:8099/test_zts_stress.php > /dev/null &
        done
        wait
        echo "--- Batch $i/10 complete ---"
    done

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
    ok "All tests passed"
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
    zts)         cmd_zts ;;
    temporal)    cmd_temporal ;;
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
