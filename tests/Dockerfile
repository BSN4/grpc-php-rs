# Stage 1: Build the Rust extension for NTS Linux
FROM php:8.5-cli AS builder

RUN apt-get update && apt-get install -y --no-install-recommends \
        curl build-essential pkg-config protobuf-compiler libclang-dev \
    && rm -rf /var/lib/apt/lists/*

# Install Rust
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
ENV PATH="/root/.cargo/bin:${PATH}"

WORKDIR /build

# Copy project files needed for build
COPY Cargo.toml Cargo.lock ./
COPY .cargo/ .cargo/
COPY clippy.toml ./
COPY src/ src/

# Build and run Rust tests
RUN cargo build --release \
    && cargo test --release

# ---------------------------------------------------------------------------
# Stage 2: Build the Rust extension for ZTS Linux
FROM dunglas/frankenphp:1-php8.5 AS builder-zts

RUN apt-get update && apt-get install -y --no-install-recommends \
        curl build-essential pkg-config protobuf-compiler libclang-dev \
    && rm -rf /var/lib/apt/lists/*

RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
ENV PATH="/root/.cargo/bin:${PATH}"

WORKDIR /build

COPY Cargo.toml Cargo.lock ./
COPY .cargo/ .cargo/
COPY clippy.toml ./
COPY src/ src/

RUN cargo build --release

# ---------------------------------------------------------------------------
# Stage 3: Base NTS test image (extension installed, no Composer deps)
FROM php:8.5-cli AS test-base

RUN apt-get update && apt-get install -y --no-install-recommends git unzip \
    && rm -rf /var/lib/apt/lists/*
COPY --from=composer:2 /usr/bin/composer /usr/bin/composer

# Install extension
RUN EXT_DIR="$(php -i | grep '^extension_dir' | head -1 | sed 's/.*=> //' | awk '{print $1}')" \
    && mkdir -p "$EXT_DIR" \
    && echo "$EXT_DIR" > /tmp/ext_dir
COPY --from=builder /build/target/release/libgrpc_php_rs.so /tmp/grpc.so
RUN EXT_DIR="$(cat /tmp/ext_dir)" \
    && cp /tmp/grpc.so "$EXT_DIR/grpc.so" \
    && echo "extension=grpc.so" > /usr/local/etc/php/conf.d/grpc.ini \
    && rm /tmp/grpc.so /tmp/ext_dir

WORKDIR /app
COPY tests/ /app/tests/

# ---------------------------------------------------------------------------
# Stage 4: Smoke test (API surface, no network)
FROM test-base AS test-smoke
CMD ["php", "tests/test_smoke.php"]

# ---------------------------------------------------------------------------
# Stage 5: grpc/grpc Composer compatibility test (Issue #4)
FROM test-base AS test-compat

# Install grpc/grpc AFTER extension is loaded (tests class inheritance compat)
RUN composer require --working-dir=/app grpc/grpc:^1.74 --prefer-dist --no-interaction --quiet
CMD ["php", "tests/test_grpc_compat.php"]

# ---------------------------------------------------------------------------
# Stage 6: Firestore client compatibility test
FROM test-base AS test-firestore

# Install Firestore SDK (tests real-world Google Cloud SDK integration)
RUN composer require --working-dir=/app google/cloud-firestore google/gax --prefer-dist --no-interaction --quiet
CMD ["php", "tests/test_firestore_fake.php"]

# ---------------------------------------------------------------------------
# Stage 6b: google/grpc-gcp compatibility test
FROM test-base AS test-grpc-gcp

RUN composer require --working-dir=/app google/grpc-gcp:^0.4 google/gax --prefer-dist --no-interaction --quiet
CMD ["php", "tests/test_grpc_gcp_compat.php"]

# ---------------------------------------------------------------------------
# Stage 7: ZTS (thread-safe) PHP via FrankenPHP
FROM dunglas/frankenphp:1-php8.5 AS test-zts

RUN EXT_DIR="$(php -i | grep '^extension_dir' | head -1 | sed 's/.*=> //' | awk '{print $1}')" \
    && mkdir -p "$EXT_DIR" \
    && echo "$EXT_DIR" > /tmp/ext_dir
COPY --from=builder-zts /build/target/release/libgrpc_php_rs.so /tmp/grpc.so
RUN EXT_DIR="$(cat /tmp/ext_dir)" \
    && cp /tmp/grpc.so "$EXT_DIR/grpc.so" \
    && echo "extension=grpc.so" > /usr/local/etc/php/conf.d/grpc.ini \
    && rm /tmp/grpc.so /tmp/ext_dir

COPY tests/ /app/tests/
# FrankenPHP reads /etc/frankenphp/Caddyfile in current images (older ones
# used /etc/caddy) — cover both, or the default config serves :80/:443 and
# the stress test hits a dead :8080.
COPY tests/Caddyfile.test /etc/caddy/Caddyfile
COPY tests/Caddyfile.test /etc/frankenphp/Caddyfile
WORKDIR /app

# ---------------------------------------------------------------------------
# Stage 8: Build the gRPC test server (standalone Rust crate)
FROM rust:1 AS test-server-builder

RUN apt-get update && apt-get install -y --no-install-recommends protobuf-compiler \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build
COPY tests/server/ .
RUN cargo build --release

# ---------------------------------------------------------------------------
# Stage 9: Memory leak test runner
FROM test-base AS test-leak

COPY --from=test-server-builder /build/target/release/grpc-test-server /usr/local/bin/grpc-test-server
COPY tests/leak_test.php /app/tests/leak_test.php

CMD ["sh", "-c", "grpc-test-server & sleep 1 && php tests/leak_test.php"]

# ---------------------------------------------------------------------------
# Stage 9b: Server streaming test runner
FROM test-base AS test-streaming

COPY --from=test-server-builder /build/target/release/grpc-test-server /usr/local/bin/grpc-test-server
COPY tests/test_streaming.php /app/tests/test_streaming.php

COPY tests/test_empty_response.php /app/tests/test_empty_response.php
COPY tests/test_plugin_exception.php /app/tests/test_plugin_exception.php

CMD ["sh", "-c", "grpc-test-server & sleep 1 && php tests/test_streaming.php && php tests/test_empty_response.php && php tests/test_plugin_exception.php && php tests/test_async_unary.php"]

# ---------------------------------------------------------------------------
# Stage 9bb: Split-batch unary test runner (start() then wait())
FROM test-base AS test-unary

COPY --from=test-server-builder /build/target/release/grpc-test-server /usr/local/bin/grpc-test-server
COPY tests/test_unary_split.php /app/tests/test_unary_split.php

CMD ["sh", "-c", "grpc-test-server & sleep 1 && php tests/test_unary_split.php"]

# ---------------------------------------------------------------------------
# Stage 9bc: gax-style promise concurrency test (issue #18) — drives the real
# grpc/grpc library + Guzzle promises, the layer google/shopping-merchant-products
# and every google-cloud-php client use for async batches.
FROM test-base AS test-promises

RUN composer require --working-dir=/app grpc/grpc:^1.74 guzzlehttp/promises:^2 --prefer-dist --no-interaction --quiet
COPY --from=test-server-builder /build/target/release/grpc-test-server /usr/local/bin/grpc-test-server
COPY tests/test_promise_concurrency.php /app/tests/test_promise_concurrency.php

CMD ["sh", "-c", "grpc-test-server & sleep 1 && php tests/test_promise_concurrency.php"]

# ---------------------------------------------------------------------------
# Stage 9bd: ext-grpc parity ledger — every known divergence from the official
# C extension as a strict-xfail case; fails on regressions in either direction.
FROM test-base AS test-parity-ledger

COPY --from=test-server-builder /build/target/release/grpc-test-server /usr/local/bin/grpc-test-server
COPY tests/test_ext_grpc_parity.php /app/tests/test_ext_grpc_parity.php

CMD ["sh", "-c", "grpc-test-server & sleep 1 && php tests/test_ext_grpc_parity.php"]

# ---------------------------------------------------------------------------
# Stage 9be: Deadline-covers-connect test — fresh channel to a blackholed
# endpoint (TEST-NET-1) must fail at the call deadline, not after the kernel's
# SYN-retry budget. No server needed; needs outbound routing only.
FROM test-base AS test-deadline-connect
COPY tests/test_deadline_connect.php /app/tests/test_deadline_connect.php
CMD ["php", "tests/test_deadline_connect.php"]

# ---------------------------------------------------------------------------
# Stage 9bf: Connection lifecycle test — restarts/kills the in-container server
# (needs procps for pidof) to prove reconnect/outage behavior under the
# per-thread runtime model.
FROM test-base AS test-lifecycle
RUN apt-get update && apt-get install -y --no-install-recommends procps \
    && rm -rf /var/lib/apt/lists/*
COPY --from=test-server-builder /build/target/release/grpc-test-server /usr/local/bin/grpc-test-server
COPY tests/test_connection_lifecycle.php /app/tests/test_connection_lifecycle.php
CMD ["sh", "-c", "grpc-test-server >/dev/null 2>&1 & sleep 1 && php tests/test_connection_lifecycle.php"]

# ---------------------------------------------------------------------------
# Stage 9c: Binary trailing metadata test runner
FROM test-base AS test-binary-trailer

COPY --from=test-server-builder /build/target/release/grpc-test-server /usr/local/bin/grpc-test-server
COPY tests/test_binary_trailer.php /app/tests/test_binary_trailer.php

CMD ["sh", "-c", "grpc-test-server & sleep 1 && php tests/test_binary_trailer.php"]

# ---------------------------------------------------------------------------
# Stage 9d: Parity probe under grpc-php-rs
FROM test-base AS test-parity-ours

COPY --from=test-server-builder /build/target/release/grpc-test-server /usr/local/bin/grpc-test-server
COPY tests/parity_probe.php /app/tests/parity_probe.php

CMD ["sh", "-c", "grpc-test-server 2>/dev/null & sleep 1 && php tests/parity_probe.php"]

# ---------------------------------------------------------------------------
# Stage 9e: Parity probe under official ext-grpc (pecl)
FROM php:8.5-cli AS test-parity-extgrpc

RUN apt-get update && apt-get install -y --no-install-recommends $PHPIZE_DEPS zlib1g-dev \
    && rm -rf /var/lib/apt/lists/* \
    && pecl install grpc \
    && docker-php-ext-enable grpc

COPY --from=test-server-builder /build/target/release/grpc-test-server /usr/local/bin/grpc-test-server
WORKDIR /app
COPY tests/parity_probe.php /app/tests/parity_probe.php
COPY tests/bench.php /app/tests/bench.php

CMD ["sh", "-c", "grpc-test-server 2>/dev/null & sleep 1 && php tests/parity_probe.php"]

# ---------------------------------------------------------------------------
# Stage 9e2: Benchmark runner (grpc-php-rs side; the ext-grpc side reuses
# test-parity-extgrpc, which also carries tests/bench.php)
FROM test-base AS test-bench

COPY --from=test-server-builder /build/target/release/grpc-test-server /usr/local/bin/grpc-test-server
CMD ["sh", "-c", "grpc-test-server 2>/dev/null & sleep 1 && php tests/bench.php"]

# ---------------------------------------------------------------------------
# Stage 9f: Google Cloud ecosystem tests — real google/cloud-* clients driven
# through gax against official emulators, plus construction-level checks for
# credential-gated libraries (google-ads, shopping-merchant).
FROM test-base AS test-integration-ecosystem

RUN composer require --working-dir=/app \
        google/cloud-pubsub google/cloud-spanner google/cloud-firestore \
        google/cloud-bigtable google/cloud-datastore \
        --prefer-dist --no-interaction --quiet
# aternos/etcd is isolated: it pins google/protobuf ^3 (conflicts with the
# google/cloud tree and carries a parsing-DoS advisory). Test-only container;
# the advisory block is relaxed here deliberately and transparently.
RUN mkdir /etcd && cd /etcd \
    && composer init --name test/etcd --no-interaction --quiet \
    && composer config policy.advisories.block false \
    && composer require aternos/etcd --prefer-dist --no-interaction --quiet --no-audit
RUN mkdir /l1 && cd /l1 && composer require \
        googleads/google-ads-php google/shopping-merchant-products \
        google/cloud-secret-manager google/cloud-tasks google/cloud-kms \
        google/cloud-translate google/cloud-speech google/cloud-logging \
        google/analytics-data google/cloud-bigquery-storage \
        --prefer-dist --no-interaction --quiet
COPY tests/integration/ /integration/

CMD ["sh", "-c", "php /integration/test_pubsub.php && php /integration/test_spanner.php && php /integration/test_firestore_emulator.php && php /integration/test_bigtable.php && php /integration/test_datastore.php && php /integration/test_etcd.php && php /integration/test_google_libs.php"]

# ---------------------------------------------------------------------------
# Stage 10: Integration test base — Temporal SDK
FROM test-base AS test-integration-base

RUN docker-php-ext-install sockets \
    && pecl install protobuf \
    && docker-php-ext-enable protobuf

COPY tests/integration/composer.json /integration/composer.json
RUN composer install --working-dir=/integration --prefer-dist --no-interaction --quiet
COPY tests/integration/ /integration/

# ---------------------------------------------------------------------------
# Stage 11: Integration test — Temporal
FROM test-integration-base AS test-integration-temporal
CMD ["php", "/integration/test_temporal.php"]

# ---------------------------------------------------------------------------
# Stage 12: Integration test — OpenTelemetry
FROM test-base AS test-integration-otel
COPY tests/integration/test_otel.php /integration/test_otel.php
CMD ["php", "/integration/test_otel.php"]
