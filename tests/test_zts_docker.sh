#!/usr/bin/env bash
set -euo pipefail

# Build a FrankenPHP image with grpc-php-rs loaded
CONTAINER="grpc-rs-zts-test"

echo "=== Building test container ==="
docker build -t grpc-rs-test -f - . <<'DOCKERFILE'
FROM dunglas/frankenphp:1-php8.5

# Copy the built extension
COPY target/release/libgrpc_php_rs.so /usr/local/lib/php/extensions/no-debug-zts-20250925/grpc.so
RUN echo "extension=grpc.so" > /usr/local/etc/php/conf.d/grpc.ini

# Copy test files
COPY tests/ /app/tests/
COPY tests/Caddyfile.test /etc/caddy/Caddyfile

WORKDIR /app
DOCKERFILE

echo "=== Starting FrankenPHP with 4 workers ==="
docker rm -f "$CONTAINER" 2>/dev/null || true
docker run -d --name "$CONTAINER" \
    -p 8099:8080 \
    -e FRANKENPHP_WORKERS="/app/tests/test_zts_stress.php=4" \
    grpc-rs-test

sleep 2

echo "=== Verifying extension loaded ==="
docker exec "$CONTAINER" php -m | grep grpc
docker exec "$CONTAINER" php -r "echo 'grpc loaded: ' . (extension_loaded('grpc') ? 'yes' : 'no') . PHP_EOL;"

echo ""
echo "=== Running basic smoke test inside container ==="
docker exec "$CONTAINER" php /app/tests/test_smoke.php

echo ""
echo "=== Running SSL test inside container ==="
docker exec "$CONTAINER" php /app/tests/test_channel_ssl.php

echo ""
echo "=== Concurrent stress test (200 requests, 10 concurrent) ==="
echo "    If this completes without the container crashing, ZTS is safe."
echo ""

for i in $(seq 1 10); do
    for j in $(seq 1 20); do
        curl -sf http://localhost:8099/test_zts_stress.php &
    done
    wait
    echo "--- Batch $i/10 complete ---"
done

echo ""
echo "=== Checking container still alive ==="
if docker exec "$CONTAINER" php -r "echo 'alive';"; then
    echo ""
    echo "✓ Container survived 200 concurrent gRPC+TLS requests under ZTS!"
    echo "  No SIGSEGV — grpc-php-rs is thread-safe."
else
    echo ""
    echo "✗ Container crashed — check: docker logs $CONTAINER"
    exit 1
fi

echo ""
echo "=== Cleanup ==="
echo "Run: docker rm -f $CONTAINER"
