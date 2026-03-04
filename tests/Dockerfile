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
# FrankenPHP ships php-config with ZTS headers, so ext-php-rs picks up ZTS
# flags automatically via its build.rs → php-config --includes.
FROM dunglas/frankenphp:1-php8.5 AS builder-zts

RUN apt-get update && apt-get install -y --no-install-recommends \
        curl build-essential pkg-config protobuf-compiler libclang-dev \
    && rm -rf /var/lib/apt/lists/*

# Install Rust
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
ENV PATH="/root/.cargo/bin:${PATH}"

WORKDIR /build

COPY Cargo.toml Cargo.lock ./
COPY .cargo/ .cargo/
COPY clippy.toml ./
COPY src/ src/

RUN cargo build --release

# ---------------------------------------------------------------------------
# Stage 3: NTS (non-thread-safe) PHP test runner
FROM php:8.5-cli AS test-nts

# Auto-detect extension directory and install the .so
RUN EXT_DIR="$(php -i | grep '^extension_dir' | head -1 | sed 's/.*=> //' | awk '{print $1}')" \
    && mkdir -p "$EXT_DIR" \
    && echo "$EXT_DIR" > /tmp/ext_dir
COPY --from=builder /build/target/release/libgrpc_php_rs.so /tmp/grpc.so
RUN EXT_DIR="$(cat /tmp/ext_dir)" \
    && cp /tmp/grpc.so "$EXT_DIR/grpc.so" \
    && echo "extension=grpc.so" > /usr/local/etc/php/conf.d/grpc.ini \
    && rm /tmp/grpc.so /tmp/ext_dir

# Install Composer and Firestore client
RUN apt-get update && apt-get install -y --no-install-recommends git unzip \
    && rm -rf /var/lib/apt/lists/*
COPY --from=composer:2 /usr/bin/composer /usr/bin/composer
RUN mkdir -p /app \
    && composer require --working-dir=/app google/cloud-firestore --prefer-dist --no-interaction --quiet

COPY tests/ /app/tests/
WORKDIR /app

CMD ["php", "tests/test_smoke.php"]

# ---------------------------------------------------------------------------
# Stage 4: ZTS (thread-safe) PHP via FrankenPHP
FROM dunglas/frankenphp:1-php8.5 AS test-zts

# Auto-detect ZTS extension directory and install the .so
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
