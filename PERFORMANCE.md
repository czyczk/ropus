# Performance evidence

Machine: Linux x86-64 (Manjaro, 16 threads), reference = libopus v1.6.1
fixed-point 16-bit build (`--disable-asm --disable-intrinsics --disable-rtcd`).
Native CLI measurements decode committed corpus cases to f32 raw and write to
`/dev/null`; medians of process runs (`scripts/bench-reference.py`).

## Default codegen baseline

`.cargo/config.toml` now sets the default baselines:

- x86-64: `-C target-cpu=x86-64-v3` (Haswell+: AVX2/FMA/BMI). Override with
  `RUSTFLAGS="-C target-cpu=x86-64" cargo build ...`.
- wasm32: `-C target-feature=+simd128` (wasm SIMD proposal). V8,
  SpiderMonkey, and wasmtime all support it.

The `simd` cargo feature still controls the explicit `wide` kernels; the
codegen baseline above is independent of that feature because Cargo features
cannot set rustc target features. `--no-default-features` remains the
bit-exact scalar-kernel fallback.

## Throughput vs fixed16 reference (default build)

| case | Rust | reference | ratio |
| --- | ---: | ---: | ---: |
| music-a-celt-096k stereo | 59.9 ms | 67.5 ms | 0.888 |
| speech-silk-012k mono | 21.3 ms | 23.7 ms | 0.900 |
| speech-hybrid-032k mono | 32.1 ms | 35.8 ms | 0.896 |
| speech-silk-dtx mono | 25.2 ms | 33.5 ms | 0.751 |
| **mean** | | | **0.859** |

Criterion decode-only (20 samples, default AVX2 build):

- celt-stereo-96k: 42.2–44.7 ms
- silk-mono-12k: 12.3–12.9 ms
- hybrid-mono-32k: 22.9–24.6 ms
- silk-dtx-mono-12k: 14.3–15.3 ms

SSE2 override (`RUSTFLAGS="-C target-cpu=x86-64"`) criterion medians:
48.1 / 13.2 / 25.3 / 14.7 ms. AVX2/FMA therefore gives roughly 11% on CELT,
4% on SILK, 6% on hybrid, and none on DTX — enough to justify the default,
with a documented override for older CPUs.

## Hotspot evidence

`cargo-show-asm` dumps are saved under `.refbuild/asm-*.txt`:

```sh
cargo asm -p opus-core --lib --release decode_with_ec
cargo asm -p opus-core --lib --release decode_native
cargo asm -p opus-core --lib --release silk_resampler
```

- `decode_with_ec`: CELT decode, dominant for stereo music.
- `decode_native`: packet demux plus per-frame dispatch.
- `silk_resampler`: SILK resampling filter kernels, main speech cost.

The reference's own asm hints (`celt/x86/*`, `silk/x86/*`, `celt/arm/*`,
`silk/arm/*`) were checked; the explicit Rust kernels live in
`celt/simd.rs` (xcorr, maxabs, band denormalisation, MDCT windowing).

## SIMD feature contract and correctness

- Default: `simd` on, explicit `wide` kernels; scalar fallback available with
  `--no-default-features`.
- Full corpus both builds: **19/19 f32 bit-exact vs fixed16, s16/s24
  byte-exact**, and SIMD/scalar outputs are byte-identical. AVX2 codegen does
  not change decoded samples.


## Linux arm64 validation run (WSL2, Qualcomm, 12 cores)

Reference built from the pinned libopus 1.6.1 tree (`22244de5a79bd1d6d623c32e72bf1954b56235be`)
with CMake, using macro-equivalent configurations:

- `fixed16`: `OPUS_FIXED_POINT=ON`, `OPUS_DISABLE_INTRINSICS=ON`,
  `CMAKE_BUILD_TYPE=Release` — `FIXED_POINT=1`, no `ENABLE_RES24`, no
  `OPUS_HAVE_RTCD` (matches the documented autotools oracle).
- `prod`: default float/NEON build, ML options off.

Results: **19/19 corpus cases bit-exact on f32/s16/s24 vs `fixed16`** for both
default-SIMD and `--no-default-features` release builds, and SIMD/scalar
outputs are byte-identical. `cargo test --workspace` passes in both feature
configurations (1,667 core tests + wrapper tests with SIMD; 1,645 with scalar).

### Process-level throughput vs fixed16 reference (default build)

Official `scripts/bench-reference.py`, 21 iterations, pinned CPU, f32 to
`/dev/null`:

| case | Rust | reference | ratio |
| --- | ---: | ---: | ---: |
| music-a-celt-096k stereo | 47.8 ms | 42.0 ms | 1.139 |
| speech-silk-012k mono | 14.9 ms | 11.4 ms | 1.311 |
| speech-hybrid-032k mono | 25.9 ms | 22.0 ms | 1.177 |
| speech-silk-dtx mono | 17.7 ms | 14.2 ms | 1.247 |
| **mean** | | | **1.219** |

The three-way interleaved medians that introduced the generic/CELT changes
gave the same ordering (CELT 1.140, SILK 1.312, hybrid 1.169, DTX 1.250),
so the improvement is not an artifact of the official script's case order.
The first arm64 baseline on this machine was **1.548**; this change recovers
about 21% of mean process time.

