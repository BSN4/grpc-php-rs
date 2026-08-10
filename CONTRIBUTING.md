# Contributing

Thanks for helping — outside contributions have fixed real production bugs in
this project, and reproductions are as valuable as patches.

## Build

- Rust stable (nightly on Windows), PHP 8.2+ dev headers
- `cargo build --release` → `target/release/libgrpc_php_rs.so`

## Tests

```sh
./test.sh all        # core suites (also what CI runs)
./test.sh zts        # FrankenPHP ZTS stress
./test.sh ledger     # ext-grpc parity ledger
./test.sh ecosystem  # google-cloud/etcd clients vs real emulators
./test.sh bench      # benchmark vs official ext-grpc
```

All suites must pass. CI runs `all` + `zts` on every PR.

## The ground rules

1. **Behavior claims need proof.** "ext-grpc does X" is only accepted with a
   runnable script executed under both extensions (`./test.sh parity` builds
   the official-ext image). We have repeatedly found the C source, the docs,
   and plausible reasoning to be wrong about ext-grpc's actual behavior.
2. **The parity ledger is the compatibility contract.**
   `tests/test_ext_grpc_parity.php` records every known divergence as a
   strict-xfail case. Fixing one: fix the code, run the ledger, flip that
   case's `fixed => true` (the run tells you when). Adding one: `expected`
   values must come from observed ext-grpc output, never from reading its
   source.
3. **No vacuous tests.** Assertions must be able to fail; benchmarks must
   verify their results; cleanup checks must re-fetch, not read caches.
4. **Do not touch the lint config** — `clippy.toml`, the `[lints.clippy]`
   section, and the crate-level `#![deny(...)]` block are maintainer-owned.
   The lints ban `unwrap`/`panic`/indexing/`std::sync` locks/`std::fs`/
   `std::path` — use `?`, `parking_lot`, `fs_err`, `camino`.
5. **Commits:** short imperative title, small body only when needed.

## Good first contributions

Open ledger divergences (run `./test.sh ledger` and look for "documented
divergence") each come with a proven expected behavior — they are
self-contained, pre-specified fixes.
