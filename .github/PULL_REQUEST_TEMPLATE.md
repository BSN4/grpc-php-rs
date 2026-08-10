## What

<!-- One or two sentences. -->

## Checklist

- [ ] `./test.sh all` passes
- [ ] Behavior changes affecting ext-grpc compatibility are proven with output
      from both extensions (see CONTRIBUTING.md — reasoning from C source is
      not accepted as proof)
- [ ] Fixed a ledger divergence? The case's `fixed` flag is flipped
- [ ] New assertions can actually fail (no vacuous checks)
- [ ] No changes to `clippy.toml` / lint configuration
