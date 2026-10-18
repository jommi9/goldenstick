# Upstream

This crate is djbooth's analyser, copied from
[gakonst/djbooth](https://github.com/gakonst/djbooth) at commit
`452c46f03c99e87df1793558dab1f69fc0a810a1`, path `rust/crates/djbooth-analyzer`.
It is MIT-licensed (`license = "MIT"` in `Cargo.toml`); its third-party notices
are in `THIRD_PARTY_LICENSES.md`. The project owner has the djbooth
author's agreement to use it in BoothReady.

It keeps upstream's rustfmt defaults through its own `rustfmt.toml`, so the
files stay comparable with djbooth.

## Local changes

- `Cargo.toml`: `rust-version = "1.88"`. Upstream uses let-chains, so the crate
  needs 1.88 although the rest of the workspace declares 1.85.
- Four fixed-size `chunks_exact` loops use `as_chunks` instead (`anlz.rs` twice,
  `flac.rs`, `rhythm.rs`), and `sample_rate_converter.rs` uses
  `saturating_sub(1)`. Both were clippy findings under BoothReady's
  `-D warnings`; behaviour is unchanged.
- One test in `stems.rs` iterates with `enumerate` instead of an index range,
  for the same reason.
- `waveform.rs`: the scalar detail-feature helpers (`FrameFeatures`,
  `extract_detail_features_scalar`, `detail_frame_samples`, `extract_detail_frame`)
  and the test-only `extract_detail_features` are gated on
  `all(target_arch = "aarch64", test)`, since only the aarch64 SIMD tests call
  them; the x86_64 variant of `extract_detail_features` is removed. Under
  `-D warnings` the x86_64 CI runners reported them as dead code.

## Not yet aligned with BoothReady's rules

The crate does not opt into the workspace lints, so `clippy::unwrap_used` is not
enforced in it. Its non-test code still has about 34 `unwrap` and `expect` calls, which
BoothReady's own crates forbid in shipped code.

## Updating

Copy `rust/crates/djbooth-analyzer` from a newer djbooth commit over this folder,
keep this file and `rustfmt.toml`, reapply the changes above if upstream has not
made them, and update the commit here.
