# grpc-php-rs

A Rust-based gRPC extension for PHP, replacing the official C-based `grpc` extension.
Solves ZTS/TSRM crashes and OpenSSL/BoringSSL conflicts.

## Stack

- **ext-php-rs** — PHP extension bindings
- **tonic** — Pure Rust gRPC (HTTP/2 + protobuf)
- **rustls** — Pure Rust TLS (no OpenSSL)
- **hyper** — HTTP/2 transport
- **prost** — Protobuf encoding/decoding
- **tokio** — Async runtime
- **parking_lot** — Sync primitives (std::sync::Mutex/RwLock are banned)
- **camino** — UTF-8 paths (std::path is banned)
- **fs_err** — Better fs error messages (std::fs methods are banned)

## Lint Rules (DO NOT MODIFY)

These files are owned by the user — do not change without explicit permission:
- `clippy.toml` — Clippy configuration (disallowed types/methods)
- `Cargo.toml` `[lints.clippy]` section — Clippy lint levels
- `src/main.rs` `#![deny(...)]` block — Crate-level lint attributes

### Key Constraints from Lints

- **No `.unwrap()` / `.expect()`** — Use `?` operator or proper error handling
- **No `panic!()` / `todo!()` / `unimplemented!()`** — Handle all cases
- **No indexing (`[]`)** — Use `.get()` with proper error handling
- **No `println!` / `eprintln!` / `dbg!`** — Use proper logging
- **No `tokio::sync::Mutex/RwLock`** — Use channels/message passing
- **No `std::sync::Mutex/RwLock`** — Use `parking_lot` equivalents
- **No `std::path::Path/PathBuf`** — Use `camino::Utf8Path/Utf8PathBuf`
- **No `std::fs::*`** — Use `fs_err` equivalents
- **No `std::thread::sleep`** — Use `tokio::time::sleep`
- **No `std::net::TcpStream`** — Use `tokio::net::TcpStream`

## Build

```sh
cargo build          # dev build
cargo build --release # release build (.so at target/release/libgrpc_php_rs.so)
```

## Architecture

```
PHP userland → ext-php-rs entry points → Rust async bridge (tokio) → tonic gRPC → rustls TLS
```

- Tokio runtime threads are Rust-managed, NEVER touch PHP allocator
- Only ext-php-rs entry points (on PHP thread) interact with PHP memory
- CallCredentials plugin callbacks MUST run on PHP thread for TSRM context
- Extension receives raw protobuf bytes from PHP, forwards as-is (transport layer only)

## PHP API (drop-in replacement for `Grpc\` namespace)

Core classes: `ChannelCredentials`, `CallCredentials`, `Channel`, `UnaryCall`,
`ServerStreamingCall`, `ClientStreamingCall`, `BidiStreamingCall`, `Timeval`
