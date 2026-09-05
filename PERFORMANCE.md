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

- Default: `simd` on = portable `wide` kernels + `neon2` (explicit aarch64
  NEON intrinsics, no-op elsewhere). Scalar fallback with
  `--no-default-features`; NEON-only measurement with
  `--no-default-features --features neon2`.
- Full corpus in all builds: **19/19 f32 bit-exact vs fixed16, s16/s24
  byte-exact**, and SIMD/scalar outputs are byte-identical. AVX2 codegen does
  not change decoded samples.


## macOS arm64 validation run (Apple M1 Pro, macOS 26.6, rustc 1.98.1)

Reference built from the official `opus-1.6.1.tar.gz` release tarball
(verified identical to the pinned tag `22244de5` via the PGP-signed tag
object) with the script's autotools flags; the tarball ships a pre-generated
`configure`, which avoids the `automake` dependency of `git clone` +
`autogen.sh`. Same fixed16 (pure-C) and prod (float) oracles.

Results: **19/19 corpus cases bit-exact on f32/s16/s24 vs `fixed16`** for all
three feature builds (default `simd`, `neon2`-only, scalar), and the three
builds' outputs are byte-identical to each other. `cargo test --workspace`
passes in release and debug (debug caught one UB bug the release build could
not: the FFT twiddle gather read packed i32s from a 2-byte-aligned
`KissTwiddleCpx` table at odd indices — fixed with `read_unaligned`).

### Method

1. Hotspots: `sample(1)` on a loop driver (`opus-decoder/examples/decode_loop.rs`)
   for SILK and CELT cases, aggregated to inclusive/self counts per symbol,
   then `otool`/`cargo-show-asm` disassembly of the hottest leaf addresses to
   see whether each loop is scalar or vectorised and what its critical path
   looks like.
2. Kernel-level A/B: `#[ignore]`d timing tests
   (`cargo test -p opus-core --release --features neon2 neon2::bench -- --ignored --nocapture`,
   `fft_bench` similarly) measuring scalar vs NEON for each candidate kernel
   before adopting it.
3. End-to-end A/B: criterion `decode` bench under three feature configs
   (scalar / `neon2` / `simd` = wide + neon2), then
   `scripts/bench-reference.py` for process-level ratios.

### Baseline (pre-neon2) criterion medians

celt-stereo-96k 37.95 ms, silk-mono-12k 10.86 ms, hybrid-mono-32k 21.92 ms,
silk-dtx-mono-12k 14.34 ms; scalar: 38.17 / 11.75 / 22.91 / 14.75 ms.

### neon2 kernels and per-kernel A/B (M1 Pro)

| kernel | scalar | NEON | verdict |
| --- | ---: | ---: | --- |
| SILK LPC synthesis subframe (40 samples, register-resident window) | 582 ns | 402 ns | **1.45x, adopted** |
| SILK resampler 8-tap fractional FIR (640 outputs) | 2817 ns | 1687 ns | **1.67x, adopted** |
| SILK up2_hq 2-lane allpass (320 inputs) | 1319 ns | 1263 ns | 1.04x, adopted (weak but free) |
| CELT MDCT dynamic-range scan (960) | 821 ns | 114 ns | **7.2x, adopted** |
| CELT MDCT pre/post-rotation (adopted; end-to-end effect below) | — | — | adopted |
| CELT FFT radix-3/4/5 j-loops + degenerate radix-4 (480-pt) | 2233 ns | 1641 ns | **1.36x, adopted** |
| CELT comb-filter constant region, stereo_merge, inner_prod_norm_shift | — | — | adopted |
| SILK LPC per-sample dot product over the memory window | 279 ns | 517 ns | **0.54x, rejected**: the 16-byte reload partially overlaps the previous sample's 4-byte store and misses store-to-load forwarding; the register-resident subframe kernel supersedes it |
| CELT stereo deemphasis (2-lane IIR) | 3287 ns | 3630 ns | **0.91x, rejected**: the per-channel chain is latency-bound; lane packing only added vector-crossing overhead |

