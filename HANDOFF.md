# Cross-platform handoff

Targets: macOS arm64, Linux arm64, Windows x86-64, Windows arm64, wasm32.

## Common prerequisites

- Rust stable >= 1.88 with the platform's standard target installed.
- Cargo network access for `thiserror`/`clap`/`anyhow`/`wide` (all pure-Rust;
  no C toolchain needed to build the decoder).
- A C toolchain only if you want to rebuild the validation references with
  `scripts/build-reference.sh` (the script also needs `git`, `curl`, `tar`).

## Build

```sh
cargo build --release --workspace                 # native, default SIMD
cargo build --release --workspace --no-default-features   # scalar kernels
```

Default codegen baselines (`.cargo/config.toml`):
- x86-64: Haswell+ (`x86-64-v3`, AVX2/FMA). Override with
  `RUSTFLAGS="-C target-cpu=x86-64"`.
- wasm32: `+simd128`; runtime/engine must support the wasm SIMD proposal.

- On macOS/Linux this produces `target/release/ropusdec`.
- On Windows the binary is `target\release\ropusdec.exe`.
- On Windows arm64 and Linux arm64, the portable `wide` kernels compile and
  LLVM can lower them to Neon. On aarch64 the `neon2` tier (implied by
  `simd`) additionally provides explicit Neon kernels for the SILK resampler
  (FIR interpolation, up2_hq), SILK LPC/LTP synthesis (register-resident
  subframe LPC), CELT FFT radix-3/4/5 butterflies, MDCT rotations/norm scan,
  comb filter and stereo merge — see `PERFORMANCE.md` for the M1 Pro evidence.
  The neon2 gate is `target_arch = "aarch64"` only (no OS gate) and uses base
  AArch64 NEON intrinsics exclusively, so it is OS-generic; `cargo check`
  passes for `aarch64-unknown-linux-gnu` and `aarch64-pc-windows-msvc`.

## Test and validate

```sh
cargo test --workspace
scripts/build-reference.sh          # once, pinning libopus v1.6.1
scripts/reproduce-golden.sh --fixed16
scripts/verify-corpus.py \
  --decoder target/debug/ropusdec \
  --fixed16 .refbuild/opus-src-fixed16/opus_demo \
  --prod    .refbuild/opus-src/opus_demo
```

Expected: `19 cases compared, 0 failures`, and `cargo test --workspace` green.
The fixed16 oracle must be built on the same platform; cross-platform
bit-exactness is not claimed between different architectures (same rule as the
C reference itself).

## Performance

```sh
python3 scripts/bench-reference.py \
  --decoder target/release/ropusdec \
  --fixed16 .refbuild/opus-src-fixed16/opus_demo --iters 10
cargo bench -p opus-decoder --bench decode
cargo asm -p opus-core --lib --release decode_with_ec   # cargo-show-asm
```

Record results in the table below. The Linux x86-64 baseline (this machine)
is in `PERFORMANCE.md`.

## Results template

| Platform | cargo test | corpus verify | mean decode ratio (Rust/ref) | notes |
| --- | --- | --- | --- | --- |
| Linux x86-64 | pass | 19/19 bit-exact | ~0.88 (simd), ~1.01 (scalar) | reference machine |
| macOS arm64 | pass (release+debug) | 19/19 bit-exact (all 3 feature builds, byte-identical) | ~0.51 (simd+neon2) | M1 Pro; see `PERFORMANCE.md` macOS section |
| Linux arm64 | 1,667 / 1,645 tests pass | 19/19 bit-exact | ~1.22 script (CELT ~1.14) | see `PERFORMANCE.md` arm64 section |
| Windows x86-64 | cross-check passes | TBD on Windows | ? | both simd/scalar compile |
| Windows arm64 | cross-check passes | TBD on Windows ARM | ? | both simd/scalar compile |
| wasm32 simd | compile + wasmtime | 19/19 matches native | ? | wasi CLI run |
| wasm32 scalar | compile | TBD | ? | scalar wasi run not repeated |

macOS note: `scripts/build-reference.sh` clones from `$HOME/src/public/opus`
and needs `automake`; on this machine the two oracles were instead built from
the official `opus-1.6.1.tar.gz` release tarball (verified as the pinned tag
via the PGP-signed tag object) with the same configure flags, which needs no
automake.

## Retesting neon2 on Linux arm64 (and other aarch64 cores)

The neon2 kernels were tuned on Apple M1 Pro. They use base AArch64 NEON and
are gated on `target_arch = "aarch64"` only, but the *magnitude* of each
kernel's win is microarchitecture-dependent. Retest procedure:

1. Correctness (must hold before any benchmark is trusted):
   `cargo test --workspace` (release **and** debug — debug caught a real
   alignment UB on M1), then `scripts/verify-corpus.py` for all three builds
   (default `simd`, `--no-default-features --features neon2`,
   `--no-default-features`), and a 3-way byte-identity check of the outputs.
2. End-to-end: `cargo bench -p opus-decoder --bench decode` under the same
   three configs, plus `scripts/bench-reference.py` for the process ratio.
   The pre-neon2 Linux arm64 record is mean ~1.22 (PERFORMANCE.md); neon2
   should improve it further.
3. Per-kernel attribution (this is how to find kernels that do **not** pay
   off on a given core): the kernel-level A/B harness lives in the tree:
   ```
   cargo test -p opus-core --release --features neon2 neon2::bench -- --ignored --nocapture
   cargo test -p opus-core --release fft_bench -- --ignored --nocapture   # repeat under --no-default-features
   ```
   It prints scalar-vs-NEON ns/iter for each kernel (SILK LPC subframe,
   resampler fir12/up2_hq, MDCT norm scan, FFT per size). A kernel below
   ~1.0x is a candidate for gating off on that hardware; between 1.0x and
   ~1.05x is a wash (see the documented `up2_hq` and reverted `deemphasis`
   cases in PERFORMANCE.md).

If a kernel needs to change or be disabled for a core where it loses, do not
delete it and do not touch the shared helpers (`s_mul4`/`cmul4`/`tw4`/
`lc4`/`sc4`): every kernel is dispatched at its own call site with the scalar
reference retained (used by the neon2 unit tests), so per-kernel gating is a
two-line `cfg` edit at the call site. Rust has no per-core `cfg`, so a kernel
that diverges between recorded machines (e.g. wins on M1 Pro, loses on the
WSL2 Qualcomm part) should be split into its own opt-in feature
(e.g. `neon2-up2hq`) rather than silently regressed on one machine; keep it
in default `simd` only while it is neutral-or-positive on both recorded
machines. Do not resurrect the per-sample LPC memory-window dot product or
the stereo-deemphasis lane packing — both are measured losses on M1 Pro
(rationale in `crates/opus-core/src/neon2.rs` and PERFORMANCE.md).

## wasm

```sh
rustup target add wasm32-unknown-unknown
cargo build -p opus-decoder --target wasm32-unknown-unknown
cargo build -p opus-decoder --target wasm32-unknown-unknown --no-default-features
```

The wasm32-wasip1 release CLI runs under wasmtime and matched native output
byte-for-byte on all 19 corpus cases. wasm32-unknown-unknown Node benchmarks
use `scripts/bench-wasm-node.mjs`; see `PERFORMANCE.md` for the wasmtime/V8
SIMD128 matrix. Cargo features cannot set codegen target features, so wasm
SIMD128 is enabled by `.cargo/config.toml` rather than by the `simd` feature.
