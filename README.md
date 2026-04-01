# grpc-php-rs

[![CI](https://github.com/BSN4/grpc-php-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/BSN4/grpc-php-rs/actions/workflows/ci.yml)
[![PIE](https://img.shields.io/badge/PIE-bsn4%2Fgrpc-blue)](https://packagist.org/packages/bsn4/grpc)
[![PHP](https://img.shields.io/badge/PHP-8.2%2B-8892BF)](https://www.php.net)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)

A Rust-based gRPC extension for PHP — **drop-in replacement** for the official `ext-grpc`.

## Why?

The official C-based `grpc` extension has long-standing issues:

- **ZTS/TSRM crashes** — segfaults under FrankenPHP, Swoole, and other threaded SAPIs
- **OpenSSL/BoringSSL conflicts** — the bundled BoringSSL collides with PHP's OpenSSL, breaking `ext-curl` and other extensions

grpc-php-rs solves both by using a pure Rust stack: [tonic](https://github.com/hyperium/tonic) for gRPC, [rustls](https://github.com/rustls/rustls) for TLS (no OpenSSL), and [ext-php-rs](https://github.com/davidcole1340/ext-php-rs) for PHP bindings.

## Install

### Docker (recommended)

One line in your Dockerfile — no build tools needed:

```dockerfile
FROM php:8.5-cli

COPY --from=ghcr.io/bsn4/grpc-php-rs:latest-php8.5 /usr/local/ /usr/local/
```

For ZTS (FrankenPHP, Swoole, etc.):

```dockerfile
COPY --from=ghcr.io/bsn4/grpc-php-rs:latest-php8.5-zts /usr/local/ /usr/local/
```

Available tags: `latest-php8.2`, `latest-php8.3`, `latest-php8.4`, `latest-php8.5` (add `-zts` for thread-safe). Version-pinned tags like `v0.1.2-php8.5` are also available.

### Via PIE

```sh
pie install bsn4/grpc
```

> **Note:** Requires PIE 1.4.0+ (pre-packaged binary support). PIE 1.3.x will fail.

In Docker (when PIE 1.4.0 stable isn't available yet):

```dockerfile
RUN apt-get update && apt-get install -y --no-install-recommends curl unzip git \
    && git clone --branch 1.4.x --depth 1 https://github.com/php/pie.git /tmp/pie \
    && curl -sLo /usr/local/bin/composer https://getcomposer.org/download/latest-stable/composer.phar \
    && chmod +x /usr/local/bin/composer \
    && cd /tmp/pie && composer install --no-dev --quiet \
    && /tmp/pie/bin/pie install bsn4/grpc \
    && rm -rf /tmp/pie /usr/local/bin/composer \
    && apt-get purge -y git unzip \
    && apt-get autoremove -y \
    && rm -rf /var/lib/apt/lists/*
```

### Manual download

Download the appropriate `.so` from the [latest release](https://github.com/BSN4/grpc-php-rs/releases/latest), then:

```sh
# Copy to your PHP extensions directory
cp grpc.so $(php -r "echo ini_get('extension_dir');")

# Enable it
echo "extension=grpc" > $(php -r "echo PHP_CONFIG_FILE_SCAN_DIR;")/grpc.ini
```

## Supported Platforms

| PHP | OS | Arch | Thread Safety |
|-----|-------|--------|---------------|
| 8.2, 8.3, 8.4, 8.5 | Linux | x86_64 | NTS, ZTS |
| 8.2, 8.3, 8.4, 8.5 | Linux | ARM64 | NTS, ZTS |
| 8.2, 8.3, 8.4, 8.5 | macOS | ARM64 | NTS |
| 8.2, 8.3, 8.4, 8.5 | Windows | x86_64 | NTS |

## Usage

grpc-php-rs is a drop-in replacement. Add to your `php.ini`:

```ini
extension=grpc
```

Then use the `Grpc\` namespace as normal:

```php
$channel = new \Grpc\Channel('localhost:50051', [
    'credentials' => \Grpc\ChannelCredentials::createInsecure(),
]);
```

All existing gRPC PHP code works unchanged — `Grpc\Channel`, `Grpc\ChannelCredentials`, `Grpc\CallCredentials`, `Grpc\Timeval`, and all call types (`UnaryCall`, `ServerStreamingCall`, `ClientStreamingCall`, `BidiStreamingCall`).

## Building from Source

Requirements:
- Rust toolchain (stable; nightly required on Windows)
- PHP 8.2+ development headers (`php-dev` / `php-devel`)

```sh
cargo build --release
# Output: target/release/libgrpc_php_rs.so (Linux) or libgrpc_php_rs.dylib (macOS) or grpc_php_rs.dll (Windows)
```

## Running Tests

```sh
./test.sh all       # Build Docker images + run smoke & compatibility tests
./test.sh zts       # ZTS stress test with FrankenPHP + concurrent requests
./test.sh smoke     # PHP smoke test only
./test.sh shell     # Drop into PHP CLI with extension loaded
```

See `./test.sh --help` for all options.

## License

MIT
