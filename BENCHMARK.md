# Benchmarks

Comparative benchmark of **grpc-php-rs** against the official C extension
(**ext-grpc**, pecl), running the identical scenario matrix under both
extensions against the same local gRPC test server.

Reproduce with:

```sh
./test.sh bench
```

## Results

Measured 2026-08-11 · grpc-php-rs `main` vs ext-grpc 1.83.0 (pecl) · PHP 8.5 ·
both sides in identical `php:8.5-cli` Docker containers with an in-container
server (loopback). Median of 50 revolutions per scenario (30 for cold, 10+ for
count=1000, 5 for async). **Ratio < 1.0 means grpc-php-rs is faster.**

| Scenario | grpc-php-rs | ext-grpc | Ratio |
|---|---:|---:|---:|
| cold: channel construct + first unary | 666.9 µs | 806.0 µs | **0.83** |
| unary payload 0 B | 150.8 µs | 150.1 µs | 1.00 |
| unary payload 100 B | 136.5 µs | 144.7 µs | **0.94** |
| unary payload 1 KB | 129.1 µs | 147.4 µs | **0.88** |
| unary payload 10 KB | 133.8 µs | 153.2 µs | **0.87** |
| unary payload 100 KB | 257.4 µs | 237.7 µs | 1.08 |
| server stream, 10 messages | 139.4 µs | 195.7 µs | **0.71** |
| server stream, 100 messages | 350.6 µs | 641.9 µs | **0.55** |
| server stream, 1000 messages | 2415.5 µs | 4947.0 µs | **0.49** |
| server stream, 100 B payloads | 123.6 µs | 169.9 µs | **0.73** |
| server stream, 1 KB payloads | 127.0 µs | 171.0 µs | **0.74** |
| server stream, 10 KB payloads | 148.6 µs | 178.9 µs | **0.83** |
| 50 concurrent async unary (250 ms server delay) | 255.1 ms | 257.0 ms | **0.99** |

Scenario-to-scenario run variance is roughly ±10%; the 100 KB unary result
flips above and below 1.0 across runs and should be read as parity.

## Methodology

- **Same code, both sides.** One script (`tests/bench.php`) using only the raw
  `Grpc\` API runs unmodified under either extension. One comparator
  (`./test.sh bench`) builds both images from the same base (`php:8.5-cli`),
  the same test server binary, and the same command line, then diffs medians.
- **Results are verified, not just timed.** Every unary checks its status
  code, every stream checks its status *and* exact message count, and every
  async call checks status and payload presence. A broken transport exits
  non-zero instead of producing fast garbage numbers. The comparator refuses
  to print a table unless both sides report the identical scenario set.
- **Cold means cold.** Both implementations keep a persistent channel registry
  keyed by target + arguments; the cold scenario passes a unique channel
  argument per revolution so each measurement includes real TCP + HTTP/2
  connection establishment on both sides.
- **Concurrency is real.** The async scenario issues 50 split-batch unary
  calls (the `UnaryCall::start()`/`wait()` pattern used by google/gax
  promises) against a server method with a 250 ms delay: ~255 ms total means
  all 50 round trips genuinely overlapped.

## Scenarios

| Scenario | What it exercises |
|---|---|
| cold | channel construction, DNS/TCP/HTTP-2 setup, first RPC |
| unary payload ladder | request/response serialization and buffer handling, 0 B – 100 KB |
| stream count ladder | per-message delivery overhead between the transport and PHP |
| stream payload ladder | streaming buffer handling at 3 messages × size |
| async 50 | in-flight request concurrency (the gax promise pattern) |

Numbers are from the maintainer's machine (Docker Desktop, Apple Silicon) and
will differ on your hardware — the harness is in the repo precisely so you can
run it yourself.
