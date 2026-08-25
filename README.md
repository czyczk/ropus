# ropus — Rust Opus decoder

A single-stream RFC 6716 Opus decoder in Rust (CELT + SILK + hybrid +
classical PLC/DTX/FEC), validated bit-exactly against libopus v1.6.1.

## Crates

| crate | role |
| --- | --- |
| `crates/opus-core` | C-isomorphic codec core (`celt/`, `silk/`, `opus/`), BSD-3-Clause port of [0x4D44/ropus](https://github.com/0x4D44/ropus) |
| `crates/opus-decoder` | public decoder API, per-module `thiserror` errors |
| `crates/opus-decoder-cli` | `ropusdec` CLI: Ogg Opus or `opus_demo` raw input, `{wav, raw} x {s16, s24, f32}` output |

See `PORTING.md` for the C-file to Rust-module map, `VALIDATION.md` for
correctness evidence, `PERFORMANCE.md` for benchmark/hotspot evidence, and
`HANDOFF.md` for cross-platform instructions.

## Build and test

```sh
cargo build --release --workspace
cargo test --workspace
```

Default features enable portable `wide`-based SIMD kernels. Use
`--no-default-features` for the bit-exact scalar fallback.

## Validate against the reference

```sh
scripts/build-reference.sh                  # libopus 1.6.1 + opusdec, in .refbuild/
scripts/reproduce-golden.sh --fixed16       # golden f32 (not committed)
scripts/verify-corpus.py \
  --decoder target/debug/ropusdec \
  --fixed16 .refbuild/opus-src-fixed16/opus_demo \
  --prod    .refbuild/opus-src/opus_demo
```

Result: 19/19 committed corpus cases bit-exact on f32, s16, and s24 against
the fixed-point 16-bit reference. The default float reference build is also
compared and reported; fixed-vs-float precision differences are documented in
`VALIDATION.md`.

## CLI

```sh
ropusdec --channels 1 --rate 48000 \
  --output-type wav --sample-format s24 corpus/speech-silk-012k-20ms.opus out.wav
```

For Ogg Opus input, `--channels` is inferred from the `OpusHead` packet.

## Scope

- No machine-learning error concealment: `dnn`/deep-PLC/DRED code is excluded
  from the default build (`opus-core` feature `ml` is opt-in for upstream
  comparison only).
- Encoder and multistream/surround are intentionally not part of this change.
