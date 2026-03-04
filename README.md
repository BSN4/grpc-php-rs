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

### Via PIE (recommended)

```sh
pie install bsn4/grpc
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

| PHP | OS | Arch | Thread Safety | libc |
|-----|-------|--------|---------------|------|
| 8.2, 8.3, 8.4, 8.5 | Linux | x86_64 | NTS, ZTS | glibc |
| 8.2, 8.3, 8.4, 8.5 | Linux | ARM64 | NTS, ZTS | musl |
| 8.2, 8.3, 8.4, 8.5 | macOS | ARM64 | NTS | bsdlibc |

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
- Rust toolchain (stable)
- PHP 8.2+ development headers (`php-dev` / `php-devel`)

```sh
cargo build --release
# Output: target/release/libgrpc_php_rs.so (Linux) or libgrpc_php_rs.dylib (macOS)
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