The LPC story is the important design lesson: the synthesis filter is
recursive, so the only vectorisation is across taps within one sample, and a
per-sample NEON dot product *loses* to the unrolled scalar loop because of
the store→wide-load forwarding miss on the recursive window. Keeping the
16-tap window in vector registers for the whole subframe (`vext` slide + one
lane insert per sample) removes the load from the loop-carried chain and
wins 1.45x.

### End-to-end (criterion decode, medians)

| case | scalar | simd (pre-neon2) | neon2-only | simd+neon2 (default) |
| --- | ---: | ---: | ---: | ---: |
| celt-stereo-96k | 38.17 | 37.95 | 33.84 | **33.80** |
| silk-mono-12k | 11.75 | 10.86 | 8.31 | **8.32** |
| hybrid-mono-32k | 22.91 | 21.92 | 17.78 | **17.77** |
| silk-dtx-mono-12k | 14.75 | 14.34 | 12.79 | **12.84** |

neon2 vs pre-neon2 simd: SILK −23%, hybrid −19%, DTX −11%, CELT −11%.
neon2-only ≈ simd+neon2 within noise — the portable `wide` kernels
contribute nothing measurable on aarch64; the explicit NEON kernels carry
the win.

### Process-level vs fixed16 reference (`bench-reference.py`, 15 iters)

| case | ratio pre-neon2 | ratio neon2 |
| --- | ---: | ---: |
| music-a-celt-096k stereo | 0.731 | **0.662** |
| speech-silk-012k mono | 0.445 | **0.388** |
| speech-hybrid-032k mono | 0.600 | **0.511** |
| speech-silk-dtx mono | 0.503 | **0.467** |
| **mean** | 0.570 | **0.507** |

(Note the oracle here is the pure-C fixed16 build without intrinsics; on
arm64 the C reference gives up more than on x86-64, hence ratios < 1.)

### Final hotspot distribution (post-neon2, sample-based)

- SILK case: LPC subframe kernel ~36% (latency-bound recursion, near its
  floor), resampler ~31% (dominated by the serial 3-stage allpass of
  `up2_hq`), decode_pulses ~8%, range-coder iCDF ~6%, nlsf2a ~5%.
- CELT case: `cwrsi` ~17% (serial table walk; a branchless variant was tried
  on Linux arm64 and reverted), `clt_mdct_backward` ~16% inclusive,
  `deemphasis` ~10% (latency-bound), `opus_fft_impl` now below ~6%.

### Feature design

`neon2` is a separate feature from `simd`: `simd = ["dep:wide", "neon2"]`, so
the default build gets the portable `wide` kernels **plus** all explicit
aarch64 NEON kernels, while `--no-default-features --features neon2` measures
NEON alone and `--no-default-features` stays the bit-exact scalar fallback.
All neon2 kernels are gated `all(target_arch = "aarch64", feature = "neon2")`
and compile to nothing elsewhere. Every kernel has a scalar-vs-NEON
randomised unit test in `src/neon2.rs`; the corpus gates bit-exactness
end-to-end.

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
  recursive LPC and LTP synthesis filters (then gated by `feature = "simd"`,
  since moved to the `neon2` tier and superseded on M1 by the
  register-resident subframe LPC kernel — see the macOS section).
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

- Explicit hand-written AVX intrinsics remain future work. On aarch64 the
  `neon2` tier now covers the SILK resampler, SILK LPC/LTP synthesis, CELT
  FFT butterflies, MDCT rotations/norm scan, comb filter and stereo merge.
- Remaining aarch64 hotspots are latency-bound recursions (SILK LPC subframe
  kernel, `up2_hq` allpass chain, CELT `deemphasis`) or serial table walks
  (`cwrsi`, range coder iCDF) that do not vectorise profitably — both
  measured and documented above.
- The remaining arm64 process gap is mostly SILK `silk_decode_core` and the
  CLI/reference stdio path; CELT is close to parity (about 1.14x).
- Browser engine variance matters: decisions should use the Node harness and,
  when possible, SpiderMonkey as a second data point.
- wasm32-wasip1 CLI under wasmtime: all 19 corpus cases byte-identical to
  native; native and wasm checksums are identical in the Node harness.