Criterion remains useful for decode-only comparisons, but on this WSL2 host
its wall-clock medians are more frequency-sensitive than the interleaved
process benchmark; use the process ratios above as the record for arm64.

### arm64 hotspot evidence (perf, armv8 PMU)

`perf` was unpacked locally (`.refbuild/tools`) without touching the system.

- `speech-silk-012k-20ms`: `silk_decode_core` dominates (~46% of sampled
  cycles), followed by IIR/FIR resampler (~15%) and up2_HQ (~13%); within the
  core, the recursive LPC dot product and gain/saturation output dominate.
- `music-a-celt-096k-20ms`: `cwrsi`, `opus_fft_impl`, `clt_mdct_backward`,
  and `deemphasis` each account for roughly 9–14% of sampled cycles;
  `compute_theta`, `exp_rotation1`, `quant_partition`, and `stereo_merge`
  are the next tier.

### arm64 optimizations applied

- `deemphasis`: dedicated stereo/no-downsample path matching C's
  `deemphasis_stereo_simple`.
- CLI: per-call PCM scratch buffers are reused instead of zero-allocated per
  packet; output capacity is pre-reserved from the encoded stream size;
  sample conversion uses iterator-based bulk byte extension. The CLI now also
  has a `simd` feature so `--no-default-features` actually builds the scalar
  decoder (previously the dependency's default feature was silently kept).
- SILK core: stack-backed working buffers replace per-frame/subframe `Vec`
  allocations; C-style unrolled LTP/LPC kernels; unchecked hot resampler FIR
  reads with proven bounds; explicit aarch64 NEON dot-product kernels for the
  recursive LPC and LTP synthesis filters (gated by `feature = "simd"` and
  bit-exact on all 19 corpus cases).
- Range coder: iterator-based iCDF walk removes per-symbol bounds checks and
  turns table overrun into an error state instead of a panic.
- Generic CELT/CLI follow-up: raw `opus_demo` input now goes through a 64 KiB
  `BufReader` (matching the reference stdio buffering); `clt_mdct_backward`
  runs its FFT in-place on the interleaved output buffer instead of allocating
  and copying a `Vec<KissFftCpx>` per MDCT (`KissFftCpx` is now `repr(C)`);
  the MDCT pre-rotation input-length test is loop-invariant; hot
  inner-product/normalisation/stereo-merge loops use unchecked reads after
  debug assertions. A branchless padded-table variant of `cwrsi` was tried and
  reverted: it was consistently ~2% slower on CELT in A/B runs, so the
  predictable bounds branch remains.
## wasm: root cause of the earlier "negative optimization"

The earlier finding was measurement noise plus a real configuration bug:
`opus-core/simd` and `--no-default-features` produced byte-identical wasm
binaries without `-C target-feature=+simd128`, because wasm SIMD is a codegen
target feature and cannot be enabled by a cargo feature. `wasm-tools print`
confirmed 0 SIMD opcodes in both. The apparent 12% slowdown was wasmtime
cold-cache noise.

The wasm32 baseline is now set to `+simd128` in `.cargo/config.toml`.

### wasmtime (Cranelift), interleaved 25-run medians

| case | no SIMD128 | +SIMD128 | ratio |
| --- | ---: | ---: | ---: |
| celt-stereo-96k | 107.2 ms | 102.2 ms | 0.954 |
| silk-mono-12k | 41.5 ms | 43.0 ms | 1.036 |
| hybrid-mono-32k | 68.6 ms | 60.1 ms | 0.875 |
| silk-dtx-mono-12k | 49.9 ms | 50.9 ms | 1.020 |

### Node/V8, wasm32-unknown-unknown module (`scripts/bench-wasm-node.mjs`)

First clean interleaved run (31 timed runs after warmup, same binaries):

| case | scalar | scalar+SIMD128 | simd feature+SIMD128 |
| --- | ---: | ---: | ---: |
| celt-stereo-96k | 77.17 ms | 74.00 ms | 71.77 ms |
| silk-mono-12k | 20.25 ms | 18.77 ms | 19.79 ms |
| hybrid-mono-32k | 44.17 ms | 40.18 ms | 40.74 ms |
| silk-dtx-mono-12k | 23.09 ms | 22.60 ms | 22.41 ms |

On V8, global SIMD128 improves every case, including SILK (4–7%). The
wasmtime SILK slowdown is Cranelift-codegen-specific, not an algorithm or Rust
writing issue. `simd`-feature binaries differ only in the four CELT kernels;
their SILK timings are equal within noise, as expected.

## Known optimization boundaries

- Explicit hand-written AVX intrinsics remain future work. The SILK LPC/LTP
  recursive filters now have explicit aarch64 NEON kernels; CELT still relies
  on the portable `wide` kernels plus LLVM auto-vectorisation.
- The remaining arm64 process gap is mostly SILK `silk_decode_core` and the
  CLI/reference stdio path; CELT is close to parity (about 1.14x).
- Browser engine variance matters: decisions should use the Node harness and,
  when possible, SpiderMonkey as a second data point.
- wasm32-wasip1 CLI under wasmtime: all 19 corpus cases byte-identical to
  native; native and wasm checksums are identical in the Node harness.
