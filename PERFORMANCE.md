# Performance evidence

Machine: Linux x86-64 (Manjaro, 16 threads), reference = libopus v1.6.1
fixed-point 16-bit build (`--disable-asm --disable-intrinsics --disable-rtcd`).
Both binaries decode the committed 19-case corpus to f32 and write to
`/dev/null`; medians of 10 process runs per case
(`scripts/bench-reference.py`).

## Throughput vs reference

Default `simd` build (`target/release/ropusdec`):

| case | Rust | reference | ratio |
| --- | ---: | ---: | ---: |
| music-a-celt-096k stereo | 60.95 ms | 58.32 ms | 1.045 |
| speech-silk-012k mono | 18.69 ms | 23.17 ms | 0.807 |
| speech-hybrid-032k mono | 32.37 ms | 34.22 ms | 0.946 |
| speech-silk-dtx mono | 20.89 ms | 28.65 ms | 0.729 |
| **mean** | | | **0.882** |

Scalar fallback (`--no-default-features`): mean ratio **1.007**, so the
`simd` feature measurably improves decode throughput (CELT +5.4%, SILK +20.6%,
hybrid +12.1%, DTX +8.5% on this matrix) while both builds remain bit-exact
on the full corpus.

Criterion micro-benchmarks (`cargo bench -p opus-decoder --bench decode`,
20 samples each, decode-only, 20 s corpus cases):

- celt-stereo-96k: 50.4–51.7 ms
- silk-mono-12k: 13.9–14.3 ms
- hybrid-mono-32k: 27.7–28.5 ms
- silk-dtx-mono-12k: 16.4–17.0 ms

## Hotspot evidence

`cargo-show-asm` dumps are saved under `.refbuild/asm-*.txt` and regenerated
with:

```sh
cargo asm -p opus-core --lib --release decode_with_ec
cargo asm -p opus-core --lib --release decode_native
cargo asm -p opus-core --lib --release silk_resampler
```

- `decode_with_ec` assembly: 9,430 lines — CELT decode is the largest frame
  path and dominates the stereo music case.
- `decode_native` assembly: packet demux plus per-frame dispatch.
- `silk_resampler`: 4,817 lines — SILK resampling is the second hotspot; its
  filter kernels are the main speech-case cost.

The reference implementation's own optimization hints (x86 `celt/x86/*`,
`silk/x86/*`, NEON `celt/arm/*`, `silk/arm/*`) were checked; the Rust core
mirrors those hot loops in `celt/simd.rs` (xcorr, maxabs, band
denormalisation, MDCT windowing) via the portable `wide` crate.

## SIMD feature contract

- `opus-decoder` default features enable `simd` → `opus-core/simd` →
  `dep:wide`.
- `--no-default-features` removes the `wide` dependency and compiles
  bit-exact scalar fallbacks for the four gated CELT kernels.
- Full-corpus invariance: both builds report `19 cases compared, 0 failures`
  against the fixed16 reference, and their raw outputs are byte-identical.
- Instruction sets actually used by `wide::i32x4` in this code:
  - x86-64: `__m128i` (SSE2 baseline). `objdump` of `target/release/ropusdec`
    shows 0 YMM/ZMM/AVX instructions and no SSE4.1-only instructions; no
    AVX is emitted because the build does not set a higher `target-cpu`.
  - aarch64: `int32x4_t` (Neon 128-bit); cross-compile verified.
  - wasm32: see below — **the cargo feature alone does not emit SIMD128**
    because wasm SIMD is a codegen target feature, not a crate feature.

## AVX2 experiment

`RUSTFLAGS="-C target-cpu=x86-64-v3"` produces 5,522 AVX/YMM-family
instructions (baseline SSE2 build: 0). Criterion decode-only medians, same
corpus cases:

| case | SSE2 | x86-64-v3 (AVX2/FMA) | speedup |
| --- | ---: | ---: | ---: |
| celt-stereo-96k | 48.1 ms | 43.3 ms | 1.11x |
| silk-mono-12k | 13.2 ms | 12.6 ms | 1.04x |
| hybrid-mono-32k | 25.3 ms | 23.8 ms | 1.06x |
| silk-dtx-mono-12k | 14.7 ms | 14.8 ms | ~1.00x |

Conclusion: AVX2 is not necessary — the SSE2/default build is already faster
than the fixed-point reference and bit-exact — but it gives a real 4–11%
decode-core gain in CELT/hybrid. It belongs in an opt-in
`simd-avx2`-style feature (or runtime dispatch), not in the portable default.

## wasm SIMD128 experiment (wasmtime)

Key finding: `opus-core/simd` and `--no-default-features` produce
**byte-identical wasm binaries** unless `-C target-feature=+simd128` is passed
at build time. `wasm-tools print` confirms 0 SIMD opcodes without the flag.
Cargo features cannot enable codegen target features, so the feature currently
has no effect on wasm.

With `-C target-feature=+simd128` (interleaved 25-run medians, warmup first,
wasmtime on x86-64):

| case | no SIMD128 | +SIMD128 | ratio |
| --- | ---: | ---: | ---: |
| celt-stereo-96k | 107.2 ms | 102.2 ms | 0.954 |
| silk-mono-12k | 41.5 ms | 43.0 ms | 1.036 |
| hybrid-mono-32k | 68.6 ms | 60.1 ms | 0.875 |
| silk-dtx-mono-12k | 49.9 ms | 50.9 ms | 1.020 |

Global SIMD128 helps CELT (~5%) and hybrid (~12.5%) but slightly hurts SILK
and DTX. The scientific next step is per-function
`#[target_feature(enable = "simd128")]` kernels for the CELT loops only, with
SILK kept scalar, then re-benchmark under wasmtime and (ideally) V8/SpiderMonkey.

## Known optimization boundaries

- Scalar fallback is roughly at reference parity; SIMD build is ~12% faster
  than reference on the benchmark matrix.
- Explicit AVX/NEON hand-written kernels beyond the `wide` port remain future
  work; the current gains come from the vendored portable kernels plus LLVM
  auto-vectorization.
- wasm32 builds compile in both feature modes. The `wasm32-wasip1` release CLI
  was run under wasmtime on all 19 corpus cases and produced byte-identical
  output to the native Rust decoder; wasm runtime benchmarking (timing) is
  still pending.
