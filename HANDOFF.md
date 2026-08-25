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
cargo build --release --workspace --no-default-features   # scalar fallback
```

- On macOS/Linux this produces `target/release/ropusdec`.
- On Windows the binary is `target\release\ropusdec.exe`.
- On Windows arm64 and Linux arm64, `--no-default-features` is the fallback
  until explicit Neon kernels land; default build still compiles and uses the
  portable `wide` kernels where LLVM can target the architecture.

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
| macOS arm64 | ? | ? | ? | |
| Linux arm64 | ? | ? | ? | |
| Windows x86-64 | ? | ? | ? | |
| Windows arm64 | ? | ? | ? | |
| wasm32 simd | compile | ? | ? | compile-verified, runtime TBD |
| wasm32 scalar | compile | ? | ? | compile-verified, runtime TBD |

## wasm

```sh
rustup target add wasm32-unknown-unknown
cargo build -p opus-decoder --target wasm32-unknown-unknown
cargo build -p opus-decoder --target wasm32-unknown-unknown --no-default-features
```

Both configurations compile. A wasmtime/node runtime decode test is the
remaining step for runtime wasm correctness; native decode is bit-exact, and
the scalar/SIMD Rust paths are identical by construction.
