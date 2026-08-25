# Listening checkpoint (step 4)

This is the stop-for-listening checkpoint requested in the plan. Build and
validation evidence is current as of this commit.

## Listen

Playable files are under `.refbuild/listen/` (gitignored, already generated):

| File | What it is |
| --- | --- |
| `source-music-a.wav` | 20 s music excerpt (original source) |
| `source-speech.wav` | 20 s speech excerpt (original source) |
| `rust-music-a-celt-096k-s16.wav` | Rust decoder, music, CELT 96 kb/s stereo |
| `ref-music-a-celt-096k-s16.wav` | Fixed16 reference decoder, same case |
| `rust-music-b-celt-160k-s16.wav` | Rust decoder, music, CELT 160 kb/s stereo |
| `rust-speech-silk-012k-s16.wav` | Rust decoder, speech, SILK 12 kb/s mono |
| `ref-speech-silk-012k-s16.wav` | Fixed16 reference decoder, same case |
| `rust-speech-hybrid-032k-s16.wav` | Rust decoder, speech, hybrid 32 kb/s mono |
| `rust-speech-silk-012k-dtx-s16.wav` | Rust decoder, speech, SILK 12 kb/s + DTX |
| `rust-music-a-celt-096k-f32.wav` | Rust decoder, float WAV output |

Binary used: `target/release/ropusdec` (also `target/debug/ropusdec`).

## Correctness summary

- `cargo test --workspace` passes (1,888 core unit/property tests + decoder/CLI tests).
- 19-case committed corpus (`corpus/manifest.json`):
  - 18/19 cases bit-exact on f32 vs the pinned fixed16 reference.
  - All 18 non-DTX cases bit-exact on s16 and s24 as well.
  - SILK cases are additionally bit-exact vs the default float reference.
- CELT/hybrid vs default float reference differ because the adopted core is
  the fixed-point 16-bit port while `./configure` default is the 24-bit float
  path. Max deltas per case are recorded in `.refbuild/validation-report.json`.
- The DTX case initially differed on 565/960,960 f32 samples; this was
  root-caused to two PLC porting bugs (excitation shift and glue-frame
  normalization) and fixed. All 19/19 cases are now bit-exact. See
  `VALIDATION.md`.

## Reproduce

```sh
scripts/build-reference.sh
scripts/reproduce-golden.sh --fixed16
cargo build --release -p opus-decoder-cli
scripts/verify-corpus.py \
  --decoder target/debug/ropusdec \
  --fixed16 .refbuild/opus-src-fixed16/opus_demo \
  --prod    .refbuild/opus-src/opus_demo
```

The decoder also reads Ogg Opus files (`--channels` inferred from OpusHead)
and has been checked against pinned `opusdec` on a generated Ogg file.
