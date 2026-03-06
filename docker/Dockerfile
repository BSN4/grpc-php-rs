# Pre-built grpc-php-rs extension image.
#
# Usage in your Dockerfile:
#   COPY --from=ghcr.io/bsn4/grpc-php-rs:latest-php8.5 /usr/local/ /usr/local/
#
# Build args:
#   PHP_IMAGE  - base PHP image to compile against (e.g. php:8.5-cli, php:8.5-zts)

ARG PHP_IMAGE=php:8.5-cli

FROM ${PHP_IMAGE} AS builder

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

# Detect the extension directory for this PHP build and stage the files
RUN EXT_DIR="$(php -i | grep '^extension_dir' | head -1 | sed 's/.*=> //' | awk '{print $1}')" \
    && mkdir -p "/output${EXT_DIR}" \
    && cp target/release/libgrpc_php_rs.so "/output${EXT_DIR}/grpc.so" \
    && mkdir -p /output/usr/local/etc/php/conf.d \
    && echo "extension=grpc.so" > /output/usr/local/etc/php/conf.d/grpc.ini

FROM scratch
COPY --from=builder /output/ /
