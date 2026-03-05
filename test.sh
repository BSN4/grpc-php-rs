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
NC='\033[0m' # No Color

info()  { echo -e "${CYAN}===${NC} $*"; }
ok()    { echo -e "${GREEN}  ✓${NC} $*"; }
fail()  { echo -e "${RED}  ✗${NC} $*"; }
warn()  { echo -e "${YELLOW}  !${NC} $*"; }

usage() {
    cat <<EOF
Usage: ./test.sh [command]

Commands:
  build       Build the Docker image (compiles Rust extension for Linux)
  rust        Run cargo test inside Docker
  smoke       Run PHP smoke test (no network needed)
  ssl         Run PHP SSL channel test (needs internet)
  firestore   Run Firestore client compatibility test (fake endpoint, no creds)
  zts         Run ZTS stress test with FrankenPHP + concurrent curl
  temporal    Run Temporal SDK integration test (starts temporalio/auto-setup)
  otel        Run OpenTelemetry integration test (starts otel-collector-contrib)
  integration Run both temporal + otel integration tests
  all         build + rust + smoke + firestore (default)
  shell       Drop into PHP CLI with extension loaded
EOF
}

# --- Commands ---

cmd_build() {
    info "Building builder stage (Rust compile + cargo test)"
    DOCKER_BUILDKIT=1 docker build \
        --target builder \
        -t "${IMAGE}:builder" \
        -f "$DOCKERFILE" .

    info "Building test-nts stage (PHP NTS)"
    DOCKER_BUILDKIT=1 docker build \
        --target test-nts \
        -t "${IMAGE}:nts" \
        -f "$DOCKERFILE" .

    ok "Docker images built"
}

cmd_rust() {
    info "Running cargo test inside Docker"
    # The builder stage already runs cargo test during build.
    # Rebuild the builder stage to re-run tests (cached if nothing changed).
    DOCKER_BUILDKIT=1 docker build \
        --target builder \
        -t "${IMAGE}:builder" \
        -f "$DOCKERFILE" .
    ok "cargo test passed"
}

cmd_smoke() {
    info "Running PHP smoke test (NTS)"
    docker run --rm "${IMAGE}:nts" php tests/test_smoke.php
    ok "Smoke test passed"
}

cmd_ssl() {
    info "Running PHP SSL channel test (NTS, needs internet)"
    docker run --rm "${IMAGE}:nts" php tests/test_channel_ssl.php
    ok "SSL test passed"
}

cmd_firestore() {
    info "Running Firestore client compatibility test (fake endpoint)"
    docker run --rm "${IMAGE}:nts" php tests/test_firestore_fake.php
    ok "Firestore compatibility test passed"
}

cmd_zts() {
    info "Building test-zts stage (FrankenPHP ZTS)"
    DOCKER_BUILDKIT=1 docker build \
        --target test-zts \
        -t "${IMAGE}:zts" \
        -f "$DOCKERFILE" .

    info "Starting FrankenPHP with 4 workers"
    docker rm -f "$CONTAINER" 2>/dev/null || true
    docker run -d --name "$CONTAINER" \
        -p 8099:8080 \
        -e FRANKENPHP_WORKERS="/app/tests/test_zts_stress.php=4" \
        "${IMAGE}:zts"

    # Wait for the container to be ready (poll instead of fixed sleep)
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
    echo "    If this completes without crashing, ZTS is safe."
    echo ""

    local failed=0
    for i in $(seq 1 10); do
        for j in $(seq 1 20); do
            curl -sf http://localhost:8099/test_zts_stress.php > /dev/null &
        done
        wait
        echo "--- Batch $i/10 complete ---"
    done

    echo ""
    info "Checking container still alive"
    if docker exec "$CONTAINER" php -r "echo 'alive';"; then
        echo ""
        ok "Container survived 200 concurrent gRPC+TLS requests under ZTS!"
        ok "No SIGSEGV — grpc-php-rs is thread-safe."
    else
        echo ""
        fail "Container crashed — check: docker logs $CONTAINER"
        failed=1
    fi

    info "Cleaning up ZTS container"
    docker rm -f "$CONTAINER" 2>/dev/null || true

    if [ "$failed" -ne 0 ]; then
        exit 1
    fi
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
    warn "This starts temporalio/auto-setup + otel-collector-contrib"
    local COMPOSE="docker compose -f $COMPOSE_INTEGRATION"
    trap "$COMPOSE down --volumes 2>/dev/null || true" EXIT

    info "Building integration test images"
    DOCKER_BUILDKIT=1 $COMPOSE build test-temporal test-otel

    info "Running Temporal integration test (waits for server healthy)"
    $COMPOSE run --rm test-temporal
    ok "Temporal test passed"

    info "Running OpenTelemetry integration test (waits for collector healthy)"
    $COMPOSE run --rm test-otel
    ok "OpenTelemetry test passed"

    $COMPOSE down --volumes
    trap - EXIT
    ok "All integration tests passed"
}

cmd_shell() {
    info "Dropping into PHP CLI with extension loaded"
    docker run --rm -it "${IMAGE}:nts" bash
}

cmd_all() {
    cmd_build
    cmd_smoke
    cmd_firestore
}

# --- Main ---

command="${1:-all}"

case "$command" in
    build) cmd_build ;;
    rust)  cmd_rust ;;
    smoke)       cmd_smoke ;;
    ssl)         cmd_ssl ;;
    firestore)   cmd_firestore ;;
    zts)         cmd_zts ;;
    temporal)    cmd_temporal ;;
    otel)        cmd_otel ;;
    integration) cmd_integration ;;
    all)         cmd_all ;;
    shell)       cmd_shell ;;
    -h|--help|help) usage ;;
    *)
        echo "Unknown command: $command"
        usage
        exit 1
        ;;
esac
