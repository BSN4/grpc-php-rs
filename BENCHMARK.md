# Benchmarks

Comparative benchmark of **grpc-php-rs** against the official C extension
(**ext-grpc**, pecl), running the identical scenario matrix under both
extensions against the same local gRPC test server.

Reproduce with:

```sh
./test.sh bench
```

## Results

Measured 2026-08-20 · grpc-php-rs `main` vs ext-grpc 1.83.0 (pecl) · PHP 8.5 ·
both sides in identical `php:8.5-cli` Docker containers with an in-container
server (loopback). Median of 50 revolutions per scenario (30 for cold, 10+ for
count=1000, 5 for async). **Ratio < 1.0 means grpc-php-rs is faster.**

| Scenario | grpc-php-rs | ext-grpc | Ratio |
|---|---:|---:|---:|
| cold: channel construct + first unary | 366.5 µs | 925.1 µs | **0.40** |
| unary, split start/wait, 0 B (the gax pattern) | 63.4 µs | 154.2 µs | **0.41** |
| unary, split start/wait, 1 KB | 73.2 µs | 154.2 µs | **0.47** |
| unary payload 0 B | 72.1 µs | 150.6 µs | **0.48** |
| unary payload 100 B | 71.2 µs | 150.4 µs | **0.47** |
| unary payload 1 KB | 71.8 µs | 151.8 µs | **0.47** |
| unary payload 10 KB | 77.4 µs | 156.7 µs | **0.49** |
| unary payload 100 KB | 182.1 µs | 230.5 µs | **0.79** |
| server stream, 10 messages | 97.2 µs | 207.1 µs | **0.47** |
| server stream, 100 messages | 330.2 µs | 709.6 µs | **0.47** |
| server stream, 1000 messages | 2540.4 µs | 5516.5 µs | **0.46** |
| server stream, 100 B payloads | 85.2 µs | 181.9 µs | **0.47** |
| server stream, 1 KB payloads | 83.2 µs | 181.9 µs | **0.46** |
| server stream, 10 KB payloads | 94.2 µs | 187.5 µs | **0.50** |
| 50 concurrent async unary (250 ms server delay) | 255.7 ms | 257.9 ms | **0.99** |

Run-to-run variance on this Docker host is real: across repeated full runs of
the same build the unary ratios have ranged roughly 0.45–0.70 and the cold
scenario swings more, so read the ratios as "roughly half the latency of
ext-grpc on small RPCs, parity on 100 KB payloads and the async scenario"
rather than as exact figures. Every scenario uses at least 30 revolutions (a
10-revolution median of the 1000-message stream once moved ±20% from host
noise alone), and the interleaved A/B runs described below are the
authoritative comparisons for any single change.

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

**Runtime model.** Each PHP thread owns a `current_thread` tokio runtime and
drives it itself from inside `block_on` while waiting for an RPC (see
`src/runtime.rs`). The previous design — a shared multi-thread runtime — made
every RPC hop PHP thread → worker → I/O driver → PHP thread; profiling showed
35–50% of per-RPC CPU in kernel thread wakeups. Switching models, measured by
interleaved A/B on the same build: unary 118–140 → 69–71 µs wall, 74–97 → 30–32
µs CPU; the Firestore-shaped TLS+auth-plugin call 143–152 → 89–90 µs; streaming
CPU −37%; under FrankenPHP (20 threads) throughput 1129 → 4010 req/s with p99
21 → 8 ms, because there is no longer a shared runtime or shared connection to
contend on. Trade-off: nothing progresses while PHP is outside a gRPC call
(server GOAWAY/PING handled at the next call, worst case one reconnect).
`GRPC_PHP_RS_RUNTIME=multi-thread` restores the shared runtime.

Per RPC we now spend ~31 µs CPU to ext-grpc's ~104 µs. Of ours: the kernel
socket path (inherent), h2/tonic framing, and ~5% HPACK Huffman-encoding of
the auth token (h2 never indexes `authorization`, by policy; C-core sends it
uncompressed, so our gax-shaped request is still ~20% smaller on the wire).
**Extension code proper** never exceeds ~3% for any single function.

- **Large receives:** ~16% kernel socket copy + ~25% userspace memcpy (two
  copies: h2 → tonic's decode buffer, then into the PHP string — tonic's
  decoder API makes the first unavoidable). A third copy was removed in
  v0.3.1.

Things measured and **refuted** on the way (documented so nobody re-proposes
them by intuition): HTTP/2 frame-size/window/adaptive-window tuning is within
noise over three interleaved rounds; mimalloc costs ~10% on small RPCs and
buys nothing reproducible; glibc malloc threshold tuning hurts large receives.

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
