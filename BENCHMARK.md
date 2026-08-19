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
| cold: channel construct + first unary | 368.4 µs | 902.7 µs | **0.41** |
| unary, split start/wait, 0 B (the gax pattern) | 127.3 µs | 177.0 µs | **0.72** |
| unary, split start/wait, 1 KB | 132.2 µs | 148.8 µs | **0.89** |
| unary payload 0 B | 119.8 µs | 137.1 µs | **0.87** |
| unary payload 100 B | 122.6 µs | 165.6 µs | **0.74** |
| unary payload 1 KB | 120.1 µs | 155.6 µs | **0.77** |
| unary payload 10 KB | 127.2 µs | 160.5 µs | **0.79** |
| unary payload 100 KB | 258.2 µs | 245.8 µs | 1.05 |
| server stream, 10 messages | 142.9 µs | 208.0 µs | **0.69** |
| server stream, 100 messages | 362.5 µs | 662.3 µs | **0.55** |
| server stream, 1000 messages | 2586.8 µs | 5037.3 µs | **0.51** |
| server stream, 100 B payloads | 127.9 µs | 167.8 µs | **0.76** |
| server stream, 1 KB payloads | 128.8 µs | 165.7 µs | **0.78** |
| server stream, 10 KB payloads | 152.1 µs | 186.2 µs | **0.82** |
| 50 concurrent async unary (250 ms server delay) | 257.5 ms | 258.1 ms | **1.00** |

Scenario-to-scenario run variance is roughly ±10% (the cold scenario swings
more); the 100 KB unary result flips above and below 1.0 across runs and
should be read as parity.

## Large payloads and non-loopback networks

Measured separately (send-only and receive-only transfers of 1–4 MB, CPU time
and syscall counts, see the `tests/` probes). On loopback, grpc-php-rs
receives 1–4 MB messages 15–25% faster than ext-grpc with lower CPU. On a
higher-latency virtual network path (container → host) large transfers can
instead run 20–30% slower while still using less CPU: the h2 crate issues one
`writev` per 16 KB DATA frame on send and reads in ~8 KB chunks on receive,
where C-core batches ~300 KB per write and ~85 KB per read. That is I/O
granularity inside h2 (upstream), not extension code; it is invisible on
loopback (sidecars) and is dwarfed by RTT on real WAN links. The C-core
HTTP/2 tuning channel args (`grpc.http2.max_frame_size`,
`grpc.http2.lookahead_bytes`, `grpc.http2.bdp_probe`) are honored for
experimentation; in three interleaved rounds none moved the medians outside
noise, so defaults are unchanged.

## Where the CPU goes (Linux `perf`, in-container)

Profiled on the deployment target (Linux, unstripped `grpc.so`), not inferred.
Per unary RPC we spend ~64 µs CPU to ext-grpc's ~104 µs; per 1000-message
stream ~2.3 ms to ~8 ms. Of our CPU:

- **~35–50% kernel thread wakeups** (`try_to_wake_up`, `eventfd_write`,
  `__wake_up_sync_key`): each RPC hops PHP thread → tokio worker → I/O driver
  → PHP thread, three wakes where C-core's PHP-thread-driven completion queue
  needs one. This is the threading model, not a bug; removing it would require
  driving the runtime from the PHP thread, which stalls HTTP/2 connection
  liveness whenever PHP is busy. Not worth it while we already use less CPU
  than C-core on every path.
- **Large receives:** ~16% kernel socket copy + ~25% userspace memcpy (two
  copies: h2 → tonic's decode buffer, then into the PHP string — tonic's
  decoder API makes the first unavoidable). A third copy was removed in
  v0.3.1.
- **Extension code proper** never exceeds ~3% for any single function on any
  path (driver loop, batch parsing, metadata conversion).

Things measured and **refuted** on the way (documented so nobody re-proposes
them by intuition): the split start/wait path is *faster* than single-batch
(1 vs 2 context switches per RPC), tokio worker count (1/2/default) changes
nothing, HTTP/2 frame-size/window/adaptive-window tuning is within noise over
three interleaved rounds.

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
