# Validation status

Normative reference: **libopus v1.6.1**, commit `22244de5a79bd1d6d623c32e72bf1954b56235be`,
built out-of-tree with `--disable-dred --disable-deep-plc --disable-osce`.

Two reference configurations are compared:

- `fixed16`: `--enable-fixed-point --disable-fixed-res24 --disable-asm
  --disable-intrinsics --disable-rtcd` — the **primary bit-exact oracle** for
  the fixed-point Rust core.
- `prod`: default float build with SIMD/intrinsics enabled — the build a user
  gets from `./configure && make`; reported, not gating.

Corpus: 19 anonymized `.opus` files in `corpus/` (see `corpus/manifest.json`):
music CELT (mono/stereo, 5/20/40/60/120 ms, 48–160 kb/s), speech SILK
(10/20/40/60 ms, 9–16 kb/s, DTX and in-band FEC variants), hybrid speech
(32 kb/s), and CELT speech (64 kb/s). Sources stay local in `.refbuild/audio/`;
only bitstreams are committed.

## Current results (f32 raw, `scripts/verify-corpus.py`)

- **19 of 19 cases are bit-exact against `fixed16`** (every decoded f32 sample
  bit-identical, including SILK, hybrid, DTX transitions, and all CELT cases).
- **s16 and s24 outputs are also bit-exact against `opus_demo -16` / `-24` for
  all 19 cases** (the s16 path follows the reference decode24→s16
  conversion, including its -32768→-32767 saturation behavior).
- **SILK cases are also bit-exact against `prod`** (0 bit differences).
- **CELT/hybrid cases differ from `prod`** because the fixed-point Rust core
  decodes through the 16-bit fixed-point path while the default libopus build
  uses the 24-bit/float path. Per-case bit-diff counts and max absolute
  deltas are stored in `.refbuild/validation-report.json` after each run. This
  is the expected fixed-vs-float precision floor, not a CELT algorithm bug.

## DTX comfort-noise tail: resolved

The adopted ropus base had a SILK PLC/DTX drift that produced 565 differing
f32 samples in `speech-silk-012k-dtx-20ms`. Root causes found and fixed:

1. `silk_plc_conceal` combined `LTP_pred` and random excitation with a single
   `>> 14` instead of C's `silk_SMLAWB(...) >> 16` followed by `<< 2`, keeping
   low remainder bits that C discards.
2. `silk_plc_glue_frames` used a simplified energy normalization instead of
   C's CLZ-based Q24 normalization, and skipped the fade-in loop at 16 kHz as
   if `ENABLE_DEEP_PLC` were defined (this repository builds without it).

After the fixes all **19/19 corpus cases are bit-exact against `fixed16`** on
f32, and s16/s24 are bit-exact against `opus_demo -16`/`-24` for every case.

## Reproduce

```sh
scripts/build-reference.sh                  # builds fixed16/prod opus_demo + opusdec
scripts/reproduce-golden.sh --fixed16       # golden f32 -> .refbuild/golden (not committed)
cargo build --workspace
scripts/verify-corpus.py \
  --decoder target/debug/ropusdec \
  --fixed16 .refbuild/opus-src-fixed16/opus_demo \
  --prod    .refbuild/opus-src/opus_demo
```

The `opusdec` (opus-tools v0.2) binary is built against the pinned libopus and
is used for Ogg-file reproduction once Ogg container output lands in the CLI.
