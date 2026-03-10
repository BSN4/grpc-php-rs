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
COPY tests/Caddyfile.test /etc/caddy/Caddyfile
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
