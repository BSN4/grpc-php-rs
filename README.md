# grpc-php-rs

[![CI](https://github.com/BSN4/grpc-php-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/BSN4/grpc-php-rs/actions/workflows/ci.yml)
[![PIE](https://img.shields.io/badge/PIE-bsn4%2Fgrpc-blue)](https://packagist.org/packages/bsn4/grpc)
[![PHP](https://img.shields.io/badge/PHP-8.2%2B-8892BF)](https://www.php.net)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)

A Rust-based gRPC extension for PHP — **drop-in replacement** for the official `ext-grpc`.

> [!IMPORTANT]
> **Please consider supporting this work.** If your company runs on `grpc-php-rs`, [sponsoring the project](https://github.com/sponsors/BSN4) directly funds its maintenance, compatibility testing, and long-term production support.

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

For Alpine:

```dockerfile
FROM php:8.5-alpine
COPY --from=ghcr.io/bsn4/grpc-php-rs:latest-php8.5-alpine /usr/local/ /usr/local/
```

Available tags: `latest-php{8.2,8.3,8.4,8.5}` for Debian, append `-alpine` for Alpine, append `-zts` for thread-safe (e.g. `latest-php8.5-alpine-zts`). Version-pinned tags like `v0.2.1-php8.5-alpine` are also available.

### Via PIE

```sh
pie install bsn4/grpc
```

Requires [PIE 1.4.0+](https://github.com/php/pie/releases/tag/1.4.0) for pre-packaged binary support.

### Manual download

Download the appropriate `.so` from the [latest release](https://github.com/BSN4/grpc-php-rs/releases/latest), then:

```sh
# Copy to your PHP extensions directory
cp grpc.so $(php -r "echo ini_get('extension_dir');")

# Enable it
echo "extension=grpc" > $(php -r "echo PHP_CONFIG_FILE_SCAN_DIR;")/grpc.ini
```

> **Alpine note:** When using the `linux-musl` binary outside of the Docker install image, you also need `apk add --no-cache libgcc`. The Docker install image bundles this for you.

## Supported Platforms

| PHP | OS | Arch | Thread Safety |
|-----|-------|--------|---------------|
| 8.2, 8.3, 8.4, 8.5 | Linux (glibc) | x86_64 | NTS, ZTS |
| 8.2, 8.3, 8.4, 8.5 | Linux (glibc) | ARM64 | NTS, ZTS |
| 8.2, 8.3, 8.4, 8.5 | Linux (musl/Alpine) | x86_64 | NTS, ZTS |
| 8.2, 8.3, 8.4, 8.5 | Linux (musl/Alpine) | ARM64 | NTS, ZTS |
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

## Compatibility & Performance

Compatibility with `ext-grpc` is tracked case by case in a machine-checked
parity ledger (`./test.sh ledger`), and the major consumers — google-cloud-php
(Pub/Sub, Spanner, Firestore, Bigtable, Datastore against official emulators),
google-ads-php, Temporal, OpenTelemetry, etcd — run as test suites
(`./test.sh ecosystem`). Benchmarks against `ext-grpc` are in
[BENCHMARK.md](BENCHMARK.md) — reproduce them with `./test.sh bench`.

# Charts from production 

<img width="1600" height="608" alt="image" src="https://github.com/user-attachments/assets/1985577f-9288-4711-9386-089d5b75e6ad" />
<img width="1600" height="608" alt="image" src="https://github.com/user-attachments/assets/d8c18c81-729f-4e3b-b162-ffa56471a5e3" />
<img width="1600" height="672" alt="image" src="https://github.com/user-attachments/assets/81e2264c-c58d-4f1a-bcc5-fb7d0da3b710" />
<img width="1600" height="672" alt="image" src="https://github.com/user-attachments/assets/a49a90d4-7432-485e-971d-1e2993702414" />


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
